//! Deterministic Program Derived Address (PDA) derivation for ZeroLock Shards.
//!
//! Emulates Solana's canonical PDA derivation algorithm:
//! PDA = sha256(seed_0 || seed_1 || ... || seed_n || program_id || "ProgramDerivedAddress")

use sha2::{Digest, Sha256};

pub const SHARD_SEED_PREFIX: &[u8] = b"shard";
pub const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";

/// Derive a deterministic 32-byte shard address from market, index, and bump.
pub fn create_shard_address(
    program_id: &[u8; 32],
    market: &[u8; 32],
    shard_index: u16,
    bump: u8,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SHARD_SEED_PREFIX);
    hasher.update(market);
    hasher.update(&shard_index.to_le_bytes());
    hasher.update(&[bump]);
    hasher.update(program_id);
    hasher.update(PDA_MARKER);
    let result = hasher.finalize();
    let mut addr = [0u8; 32];
    addr.copy_from_slice(&result);
    addr
}

/// Find canonical bump and address for a given shard partition.
pub fn find_shard_address(
    program_id: &[u8; 32],
    market: &[u8; 32],
    shard_index: u16,
) -> ([u8; 32], u8) {
    let mut bump = 255u8;
    loop {
        let addr = create_shard_address(program_id, market, shard_index, bump);
        // Canonical non-zero byte check
        if addr[31] != 0 || bump == 0 {
            return (addr, bump);
        }
        bump -= 1;
    }
}
