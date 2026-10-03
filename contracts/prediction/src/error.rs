use cosmwasm_std::StdError;
use cw_utils::PaymentError;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("{0}")]
    Payment(#[from] PaymentError),

    #[error("unauthorized")]
    Unauthorized {},

    #[error("contract is paused")]
    Paused {},

    #[error("contract is not paused")]
    NotPaused {},

    #[error("bet is too early or too late for epoch {epoch}")]
    WrongEpoch { epoch: u64 },

    #[error("round {epoch} is not accepting bets")]
    RoundNotBettable { epoch: u64 },

    #[error("bet must be at least {min}")]
    BetTooSmall { min: String },

    #[error("already bet on round {epoch}")]
    AlreadyBet { epoch: u64 },

    #[error("genesis start already done")]
    GenesisStartDone {},

    #[error("genesis start not done")]
    GenesisStartNotDone {},

    #[error("genesis lock already done")]
    GenesisLockDone {},

    #[error("genesis lock not done")]
    GenesisLockNotDone {},

    #[error("round {epoch} cannot start yet")]
    CannotStart { epoch: u64 },

    #[error("round {epoch} can only be locked between {from} and {to}")]
    LockWindow { epoch: u64, from: u64, to: u64 },

    #[error("round {epoch} can only be closed between {from} and {to}")]
    CloseWindow { epoch: u64, from: u64, to: u64 },

    #[error("round {epoch} is not in the expected state")]
    BadRoundState { epoch: u64 },

    #[error("price must be positive")]
    InvalidPrice {},

    #[error("price published at {publish_time} is too far from block time {block_time}")]
    StalePrice { publish_time: u64, block_time: u64 },

    #[error("price source not supported yet")]
    UnsupportedPriceSource {},

    #[error("no bet on round {epoch}")]
    NoBet { epoch: u64 },

    #[error("round {epoch} already claimed")]
    AlreadyClaimed { epoch: u64 },

    #[error("round {epoch} is not claimable")]
    NotClaimable { epoch: u64 },

    #[error("nothing to claim")]
    NothingToClaim {},

    #[error("invalid config: {reason}")]
    InvalidConfig { reason: String },

    #[error("no pending admin")]
    NoPendingAdmin {},
}
