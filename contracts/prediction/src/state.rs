use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Uint128};
use cw_storage_plus::{Item, Map};

pub const MAX_FEE_BPS: u16 = 1000;

#[cw_serde]
#[derive(Copy, Eq)]
pub enum Position {
    Bull,
    Bear,
}

#[cw_serde]
#[derive(Copy, Eq)]
pub enum CancelReason {
    /// Nobody bet on one of the two sides: there is no counterparty, so no game.
    OneSided,
    /// Close price equals lock price.
    Tie,
}

#[cw_serde]
pub enum RoundStatus {
    /// Accepting bets until `lock_ts`.
    Open,
    /// Lock price recorded, waiting for close.
    Locked,
    Settled { winner: Position },
    /// Every bet is refunded in full, no fee.
    Cancelled { reason: CancelReason },
}

/// Where lock/close prices come from. Only `Operator` is live: the keeper pushes
/// the price. `Pyth` is reserved so a later migration can switch without a new
/// storage layout.
#[cw_serde]
pub enum PriceSource {
    Operator,
    Pyth { contract: Addr, feed_id: String },
}

#[cw_serde]
pub struct Config {
    /// Bet denom (USDC.inj over IBC, via Osmosis channel-7).
    pub denom: String,
    /// Tracked asset, e.g. "BTC" or "ETH".
    pub market: String,
    pub interval_seconds: u64,
    pub buffer_seconds: u64,
    pub min_bet: Uint128,
    pub fee_bps: u16,
    pub max_price_age_seconds: u64,
    pub admin: Addr,
    pub operator: Addr,
    pub treasury: Addr,
    pub price_source: PriceSource,
}

#[cw_serde]
pub struct State {
    pub current_epoch: u64,
    pub paused: bool,
    pub genesis_start_once: bool,
    pub genesis_lock_once: bool,
    pub treasury_accrued: Uint128,
}

#[cw_serde]
pub struct Round {
    pub epoch: u64,
    pub start_ts: u64,
    pub lock_ts: u64,
    pub close_ts: u64,
    pub lock_price: Option<Decimal>,
    pub close_price: Option<Decimal>,
    pub total_amount: Uint128,
    pub bull_amount: Uint128,
    pub bear_amount: Uint128,
    /// Winning side's pool: the denominator of every payout.
    pub reward_base: Uint128,
    /// Pot shared by the winners (total minus fee).
    pub reward_amount: Uint128,
    pub fee_amount: Uint128,
    pub status: RoundStatus,
}

#[cw_serde]
pub struct Bet {
    pub position: Position,
    pub amount: Uint128,
    pub claimed: bool,
}

#[cw_serde]
#[derive(Default)]
pub struct UserStats {
    pub rounds_played: u64,
    pub rounds_won: u64,
    pub total_bet: Uint128,
    pub total_payout: Uint128,
}

pub const CONFIG: Item<Config> = Item::new("config");
pub const STATE: Item<State> = Item::new("state");
pub const PENDING_ADMIN: Item<Addr> = Item::new("pending_admin");
pub const ROUNDS: Map<u64, Round> = Map::new("rounds");
pub const BETS: Map<(u64, &Addr), Bet> = Map::new("bets");
pub const USER_ROUNDS: Map<(&Addr, u64), ()> = Map::new("user_rounds");
pub const USER_STATS: Map<&Addr, UserStats> = Map::new("user_stats");
