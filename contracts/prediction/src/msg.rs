use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Decimal, Uint128};

use crate::state::{Bet, Config, Round, UserStats};

#[cw_serde]
pub struct InstantiateMsg {
    pub denom: String,
    pub market: String,
    pub interval_seconds: u64,
    pub buffer_seconds: u64,
    pub min_bet: Uint128,
    pub fee_bps: u16,
    pub max_price_age_seconds: u64,
    /// Defaults to the instantiator.
    pub admin: Option<String>,
    pub operator: String,
    pub treasury: String,
}

#[cw_serde]
pub enum ExecuteMsg {
    /// Bet the attached funds that the price goes up.
    BetBull { epoch: u64 },
    /// Bet the attached funds that the price goes down.
    BetBear { epoch: u64 },
    /// Collect winnings and refunds for the given rounds.
    Claim { epochs: Vec<u64> },

    // Operator (keeper)
    GenesisStartRound {},
    GenesisLockRound { price: Decimal, publish_time: u64 },
    /// Locks the current round, closes the previous one and starts the next,
    /// all at the same price.
    ExecuteRound { price: Decimal, publish_time: u64 },
    /// Admin or operator.
    Pause {},
    /// Admin or operator. Clears the genesis flags: the operator must genesis again.
    Unpause {},

    // Admin
    /// Only while paused.
    UpdateConfig {
        interval_seconds: Option<u64>,
        buffer_seconds: Option<u64>,
        min_bet: Option<Uint128>,
        fee_bps: Option<u16>,
        max_price_age_seconds: Option<u64>,
        treasury: Option<String>,
    },
    SetOperator { operator: String },
    ProposeAdmin { admin: String },
    AcceptAdmin {},
    ClaimTreasury {},
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Config)]
    Config {},
    #[returns(StatusResponse)]
    Status {},
    #[returns(Option<Round>)]
    Round { epoch: u64 },
    /// Newest first.
    #[returns(Vec<Round>)]
    Rounds { before: Option<u64>, limit: Option<u32> },
    /// Newest first.
    #[returns(Vec<UserBetInfo>)]
    UserBets {
        address: String,
        before: Option<u64>,
        limit: Option<u32>,
    },
    /// Rounds the user can claim right now, scanning the latest `limit` bets.
    #[returns(PendingClaimsResponse)]
    PendingClaims { address: String, limit: Option<u32> },
    #[returns(UserStats)]
    UserStats { address: String },
    #[returns(Vec<LeaderboardEntry>)]
    Leaderboard {
        start_after: Option<String>,
        limit: Option<u32>,
    },
}

#[cw_serde]
pub struct MigrateMsg {}

#[cw_serde]
pub struct StatusResponse {
    pub current_epoch: u64,
    pub paused: bool,
    pub genesis_start_once: bool,
    pub genesis_lock_once: bool,
    pub treasury_accrued: Uint128,
    pub block_time: u64,
}

#[cw_serde]
pub struct UserBetInfo {
    pub epoch: u64,
    pub bet: Bet,
    pub round: Round,
    pub claimable: bool,
    pub refundable: bool,
    /// What `Claim` pays (or paid) for this bet; zero for a loss.
    pub payout: Uint128,
}

#[cw_serde]
pub struct PendingClaimsResponse {
    pub epochs: Vec<u64>,
    pub total: Uint128,
}

#[cw_serde]
pub struct LeaderboardEntry {
    pub address: String,
    pub stats: UserStats,
}
