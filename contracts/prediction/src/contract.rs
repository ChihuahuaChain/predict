#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    coins, to_json_binary, Addr, BankMsg, Binary, Decimal, Deps, DepsMut, Env, MessageInfo, Order,
    Response, StdResult, Storage, Uint128,
};
use cw2::set_contract_version;
use cw_storage_plus::Bound;
use cw_utils::must_pay;

use crate::error::ContractError;
use crate::msg::{
    ExecuteMsg, InstantiateMsg, LeaderboardEntry, MigrateMsg, PendingClaimsResponse, QueryMsg,
    StatusResponse, UserBetInfo,
};
use crate::state::{
    Bet, CancelReason, Config, Position, PriceSource, Round, RoundStatus, State,
    BETS, CONFIG, MAX_FEE_BPS, PENDING_ADMIN, ROUNDS, STATE, USER_ROUNDS, USER_STATS,
};

const CONTRACT_NAME: &str = "crates.io:huahua-prediction";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;
const MAX_PENDING_SCAN: u32 = 500;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let admin = match msg.admin {
        Some(a) => deps.api.addr_validate(&a)?,
        None => info.sender,
    };
    let config = Config {
        denom: msg.denom,
        market: msg.market,
        interval_seconds: msg.interval_seconds,
        buffer_seconds: msg.buffer_seconds,
        min_bet: msg.min_bet,
        fee_bps: msg.fee_bps,
        max_price_age_seconds: msg.max_price_age_seconds,
        admin,
        operator: deps.api.addr_validate(&msg.operator)?,
        treasury: deps.api.addr_validate(&msg.treasury)?,
        price_source: PriceSource::Operator,
    };
    validate_config(&config)?;
    CONFIG.save(deps.storage, &config)?;
    STATE.save(
        deps.storage,
        &State {
            current_epoch: 0,
            paused: false,
            genesis_start_once: false,
            genesis_lock_once: false,
            treasury_accrued: Uint128::zero(),
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("market", config.market)
        .add_attribute("denom", config.denom))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::BetBull { epoch } => bet(deps, env, info, epoch, Position::Bull),
        ExecuteMsg::BetBear { epoch } => bet(deps, env, info, epoch, Position::Bear),
        ExecuteMsg::Claim { epochs } => claim(deps, env, info, epochs),
        ExecuteMsg::GenesisStartRound {} => genesis_start_round(deps, env, info),
        ExecuteMsg::GenesisLockRound {
            price,
            publish_time,
        } => genesis_lock_round(deps, env, info, price, publish_time),
        ExecuteMsg::ExecuteRound {
            price,
            publish_time,
        } => execute_round(deps, env, info, price, publish_time),
        ExecuteMsg::Pause {} => pause(deps, info),
        ExecuteMsg::Unpause {} => unpause(deps, info),
        ExecuteMsg::UpdateConfig {
            interval_seconds,
            buffer_seconds,
            min_bet,
            fee_bps,
            max_price_age_seconds,
            treasury,
        } => update_config(
            deps,
            info,
            interval_seconds,
            buffer_seconds,
            min_bet,
            fee_bps,
            max_price_age_seconds,
            treasury,
        ),
        ExecuteMsg::SetOperator { operator } => set_operator(deps, info, operator),
        ExecuteMsg::ProposeAdmin { admin } => propose_admin(deps, info, admin),
        ExecuteMsg::AcceptAdmin {} => accept_admin(deps, info),
        ExecuteMsg::ClaimTreasury {} => claim_treasury(deps, info),
    }
}

// ---------------------------------------------------------------- players

fn bet(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    epoch: u64,
    position: Position,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    let state = STATE.load(deps.storage)?;
    if state.paused {
        return Err(ContractError::Paused {});
    }
    if epoch != state.current_epoch {
        return Err(ContractError::WrongEpoch { epoch });
    }
    let amount = must_pay(&info, &config.denom)?;
    if amount < config.min_bet {
        return Err(ContractError::BetTooSmall {
            min: config.min_bet.to_string(),
        });
    }

    let mut round = ROUNDS
        .may_load(deps.storage, epoch)?
        .ok_or(ContractError::RoundNotBettable { epoch })?;
    let now = env.block.time.seconds();
    if round.status != RoundStatus::Open || now < round.start_ts || now >= round.lock_ts {
        return Err(ContractError::RoundNotBettable { epoch });
    }
    if BETS.has(deps.storage, (epoch, &info.sender)) {
        return Err(ContractError::AlreadyBet { epoch });
    }

    round.total_amount += amount;
    match position {
        Position::Bull => round.bull_amount += amount,
        Position::Bear => round.bear_amount += amount,
    }
    ROUNDS.save(deps.storage, epoch, &round)?;
    BETS.save(
        deps.storage,
        (epoch, &info.sender),
        &Bet {
            position,
            amount,
            claimed: false,
        },
    )?;
    USER_ROUNDS.save(deps.storage, (&info.sender, epoch), &())?;
    USER_STATS.update(deps.storage, &info.sender, |s| -> StdResult<_> {
        let mut s = s.unwrap_or_default();
        s.rounds_played += 1;
        s.total_bet += amount;
        Ok(s)
    })?;

    Ok(Response::new()
        .add_attribute("action", "bet")
        .add_attribute("epoch", epoch.to_string())
        .add_attribute("sender", info.sender)
        .add_attribute("position", position_str(position))
        .add_attribute("amount", amount))
}

/// What a bet pays right now, if anything. `refund` is true for cancelled or
/// never-settled rounds, which return the full stake.
struct Claimable {
    amount: Uint128,
    refund: bool,
}

fn claimable(config: &Config, round: &Round, bet: &Bet, now: u64) -> Option<Claimable> {
    if bet.claimed || bet.amount.is_zero() {
        return None;
    }
    match &round.status {
        RoundStatus::Settled { winner } if *winner == bet.position => Some(Claimable {
            amount: bet
                .amount
                .multiply_ratio(round.reward_amount, round.reward_base),
            refund: false,
        }),
        RoundStatus::Settled { .. } => None,
        RoundStatus::Cancelled { .. } => Some(Claimable {
            amount: bet.amount,
            refund: true,
        }),
        // The keeper missed the window: once the buffer is over the round can
        // never settle, so stakes come back.
        RoundStatus::Open | RoundStatus::Locked => {
            (now > round.close_ts + config.buffer_seconds).then_some(Claimable {
                amount: bet.amount,
                refund: true,
            })
        }
    }
}

fn claim(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    epochs: Vec<u64>,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    let now = env.block.time.seconds();
    let mut total = Uint128::zero();
    let mut stats = USER_STATS
        .may_load(deps.storage, &info.sender)?
        .unwrap_or_default();

    for epoch in &epochs {
        let epoch = *epoch;
        let round = ROUNDS
            .may_load(deps.storage, epoch)?
            .ok_or(ContractError::NoBet { epoch })?;
        let mut bet = BETS
            .may_load(deps.storage, (epoch, &info.sender))?
            .ok_or(ContractError::NoBet { epoch })?;
        if bet.claimed {
            return Err(ContractError::AlreadyClaimed { epoch });
        }
        let c = claimable(&config, &round, &bet, now)
            .ok_or(ContractError::NotClaimable { epoch })?;

        if c.refund {
            // A refunded round was not really played.
            stats.rounds_played = stats.rounds_played.saturating_sub(1);
            stats.total_bet = stats.total_bet.saturating_sub(bet.amount);
        } else {
            stats.rounds_won += 1;
            stats.total_payout += c.amount;
        }
        bet.claimed = true;
        BETS.save(deps.storage, (epoch, &info.sender), &bet)?;
        total += c.amount;
    }
    USER_STATS.save(deps.storage, &info.sender, &stats)?;

    let mut res = Response::new()
        .add_attribute("action", "claim")
        .add_attribute("sender", info.sender.as_str())
        .add_attribute(
            "epochs",
            epochs
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .add_attribute("amount", total);
    if !total.is_zero() {
        res = res.add_message(BankMsg::Send {
            to_address: info.sender.into_string(),
            amount: coins(total.u128(), config.denom),
        });
    }
    Ok(res)
}

// ---------------------------------------------------------------- operator

fn genesis_start_round(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_operator(&config, &info.sender)?;
    let mut state = STATE.load(deps.storage)?;
    ensure_not_paused(&state)?;
    if state.genesis_start_once {
        return Err(ContractError::GenesisStartDone {});
    }

    state.current_epoch += 1;
    start_round(deps.storage, &config, &env, state.current_epoch)?;
    state.genesis_start_once = true;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new()
        .add_attribute("action", "genesis_start_round")
        .add_attribute("epoch", state.current_epoch.to_string()))
}

fn genesis_lock_round(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    price: Decimal,
    publish_time: u64,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_operator(&config, &info.sender)?;
    let mut state = STATE.load(deps.storage)?;
    ensure_not_paused(&state)?;
    if !state.genesis_start_once {
        return Err(ContractError::GenesisStartNotDone {});
    }
    if state.genesis_lock_once {
        return Err(ContractError::GenesisLockDone {});
    }
    check_price(&config, &env, price, publish_time)?;

    let locked = state.current_epoch;
    lock_round(deps.storage, &config, &env, locked, price)?;
    state.current_epoch += 1;
    start_round(deps.storage, &config, &env, state.current_epoch)?;
    state.genesis_lock_once = true;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new()
        .add_attribute("action", "genesis_lock_round")
        .add_attribute("locked_epoch", locked.to_string())
        .add_attribute("lock_price", price.to_string())
        .add_attribute("epoch", state.current_epoch.to_string()))
}

fn execute_round(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    price: Decimal,
    publish_time: u64,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_operator(&config, &info.sender)?;
    let mut state = STATE.load(deps.storage)?;
    ensure_not_paused(&state)?;
    if !state.genesis_start_once {
        return Err(ContractError::GenesisStartNotDone {});
    }
    if !state.genesis_lock_once {
        return Err(ContractError::GenesisLockNotDone {});
    }
    check_price(&config, &env, price, publish_time)?;

    let locked = state.current_epoch;
    let ended = locked - 1;
    lock_round(deps.storage, &config, &env, locked, price)?;
    let settled = end_round(deps.storage, &config, &env, ended, price)?;
    if let RoundStatus::Settled { .. } = settled.status {
        state.treasury_accrued += settled.fee_amount;
    }

    state.current_epoch += 1;
    // Pancake's _safeStartRound: the round two epochs back must be over.
    let prev = ROUNDS.load(deps.storage, state.current_epoch - 2)?;
    if prev.close_ts == 0 || env.block.time.seconds() < prev.close_ts {
        return Err(ContractError::CannotStart {
            epoch: state.current_epoch,
        });
    }
    start_round(deps.storage, &config, &env, state.current_epoch)?;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new()
        .add_attribute("action", "execute_round")
        .add_attribute("price", price.to_string())
        .add_attribute("locked_epoch", locked.to_string())
        .add_attribute("ended_epoch", ended.to_string())
        .add_attribute("result", status_str(&settled.status))
        .add_attribute("fee", settled.fee_amount)
        .add_attribute("epoch", state.current_epoch.to_string()))
}

fn start_round(
    storage: &mut dyn Storage,
    config: &Config,
    env: &Env,
    epoch: u64,
) -> Result<(), ContractError> {
    let now = env.block.time.seconds();
    ROUNDS.save(
        storage,
        epoch,
        &Round {
            epoch,
            start_ts: now,
            lock_ts: now + config.interval_seconds,
            close_ts: now + 2 * config.interval_seconds,
            lock_price: None,
            close_price: None,
            total_amount: Uint128::zero(),
            bull_amount: Uint128::zero(),
            bear_amount: Uint128::zero(),
            reward_base: Uint128::zero(),
            reward_amount: Uint128::zero(),
            fee_amount: Uint128::zero(),
            status: RoundStatus::Open,
        },
    )?;
    Ok(())
}

fn lock_round(
    storage: &mut dyn Storage,
    config: &Config,
    env: &Env,
    epoch: u64,
    price: Decimal,
) -> Result<(), ContractError> {
    let mut round = ROUNDS
        .may_load(storage, epoch)?
        .ok_or(ContractError::BadRoundState { epoch })?;
    if round.status != RoundStatus::Open {
        return Err(ContractError::BadRoundState { epoch });
    }
    let now = env.block.time.seconds();
    let to = round.lock_ts + config.buffer_seconds;
    if now < round.lock_ts || now > to {
        return Err(ContractError::LockWindow {
            epoch,
            from: round.lock_ts,
            to,
        });
    }
    // A late lock pushes the close, so every round lasts a full interval.
    round.close_ts = now + config.interval_seconds;
    round.lock_price = Some(price);
    round.status = RoundStatus::Locked;
    ROUNDS.save(storage, epoch, &round)?;
    Ok(())
}

/// Records the close price and decides the round. A round is only played when
/// both sides have a stake and the price moved; otherwise it is cancelled and
/// everyone is refunded without fee.
fn end_round(
    storage: &mut dyn Storage,
    config: &Config,
    env: &Env,
    epoch: u64,
    price: Decimal,
) -> Result<Round, ContractError> {
    let mut round = ROUNDS
        .may_load(storage, epoch)?
        .ok_or(ContractError::BadRoundState { epoch })?;
    if round.status != RoundStatus::Locked {
        return Err(ContractError::BadRoundState { epoch });
    }
    let now = env.block.time.seconds();
    let to = round.close_ts + config.buffer_seconds;
    if now < round.close_ts || now > to {
        return Err(ContractError::CloseWindow {
            epoch,
            from: round.close_ts,
            to,
        });
    }
    round.close_price = Some(price);
    let lock_price = round.lock_price.ok_or(ContractError::BadRoundState { epoch })?;

    round.status = if round.bull_amount.is_zero() || round.bear_amount.is_zero() {
        RoundStatus::Cancelled {
            reason: CancelReason::OneSided,
        }
    } else if price == lock_price {
        RoundStatus::Cancelled {
            reason: CancelReason::Tie,
        }
    } else {
        let winner = if price > lock_price {
            Position::Bull
        } else {
            Position::Bear
        };
        round.fee_amount = round
            .total_amount
            .multiply_ratio(config.fee_bps, 10_000u128);
        round.reward_amount = round.total_amount - round.fee_amount;
        round.reward_base = match winner {
            Position::Bull => round.bull_amount,
            Position::Bear => round.bear_amount,
        };
        RoundStatus::Settled { winner }
    };
    ROUNDS.save(storage, epoch, &round)?;
    Ok(round)
}

fn pause(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.admin && info.sender != config.operator {
        return Err(ContractError::Unauthorized {});
    }
    let mut state = STATE.load(deps.storage)?;
    ensure_not_paused(&state)?;
    state.paused = true;
    STATE.save(deps.storage, &state)?;
    Ok(Response::new()
        .add_attribute("action", "pause")
        .add_attribute("epoch", state.current_epoch.to_string()))
}

/// Admin or operator, as in Pancake V3: the keeper recovers from a missed
/// window on its own (pause, unpause, genesis again).
fn unpause(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.admin && info.sender != config.operator {
        return Err(ContractError::Unauthorized {});
    }
    let mut state = STATE.load(deps.storage)?;
    if !state.paused {
        return Err(ContractError::NotPaused {});
    }
    state.paused = false;
    state.genesis_start_once = false;
    state.genesis_lock_once = false;
    STATE.save(deps.storage, &state)?;
    Ok(Response::new()
        .add_attribute("action", "unpause")
        .add_attribute("epoch", state.current_epoch.to_string()))
}

// ---------------------------------------------------------------- admin

#[allow(clippy::too_many_arguments)]
fn update_config(
    deps: DepsMut,
    info: MessageInfo,
    interval_seconds: Option<u64>,
    buffer_seconds: Option<u64>,
    min_bet: Option<Uint128>,
    fee_bps: Option<u16>,
    max_price_age_seconds: Option<u64>,
    treasury: Option<String>,
) -> Result<Response, ContractError> {
    let mut config = CONFIG.load(deps.storage)?;
    ensure_admin(&config, &info.sender)?;
    if !STATE.load(deps.storage)?.paused {
        return Err(ContractError::NotPaused {});
    }
    if let Some(v) = interval_seconds {
        config.interval_seconds = v;
    }
    if let Some(v) = buffer_seconds {
        config.buffer_seconds = v;
    }
    if let Some(v) = min_bet {
        config.min_bet = v;
    }
    if let Some(v) = fee_bps {
        config.fee_bps = v;
    }
    if let Some(v) = max_price_age_seconds {
        config.max_price_age_seconds = v;
    }
    if let Some(v) = treasury {
        config.treasury = deps.api.addr_validate(&v)?;
    }
    validate_config(&config)?;
    CONFIG.save(deps.storage, &config)?;
    Ok(Response::new().add_attribute("action", "update_config"))
}

fn set_operator(
    deps: DepsMut,
    info: MessageInfo,
    operator: String,
) -> Result<Response, ContractError> {
    let mut config = CONFIG.load(deps.storage)?;
    ensure_admin(&config, &info.sender)?;
    config.operator = deps.api.addr_validate(&operator)?;
    CONFIG.save(deps.storage, &config)?;
    Ok(Response::new()
        .add_attribute("action", "set_operator")
        .add_attribute("operator", operator))
}

fn propose_admin(
    deps: DepsMut,
    info: MessageInfo,
    admin: String,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_admin(&config, &info.sender)?;
    let admin = deps.api.addr_validate(&admin)?;
    PENDING_ADMIN.save(deps.storage, &admin)?;
    Ok(Response::new()
        .add_attribute("action", "propose_admin")
        .add_attribute("admin", admin))
}

fn accept_admin(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let pending = PENDING_ADMIN
        .may_load(deps.storage)?
        .ok_or(ContractError::NoPendingAdmin {})?;
    if info.sender != pending {
        return Err(ContractError::Unauthorized {});
    }
    let mut config = CONFIG.load(deps.storage)?;
    config.admin = pending;
    CONFIG.save(deps.storage, &config)?;
    PENDING_ADMIN.remove(deps.storage);
    Ok(Response::new()
        .add_attribute("action", "accept_admin")
        .add_attribute("admin", config.admin))
}

fn claim_treasury(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_admin(&config, &info.sender)?;
    let mut state = STATE.load(deps.storage)?;
    let amount = state.treasury_accrued;
    if amount.is_zero() {
        return Err(ContractError::NothingToClaim {});
    }
    state.treasury_accrued = Uint128::zero();
    STATE.save(deps.storage, &state)?;
    Ok(Response::new()
        .add_message(BankMsg::Send {
            to_address: config.treasury.to_string(),
            amount: coins(amount.u128(), &config.denom),
        })
        .add_attribute("action", "claim_treasury")
        .add_attribute("amount", amount))
}

// ---------------------------------------------------------------- helpers

fn validate_config(config: &Config) -> Result<(), ContractError> {
    let invalid = |reason: &str| {
        Err(ContractError::InvalidConfig {
            reason: reason.to_string(),
        })
    };
    if config.denom.is_empty() {
        return invalid("denom is empty");
    }
    if config.interval_seconds == 0 {
        return invalid("interval must be positive");
    }
    if config.buffer_seconds >= config.interval_seconds {
        return invalid("buffer must be shorter than interval");
    }
    if config.min_bet.is_zero() {
        return invalid("min bet must be positive");
    }
    if config.fee_bps > MAX_FEE_BPS {
        return invalid("fee too high");
    }
    if config.max_price_age_seconds == 0 {
        return invalid("max price age must be positive");
    }
    Ok(())
}

fn check_price(
    config: &Config,
    env: &Env,
    price: Decimal,
    publish_time: u64,
) -> Result<(), ContractError> {
    if config.price_source != PriceSource::Operator {
        return Err(ContractError::UnsupportedPriceSource {});
    }
    if price.is_zero() {
        return Err(ContractError::InvalidPrice {});
    }
    let block_time = env.block.time.seconds();
    if block_time.abs_diff(publish_time) > config.max_price_age_seconds {
        return Err(ContractError::StalePrice {
            publish_time,
            block_time,
        });
    }
    Ok(())
}

fn ensure_operator(config: &Config, sender: &Addr) -> Result<(), ContractError> {
    if *sender != config.operator {
        return Err(ContractError::Unauthorized {});
    }
    Ok(())
}

fn ensure_admin(config: &Config, sender: &Addr) -> Result<(), ContractError> {
    if *sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }
    Ok(())
}

fn ensure_not_paused(state: &State) -> Result<(), ContractError> {
    if state.paused {
        return Err(ContractError::Paused {});
    }
    Ok(())
}

fn position_str(p: Position) -> &'static str {
    match p {
        Position::Bull => "bull",
        Position::Bear => "bear",
    }
}

fn status_str(s: &RoundStatus) -> &'static str {
    match s {
        RoundStatus::Open => "open",
        RoundStatus::Locked => "locked",
        RoundStatus::Settled {
            winner: Position::Bull,
        } => "bull",
        RoundStatus::Settled {
            winner: Position::Bear,
        } => "bear",
        RoundStatus::Cancelled {
            reason: CancelReason::OneSided,
        } => "cancelled_one_sided",
        RoundStatus::Cancelled {
            reason: CancelReason::Tie,
        } => "cancelled_tie",
    }
}

// ---------------------------------------------------------------- queries

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&CONFIG.load(deps.storage)?),
        QueryMsg::Status {} => {
            let s = STATE.load(deps.storage)?;
            to_json_binary(&StatusResponse {
                current_epoch: s.current_epoch,
                paused: s.paused,
                genesis_start_once: s.genesis_start_once,
                genesis_lock_once: s.genesis_lock_once,
                treasury_accrued: s.treasury_accrued,
                block_time: env.block.time.seconds(),
            })
        }
        QueryMsg::Round { epoch } => to_json_binary(&ROUNDS.may_load(deps.storage, epoch)?),
        QueryMsg::Rounds { before, limit } => {
            let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
            let rounds = ROUNDS
                .range(
                    deps.storage,
                    None,
                    before.map(Bound::exclusive),
                    Order::Descending,
                )
                .take(limit)
                .map(|r| r.map(|(_, v)| v))
                .collect::<StdResult<Vec<_>>>()?;
            to_json_binary(&rounds)
        }
        QueryMsg::UserBets {
            address,
            before,
            limit,
        } => to_json_binary(&query_user_bets(deps, env, address, before, limit)?),
        QueryMsg::PendingClaims { address, limit } => {
            to_json_binary(&query_pending_claims(deps, env, address, limit)?)
        }
        QueryMsg::UserStats { address } => {
            let addr = deps.api.addr_validate(&address)?;
            to_json_binary(
                &USER_STATS
                    .may_load(deps.storage, &addr)?
                    .unwrap_or_default(),
            )
        }
        QueryMsg::Leaderboard { start_after, limit } => {
            let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
            let start = start_after
                .map(|a| deps.api.addr_validate(&a))
                .transpose()?;
            let entries = USER_STATS
                .range(
                    deps.storage,
                    start.as_ref().map(Bound::exclusive),
                    None,
                    Order::Ascending,
                )
                .take(limit)
                .map(|r| {
                    r.map(|(address, stats)| LeaderboardEntry {
                        address: address.into_string(),
                        stats,
                    })
                })
                .collect::<StdResult<Vec<_>>>()?;
            to_json_binary(&entries)
        }
    }
}

fn user_bet_info(
    config: &Config,
    now: u64,
    epoch: u64,
    bet: Bet,
    round: Round,
) -> UserBetInfo {
    let c = claimable(config, &round, &bet, now);
    let payout = match (&c, &round.status) {
        (Some(c), _) => c.amount,
        (None, RoundStatus::Settled { winner }) if *winner == bet.position => bet
            .amount
            .multiply_ratio(round.reward_amount, round.reward_base),
        (None, RoundStatus::Cancelled { .. }) => bet.amount,
        _ => Uint128::zero(),
    };
    UserBetInfo {
        epoch,
        claimable: matches!(&c, Some(c) if !c.refund),
        refundable: matches!(&c, Some(c) if c.refund),
        payout,
        bet,
        round,
    }
}

fn query_user_bets(
    deps: Deps,
    env: Env,
    address: String,
    before: Option<u64>,
    limit: Option<u32>,
) -> StdResult<Vec<UserBetInfo>> {
    let config = CONFIG.load(deps.storage)?;
    let addr = deps.api.addr_validate(&address)?;
    let now = env.block.time.seconds();
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    USER_ROUNDS
        .prefix(&addr)
        .keys(
            deps.storage,
            None,
            before.map(Bound::exclusive),
            Order::Descending,
        )
        .take(limit)
        .map(|epoch| {
            let epoch = epoch?;
            let bet = BETS.load(deps.storage, (epoch, &addr))?;
            let round = ROUNDS.load(deps.storage, epoch)?;
            Ok(user_bet_info(&config, now, epoch, bet, round))
        })
        .collect()
}

fn query_pending_claims(
    deps: Deps,
    env: Env,
    address: String,
    limit: Option<u32>,
) -> StdResult<PendingClaimsResponse> {
    let config = CONFIG.load(deps.storage)?;
    let addr = deps.api.addr_validate(&address)?;
    let now = env.block.time.seconds();
    let limit = limit.unwrap_or(MAX_PENDING_SCAN).min(MAX_PENDING_SCAN) as usize;
    let mut epochs = vec![];
    let mut total = Uint128::zero();
    for epoch in USER_ROUNDS
        .prefix(&addr)
        .keys(deps.storage, None, None, Order::Descending)
        .take(limit)
    {
        let epoch = epoch?;
        let bet = BETS.load(deps.storage, (epoch, &addr))?;
        let round = ROUNDS.load(deps.storage, epoch)?;
        if let Some(c) = claimable(&config, &round, &bet, now) {
            epochs.push(epoch);
            total += c.amount;
        }
    }
    epochs.reverse();
    Ok(PendingClaimsResponse { epochs, total })
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    Ok(Response::new().add_attribute("action", "migrate"))
}
