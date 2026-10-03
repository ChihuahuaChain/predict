use std::str::FromStr;

use cosmwasm_std::{coin, coins, Addr, Decimal, Empty, Uint128};
use cw_multi_test::{App, AppResponse, Contract, ContractWrapper, Executor};

use crate::contract::{execute, instantiate, migrate, query};
use crate::msg::{
    ExecuteMsg, InstantiateMsg, LeaderboardEntry, PendingClaimsResponse, QueryMsg,
    StatusResponse, UserBetInfo,
};
use crate::state::{CancelReason, Position, Round, RoundStatus, UserStats};
use crate::ContractError;

const USDC: &str = "ibc/usdc";
const INTERVAL: u64 = 300;
const BUFFER: u64 = 30;
const USDC_UNIT: u128 = 1_000_000;

fn contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(execute, instantiate, query).with_migrate(migrate))
}

struct Suite {
    app: App,
    addr: Addr,
    admin: Addr,
    operator: Addr,
    treasury: Addr,
    players: Vec<Addr>,
}

fn usdc(n: u128) -> u128 {
    n * USDC_UNIT
}

fn price(p: &str) -> Decimal {
    Decimal::from_str(p).unwrap()
}

impl Suite {
    fn new() -> Self {
        let mut app = App::default();
        let api = app.api();
        let admin = api.addr_make("admin");
        let operator = api.addr_make("operator");
        let treasury = api.addr_make("treasury");
        let players: Vec<Addr> = ["alice", "bob", "carol", "dave"]
            .iter()
            .map(|n| api.addr_make(n))
            .collect();
        app.init_modules(|router, _, storage| {
            for p in &players {
                router
                    .bank
                    .init_balance(
                        storage,
                        p,
                        vec![coin(usdc(1000), USDC), coin(1000, "uhuahua")],
                    )
                    .unwrap();
            }
        });
        let code_id = app.store_code(contract());
        let addr = app
            .instantiate_contract(
                code_id,
                admin.clone(),
                &InstantiateMsg {
                    denom: USDC.to_string(),
                    market: "BTC".to_string(),
                    interval_seconds: INTERVAL,
                    buffer_seconds: BUFFER,
                    min_bet: Uint128::new(USDC_UNIT),
                    fee_bps: 200,
                    max_price_age_seconds: 60,
                    admin: None,
                    operator: operator.to_string(),
                    treasury: treasury.to_string(),
                },
                &[],
                "prediction-btc",
                Some(admin.to_string()),
            )
            .unwrap();
        Suite {
            app,
            addr,
            admin,
            operator,
            treasury,
            players,
        }
    }

    fn now(&self) -> u64 {
        self.app.block_info().time.seconds()
    }

    fn advance(&mut self, secs: u64) {
        self.app.update_block(|b| {
            b.time = b.time.plus_seconds(secs);
            b.height += secs / 6 + 1;
        });
    }

    fn advance_to(&mut self, ts: u64) {
        let now = self.now();
        assert!(ts >= now, "cannot go back in time");
        self.advance(ts - now);
    }

    fn exec(&mut self, sender: &Addr, msg: &ExecuteMsg) -> Result<AppResponse, ContractError> {
        self.app
            .execute_contract(sender.clone(), self.addr.clone(), msg, &[])
            .map_err(|e| e.downcast().unwrap())
    }

    fn bet(
        &mut self,
        player: usize,
        epoch: u64,
        pos: Position,
        amount: u128,
    ) -> Result<AppResponse, ContractError> {
        let msg = match pos {
            Position::Bull => ExecuteMsg::BetBull { epoch },
            Position::Bear => ExecuteMsg::BetBear { epoch },
        };
        let sender = self.players[player].clone();
        self.app
            .execute_contract(sender, self.addr.clone(), &msg, &coins(amount, USDC))
            .map_err(|e| e.downcast().unwrap())
    }

    fn operator_msg(&mut self, msg: ExecuteMsg) -> Result<AppResponse, ContractError> {
        let op = self.operator.clone();
        self.exec(&op, &msg)
    }

    /// Genesis start + genesis lock at `p`: round 1 locked, round 2 open.
    fn genesis(&mut self, p: &str) {
        self.operator_msg(ExecuteMsg::GenesisStartRound {}).unwrap();
        self.advance(INTERVAL);
        let now = self.now();
        self.operator_msg(ExecuteMsg::GenesisLockRound {
            price: price(p),
            publish_time: now,
        })
        .unwrap();
    }

    /// Waits for the current round's lock time and executes at `p`.
    fn execute_at(&mut self, p: &str) -> Result<AppResponse, ContractError> {
        let lock_ts = self.round(self.status().current_epoch).lock_ts;
        self.advance_to(lock_ts);
        let now = self.now();
        self.operator_msg(ExecuteMsg::ExecuteRound {
            price: price(p),
            publish_time: now,
        })
    }

    fn claim(&mut self, player: usize, epochs: Vec<u64>) -> Result<AppResponse, ContractError> {
        let sender = self.players[player].clone();
        self.exec(&sender, &ExecuteMsg::Claim { epochs })
    }

    fn status(&self) -> StatusResponse {
        self.app
            .wrap()
            .query_wasm_smart(&self.addr, &QueryMsg::Status {})
            .unwrap()
    }

    fn round(&self, epoch: u64) -> Round {
        let r: Option<Round> = self
            .app
            .wrap()
            .query_wasm_smart(&self.addr, &QueryMsg::Round { epoch })
            .unwrap();
        r.unwrap()
    }

    fn balance(&self, addr: &Addr) -> u128 {
        self.app
            .wrap()
            .query_balance(addr, USDC)
            .unwrap()
            .amount
            .u128()
    }

    fn player_balance(&self, player: usize) -> u128 {
        self.balance(&self.players[player])
    }

    fn contract_balance(&self) -> u128 {
        self.balance(&self.addr)
    }
}

#[test]
fn genesis_flow() {
    let mut s = Suite::new();
    assert_eq!(s.status().current_epoch, 0);
    s.genesis("100");
    let st = s.status();
    assert_eq!(st.current_epoch, 2);
    let r1 = s.round(1);
    assert_eq!(r1.status, RoundStatus::Locked);
    assert_eq!(r1.lock_price, Some(price("100")));
    assert_eq!(r1.close_ts, s.now() + INTERVAL);
    let r2 = s.round(2);
    assert_eq!(r2.status, RoundStatus::Open);
    assert_eq!(r2.lock_ts, s.now() + INTERVAL);

    // genesis cannot run twice
    assert_eq!(
        s.operator_msg(ExecuteMsg::GenesisStartRound {}).unwrap_err(),
        ContractError::GenesisStartDone {}
    );
}

#[test]
fn bull_wins_with_fee() {
    let mut s = Suite::new();
    s.genesis("100");
    s.bet(0, 2, Position::Bull, usdc(10)).unwrap();
    s.bet(1, 2, Position::Bear, usdc(30)).unwrap();
    s.bet(2, 2, Position::Bull, usdc(20)).unwrap();

    // lock round 2 at 100 (round 1 had no bets: cancelled)
    s.execute_at("100").unwrap();
    assert_eq!(
        s.round(1).status,
        RoundStatus::Cancelled {
            reason: CancelReason::OneSided
        }
    );
    // close round 2 at 110: bull wins
    s.execute_at("110").unwrap();
    let r2 = s.round(2);
    assert_eq!(
        r2.status,
        RoundStatus::Settled {
            winner: Position::Bull
        }
    );
    // 2% of 60 = 1.2 USDC
    assert_eq!(r2.fee_amount.u128(), 1_200_000);
    assert_eq!(r2.reward_amount.u128(), 58_800_000);
    assert_eq!(s.status().treasury_accrued.u128(), 1_200_000);

    let before_a = s.player_balance(0);
    s.claim(0, vec![2]).unwrap();
    assert_eq!(s.player_balance(0) - before_a, 19_600_000); // 10 * 58.8 / 30
    let before_c = s.player_balance(2);
    s.claim(2, vec![2]).unwrap();
    assert_eq!(s.player_balance(2) - before_c, 39_200_000);

    // loser cannot claim, winner cannot claim twice
    assert_eq!(
        s.claim(1, vec![2]).unwrap_err(),
        ContractError::NotClaimable { epoch: 2 }
    );
    assert_eq!(
        s.claim(0, vec![2]).unwrap_err(),
        ContractError::AlreadyClaimed { epoch: 2 }
    );

    // house takes exactly its 2%
    let admin = s.admin.clone();
    s.exec(&admin, &ExecuteMsg::ClaimTreasury {}).unwrap();
    assert_eq!(s.balance(&s.treasury.clone()), 1_200_000);
    assert_eq!(s.contract_balance(), 0);
    assert_eq!(
        s.exec(&admin, &ExecuteMsg::ClaimTreasury {}).unwrap_err(),
        ContractError::NothingToClaim {}
    );
}

#[test]
fn bear_wins() {
    let mut s = Suite::new();
    s.genesis("2500");
    s.bet(0, 2, Position::Bull, usdc(50)).unwrap();
    s.bet(1, 2, Position::Bear, usdc(50)).unwrap();
    s.execute_at("2500").unwrap();
    s.execute_at("2499.99").unwrap();
    assert_eq!(
        s.round(2).status,
        RoundStatus::Settled {
            winner: Position::Bear
        }
    );
    let before = s.player_balance(1);
    s.claim(1, vec![2]).unwrap();
    assert_eq!(s.player_balance(1) - before, usdc(98));
    assert_eq!(
        s.claim(0, vec![2]).unwrap_err(),
        ContractError::NotClaimable { epoch: 2 }
    );
}

#[test]
fn one_sided_round_refunds_without_fee() {
    let mut s = Suite::new();
    s.genesis("100");
    // two players, but both on the same side: nobody to play against
    s.bet(0, 2, Position::Bull, usdc(10)).unwrap();
    s.bet(1, 2, Position::Bull, usdc(5)).unwrap();
    s.execute_at("100").unwrap();
    s.execute_at("90").unwrap();
    let r2 = s.round(2);
    assert_eq!(
        r2.status,
        RoundStatus::Cancelled {
            reason: CancelReason::OneSided
        }
    );
    assert!(r2.fee_amount.is_zero());
    assert!(s.status().treasury_accrued.is_zero());

    s.claim(0, vec![2]).unwrap();
    s.claim(1, vec![2]).unwrap();
    assert_eq!(s.player_balance(0), usdc(1000));
    assert_eq!(s.player_balance(1), usdc(1000));
    assert_eq!(s.contract_balance(), 0);

    // a refunded round does not count as played
    let stats: UserStats = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.addr,
            &QueryMsg::UserStats {
                address: s.players[0].to_string(),
            },
        )
        .unwrap();
    assert_eq!(stats, UserStats::default());
}

#[test]
fn single_player_refund() {
    let mut s = Suite::new();
    s.genesis("100");
    s.bet(0, 2, Position::Bear, usdc(3)).unwrap();
    s.execute_at("100").unwrap();
    s.execute_at("90").unwrap();
    s.claim(0, vec![2]).unwrap();
    assert_eq!(s.player_balance(0), usdc(1000));
}

#[test]
fn tie_refunds_without_fee() {
    let mut s = Suite::new();
    s.genesis("100");
    s.bet(0, 2, Position::Bull, usdc(10)).unwrap();
    s.bet(1, 2, Position::Bear, usdc(20)).unwrap();
    s.execute_at("100.5").unwrap();
    s.execute_at("100.5").unwrap();
    assert_eq!(
        s.round(2).status,
        RoundStatus::Cancelled {
            reason: CancelReason::Tie
        }
    );
    s.claim(0, vec![2]).unwrap();
    s.claim(1, vec![2]).unwrap();
    assert_eq!(s.player_balance(0), usdc(1000));
    assert_eq!(s.player_balance(1), usdc(1000));
    assert!(s.status().treasury_accrued.is_zero());
}

#[test]
fn bet_validation() {
    let mut s = Suite::new();
    // nothing started yet
    assert_eq!(
        s.bet(0, 1, Position::Bull, usdc(1)).unwrap_err(),
        ContractError::WrongEpoch { epoch: 1 }
    );
    s.genesis("100");
    assert_eq!(
        s.bet(0, 1, Position::Bull, usdc(1)).unwrap_err(),
        ContractError::WrongEpoch { epoch: 1 }
    );
    assert_eq!(
        s.bet(0, 3, Position::Bull, usdc(1)).unwrap_err(),
        ContractError::WrongEpoch { epoch: 3 }
    );
    assert_eq!(
        s.bet(0, 2, Position::Bull, 999_999).unwrap_err(),
        ContractError::BetTooSmall {
            min: "1000000".to_string()
        }
    );
    // wrong denom
    let alice = s.players[0].clone();
    let err: ContractError = s
        .app
        .execute_contract(
            alice.clone(),
            s.addr.clone(),
            &ExecuteMsg::BetBull { epoch: 2 },
            &coins(1000, "uhuahua"),
        )
        .unwrap_err()
        .downcast()
        .unwrap();
    assert!(matches!(err, ContractError::Payment(_)));
    // no funds
    let err: ContractError = s
        .app
        .execute_contract(alice, s.addr.clone(), &ExecuteMsg::BetBull { epoch: 2 }, &[])
        .unwrap_err()
        .downcast()
        .unwrap();
    assert!(matches!(err, ContractError::Payment(_)));

    s.bet(0, 2, Position::Bull, usdc(1)).unwrap();
    // one bet per round, even on the other side
    assert_eq!(
        s.bet(0, 2, Position::Bear, usdc(1)).unwrap_err(),
        ContractError::AlreadyBet { epoch: 2 }
    );

    // too late: lock time reached but keeper not executed yet
    let lock_ts = s.round(2).lock_ts;
    s.advance_to(lock_ts);
    assert_eq!(
        s.bet(1, 2, Position::Bear, usdc(1)).unwrap_err(),
        ContractError::RoundNotBettable { epoch: 2 }
    );

    // paused
    let op = s.operator.clone();
    s.exec(&op, &ExecuteMsg::Pause {}).unwrap();
    assert_eq!(
        s.bet(1, 2, Position::Bear, usdc(1)).unwrap_err(),
        ContractError::Paused {}
    );
}

#[test]
fn missed_window_refunds_and_regenesis() {
    let mut s = Suite::new();
    s.genesis("100");
    s.bet(0, 2, Position::Bull, usdc(10)).unwrap();
    s.bet(1, 2, Position::Bear, usdc(10)).unwrap();
    s.execute_at("100").unwrap(); // round 2 locked, round 3 open
    s.bet(2, 3, Position::Bull, usdc(7)).unwrap();

    // keeper goes silent past round 2's close + buffer
    let r2 = s.round(2);
    s.advance_to(r2.close_ts + BUFFER + 1);
    let now = s.now();
    let err = s
        .operator_msg(ExecuteMsg::ExecuteRound {
            price: price("120"),
            publish_time: now,
        })
        .unwrap_err();
    assert!(matches!(err, ContractError::LockWindow { .. }));

    // round 2 (locked) is refundable now
    s.claim(0, vec![2]).unwrap();
    s.claim(1, vec![2]).unwrap();
    assert_eq!(s.player_balance(0), usdc(1000));
    assert_eq!(s.player_balance(1), usdc(1000));
    // round 3 (open, never locked) only after its close + buffer
    assert_eq!(
        s.claim(2, vec![3]).unwrap_err(),
        ContractError::NotClaimable { epoch: 3 }
    );
    let r3 = s.round(3);
    s.advance_to(r3.close_ts + BUFFER + 1);
    s.claim(2, vec![3]).unwrap();
    assert_eq!(s.player_balance(2), usdc(1000));

    // recovery: pause, unpause, genesis again from the next epoch
    let op = s.operator.clone();
    let alice = s.players[0].clone();
    s.exec(&op, &ExecuteMsg::Pause {}).unwrap();
    assert_eq!(
        s.exec(&alice, &ExecuteMsg::Unpause {}).unwrap_err(),
        ContractError::Unauthorized {}
    );
    s.exec(&op, &ExecuteMsg::Unpause {}).unwrap();
    s.genesis("130");
    assert_eq!(s.status().current_epoch, 5);
    s.bet(0, 5, Position::Bull, usdc(1)).unwrap();
    s.bet(1, 5, Position::Bear, usdc(1)).unwrap();
    s.execute_at("130").unwrap();
    s.execute_at("131").unwrap();
    assert_eq!(
        s.round(5).status,
        RoundStatus::Settled {
            winner: Position::Bull
        }
    );
    assert_eq!(s.contract_balance(), usdc(2));
}

#[test]
fn late_lock_shifts_close() {
    let mut s = Suite::new();
    s.genesis("100");
    let lock_ts = s.round(2).lock_ts;
    s.advance_to(lock_ts + 20); // inside buffer
    let now = s.now();
    s.operator_msg(ExecuteMsg::ExecuteRound {
        price: price("100"),
        publish_time: now,
    })
    .unwrap();
    assert_eq!(s.round(2).close_ts, now + INTERVAL);
}

#[test]
fn price_checks() {
    let mut s = Suite::new();
    s.operator_msg(ExecuteMsg::GenesisStartRound {}).unwrap();
    s.advance(INTERVAL);
    let now = s.now();
    assert_eq!(
        s.operator_msg(ExecuteMsg::GenesisLockRound {
            price: Decimal::zero(),
            publish_time: now,
        })
        .unwrap_err(),
        ContractError::InvalidPrice {}
    );
    assert_eq!(
        s.operator_msg(ExecuteMsg::GenesisLockRound {
            price: price("1"),
            publish_time: now - 61,
        })
        .unwrap_err(),
        ContractError::StalePrice {
            publish_time: now - 61,
            block_time: now
        }
    );
}

#[test]
fn permissions() {
    let mut s = Suite::new();
    let alice = s.players[0].clone();
    let admin = s.admin.clone();
    assert_eq!(
        s.exec(&alice, &ExecuteMsg::GenesisStartRound {})
            .unwrap_err(),
        ContractError::Unauthorized {}
    );
    // admin is not the operator
    assert_eq!(
        s.exec(&admin, &ExecuteMsg::GenesisStartRound {})
            .unwrap_err(),
        ContractError::Unauthorized {}
    );
    assert_eq!(
        s.exec(&alice, &ExecuteMsg::Pause {}).unwrap_err(),
        ContractError::Unauthorized {}
    );
    assert_eq!(
        s.exec(&alice, &ExecuteMsg::ClaimTreasury {}).unwrap_err(),
        ContractError::Unauthorized {}
    );
    let fee = |bps| ExecuteMsg::UpdateConfig {
        interval_seconds: None,
        buffer_seconds: None,
        min_bet: None,
        fee_bps: Some(bps),
        max_price_age_seconds: None,
        treasury: None,
    };
    assert_eq!(
        s.exec(&admin, &fee(300)).unwrap_err(),
        ContractError::NotPaused {}
    );
    s.exec(&admin, &ExecuteMsg::Pause {}).unwrap();
    assert_eq!(
        s.exec(&alice, &fee(300)).unwrap_err(),
        ContractError::Unauthorized {}
    );
    s.exec(&admin, &fee(300)).unwrap();
    assert!(matches!(
        s.exec(&admin, &fee(1001)).unwrap_err(),
        ContractError::InvalidConfig { .. }
    ));

    // two-step admin handover
    let bob = s.players[1].clone();
    s.exec(
        &admin,
        &ExecuteMsg::ProposeAdmin {
            admin: bob.to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        s.exec(&alice, &ExecuteMsg::AcceptAdmin {}).unwrap_err(),
        ContractError::Unauthorized {}
    );
    s.exec(&bob, &ExecuteMsg::AcceptAdmin {}).unwrap();
    assert_eq!(
        s.exec(&admin, &ExecuteMsg::Unpause {}).unwrap_err(),
        ContractError::Unauthorized {}
    );
    s.exec(&bob, &ExecuteMsg::Unpause {}).unwrap();

    // new operator
    s.exec(
        &bob,
        &ExecuteMsg::SetOperator {
            operator: alice.to_string(),
        },
    )
    .unwrap();
    s.exec(&alice, &ExecuteMsg::GenesisStartRound {}).unwrap();
}

#[test]
fn rounding_keeps_contract_solvent() {
    let mut s = Suite::new();
    s.genesis("100");
    s.bet(0, 2, Position::Bull, 1_000_001).unwrap();
    s.bet(1, 2, Position::Bull, 3_333_333).unwrap();
    s.bet(2, 2, Position::Bull, 1_234_567).unwrap();
    s.bet(3, 2, Position::Bear, 7_777_777).unwrap();
    s.execute_at("100").unwrap();
    s.execute_at("100.000000000000000001").unwrap();

    let r2 = s.round(2);
    let treasury = s.status().treasury_accrued.u128();
    let mut paid = 0;
    for p in 0..3 {
        let before = s.player_balance(p);
        s.claim(p, vec![2]).unwrap();
        paid += s.player_balance(p) - before;
    }
    // winners never receive more than the pot; any dust stays in the contract
    assert!(paid <= r2.reward_amount.u128());
    assert!(r2.reward_amount.u128() - paid < 3);
    assert_eq!(treasury + r2.reward_amount.u128(), r2.total_amount.u128());
    assert!(s.contract_balance() >= treasury);
}

#[test]
fn queries_for_ui() {
    let mut s = Suite::new();
    s.genesis("100");
    s.bet(0, 2, Position::Bull, usdc(10)).unwrap();
    s.bet(1, 2, Position::Bear, usdc(10)).unwrap();
    s.execute_at("100").unwrap();
    s.bet(0, 3, Position::Bull, usdc(4)).unwrap();
    s.execute_at("105").unwrap(); // round 2: bull wins; round 3 (one-sided) locked
    s.execute_at("104").unwrap(); // round 3 cancelled

    let bets: Vec<UserBetInfo> = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.addr,
            &QueryMsg::UserBets {
                address: s.players[0].to_string(),
                before: None,
                limit: None,
            },
        )
        .unwrap();
    assert_eq!(bets.len(), 2);
    assert_eq!(bets[0].epoch, 3);
    assert!(bets[0].refundable && !bets[0].claimable);
    assert_eq!(bets[0].payout.u128(), usdc(4));
    assert_eq!(bets[1].epoch, 2);
    assert!(bets[1].claimable && !bets[1].refundable);
    assert_eq!(bets[1].payout.u128(), 19_600_000);

    let pending: PendingClaimsResponse = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.addr,
            &QueryMsg::PendingClaims {
                address: s.players[0].to_string(),
                limit: None,
            },
        )
        .unwrap();
    assert_eq!(pending.epochs, vec![2, 3]);
    assert_eq!(pending.total.u128(), 19_600_000 + usdc(4));
    s.claim(0, pending.epochs).unwrap();

    let rounds: Vec<Round> = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.addr,
            &QueryMsg::Rounds {
                before: None,
                limit: Some(3),
            },
        )
        .unwrap();
    assert_eq!(
        rounds.iter().map(|r| r.epoch).collect::<Vec<_>>(),
        vec![5, 4, 3]
    );

    let board: Vec<LeaderboardEntry> = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.addr,
            &QueryMsg::Leaderboard {
                start_after: None,
                limit: None,
            },
        )
        .unwrap();
    assert_eq!(board.len(), 2);
    let alice = board
        .iter()
        .find(|e| e.address == s.players[0].as_str())
        .unwrap();
    assert_eq!(alice.stats.rounds_played, 1);
    assert_eq!(alice.stats.rounds_won, 1);
    assert_eq!(alice.stats.total_bet.u128(), usdc(10));
    assert_eq!(alice.stats.total_payout.u128(), 19_600_000);
}
