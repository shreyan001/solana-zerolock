//! Integration and stress test suite for ZeroLock Engine.

use solana_zerolock::error::ZeroLockError;
use solana_zerolock::instruction::ZeroLockInstruction;
use solana_zerolock::math::calculate_shard_divergence_bps;
use solana_zerolock::pda::{create_shard_address, find_shard_address};
use solana_zerolock::processor::{
    process_initialize_market, process_initialize_shard, process_rebalance_shards,
    process_shard_swap,
};
use solana_zerolock::state::{GlobalMarketHeader, ShardBucketState};
use solana_zerolock::estimate_cu_savings;

const MOCK_PROGRAM_ID: [u8; 32] = [1u8; 32];
const MOCK_AUTHORITY: [u8; 32] = [2u8; 32];
const MOCK_BASE_MINT: [u8; 32] = [3u8; 32];
const MOCK_QUOTE_MINT: [u8; 32] = [4u8; 32];

fn create_market_buffer() -> Vec<u8> {
    vec![0u8; core::mem::size_of::<GlobalMarketHeader>()]
}

fn create_shard_buffer() -> Vec<u8> {
    vec![0u8; core::mem::size_of::<ShardBucketState>()]
}

#[test]
fn test_zero_copy_alignment_and_pod_guarantees() {
    let mut market_buf = create_market_buffer();
    assert_eq!(market_buf.len(), 184);

    // Initialize market
    process_initialize_market(
        &mut market_buf,
        MOCK_AUTHORITY,
        MOCK_BASE_MINT,
        MOCK_QUOTE_MINT,
        4,
        25, // 0.25% fee
        50, // 0.50% rebalance threshold
    )
    .expect("Market initialization failed");

    // Zero-copy read view
    let market_ref = GlobalMarketHeader::load_ref(&market_buf).expect("Failed to load market ref");
    assert_eq!(market_ref.authority, MOCK_AUTHORITY);
    assert_eq!(market_ref.fee_bps, 25);
    assert_eq!(market_ref.shard_count, 4);

    // Corrupt buffer length
    let truncated = &market_buf[..50];
    assert_eq!(
        GlobalMarketHeader::load_ref(truncated).unwrap_err(),
        ZeroLockError::BufferTooSmall
    );
}

#[test]
fn test_deterministic_pda_derivation() {
    let market_addr = [9u8; 32];
    let (shard_pda, bump) = find_shard_address(&MOCK_PROGRAM_ID, &market_addr, 0);
    let derived = create_shard_address(&MOCK_PROGRAM_ID, &market_addr, 0, bump);
    assert_eq!(shard_pda, derived);

    // Distinct shard indices yield distinct addresses
    let (shard_pda_1, _) = find_shard_address(&MOCK_PROGRAM_ID, &market_addr, 1);
    assert_ne!(shard_pda, shard_pda_1);
}

#[test]
fn test_constant_product_swap_and_invariant_preservation() {
    let mut market_buf = create_market_buffer();
    let mut shard_buf = create_shard_buffer();

    let market_addr = [7u8; 32];
    process_initialize_market(
        &mut market_buf,
        MOCK_AUTHORITY,
        MOCK_BASE_MINT,
        MOCK_QUOTE_MINT,
        4,
        30, // 0.30% fee
        50,
    )
    .unwrap();

    let initial_base = 1_000_000u64;
    let initial_quote = 10_000_000u64;

    process_initialize_shard(
        &mut shard_buf,
        market_addr,
        0,
        255,
        initial_base,
        initial_quote,
    )
    .unwrap();

    let initial_k = (initial_base as u128) * (initial_quote as u128);

    // Execute swap: 10,000 Base in -> expect Quote out
    let receipt = process_shard_swap(
        &mut market_buf,
        &mut shard_buf,
        true, // base -> quote
        10_000,
        90_000, // min out
        100,    // slot
    )
    .expect("Swap failed");

    assert!(receipt.amount_out > 90_000);
    assert_eq!(receipt.fee_paid, 30); // 10,000 * 0.003 = 30

    let shard_ref = ShardBucketState::load_ref(&shard_buf).unwrap();
    let post_k = (shard_ref.reserves_base as u128) * (shard_ref.reserves_quote as u128);

    // Mathematical guarantee: post_k must be >= initial_k
    assert!(post_k >= initial_k);
}

#[test]
fn test_slippage_protection_enforcement() {
    let mut market_buf = create_market_buffer();
    let mut shard_buf = create_shard_buffer();

    process_initialize_market(
        &mut market_buf,
        MOCK_AUTHORITY,
        MOCK_BASE_MINT,
        MOCK_QUOTE_MINT,
        2,
        25,
        50,
    )
    .unwrap();

    process_initialize_shard(&mut shard_buf, [0u8; 32], 0, 255, 1_000_000, 1_000_000).unwrap();

    let res = process_shard_swap(
        &mut market_buf,
        &mut shard_buf,
        true,
        50_000,
        999_999, // Unreasonable slippage expectation
        101,
    );

    assert_eq!(res.unwrap_err(), ZeroLockError::SlippageExceeded);

    // Shard reserves must remain untampered
    let shard_ref = ShardBucketState::load_ref(&shard_buf).unwrap();
    assert_eq!(shard_ref.reserves_base, 1_000_000);
    assert_eq!(shard_ref.reserves_quote, 1_000_000);
}

#[test]
fn test_parallel_shard_independence() {
    let mut market_buf = create_market_buffer();
    let mut shard_0_buf = create_shard_buffer();
    let mut shard_1_buf = create_shard_buffer();

    process_initialize_market(
        &mut market_buf,
        MOCK_AUTHORITY,
        MOCK_BASE_MINT,
        MOCK_QUOTE_MINT,
        2,
        25,
        50,
    )
    .unwrap();

    process_initialize_shard(&mut shard_0_buf, [1u8; 32], 0, 255, 5_000_000, 5_000_000).unwrap();
    process_initialize_shard(&mut shard_1_buf, [1u8; 32], 1, 255, 5_000_000, 5_000_000).unwrap();

    // Swap on Shard 0
    let receipt_0 = process_shard_swap(
        &mut market_buf,
        &mut shard_0_buf,
        true,
        25_000,
        20_000,
        200,
    )
    .unwrap();

    // Swap on Shard 1
    let receipt_1 = process_shard_swap(
        &mut market_buf,
        &mut shard_1_buf,
        false, // quote -> base
        40_000,
        35_000,
        200,
    )
    .unwrap();

    assert_eq!(receipt_0.shard_index, 0);
    assert_eq!(receipt_1.shard_index, 1);
    assert_eq!(receipt_0.new_shard_base, 5_025_000);
    assert_eq!(receipt_1.new_shard_quote, 5_040_000);
}

#[test]
fn test_shard_rebalance_trigger_and_convergence() {
    let mut market_buf = create_market_buffer();
    let mut shard_a_buf = create_shard_buffer();
    let mut shard_b_buf = create_shard_buffer();

    process_initialize_market(
        &mut market_buf,
        MOCK_AUTHORITY,
        MOCK_BASE_MINT,
        MOCK_QUOTE_MINT,
        2,
        25,
        50, // 50 bps (0.5%) divergence threshold
    )
    .unwrap();

    process_initialize_shard(&mut shard_a_buf, [1u8; 32], 0, 255, 1_000_000, 1_000_000).unwrap();
    process_initialize_shard(&mut shard_b_buf, [1u8; 32], 1, 255, 1_000_000, 1_000_000).unwrap();

    // Heavy trade on Shard A to induce price skew
    process_shard_swap(
        &mut market_buf,
        &mut shard_a_buf,
        true,
        200_000, // 20% trade
        100_000,
        300,
    )
    .unwrap();

    let s_a = ShardBucketState::load_ref(&shard_a_buf).unwrap();
    let s_b = ShardBucketState::load_ref(&shard_b_buf).unwrap();

    let divergence = calculate_shard_divergence_bps(
        s_a.reserves_base,
        s_a.reserves_quote,
        s_b.reserves_base,
        s_b.reserves_quote,
    )
    .unwrap();

    assert!(divergence > 50);

    // Trigger keeper rebalance
    let rebalance_receipt = process_rebalance_shards(
        &mut market_buf,
        &mut shard_a_buf,
        &mut shard_b_buf,
        301,
    )
    .expect("Rebalance execution failed");

    assert_eq!(rebalance_receipt.shard_a_index, 0);
    assert_eq!(rebalance_receipt.shard_b_index, 1);

    // Post-rebalance divergence check
    let post_a = ShardBucketState::load_ref(&shard_a_buf).unwrap();
    let post_b = ShardBucketState::load_ref(&shard_b_buf).unwrap();

    let post_divergence = calculate_shard_divergence_bps(
        post_a.reserves_base,
        post_a.reserves_quote,
        post_b.reserves_base,
        post_b.reserves_quote,
    )
    .unwrap();

    assert!(post_divergence < divergence);
    assert_eq!(post_divergence, 0);
}

#[test]
fn test_instruction_unpacking() {
    let mut payload = vec![2u8, 1u8];
    payload.extend_from_slice(&100_000u64.to_le_bytes());
    payload.extend_from_slice(&95_000u64.to_le_bytes());

    let ix = ZeroLockInstruction::unpack(&payload).expect("Failed to unpack");
    match ix {
        ZeroLockInstruction::ShardSwap {
            is_base_to_quote,
            amount_in,
            min_amount_out,
        } => {
            assert!(is_base_to_quote);
            assert_eq!(amount_in, 100_000);
            assert_eq!(min_amount_out, 95_000);
        }
        _ => panic!("Expected ShardSwap instruction"),
    }
}

#[test]
fn test_compute_unit_profile_savings() {
    let profile = estimate_cu_savings();
    assert!(profile.cu_savings_percent > 99.0);
    assert_eq!(profile.zero_copy_cu, 85);
}

#[test]
fn test_multi_shard_fuzz_simulation() {
    let mut market_buf = create_market_buffer();
    const NUM_SHARDS: usize = 4;
    let mut shard_bufs: Vec<Vec<u8>> = (0..NUM_SHARDS).map(|_| create_shard_buffer()).collect();

    process_initialize_market(
        &mut market_buf,
        MOCK_AUTHORITY,
        MOCK_BASE_MINT,
        MOCK_QUOTE_MINT,
        NUM_SHARDS as u16,
        25,
        50,
    )
    .unwrap();

    for i in 0..NUM_SHARDS {
        process_initialize_shard(
            &mut shard_bufs[i],
            [1u8; 32],
            i as u16,
            255,
            10_000_000,
            10_000_000,
        )
        .unwrap();
    }

    // Run 200 consecutive trades randomly distributed across shards
    let mut pseudo_rand: u64 = 1337;
    for slot in 1..=200 {
        pseudo_rand = pseudo_rand.wrapping_mul(6364136223846793005).wrapping_add(1);
        let shard_idx = (pseudo_rand % NUM_SHARDS as u64) as usize;
        let is_base_to_quote = (pseudo_rand >> 16) % 2 == 0;
        let trade_amount = 1_000 + (pseudo_rand % 5_000);

        let res = process_shard_swap(
            &mut market_buf,
            &mut shard_bufs[shard_idx],
            is_base_to_quote,
            trade_amount,
            1, // loose min out for fuzz testing
            slot,
        );
        assert!(res.is_ok());
    }

    let market_ref = GlobalMarketHeader::load_ref(&market_buf).unwrap();
    assert!(market_ref.cumulative_volume() > 0);
}
