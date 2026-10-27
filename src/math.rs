//! Fixed-Point Arithmetic & Constant-Product AMM Math for ZeroLock.
//!
//! Provides overflow-safe 128-bit integer calculations for constant product
//! invariants, dynamic fee deductions, and cross-shard balance divergence.

use crate::error::ZeroLockError;

pub const BPS_DENOMINATOR: u128 = 10_000;
pub const Q64_RESOLUTION: u128 = 1 << 64;

/// Result structure for an executed swap calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapCalculationResult {
    /// Token output delivered to the swapper.
    pub amount_out: u64,
    /// Protocol fee retained in the input token.
    pub fee_amount: u64,
    /// Updated input asset reserve in the shard.
    pub new_reserve_in: u64,
    /// Updated output asset reserve in the shard.
    pub new_reserve_out: u64,
}

/// Compute exact-in swap amounts with constant-product invariant:
///
/// (reserve_in + amount_in_net) * (reserve_out - amount_out) >= reserve_in * reserve_out
pub fn calculate_swap_exact_in(
    reserve_in: u64,
    reserve_out: u64,
    amount_in: u64,
    fee_bps: u16,
) -> Result<SwapCalculationResult, ZeroLockError> {
    if amount_in == 0 {
        return Err(ZeroLockError::ZeroAmount);
    }
    if reserve_in == 0 || reserve_out == 0 {
        return Err(ZeroLockError::ZeroDivision);
    }
    if fee_bps as u128 > BPS_DENOMINATOR {
        return Err(ZeroLockError::InvalidFeeConfig);
    }

    let r_in = reserve_in as u128;
    let r_out = reserve_out as u128;
    let a_in = amount_in as u128;
    let fee_rate = fee_bps as u128;

    // Fee calculation with ceiling division for protocol security
    let fee_amount = a_in
        .checked_mul(fee_rate)
        .ok_or(ZeroLockError::MathOverflow)?
        .checked_add(BPS_DENOMINATOR - 1)
        .ok_or(ZeroLockError::MathOverflow)?
        .checked_div(BPS_DENOMINATOR)
        .ok_or(ZeroLockError::MathOverflow)?;

    let net_amount_in = a_in
        .checked_sub(fee_amount)
        .ok_or(ZeroLockError::MathOverflow)?;

    if net_amount_in == 0 {
        return Err(ZeroLockError::ZeroAmount);
    }

    // Invariant: dy = (r_out * dx_net) / (r_in + dx_net)
    let numerator = r_out
        .checked_mul(net_amount_in)
        .ok_or(ZeroLockError::MathOverflow)?;

    let denominator = r_in
        .checked_add(net_amount_in)
        .ok_or(ZeroLockError::MathOverflow)?;

    let amount_out_u128 = numerator
        .checked_div(denominator)
        .ok_or(ZeroLockError::ZeroDivision)?;

    if amount_out_u128 == 0 {
        return Err(ZeroLockError::ZeroAmount);
    }

    if amount_out_u128 >= r_out {
        return Err(ZeroLockError::MathOverflow);
    }

    let new_reserve_in = (r_in + a_in) as u64;
    let new_reserve_out = (r_out - amount_out_u128) as u64;

    // Verification check: ensure k_after >= k_before
    let k_before = r_in.checked_mul(r_out).ok_or(ZeroLockError::MathOverflow)?;
    let k_after = (new_reserve_in as u128)
        .checked_mul(new_reserve_out as u128)
        .ok_or(ZeroLockError::MathOverflow)?;

    if k_after < k_before {
        return Err(ZeroLockError::MathOverflow);
    }

    Ok(SwapCalculationResult {
        amount_out: amount_out_u128 as u64,
        fee_amount: fee_amount as u64,
        new_reserve_in,
        new_reserve_out,
    })
}

/// Compute spot price in Q64.64 fixed-point format:
/// price = (quote_reserves << 64) / base_reserves
pub fn compute_spot_price_q64(base_reserves: u64, quote_reserves: u64) -> Result<u128, ZeroLockError> {
    if base_reserves == 0 {
        return Err(ZeroLockError::ZeroDivision);
    }
    let quote_u128 = (quote_reserves as u128)
        .checked_mul(Q64_RESOLUTION)
        .ok_or(ZeroLockError::MathOverflow)?;
    Ok(quote_u128 / (base_reserves as u128))
}

/// Calculate the relative price divergence in basis points between two shards.
pub fn calculate_shard_divergence_bps(
    shard_a_base: u64,
    shard_a_quote: u64,
    shard_b_base: u64,
    shard_b_quote: u64,
) -> Result<u16, ZeroLockError> {
    let price_a = compute_spot_price_q64(shard_a_base, shard_a_quote)?;
    let price_b = compute_spot_price_q64(shard_b_base, shard_b_quote)?;

    let (max_p, min_p) = if price_a >= price_b {
        (price_a, price_b)
    } else {
        (price_b, price_a)
    };

    if min_p == 0 {
        return Err(ZeroLockError::ZeroDivision);
    }

    let diff = max_p - min_p;
    let bps = diff
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(ZeroLockError::MathOverflow)?
        / min_p;

    Ok(bps.min(u16::MAX as u128) as u16)
}

/// Compute optimal reserve transfer to rebalance two divergent shards.
///
/// Returns (delta_base, delta_quote) to move from Shard A to Shard B.
pub fn calculate_rebalance_transfer(
    shard_a_base: u64,
    shard_a_quote: u64,
    shard_b_base: u64,
    shard_b_quote: u64,
) -> Result<(i64, i64), ZeroLockError> {
    let total_base = (shard_a_base as u128)
        .checked_add(shard_b_base as u128)
        .ok_or(ZeroLockError::MathOverflow)?;
    let total_quote = (shard_a_quote as u128)
        .checked_add(shard_b_quote as u128)
        .ok_or(ZeroLockError::MathOverflow)?;

    let target_base_per_shard = total_base / 2;
    let target_quote_per_shard = total_quote / 2;

    let delta_base = (shard_a_base as i128) - (target_base_per_shard as i128);
    let delta_quote = (shard_a_quote as i128) - (target_quote_per_shard as i128);

    Ok((delta_base as i64, delta_quote as i64))
}
