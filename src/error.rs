//! Error definitions for the ZeroLock Engine.

use thiserror::Error;

/// Core error codes for the ZeroLock program.
#[derive(Error, Debug, Copy, Clone, PartialEq, Eq)]
pub enum ZeroLockError {
    #[error("Account data buffer is smaller than required struct size")]
    BufferTooSmall,

    #[error("Account data buffer alignment does not match struct requirement")]
    MisalignedBuffer,

    #[error("Account discriminator mismatch or uninitialized state")]
    InvalidDiscriminator,

    #[error("Mathematical overflow during calculation")]
    MathOverflow,

    #[error("Zero division encountered in fixed-point calculation")]
    ZeroDivision,

    #[error("Slippage tolerance exceeded: received output is less than minimum")]
    SlippageExceeded,

    #[error("Provided shard index is out of bounds for market shard count")]
    InvalidShardIndex,

    #[error("Reentrancy detected or transient lock is already active")]
    ReentrancyLocked,

    #[error("Market is currently paused by authority")]
    MarketPaused,

    #[error("Unauthorized: signer does not match required market authority")]
    Unauthorized,

    #[error("Shard reserve imbalance is below the rebalance threshold")]
    RebalanceThresholdNotMet,

    #[error("Shard is currently locked in a rebalance transition")]
    ShardLocked,

    #[error("Target slot has drifted past allowable epoch threshold")]
    SlotDriftExceeded,

    #[error("Invalid fee basis points configuration (must be <= 10000)")]
    InvalidFeeConfig,

    #[error("Zero swap amount provided")]
    ZeroAmount,
}

impl From<ZeroLockError> for u32 {
    fn from(err: ZeroLockError) -> Self {
        match err {
            ZeroLockError::BufferTooSmall => 6000,
            ZeroLockError::MisalignedBuffer => 6001,
            ZeroLockError::InvalidDiscriminator => 6002,
            ZeroLockError::MathOverflow => 6003,
            ZeroLockError::ZeroDivision => 6004,
            ZeroLockError::SlippageExceeded => 6005,
            ZeroLockError::InvalidShardIndex => 6006,
            ZeroLockError::ReentrancyLocked => 6007,
            ZeroLockError::MarketPaused => 6008,
            ZeroLockError::Unauthorized => 6009,
            ZeroLockError::RebalanceThresholdNotMet => 6010,
            ZeroLockError::ShardLocked => 6011,
            ZeroLockError::SlotDriftExceeded => 6012,
            ZeroLockError::InvalidFeeConfig => 6013,
            ZeroLockError::ZeroAmount => 6014,
        }
    }
}
