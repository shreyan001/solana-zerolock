//! Instruction representations and byte packing for ZeroLock.

use crate::error::ZeroLockError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroLockInstruction {
    /// Initialize market header: discriminator (0) + params
    InitializeMarket {
        authority: [u8; 32],
        base_mint: [u8; 32],
        quote_mint: [u8; 32],
        shard_count: u16,
        fee_bps: u16,
        rebalance_threshold_bps: u16,
    },
    /// Initialize shard bucket: discriminator (1) + params
    InitializeShard {
        shard_index: u16,
        bump: u8,
        initial_base: u64,
        initial_quote: u64,
    },
    /// Swap against shard: discriminator (2) + params
    ShardSwap {
        is_base_to_quote: bool,
        amount_in: u64,
        min_amount_out: u64,
    },
    /// Rebalance two divergent shards: discriminator (3)
    RebalanceShards,
    /// Pause or unpause market: discriminator (4)
    SetMarketStatus {
        paused: bool,
    },
}

impl ZeroLockInstruction {
    /// Zero-copy byte unpacker for incoming instruction packets.
    pub fn unpack(input: &[u8]) -> Result<Self, ZeroLockError> {
        let (&tag, rest) = input.split_first().ok_or(ZeroLockError::BufferTooSmall)?;
        match tag {
            0 => {
                // InitializeMarket: 32 + 32 + 32 + 2 + 2 + 2 = 102 bytes
                if rest.len() < 102 {
                    return Err(ZeroLockError::BufferTooSmall);
                }
                let mut authority = [0u8; 32];
                let mut base_mint = [0u8; 32];
                let mut quote_mint = [0u8; 32];
                authority.copy_from_slice(&rest[0..32]);
                base_mint.copy_from_slice(&rest[32..64]);
                quote_mint.copy_from_slice(&rest[64..96]);
                let shard_count = u16::from_le_bytes([rest[96], rest[97]]);
                let fee_bps = u16::from_le_bytes([rest[98], rest[99]]);
                let rebalance_threshold_bps = u16::from_le_bytes([rest[100], rest[101]]);
                Ok(Self::InitializeMarket {
                    authority,
                    base_mint,
                    quote_mint,
                    shard_count,
                    fee_bps,
                    rebalance_threshold_bps,
                })
            }
            1 => {
                // InitializeShard: 2 + 1 + 8 + 8 = 19 bytes
                if rest.len() < 19 {
                    return Err(ZeroLockError::BufferTooSmall);
                }
                let shard_index = u16::from_le_bytes([rest[0], rest[1]]);
                let bump = rest[2];
                let initial_base = u64::from_le_bytes(rest[3..11].try_into().unwrap());
                let initial_quote = u64::from_le_bytes(rest[11..19].try_into().unwrap());
                Ok(Self::InitializeShard {
                    shard_index,
                    bump,
                    initial_base,
                    initial_quote,
                })
            }
            2 => {
                // ShardSwap: 1 + 8 + 8 = 17 bytes
                if rest.len() < 17 {
                    return Err(ZeroLockError::BufferTooSmall);
                }
                let is_base_to_quote = rest[0] != 0;
                let amount_in = u64::from_le_bytes(rest[1..9].try_into().unwrap());
                let min_amount_out = u64::from_le_bytes(rest[9..17].try_into().unwrap());
                Ok(Self::ShardSwap {
                    is_base_to_quote,
                    amount_in,
                    min_amount_out,
                })
            }
            3 => Ok(Self::RebalanceShards),
            4 => {
                if rest.is_empty() {
                    return Err(ZeroLockError::BufferTooSmall);
                }
                Ok(Self::SetMarketStatus {
                    paused: rest[0] != 0,
                })
            }
            _ => Err(ZeroLockError::InvalidDiscriminator),
        }
    }
}
