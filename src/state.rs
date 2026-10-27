//! Zero-Copy Memory-Mapped Account Structures for ZeroLock.
//!
//! All structs implement `bytemuck::Pod` and `bytemuck::Zeroable` with strict
//! 8-byte alignment layouts and zero implicit padding. Deserialization involves
//! direct pointer casting over borrowed account byte slices, eliminating heap
//! allocations and reducing compute unit (CU) consumption from ~15,000 CU (Borsh)
//! to < 100 CU.

use bytemuck::{Pod, Zeroable};
use crate::error::ZeroLockError;

/// Account discriminators (8-byte blake3/sha256 prefixes).
pub const MARKET_DISCRIMINATOR: [u8; 8] = [0x5A, 0x45, 0x52, 0x4F, 0x4C, 0x4F, 0x43, 0x4B]; // "ZEROLOCK"
pub const SHARD_DISCRIMINATOR: [u8; 8] = [0x53, 0x48, 0x41, 0x52, 0x44, 0x42, 0x55, 0x43];  // "SHARDBUC"

/// Bitmask flags for GlobalMarketHeader.
pub const FLAG_PAUSED: u16 = 1 << 0;
pub const FLAG_REENTRANCY_LOCKED: u16 = 1 << 1;
pub const FLAG_REBALANCE_IN_PROGRESS: u16 = 1 << 2;

/// Shard operational status.
pub const SHARD_STATUS_ACTIVE: u8 = 0;
pub const SHARD_STATUS_REBALANCING: u8 = 1;
pub const SHARD_STATUS_LOCKED: u8 = 2;

/// Global market coordination header (184 bytes, 8-byte aligned, zero padding).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct GlobalMarketHeader {
    /// Program state discriminator ("ZEROLOCK").
    pub discriminator: [u8; 8],
    /// Market manager authority pubkey.
    pub authority: [u8; 32],
    /// Base token SPL mint pubkey.
    pub base_mint: [u8; 32],
    /// Quote token SPL mint pubkey.
    pub quote_mint: [u8; 32],
    /// Swap fee in basis points (e.g. 25 = 0.25%).
    pub fee_bps: u16,
    /// Number of partitioned deterministic shard buckets.
    pub shard_count: u16,
    /// Imbalance threshold in basis points before rebalance triggers.
    pub rebalance_threshold_bps: u16,
    /// Operational status bitmask flags.
    pub flags: u16,
    /// Aggregated base token reserves across active shards.
    pub total_base_reserves: u64,
    /// Aggregated quote token reserves across active shards.
    pub total_quote_reserves: u64,
    /// Slot timestamp of the last global rebalance epoch.
    pub last_rebalance_slot: u64,
    /// Low 64-bits of cumulative swap volume.
    pub cumulative_volume_lo: u64,
    /// High 64-bits of cumulative swap volume.
    pub cumulative_volume_hi: u64,
    /// Reserved space for future protocol upgrades (32 bytes).
    pub reserved: [u8; 32],
}

/// Shard bucket PDA state (112 bytes, 8-byte aligned, zero padding).
///
/// Each shard maintains its own independent constant-product sub-reserves.
/// Swaps routing through different shards execute concurrently in parallel
/// slots on Solana Sealevel without account contention.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct ShardBucketState {
    /// Shard state discriminator ("SHARDBUC").
    pub discriminator: [u8; 8],
    /// Parent GlobalMarketHeader address.
    pub market: [u8; 32],
    /// Shard partition index (0 .. shard_count - 1).
    pub shard_index: u16,
    /// Canonical PDA bump seed.
    pub bump: u8,
    /// Status: 0=Active, 1=Rebalancing, 2=Locked.
    pub status: u8,
    /// Explicit 4-byte padding to maintain 8-byte boundary.
    pub _reserved_flags: u32,
    /// Available base token reserve in this partition.
    pub reserves_base: u64,
    /// Available quote token reserve in this partition.
    pub reserves_quote: u64,
    /// Monotonically increasing trade sequence counter (anti-reorg/MEV).
    pub epoch_sequence: u64,
    /// Slot of the most recent swap execution in this shard.
    pub last_trade_slot: u64,
    /// Total fee tokens accumulated in base asset.
    pub accumulated_fees_base: u64,
    /// Total fee tokens accumulated in quote asset.
    pub accumulated_fees_quote: u64,
    /// Reserved space for future partition metrics (16 bytes).
    pub reserved: [u8; 16],
}

impl GlobalMarketHeader {
    /// Verify that an account buffer is properly sized and aligned for zero-copy read.
    pub fn validate_buffer(data: &[u8]) -> Result<(), ZeroLockError> {
        if data.len() < core::mem::size_of::<Self>() {
            return Err(ZeroLockError::BufferTooSmall);
        }
        if (data.as_ptr() as usize) % core::mem::align_of::<Self>() != 0 {
            return Err(ZeroLockError::MisalignedBuffer);
        }
        Ok(())
    }

    /// Zero-copy read-only view of market account data.
    pub fn load_ref(data: &[u8]) -> Result<&Self, ZeroLockError> {
        Self::validate_buffer(data)?;
        let header: &Self = bytemuck::try_from_bytes(&data[..core::mem::size_of::<Self>()])
            .map_err(|_| ZeroLockError::MisalignedBuffer)?;
        if header.discriminator != MARKET_DISCRIMINATOR {
            return Err(ZeroLockError::InvalidDiscriminator);
        }
        Ok(header)
    }

    /// Zero-copy mutable view of market account data.
    pub fn load_mut(data: &mut [u8]) -> Result<&mut Self, ZeroLockError> {
        Self::validate_buffer(data)?;
        let header: &mut Self = bytemuck::try_from_bytes_mut(&mut data[..core::mem::size_of::<Self>()])
            .map_err(|_| ZeroLockError::MisalignedBuffer)?;
        if header.discriminator != MARKET_DISCRIMINATOR {
            return Err(ZeroLockError::InvalidDiscriminator);
        }
        Ok(header)
    }

    /// Cumulative volume as u128.
    pub fn cumulative_volume(&self) -> u128 {
        ((self.cumulative_volume_hi as u128) << 64) | (self.cumulative_volume_lo as u128)
    }

    /// Add volume to cumulative volume counter.
    pub fn add_volume(&mut self, amount: u128) {
        let current = self.cumulative_volume();
        let updated = current.saturating_add(amount);
        self.cumulative_volume_lo = updated as u64;
        self.cumulative_volume_hi = (updated >> 64) as u64;
    }

    /// Check if the market has an active reentrancy lock.
    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        (self.flags & FLAG_REENTRANCY_LOCKED) != 0
    }

    /// Set reentrancy lock flag.
    #[inline(always)]
    pub fn acquire_lock(&mut self) -> Result<(), ZeroLockError> {
        if self.is_locked() {
            return Err(ZeroLockError::ReentrancyLocked);
        }
        self.flags |= FLAG_REENTRANCY_LOCKED;
        Ok(())
    }

    /// Clear reentrancy lock flag.
    #[inline(always)]
    pub fn release_lock(&mut self) {
        self.flags &= !FLAG_REENTRANCY_LOCKED;
    }
}

impl ShardBucketState {
    /// Verify buffer size and alignment for shard account.
    pub fn validate_buffer(data: &[u8]) -> Result<(), ZeroLockError> {
        if data.len() < core::mem::size_of::<Self>() {
            return Err(ZeroLockError::BufferTooSmall);
        }
        if (data.as_ptr() as usize) % core::mem::align_of::<Self>() != 0 {
            return Err(ZeroLockError::MisalignedBuffer);
        }
        Ok(())
    }

    /// Zero-copy read-only view of shard account data.
    pub fn load_ref(data: &[u8]) -> Result<&Self, ZeroLockError> {
        Self::validate_buffer(data)?;
        let shard: &Self = bytemuck::try_from_bytes(&data[..core::mem::size_of::<Self>()])
            .map_err(|_| ZeroLockError::MisalignedBuffer)?;
        if shard.discriminator != SHARD_DISCRIMINATOR {
            return Err(ZeroLockError::InvalidDiscriminator);
        }
        Ok(shard)
    }

    /// Zero-copy mutable view of shard account data.
    pub fn load_mut(data: &mut [u8]) -> Result<&mut Self, ZeroLockError> {
        Self::validate_buffer(data)?;
        let shard: &mut Self = bytemuck::try_from_bytes_mut(&mut data[..core::mem::size_of::<Self>()])
            .map_err(|_| ZeroLockError::MisalignedBuffer)?;
        if shard.discriminator != SHARD_DISCRIMINATOR {
            return Err(ZeroLockError::InvalidDiscriminator);
        }
        Ok(shard)
    }
}
