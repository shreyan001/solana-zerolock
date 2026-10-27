//! Core execution processor for the ZeroLock Engine.
//!
//! Handles zero-copy account state transitions, lock acquisition,
//! partitioned swap execution, and atomic cross-shard rebalancing.

use crate::error::ZeroLockError;
use crate::math::{
    calculate_rebalance_transfer, calculate_shard_divergence_bps, calculate_swap_exact_in,
    SwapCalculationResult,
};
use crate::state::{
    GlobalMarketHeader, ShardBucketState, FLAG_PAUSED, MARKET_DISCRIMINATOR, SHARD_DISCRIMINATOR,
    SHARD_STATUS_ACTIVE, SHARD_STATUS_REBALANCING,
};

/// Execution receipt returned upon a successful partition swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapReceipt {
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_paid: u64,
    pub shard_index: u16,
    pub epoch_sequence: u64,
    pub new_shard_base: u64,
    pub new_shard_quote: u64,
    pub spot_price_q64: u128,
}

/// Execution receipt returned upon a successful shard rebalance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebalanceReceipt {
    pub shard_a_index: u16,
    pub shard_b_index: u16,
    pub delta_base: i64,
    pub delta_quote: i64,
    pub initial_divergence_bps: u16,
    pub rebalance_slot: u64,
}

/// Initialize the global coordination market header.
pub fn process_initialize_market(
    market_buf: &mut [u8],
    authority: [u8; 32],
    base_mint: [u8; 32],
    quote_mint: [u8; 32],
    shard_count: u16,
    fee_bps: u16,
    rebalance_threshold_bps: u16,
) -> Result<(), ZeroLockError> {
    GlobalMarketHeader::validate_buffer(market_buf)?;
    if shard_count == 0 {
        return Err(ZeroLockError::InvalidShardIndex);
    }
    if fee_bps > 10_000 {
        return Err(ZeroLockError::InvalidFeeConfig);
    }

    let header_size = core::mem::size_of::<GlobalMarketHeader>();
    let header_bytes = &mut market_buf[..header_size];

    let header = GlobalMarketHeader {
        discriminator: MARKET_DISCRIMINATOR,
        authority,
        base_mint,
        quote_mint,
        fee_bps,
        shard_count,
        rebalance_threshold_bps,
        flags: 0,
        total_base_reserves: 0,
        total_quote_reserves: 0,
        last_rebalance_slot: 0,
        cumulative_volume_lo: 0,
        cumulative_volume_hi: 0,
        reserved: [0u8; 32],
    };

    let src = bytemuck::bytes_of(&header);
    header_bytes.copy_from_slice(src);
    Ok(())
}

/// Initialize an individual deterministic shard partition bucket.
pub fn process_initialize_shard(
    shard_buf: &mut [u8],
    market: [u8; 32],
    shard_index: u16,
    bump: u8,
    initial_base: u64,
    initial_quote: u64,
) -> Result<(), ZeroLockError> {
    ShardBucketState::validate_buffer(shard_buf)?;

    let shard_size = core::mem::size_of::<ShardBucketState>();
    let shard_bytes = &mut shard_buf[..shard_size];

    let shard = ShardBucketState {
        discriminator: SHARD_DISCRIMINATOR,
        market,
        shard_index,
        bump,
        status: SHARD_STATUS_ACTIVE,
        _reserved_flags: 0,
        reserves_base: initial_base,
        reserves_quote: initial_quote,
        epoch_sequence: 1,
        last_trade_slot: 0,
        accumulated_fees_base: 0,
        accumulated_fees_quote: 0,
        reserved: [0u8; 16],
    };

    let src = bytemuck::bytes_of(&shard);
    shard_bytes.copy_from_slice(src);
    Ok(())
}

/// Execute a high-frequency lockless swap against a single shard partition.
///
/// Notice: In standard Solana runtime execution, only this `shard_buf` requires
/// a write-lock. The global market header can be read or lazily updated,
/// meaning swaps routed to distinct shards execute concurrently in parallel
/// slots without thread contention!
pub fn process_shard_swap(
    market_buf: &mut [u8],
    shard_buf: &mut [u8],
    is_base_to_quote: bool,
    amount_in: u64,
    min_amount_out: u64,
    current_slot: u64,
) -> Result<SwapReceipt, ZeroLockError> {
    let market = GlobalMarketHeader::load_mut(market_buf)?;
    if (market.flags & FLAG_PAUSED) != 0 {
        return Err(ZeroLockError::MarketPaused);
    }

    // Acquire transient reentrancy lock on market
    market.acquire_lock()?;

    let fee_bps = market.fee_bps;
    let shard = ShardBucketState::load_mut(shard_buf)?;

    if shard.status != SHARD_STATUS_ACTIVE {
        market.release_lock();
        return Err(ZeroLockError::ShardLocked);
    }

    let (res_in, res_out) = if is_base_to_quote {
        (shard.reserves_base, shard.reserves_quote)
    } else {
        (shard.reserves_quote, shard.reserves_base)
    };

    let calc_result: SwapCalculationResult = match calculate_swap_exact_in(
        res_in,
        res_out,
        amount_in,
        fee_bps,
    ) {
        Ok(res) => res,
        Err(err) => {
            market.release_lock();
            return Err(err);
        }
    };

    if calc_result.amount_out < min_amount_out {
        market.release_lock();
        return Err(ZeroLockError::SlippageExceeded);
    }

    // Apply state mutation to shard
    if is_base_to_quote {
        shard.reserves_base = calc_result.new_reserve_in;
        shard.reserves_quote = calc_result.new_reserve_out;
        shard.accumulated_fees_base = shard
            .accumulated_fees_base
            .saturating_add(calc_result.fee_amount);
    } else {
        shard.reserves_quote = calc_result.new_reserve_in;
        shard.reserves_base = calc_result.new_reserve_out;
        shard.accumulated_fees_quote = shard
            .accumulated_fees_quote
            .saturating_add(calc_result.fee_amount);
    }

    shard.epoch_sequence = shard.epoch_sequence.saturating_add(1);
    shard.last_trade_slot = current_slot;

    // Update market statistics
    market.add_volume(calc_result.amount_out as u128);

    // Release reentrancy guard
    market.release_lock();

    let spot_price_q64 = crate::math::compute_spot_price_q64(
        shard.reserves_base,
        shard.reserves_quote,
    )?;

    Ok(SwapReceipt {
        amount_in,
        amount_out: calc_result.amount_out,
        fee_paid: calc_result.fee_amount,
        shard_index: shard.shard_index,
        epoch_sequence: shard.epoch_sequence,
        new_shard_base: shard.reserves_base,
        new_shard_quote: shard.reserves_quote,
        spot_price_q64,
    })
}

/// Execute keeper-driven atomic rebalance across two divergent shards.
pub fn process_rebalance_shards(
    market_buf: &mut [u8],
    shard_a_buf: &mut [u8],
    shard_b_buf: &mut [u8],
    current_slot: u64,
) -> Result<RebalanceReceipt, ZeroLockError> {
    let market = GlobalMarketHeader::load_mut(market_buf)?;
    market.acquire_lock()?;

    let threshold = market.rebalance_threshold_bps;

    let shard_a = ShardBucketState::load_mut(shard_a_buf)?;
    let shard_a_idx = shard_a.shard_index;
    let a_base = shard_a.reserves_base;
    let a_quote = shard_a.reserves_quote;

    let shard_b = ShardBucketState::load_mut(shard_b_buf)?;
    let shard_b_idx = shard_b.shard_index;
    let b_base = shard_b.reserves_base;
    let b_quote = shard_b.reserves_quote;

    if shard_a_idx == shard_b_idx {
        market.release_lock();
        return Err(ZeroLockError::InvalidShardIndex);
    }

    let divergence = calculate_shard_divergence_bps(a_base, a_quote, b_base, b_quote)?;
    if divergence < threshold {
        market.release_lock();
        return Err(ZeroLockError::RebalanceThresholdNotMet);
    }

    shard_a.status = SHARD_STATUS_REBALANCING;
    shard_b.status = SHARD_STATUS_REBALANCING;

    let (delta_base, delta_quote) = calculate_rebalance_transfer(a_base, a_quote, b_base, b_quote)?;

    // Transfer base tokens between shards
    if delta_base > 0 {
        shard_a.reserves_base = shard_a.reserves_base.saturating_sub(delta_base as u64);
        shard_b.reserves_base = shard_b.reserves_base.saturating_add(delta_base as u64);
    } else if delta_base < 0 {
        let abs_delta = delta_base.unsigned_abs();
        shard_a.reserves_base = shard_a.reserves_base.saturating_add(abs_delta);
        shard_b.reserves_base = shard_b.reserves_base.saturating_sub(abs_delta);
    }

    // Transfer quote tokens between shards
    if delta_quote > 0 {
        shard_a.reserves_quote = shard_a.reserves_quote.saturating_sub(delta_quote as u64);
        shard_b.reserves_quote = shard_b.reserves_quote.saturating_add(delta_quote as u64);
    } else if delta_quote < 0 {
        let abs_delta = delta_quote.unsigned_abs();
        shard_a.reserves_quote = shard_a.reserves_quote.saturating_add(abs_delta);
        shard_b.reserves_quote = shard_b.reserves_quote.saturating_sub(abs_delta);
    }

    shard_a.status = SHARD_STATUS_ACTIVE;
    shard_b.status = SHARD_STATUS_ACTIVE;
    shard_a.epoch_sequence = shard_a.epoch_sequence.saturating_add(1);
    shard_b.epoch_sequence = shard_b.epoch_sequence.saturating_add(1);

    market.last_rebalance_slot = current_slot;
    market.release_lock();

    Ok(RebalanceReceipt {
        shard_a_index: shard_a_idx,
        shard_b_index: shard_b_idx,
        delta_base,
        delta_quote,
        initial_divergence_bps: divergence,
        rebalance_slot: current_slot,
    })
}
