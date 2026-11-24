//! Tests of the escrow contract. They run on OdraVM (`cargo odra test`) and on the Casper VM
//! (`cargo odra test -b casper`). On the Casper VM a caller pays gas in CSPR, so the CSPR tests
//! look at the balances of the accounts that did not call.

use odra::casper_types::{U256, U512};
use odra::host::{Deployer, HostEnv, HostRef, NoArgs};
use odra::prelude::ExecutionError;
use odra::prelude::*;

use crate::escrow::*;
use crate::testing::*;

const HOUR: u64 = 60 * 60 * 1000;
const DAY: u64 = 24 * HOUR;
/// The delivery deadline of a test deal, counted from its creation.
const DELIVERY: u64 = 10 * DAY;
const REVIEW: u64 = 7 * DAY;
const DISPUTE: u64 = 14 * DAY;
const AMOUNT: u64 = 1_000;
const FEE: u64 = 50;
const SELLER_BPS: u32 = 3_000;

struct Setup {
    env: HostEnv,
    token: TestTokenHostRef,
    escrow: EscrowHostRef,
    buyer: Address,
    seller: Address,
    arbiter: Address,
    stranger: Address,
    /// The account commissions are paid to.
    collector: Address,
}

/// Deploys the contracts and gives the buyer tokens and an allowance for all of them.
fn setup() -> Setup {
    setup_with_commission(0)
}

/// Deploys the contracts with the given commission, paid to account 5.
fn setup_with_commission(commission_bps: u32) -> Setup {
    let env = odra_test::env();
    let buyer = env.get_account(1);
    let seller = env.get_account(2);
    let arbiter = env.get_account(3);
    let stranger = env.get_account(4);
    let mut token = TestToken::deploy(&env, NoArgs);
    let collector = env.get_account(5);
    let escrow = Escrow::deploy(
        &env,
        EscrowInitArgs {
            commission_bps,
            commission_recipient: collector,
        },
    );

    env.set_caller(buyer);
    token.faucet();
    token.approve(&escrow.address(), &TestToken::faucet_amount());

    Setup {
        env,
        token,
        escrow,
        buyer,
        seller,
        arbiter,
        stranger,
        collector,
    }
}

/// The arguments of `create_deal`, with the values most tests use.
struct New {
    seller: Address,
    arbiter: Option<Address>,
    token: Option<Address>,
    deposit: U256,
    fee: U256,
    bps: u32,
    deadline: u64,
    review: u64,
    dispute: u64,
    memo: String,
}

impl New {
    /// A token deal of 1000 units with an arbiter who takes 50.
    fn token(s: &Setup) -> Self {
        New {
            seller: s.seller,
            arbiter: Some(s.arbiter),
            token: Some(s.token.address()),
            deposit: U256::from(AMOUNT),
            fee: U256::from(FEE),
            bps: SELLER_BPS,
            deadline: s.env.block_time() + DELIVERY,
            review: REVIEW,
            dispute: DISPUTE,
            memo: String::from("Logo design, invoice 12/2026"),
        }
    }

    /// The same deal in CSPR.
    fn cspr(s: &Setup) -> Self {
        New {
            token: None,
            ..Self::token(s)
        }
    }

    /// Sends `create_deal` from the buyer, attaching the amount if the deal is in CSPR.
    fn try_create(self, s: &mut Setup) -> OdraResult<u64> {
        let attached = if self.token.is_none() {
            to_u512(self.deposit)
        } else {
            U512::zero()
        };
        self.try_create_with(s, attached)
    }

    fn try_create_with(self, s: &mut Setup, attached: U512) -> OdraResult<u64> {
        let id = s.escrow.deals_count();
        s.env.set_caller(s.buyer);
        s.escrow
            .with_tokens(attached)
            .try_create_deal(
                self.seller,
                self.arbiter,
                self.token,
                self.deposit,
                self.fee,
                self.bps,
                self.deadline,
                self.review,
                self.dispute,
                self.memo,
            )
            .map(|_| id)
    }

    fn create(self, s: &mut Setup) -> u64 {
        self.try_create(s).expect("create_deal failed")
    }
}

fn to_u512(value: U256) -> U512 {
    U512::from_dec_str(&value.to_string()).unwrap()
}

fn err<T>(error: impl Into<OdraError>) -> OdraResult<T> {
    Err(error.into())
}

fn deliver(s: &mut Setup, id: u64) {
    s.env.set_caller(s.seller);
    s.escrow.mark_delivered(id);
}

fn dispute(s: &mut Setup, id: u64) {
    s.env.set_caller(s.buyer);
    s.escrow.dispute(id);
}

/// Token balances of the buyer, the seller, the arbiter and the contract.
fn balances(s: &Setup) -> [U256; 4] {
    [
        s.token.balance_of(&s.buyer),
        s.token.balance_of(&s.seller),
        s.token.balance_of(&s.arbiter),
        s.token.balance_of(&s.escrow.address()),
    ]
}

fn after_deposit_plus(to_buyer: u64, to_seller: u64, to_arbiter: u64, held: u64) -> [U256; 4] {
    [
        TestToken::faucet_amount() - U256::from(AMOUNT) + U256::from(to_buyer),
        U256::from(to_seller),
        U256::from(to_arbiter),
        U256::from(held),
    ]
}

// ---------------------------------------------------------------------------------------------
// Creating a deal
// ---------------------------------------------------------------------------------------------

#[test]
fn creating_a_token_deal_takes_the_deposit() {
    let mut s = setup();
    let deadline = s.env.block_time() + DELIVERY;
    let now = s.env.block_time();
    let id = New::token(&s).create(&mut s);

    assert_eq!(id, 0);
    assert_eq!(s.escrow.deals_count(), 1);
    assert_eq!(balances(&s), after_deposit_plus(0, 0, 0, AMOUNT));

    let deal = s.escrow.deal(id);
    assert_eq!(deal.buyer, s.buyer);
    assert_eq!(deal.seller, s.seller);
    assert_eq!(deal.arbiter, Some(s.arbiter));
    assert_eq!(deal.token, Some(s.token.address()));
    assert_eq!(deal.deposit, U256::from(AMOUNT));
    assert_eq!(deal.arbiter_fee, U256::from(FEE));
    assert_eq!(deal.default_seller_bps, SELLER_BPS);
    assert_eq!(deal.delivery_deadline, deadline);
    assert_eq!(deal.review_period, REVIEW);
    assert_eq!(deal.dispute_period, DISPUTE);
    assert_eq!(deal.memo, "Logo design, invoice 12/2026");
    assert_eq!(deal.state, DealState::Funded);
    assert_eq!(deal.created_at, now);
    assert_eq!(
        (deal.delivered_at, deal.disputed_at, deal.closed_at),
        (0, 0, 0)
    );

    assert!(s.env.emitted_event(
        &s.escrow,
        DealCreated {
            deal_id: 0,
            buyer: s.buyer,
            seller: s.seller,
            arbiter: Some(s.arbiter),
            token: Some(s.token.address()),
            deposit: U256::from(AMOUNT),
            arbiter_fee: U256::from(FEE),
            default_seller_bps: SELLER_BPS,
            delivery_deadline: deadline,
            review_period: REVIEW,
            dispute_period: DISPUTE,
            memo: String::from("Logo design, invoice 12/2026"),
        }
    ));
}

#[test]
fn creating_a_cspr_deal_takes_the_attached_amount() {
    let mut s = setup();
    let id = New::cspr(&s).create(&mut s);

    assert_eq!(s.env.balance_of(&s.escrow), U512::from(AMOUNT));
    let deal = s.escrow.deal(id);
    assert_eq!(deal.token, None);
    assert_eq!(deal.deposit, U256::from(AMOUNT));
    // No token moved.
    assert_eq!(s.token.balance_of(&s.buyer), TestToken::faucet_amount());
}

#[test]
fn a_cspr_deal_needs_exactly_its_amount_attached() {
    let mut s = setup();
    for attached in [0, AMOUNT - 1, AMOUNT + 1] {
        assert_eq!(
            New::cspr(&s).try_create_with(&mut s, U512::from(attached)),
            err(Error::WrongAttachedValue)
        );
    }
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

#[test]
fn a_cspr_deal_has_no_contracts_as_parties() {
    // Casper pays CSPR to accounts only: a payout to a contract would fail.
    let mut s = setup();
    let contract = s.token.address();
    let seller = New {
        seller: contract,
        ..New::cspr(&s)
    };
    assert_eq!(seller.try_create(&mut s), err(Error::ContractInCsprDeal));
    let arbiter = New {
        arbiter: Some(contract),
        ..New::cspr(&s)
    };
    assert_eq!(arbiter.try_create(&mut s), err(Error::ContractInCsprDeal));

    // A token deal can have them.
    New {
        seller: contract,
        ..New::token(&s)
    }
    .create(&mut s);
}

#[test]
fn a_token_deal_takes_no_cspr() {
    let mut s = setup();
    assert_eq!(
        New::token(&s).try_create_with(&mut s, U512::from(1)),
        err(Error::UnexpectedAttachedValue)
    );
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

#[test]
fn a_deal_without_an_arbiter_can_be_created() {
    let mut s = setup();
    let id = New {
        arbiter: None,
        fee: U256::zero(),
        ..New::token(&s)
    }
    .create(&mut s);
    assert_eq!(s.escrow.deal(id).arbiter, None);
}

#[test]
fn wrong_arguments_are_rejected() {
    let mut s = setup();
    let now = s.env.block_time();
    let cases: Vec<(New, Error)> = vec![
        (
            New {
                deposit: U256::zero(),
                fee: U256::zero(),
                ..New::token(&s)
            },
            Error::ZeroDeposit,
        ),
        (
            New {
                deposit: U256::MAX / U256::from(BPS) + 1,
                ..New::token(&s)
            },
            Error::DepositTooLarge,
        ),
        (
            New {
                seller: s.buyer,
                ..New::token(&s)
            },
            Error::SameParty,
        ),
        (
            New {
                arbiter: Some(s.buyer),
                ..New::token(&s)
            },
            Error::SameParty,
        ),
        (
            New {
                arbiter: Some(s.seller),
                ..New::token(&s)
            },
            Error::SameParty,
        ),
        (
            New {
                arbiter: None,
                ..New::token(&s)
            },
            Error::FeeWithoutArbiter,
        ),
        (
            New {
                fee: U256::from(AMOUNT),
                ..New::token(&s)
            },
            Error::FeeTooLarge,
        ),
        (
            New {
                bps: BPS + 1,
                ..New::token(&s)
            },
            Error::BpsTooLarge,
        ),
        (
            New {
                deadline: now,
                ..New::token(&s)
            },
            Error::DeadlineNotInFuture,
        ),
        (
            New {
                deadline: now + MAX_PERIOD + 1,
                ..New::token(&s)
            },
            Error::DeadlineTooFar,
        ),
        (
            New {
                review: MIN_PERIOD - 1,
                ..New::token(&s)
            },
            Error::PeriodTooShort,
        ),
        (
            New {
                dispute: MIN_PERIOD - 1,
                ..New::token(&s)
            },
            Error::PeriodTooShort,
        ),
        (
            New {
                review: MAX_PERIOD + 1,
                ..New::token(&s)
            },
            Error::PeriodTooLong,
        ),
        (
            New {
                dispute: MAX_PERIOD + 1,
                ..New::token(&s)
            },
            Error::PeriodTooLong,
        ),
        (
            New {
                memo: "x".repeat(MAX_MEMO_LEN + 1),
                ..New::token(&s)
            },
            Error::MemoTooLong,
        ),
    ];
    for (case, (new, error)) in cases.into_iter().enumerate() {
        assert_eq!(new.try_create(&mut s), err(error), "case {case}");
    }
    assert_eq!(s.escrow.deals_count(), 0);
    assert_eq!(s.token.balance_of(&s.escrow.address()), U256::zero());
}

#[test]
fn the_limits_themselves_are_accepted() {
    let mut s = setup();
    let now = s.env.block_time();
    New {
        fee: U256::from(AMOUNT - 1),
        bps: BPS,
        deadline: now + MAX_PERIOD,
        review: MIN_PERIOD,
        dispute: MAX_PERIOD,
        memo: "x".repeat(MAX_MEMO_LEN),
        ..New::token(&s)
    }
    .create(&mut s);
}

#[test]
fn a_deposit_without_an_allowance_fails() {
    let mut s = setup();
    s.env.set_caller(s.buyer);
    s.token
        .approve(&s.escrow.address(), &U256::from(AMOUNT - 1));
    assert!(New::token(&s).try_create(&mut s).is_err());
    assert_eq!(s.escrow.deals_count(), 0);
}

// ---------------------------------------------------------------------------------------------
// Who may do what, in which state
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Funded,
    Delivered,
    Disputed,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    MarkDelivered,
    Dispute,
    Release,
    Refund,
    Resolve,
    OfferSplit,
    Settle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Who {
    Buyer,
    Seller,
    Arbiter,
    Stranger,
}

/// What the contract must answer, with every time limit still ahead.
fn expected(stage: Stage, action: Action, who: Who) -> Result<(), Error> {
    use Action::*;
    use Stage::*;
    if stage == Closed {
        return Err(Error::DealClosed);
    }
    match action {
        MarkDelivered if who != Who::Seller => Err(Error::NotSeller),
        MarkDelivered if stage != Funded => Err(Error::NotFunded),
        MarkDelivered => Ok(()),
        Dispute if who != Who::Buyer => Err(Error::NotBuyer),
        Dispute if stage != Delivered => Err(Error::NotDelivered),
        Dispute => Ok(()),
        Release if who != Who::Buyer => Err(Error::NotBuyer),
        Release => Ok(()),
        Refund if who != Who::Seller => Err(Error::NotSeller),
        Refund => Ok(()),
        Resolve if who != Who::Arbiter => Err(Error::NotArbiter),
        Resolve if stage != Disputed => Err(Error::NotDisputed),
        Resolve => Ok(()),
        OfferSplit if who != Who::Buyer && who != Who::Seller => Err(Error::NotParty),
        OfferSplit => Ok(()),
        Settle => Err(Error::TooEarly),
    }
}

#[test]
fn every_action_by_every_account_in_every_state() {
    let mut s = setup();
    for stage in [
        Stage::Funded,
        Stage::Delivered,
        Stage::Disputed,
        Stage::Closed,
    ] {
        for action in [
            Action::MarkDelivered,
            Action::Dispute,
            Action::Release,
            Action::Refund,
            Action::Resolve,
            Action::OfferSplit,
            Action::Settle,
        ] {
            for who in [Who::Buyer, Who::Seller, Who::Arbiter, Who::Stranger] {
                let id = New::token(&s).create(&mut s);
                if stage != Stage::Funded {
                    deliver(&mut s, id);
                }
                if stage == Stage::Disputed {
                    dispute(&mut s, id);
                }
                if stage == Stage::Closed {
                    s.env.set_caller(s.seller);
                    s.escrow.refund(id);
                }

                s.env.set_caller(match who {
                    Who::Buyer => s.buyer,
                    Who::Seller => s.seller,
                    Who::Arbiter => s.arbiter,
                    Who::Stranger => s.stranger,
                });
                let result = match action {
                    Action::MarkDelivered => s.escrow.try_mark_delivered(id),
                    Action::Dispute => s.escrow.try_dispute(id),
                    Action::Release => s.escrow.try_release(id),
                    Action::Refund => s.escrow.try_refund(id),
                    Action::Resolve => s.escrow.try_resolve(id, U256::from(1)),
                    Action::OfferSplit => s.escrow.try_offer_split(id, Some(U256::from(1))),
                    Action::Settle => s.escrow.try_settle(id),
                };
                assert_eq!(
                    result,
                    expected(stage, action, who).map_err(Into::into),
                    "{action:?} by {who:?} on a {stage:?} deal"
                );
            }
        }
    }
}

#[test]
fn a_deal_that_does_not_exist_is_not_found() {
    let mut s = setup();
    s.env.set_caller(s.buyer);
    assert_eq!(s.escrow.try_release(0), err(Error::DealNotFound));
    assert_eq!(s.escrow.try_settle(7), err(Error::DealNotFound));
    assert_eq!(s.escrow.try_deal(0), err(Error::DealNotFound));
}

#[test]
fn a_deal_without_an_arbiter_has_nobody_to_resolve_it() {
    let mut s = setup();
    let id = New {
        arbiter: None,
        fee: U256::zero(),
        ..New::token(&s)
    }
    .create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    for account in [s.buyer, s.seller, s.arbiter, s.stranger] {
        s.env.set_caller(account);
        assert_eq!(
            s.escrow.try_resolve(id, U256::zero()),
            err(Error::NotArbiter)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The paths of a deal, in tokens
// ---------------------------------------------------------------------------------------------

#[test]
fn the_buyer_releases_to_the_seller() {
    for stage in [Stage::Funded, Stage::Delivered, Stage::Disputed] {
        let mut s = setup();
        let id = New::token(&s).create(&mut s);
        if stage != Stage::Funded {
            deliver(&mut s, id);
        }
        if stage == Stage::Disputed {
            dispute(&mut s, id);
        }
        s.env.advance_block_time(HOUR);
        let now = s.env.block_time();
        s.env.set_caller(s.buyer);
        s.escrow.release(id);

        assert_eq!(balances(&s), after_deposit_plus(0, AMOUNT, 0, 0));
        let deal = s.escrow.deal(id);
        assert_eq!(deal.state, DealState::Released);
        assert_eq!(deal.closed_at, now);
        assert_eq!(deal.to_seller, U256::from(AMOUNT));
        assert_eq!(deal.to_buyer, U256::zero());
        assert_eq!(deal.to_arbiter, U256::zero());
        assert!(s.env.emitted_event(
            &s.escrow,
            DealClosed {
                deal_id: id,
                state: DealState::Released,
                to_seller: U256::from(AMOUNT),
                to_buyer: U256::zero(),
                to_arbiter: U256::zero(),
                commission: U256::zero(),
            }
        ));
    }
}

#[test]
fn the_seller_refunds_the_buyer() {
    for stage in [Stage::Funded, Stage::Delivered, Stage::Disputed] {
        let mut s = setup();
        let id = New::token(&s).create(&mut s);
        if stage != Stage::Funded {
            deliver(&mut s, id);
        }
        if stage == Stage::Disputed {
            dispute(&mut s, id);
        }
        s.env.set_caller(s.seller);
        s.escrow.refund(id);

        assert_eq!(balances(&s), after_deposit_plus(AMOUNT, 0, 0, 0));
        let deal = s.escrow.deal(id);
        assert_eq!(deal.state, DealState::Refunded);
        assert_eq!(deal.to_buyer, U256::from(AMOUNT));
    }
}

#[test]
fn delivery_starts_the_review_period() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    s.env.advance_block_time(DAY);
    let now = s.env.block_time();
    deliver(&mut s, id);

    let deal = s.escrow.deal(id);
    assert_eq!(deal.state, DealState::Delivered);
    assert_eq!(deal.delivered_at, now);
    assert!(s.env.emitted_event(
        &s.escrow,
        DealDelivered {
            deal_id: id,
            review_ends_at: now + REVIEW,
        }
    ));
}

#[test]
fn an_undelivered_deal_goes_back_to_the_buyer_at_the_deadline() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);

    // One millisecond before the deadline the seller can still deliver and nobody can settle.
    s.env.advance_block_time(DELIVERY - 1);
    s.env.set_caller(s.stranger);
    assert_eq!(s.escrow.try_settle(id), err(Error::TooEarly));

    s.env.advance_block_time(1);
    s.env.set_caller(s.seller);
    assert_eq!(
        s.escrow.try_mark_delivered(id),
        err(Error::DeliveryDeadlinePassed)
    );
    s.env.set_caller(s.stranger);
    s.escrow.settle(id);

    assert_eq!(balances(&s), after_deposit_plus(AMOUNT, 0, 0, 0));
    assert_eq!(s.escrow.deal(id).state, DealState::Refunded);
}

#[test]
fn the_seller_can_deliver_until_the_last_moment() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    s.env.advance_block_time(DELIVERY - 1);
    deliver(&mut s, id);
    // The review period runs from the delivery, past the delivery deadline.
    s.env.advance_block_time(REVIEW - 1);
    assert_eq!(s.escrow.try_settle(id), err(Error::TooEarly));
    dispute(&mut s, id);
    assert_eq!(s.escrow.deal(id).state, DealState::Disputed);
}

#[test]
fn an_undisputed_delivery_goes_to_the_seller_after_the_review() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);

    s.env.advance_block_time(REVIEW - 1);
    s.env.set_caller(s.stranger);
    assert_eq!(s.escrow.try_settle(id), err(Error::TooEarly));

    s.env.advance_block_time(1);
    s.env.set_caller(s.buyer);
    assert_eq!(s.escrow.try_dispute(id), err(Error::ReviewPeriodPassed));
    s.env.set_caller(s.stranger);
    s.escrow.settle(id);

    assert_eq!(balances(&s), after_deposit_plus(0, AMOUNT, 0, 0));
    assert_eq!(s.escrow.deal(id).state, DealState::Released);
}

#[test]
fn a_dispute_starts_the_dispute_period() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    s.env.advance_block_time(REVIEW - 1);
    let now = s.env.block_time();
    dispute(&mut s, id);

    let deal = s.escrow.deal(id);
    assert_eq!(deal.state, DealState::Disputed);
    assert_eq!(deal.disputed_at, now);
    assert!(s.env.emitted_event(
        &s.escrow,
        DealDisputed {
            deal_id: id,
            dispute_ends_at: now + DISPUTE,
        }
    ));
}

#[test]
fn the_arbiter_splits_the_deposit_and_takes_the_fee() {
    // The seller's share, and what the buyer gets back: the two add up to the amount less the fee.
    for (share, back) in [
        (0, AMOUNT - FEE),
        (600, AMOUNT - FEE - 600),
        (AMOUNT - FEE, 0),
    ] {
        let mut s = setup();
        let id = New::token(&s).create(&mut s);
        deliver(&mut s, id);
        dispute(&mut s, id);
        s.env.advance_block_time(DISPUTE - 1);
        s.env.set_caller(s.arbiter);
        s.escrow.resolve(id, U256::from(share));

        assert_eq!(balances(&s), after_deposit_plus(back, share, FEE, 0));
        let deal = s.escrow.deal(id);
        assert_eq!(deal.state, DealState::Resolved);
        assert_eq!(deal.to_seller, U256::from(share));
        assert_eq!(deal.to_buyer, U256::from(back));
        assert_eq!(deal.to_arbiter, U256::from(FEE));
    }
}

#[test]
fn the_arbiter_cannot_give_away_the_fee_or_more_than_the_deposit() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    s.env.set_caller(s.arbiter);
    assert_eq!(
        s.escrow.try_resolve(id, U256::from(AMOUNT - FEE + 1)),
        err(Error::ShareTooLarge)
    );
    assert_eq!(
        s.escrow.try_resolve(id, U256::MAX),
        err(Error::ShareTooLarge)
    );
    assert_eq!(balances(&s), after_deposit_plus(0, 0, 0, AMOUNT));
}

#[test]
fn an_undecided_dispute_is_split_by_the_rule_of_the_deal() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);

    s.env.advance_block_time(DISPUTE - 1);
    s.env.set_caller(s.stranger);
    assert_eq!(s.escrow.try_settle(id), err(Error::TooEarly));

    s.env.advance_block_time(1);
    s.env.set_caller(s.arbiter);
    assert_eq!(
        s.escrow.try_resolve(id, U256::zero()),
        err(Error::DisputePeriodPassed)
    );
    s.env.set_caller(s.stranger);
    s.escrow.settle(id);

    // 30% to the seller, the rest to the buyer; an arbiter who decided nothing gets nothing.
    assert_eq!(balances(&s), after_deposit_plus(700, 300, 0, 0));
    let deal = s.escrow.deal(id);
    assert_eq!(deal.state, DealState::Split);
    assert_eq!(deal.to_arbiter, U256::zero());
}

#[test]
fn a_split_rounds_down_for_the_seller_and_loses_nothing() {
    for (amount, bps, to_seller) in [
        (1_001u64, 3_333u32, 333u64),
        (1, 9_999, 0),
        (3, 5_000, 1),
        (AMOUNT, 0, 0),
        (AMOUNT, BPS, AMOUNT),
    ] {
        let mut s = setup();
        let id = New {
            deposit: U256::from(amount),
            fee: U256::zero(),
            bps,
            ..New::token(&s)
        }
        .create(&mut s);
        deliver(&mut s, id);
        dispute(&mut s, id);
        s.env.advance_block_time(DISPUTE);
        s.escrow.settle(id);

        let deal = s.escrow.deal(id);
        assert_eq!(deal.to_seller, U256::from(to_seller));
        assert_eq!(deal.to_buyer, U256::from(amount - to_seller));
        assert_eq!(s.token.balance_of(&s.seller), U256::from(to_seller));
        assert_eq!(s.token.balance_of(&s.escrow.address()), U256::zero());
    }
}

#[test]
fn the_largest_amount_can_be_split() {
    let mut s = setup();
    let mut evil = EvilToken::deploy(&s.env, NoArgs);
    let amount = U256::MAX / U256::from(BPS);
    evil.mint(s.buyer, amount);
    let id = New {
        token: Some(evil.address()),
        deposit: amount,
        bps: BPS - 1,
        ..New::token(&s)
    }
    .create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    s.env.advance_block_time(DISPUTE);
    s.escrow.settle(id);
    assert_eq!(evil.balance_of(&s.escrow.address()), U256::zero());
    assert_eq!(
        evil.balance_of(&s.seller) + evil.balance_of(&s.buyer),
        amount
    );
}

// ---------------------------------------------------------------------------------------------
// Settling between the buyer and the seller
// ---------------------------------------------------------------------------------------------

fn offer(s: &mut Setup, by: Address, id: u64, share: Option<u64>) -> OdraResult<()> {
    s.env.set_caller(by);
    s.escrow.try_offer_split(id, share.map(U256::from))
}

#[test]
fn equal_offers_settle_the_deal_in_every_open_state() {
    for stage in [Stage::Funded, Stage::Delivered, Stage::Disputed] {
        for buyer_first in [true, false] {
            let mut s = setup();
            let id = New::token(&s).create(&mut s);
            if stage != Stage::Funded {
                deliver(&mut s, id);
            }
            if stage == Stage::Disputed {
                dispute(&mut s, id);
            }
            let (first, second) = if buyer_first {
                (s.buyer, s.seller)
            } else {
                (s.seller, s.buyer)
            };

            // One offer changes nothing but the offer itself.
            offer(&mut s, first, id, Some(700)).unwrap();
            assert_eq!(balances(&s), after_deposit_plus(0, 0, 0, AMOUNT));
            let deal = s.escrow.deal(id);
            assert_eq!(deal.closed_at, 0);
            let (mine, theirs) = if buyer_first {
                (deal.buyer_offer, deal.seller_offer)
            } else {
                (deal.seller_offer, deal.buyer_offer)
            };
            assert_eq!((mine, theirs), (Some(U256::from(700)), None));
            assert!(s.env.emitted_event(
                &s.escrow,
                SplitOffered {
                    deal_id: id,
                    by: first,
                    seller_share: Some(U256::from(700)),
                }
            ));

            // The same amount from the other party closes the deal; the arbiter gets nothing.
            offer(&mut s, second, id, Some(700)).unwrap();
            assert_eq!(balances(&s), after_deposit_plus(300, 700, 0, 0));
            let deal = s.escrow.deal(id);
            assert_eq!(deal.state, DealState::Settled);
            assert_eq!(deal.to_seller, U256::from(700));
            assert_eq!(deal.to_buyer, U256::from(300));
            assert_eq!(deal.to_arbiter, U256::zero());
            assert!(s.env.emitted_event(
                &s.escrow,
                DealClosed {
                    deal_id: id,
                    state: DealState::Settled,
                    to_seller: U256::from(700),
                    to_buyer: U256::from(300),
                    to_arbiter: U256::zero(),
                    commission: U256::zero(),
                }
            ));
        }
    }
}

#[test]
fn different_offers_do_not_settle_until_one_is_changed() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    let (buyer, seller) = (s.buyer, s.seller);
    offer(&mut s, buyer, id, Some(600)).unwrap();
    offer(&mut s, seller, id, Some(800)).unwrap();
    assert_eq!(s.escrow.deal(id).state, DealState::Funded);
    assert_eq!(balances(&s), after_deposit_plus(0, 0, 0, AMOUNT));

    // The buyer comes up to what the seller asks.
    offer(&mut s, buyer, id, Some(800)).unwrap();
    assert_eq!(s.escrow.deal(id).state, DealState::Settled);
    assert_eq!(balances(&s), after_deposit_plus(200, 800, 0, 0));
}

#[test]
fn a_withdrawn_offer_cannot_be_accepted() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    let (buyer, seller) = (s.buyer, s.seller);
    offer(&mut s, buyer, id, Some(700)).unwrap();
    offer(&mut s, buyer, id, None).unwrap();
    assert_eq!(s.escrow.deal(id).buyer_offer, None);
    assert!(s.env.emitted_event(
        &s.escrow,
        SplitOffered {
            deal_id: id,
            by: buyer,
            seller_share: None,
        }
    ));

    offer(&mut s, seller, id, Some(700)).unwrap();
    assert_eq!(s.escrow.deal(id).state, DealState::Funded);

    // Two withdrawn offers are equal too, and settle nothing.
    offer(&mut s, seller, id, None).unwrap();
    assert_eq!(s.escrow.deal(id).state, DealState::Funded);
    assert_eq!(balances(&s), after_deposit_plus(0, 0, 0, AMOUNT));
}

#[test]
fn an_offer_is_at_most_the_deposit() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    let (buyer, seller) = (s.buyer, s.seller);
    assert_eq!(
        offer(&mut s, buyer, id, Some(AMOUNT + 1)),
        err(Error::ShareTooLarge)
    );
    // Everything to the seller is a valid settlement, and so is nothing.
    offer(&mut s, buyer, id, Some(AMOUNT)).unwrap();
    offer(&mut s, seller, id, Some(AMOUNT)).unwrap();
    assert_eq!(balances(&s), after_deposit_plus(0, AMOUNT, 0, 0));

    let id = New::token(&s).create(&mut s);
    offer(&mut s, seller, id, Some(0)).unwrap();
    offer(&mut s, buyer, id, Some(0)).unwrap();
    assert_eq!(s.escrow.deal(id).state, DealState::Settled);
    assert_eq!(s.escrow.deal(id).to_buyer, U256::from(AMOUNT));
}

#[test]
fn an_offer_does_not_stop_the_time_limits() {
    let mut s = setup();
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    let (buyer, stranger) = (s.buyer, s.stranger);
    offer(&mut s, buyer, id, Some(100)).unwrap();

    // The buyer negotiated and did not dispute: the review ends and the seller is paid in full.
    s.env.advance_block_time(REVIEW);
    assert_eq!(s.escrow.try_dispute(id), err(Error::ReviewPeriodPassed));
    s.env.set_caller(stranger);
    s.escrow.settle(id);
    assert_eq!(balances(&s), after_deposit_plus(0, AMOUNT, 0, 0));
    assert_eq!(offer(&mut s, buyer, id, Some(100)), err(Error::DealClosed));
}

#[test]
fn cspr_is_settled_between_the_parties() {
    let mut s = setup();
    let id = New::cspr(&s).create(&mut s);
    let (buyer, seller) = (s.buyer, s.seller);
    offer(&mut s, seller, id, Some(250)).unwrap();
    let seller_before = cspr(&s, s.seller);
    offer(&mut s, buyer, id, Some(250)).unwrap();

    assert_eq!(cspr(&s, s.seller), seller_before + 250);
    assert_eq!(s.escrow.deal(id).to_buyer, U256::from(AMOUNT - 250));
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

// ---------------------------------------------------------------------------------------------
// The paths of a deal, in CSPR
// ---------------------------------------------------------------------------------------------

/// Balances of the accounts that do not call in a test, so no gas changes them.
fn cspr(s: &Setup, account: Address) -> U512 {
    s.env.balance_of(&account)
}

#[test]
fn cspr_is_released_to_the_seller() {
    let mut s = setup();
    let id = New::cspr(&s).create(&mut s);
    let seller = cspr(&s, s.seller);
    s.env.set_caller(s.buyer);
    s.escrow.release(id);

    assert_eq!(cspr(&s, s.seller), seller + AMOUNT);
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

#[test]
fn cspr_is_refunded_to_the_buyer() {
    let mut s = setup();
    let id = New::cspr(&s).create(&mut s);
    let buyer = cspr(&s, s.buyer);
    s.env.set_caller(s.seller);
    s.escrow.refund(id);

    assert_eq!(cspr(&s, s.buyer), buyer + AMOUNT);
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

#[test]
fn cspr_is_settled_by_a_stranger_in_every_state() {
    // Funded: back to the buyer.
    let mut s = setup();
    let id = New::cspr(&s).create(&mut s);
    let buyer = cspr(&s, s.buyer);
    s.env.advance_block_time(DELIVERY);
    s.env.set_caller(s.stranger);
    s.escrow.settle(id);
    assert_eq!(cspr(&s, s.buyer), buyer + AMOUNT);

    // Delivered: to the seller.
    let id = New::cspr(&s).create(&mut s);
    deliver(&mut s, id);
    let seller = cspr(&s, s.seller);
    s.env.advance_block_time(REVIEW);
    s.env.set_caller(s.stranger);
    s.escrow.settle(id);
    assert_eq!(cspr(&s, s.seller), seller + AMOUNT);

    // Disputed: split, nothing for the arbiter.
    let id = New::cspr(&s).create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    let (buyer, seller, arbiter) = (cspr(&s, s.buyer), cspr(&s, s.seller), cspr(&s, s.arbiter));
    s.env.advance_block_time(DISPUTE);
    s.env.set_caller(s.stranger);
    s.escrow.settle(id);
    assert_eq!(cspr(&s, s.buyer), buyer + 700);
    assert_eq!(cspr(&s, s.seller), seller + 300);
    assert_eq!(cspr(&s, s.arbiter), arbiter);

    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

#[test]
fn cspr_is_split_by_the_arbiter() {
    let mut s = setup();
    let id = New::cspr(&s).create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    let (buyer, seller) = (cspr(&s, s.buyer), cspr(&s, s.seller));
    s.env.set_caller(s.arbiter);
    s.escrow.resolve(id, U256::from(600));

    assert_eq!(cspr(&s, s.seller), seller + 600);
    assert_eq!(cspr(&s, s.buyer), buyer + (AMOUNT - FEE - 600));
    assert_eq!(s.escrow.deal(id).to_arbiter, U256::from(FEE));
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

// ---------------------------------------------------------------------------------------------
// The commission of a deployment
// ---------------------------------------------------------------------------------------------

/// A commission of 2.5%.
const COMMISSION: u32 = 250;

fn collected(s: &Setup) -> U256 {
    s.token.balance_of(&s.collector)
}

#[test]
fn the_commission_is_set_at_deployment() {
    let s = setup_with_commission(COMMISSION);
    assert_eq!(s.escrow.commission_bps(), COMMISSION);
    assert_eq!(s.escrow.commission_recipient(), s.collector);

    let free = setup();
    assert_eq!(free.escrow.commission_bps(), 0);
}

#[test]
fn a_commission_over_the_limit_or_paid_to_a_contract_is_rejected() {
    let env = odra_test::env();
    let collector = env.get_account(5);
    let contract = TestToken::deploy(&env, NoArgs).address();
    assert_eq!(
        Escrow::try_deploy(
            &env,
            EscrowInitArgs {
                commission_bps: MAX_COMMISSION_BPS + 1,
                commission_recipient: collector,
            }
        )
        .map(|_| ()),
        err(Error::CommissionTooLarge)
    );
    assert_eq!(
        Escrow::try_deploy(
            &env,
            EscrowInitArgs {
                commission_bps: COMMISSION,
                commission_recipient: contract,
            }
        )
        .map(|_| ()),
        err(Error::CommissionToContract)
    );
    let largest = Escrow::deploy(
        &env,
        EscrowInitArgs {
            commission_bps: MAX_COMMISSION_BPS,
            commission_recipient: collector,
        },
    );
    assert_eq!(largest.commission_bps(), MAX_COMMISSION_BPS);
}

#[test]
fn the_commission_comes_out_of_a_release() {
    let mut s = setup_with_commission(COMMISSION);
    let id = New::token(&s).create(&mut s);
    s.env.set_caller(s.buyer);
    s.escrow.release(id);

    // 2.5% of 1000 is 25: the seller gets 975.
    assert_eq!(balances(&s), after_deposit_plus(0, 975, 0, 0));
    assert_eq!(collected(&s), U256::from(25));
    let deal = s.escrow.deal(id);
    assert_eq!(deal.to_seller, U256::from(975));
    assert_eq!(deal.commission, U256::from(25));
    assert!(s.env.emitted_event(
        &s.escrow,
        DealClosed {
            deal_id: id,
            state: DealState::Released,
            to_seller: U256::from(975),
            to_buyer: U256::zero(),
            to_arbiter: U256::zero(),
            commission: U256::from(25),
        }
    ));
}

#[test]
fn the_commission_comes_out_of_a_payment_after_the_review() {
    let mut s = setup_with_commission(COMMISSION);
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    s.env.advance_block_time(REVIEW);
    s.escrow.settle(id);
    assert_eq!(balances(&s), after_deposit_plus(0, 975, 0, 0));
    assert_eq!(collected(&s), U256::from(25));
}

#[test]
fn the_commission_comes_out_of_the_sellers_part_of_every_split() {
    // The arbiter awards 600: 15 of it is the commission. The buyer and the arbiter lose nothing.
    let mut s = setup_with_commission(COMMISSION);
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    s.env.set_caller(s.arbiter);
    s.escrow.resolve(id, U256::from(600));
    assert_eq!(balances(&s), after_deposit_plus(350, 585, FEE, 0));
    assert_eq!(collected(&s), U256::from(15));

    // The default split gives the seller 300: the commission is 7.5, rounded down to 7.
    let mut s = setup_with_commission(COMMISSION);
    let id = New::token(&s).create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    s.env.advance_block_time(DISPUTE);
    s.escrow.settle(id);
    assert_eq!(balances(&s), after_deposit_plus(700, 293, 0, 0));
    assert_eq!(collected(&s), U256::from(7));

    // The parties settle on 700 for the seller: the commission is 17.5, rounded down to 17.
    let mut s = setup_with_commission(COMMISSION);
    let id = New::token(&s).create(&mut s);
    let (buyer, seller) = (s.buyer, s.seller);
    offer(&mut s, buyer, id, Some(700)).unwrap();
    offer(&mut s, seller, id, Some(700)).unwrap();
    assert_eq!(balances(&s), after_deposit_plus(300, 683, 0, 0));
    assert_eq!(collected(&s), U256::from(17));
    assert_eq!(s.escrow.deal(id).commission, U256::from(17));
}

#[test]
fn nothing_is_taken_from_a_deposit_that_goes_back() {
    let mut s = setup_with_commission(COMMISSION);
    let refunded = New::token(&s).create(&mut s);
    s.env.set_caller(s.seller);
    s.escrow.refund(refunded);
    assert_eq!(balances(&s), after_deposit_plus(AMOUNT, 0, 0, 0));

    let undelivered = New::token(&s).create(&mut s);
    s.env.advance_block_time(DELIVERY);
    s.escrow.settle(undelivered);
    assert_eq!(collected(&s), U256::zero());
    assert_eq!(s.escrow.deal(undelivered).commission, U256::zero());
    assert_eq!(s.token.balance_of(&s.buyer), TestToken::faucet_amount());
}

#[test]
fn a_commission_smaller_than_one_unit_is_not_taken() {
    let mut s = setup_with_commission(COMMISSION);
    let id = New {
        deposit: U256::from(39),
        fee: U256::zero(),
        ..New::token(&s)
    }
    .create(&mut s);
    s.env.set_caller(s.buyer);
    s.escrow.release(id);
    // 2.5% of 39 is 0.975.
    assert_eq!(s.token.balance_of(&s.seller), U256::from(39));
    assert_eq!(collected(&s), U256::zero());
}

#[test]
fn the_commission_is_paid_in_cspr_too() {
    let mut s = setup_with_commission(COMMISSION);
    let id = New::cspr(&s).create(&mut s);
    let (seller, collector) = (cspr(&s, s.seller), cspr(&s, s.collector));
    s.env.set_caller(s.buyer);
    s.escrow.release(id);
    assert_eq!(cspr(&s, s.seller), seller + 975);
    assert_eq!(cspr(&s, s.collector), collector + 25);
    assert_eq!(s.env.balance_of(&s.escrow), U512::zero());
}

#[test]
fn a_token_that_refuses_the_commission_leaves_the_refund_open() {
    let mut s = setup_with_commission(COMMISSION);
    let mut evil = evil(&s, HONEST);
    let id = New {
        token: Some(evil.address()),
        ..New::token(&s)
    }
    .create(&mut s);
    evil.set_blocked(s.collector);
    evil.set_mode(BLOCK);

    s.env.set_caller(s.buyer);
    assert_eq!(s.escrow.try_release(id), err(EvilTokenError::Blocked));
    assert_eq!(evil.balance_of(&s.escrow.address()), U256::from(AMOUNT));
    s.env.set_caller(s.seller);
    s.escrow.refund(id);
    assert_eq!(evil.balance_of(&s.buyer), U256::from(AMOUNT));
}

// ---------------------------------------------------------------------------------------------
// Many deals at once
// ---------------------------------------------------------------------------------------------

#[test]
fn the_contract_holds_exactly_the_open_deposits() {
    let mut s = setup();
    let held = |s: &Setup| {
        (
            s.token.balance_of(&s.escrow.address()),
            s.env.balance_of(&s.escrow),
        )
    };
    let amount = |n: u64| (U256::from(n * AMOUNT), U512::from(n * AMOUNT));

    let t0 = New::token(&s).create(&mut s);
    let c0 = New::cspr(&s).create(&mut s);
    let t1 = New::token(&s).create(&mut s);
    let c1 = New::cspr(&s).create(&mut s);
    let t2 = New::token(&s).create(&mut s);
    assert_eq!(held(&s), (amount(3).0, amount(2).1));

    s.env.set_caller(s.buyer);
    s.escrow.release(t0);
    s.env.set_caller(s.seller);
    s.escrow.refund(c0);
    assert_eq!(held(&s), (amount(2).0, amount(1).1));

    for id in [t1, c1, t2] {
        deliver(&mut s, id);
    }
    dispute(&mut s, t1);
    dispute(&mut s, c1);
    s.env.set_caller(s.arbiter);
    s.escrow.resolve(t1, U256::from(123));
    assert_eq!(held(&s), (amount(1).0, amount(1).1));

    s.env.advance_block_time(DISPUTE);
    s.env.set_caller(s.stranger);
    s.escrow.settle(c1);
    s.escrow.settle(t2);
    assert_eq!(held(&s), (U256::zero(), U512::zero()));

    // A closed deal cannot be paid out a second time.
    for id in [t0, c0, t1, c1, t2] {
        assert_eq!(s.escrow.try_settle(id), err(Error::DealClosed));
    }
}

#[test]
fn every_party_finds_its_deals() {
    let mut s = setup();
    let first = New::token(&s).create(&mut s);
    let second = New {
        arbiter: None,
        fee: U256::zero(),
        ..New::token(&s)
    }
    .create(&mut s);
    // In the third deal the roles are shuffled: the arbiter of the others is the seller.
    let third = New {
        seller: s.arbiter,
        arbiter: Some(s.stranger),
        ..New::token(&s)
    }
    .create(&mut s);

    let ids = |s: &Setup, account: Address| -> Vec<u64> {
        (0..s.escrow.deals_count_of(account))
            .map(|i| s.escrow.deal_id_of(account, i))
            .collect()
    };
    assert_eq!(ids(&s, s.buyer), vec![first, second, third]);
    assert_eq!(ids(&s, s.seller), vec![first, second]);
    assert_eq!(ids(&s, s.arbiter), vec![first, third]);
    assert_eq!(ids(&s, s.stranger), vec![third]);
    assert_eq!(
        s.escrow.try_deal_id_of(s.stranger, 1),
        err(Error::DealNotFound)
    );
}

// ---------------------------------------------------------------------------------------------
// Tokens that misbehave
// ---------------------------------------------------------------------------------------------

/// Deploys the misbehaving token, in the given mode, with 1000 units for the buyer.
fn evil(s: &Setup, mode: u8) -> EvilTokenHostRef {
    let mut evil = EvilToken::deploy(&s.env, NoArgs);
    evil.mint(s.buyer, U256::from(AMOUNT));
    evil.set_target(s.escrow.address());
    evil.set_mode(mode);
    evil
}

#[test]
fn a_token_that_keeps_a_part_of_the_deposit_is_rejected() {
    let mut s = setup();
    let evil = evil(&s, SKIM);
    let new = New {
        token: Some(evil.address()),
        ..New::token(&s)
    };
    assert_eq!(new.try_create(&mut s), err(Error::DepositMismatch));
    assert_eq!(evil.balance_of(&s.buyer), U256::from(AMOUNT));
    assert_eq!(s.escrow.deals_count(), 0);
}

#[test]
fn a_token_that_moves_nothing_is_rejected() {
    let mut s = setup();
    let evil = evil(&s, PRETEND);
    let new = New {
        token: Some(evil.address()),
        ..New::token(&s)
    };
    assert_eq!(new.try_create(&mut s), err(Error::DepositMismatch));
    assert_eq!(s.escrow.deals_count(), 0);
}

#[test]
fn a_token_cannot_call_back_into_the_contract() {
    let mut s = setup();
    // An open deal the token tries to settle from inside `create_deal`, its time limit passed.
    let open = New::token(&s).create(&mut s);
    s.env.advance_block_time(DELIVERY);
    assert_eq!(open, 0);

    let evil = evil(&s, REENTER);
    let new = New {
        token: Some(evil.address()),
        ..New::token(&s)
    };
    assert_eq!(
        new.try_create(&mut s),
        Err(ExecutionError::ReentrantCall.into())
    );
    assert_eq!(s.escrow.deal(open).state, DealState::Funded);
    assert_eq!(s.escrow.deals_count(), 1);
}

#[test]
fn a_token_that_refuses_the_seller_leaves_the_refund_open() {
    let mut s = setup();
    let mut evil = evil(&s, HONEST);
    let id = New {
        token: Some(evil.address()),
        ..New::token(&s)
    }
    .create(&mut s);
    deliver(&mut s, id);
    evil.set_blocked(s.seller);
    evil.set_mode(BLOCK);

    // Nothing that pays the seller goes through, and nothing is lost by trying.
    s.env.set_caller(s.buyer);
    assert_eq!(s.escrow.try_release(id), err(EvilTokenError::Blocked));
    s.env.advance_block_time(REVIEW);
    assert_eq!(s.escrow.try_settle(id), err(EvilTokenError::Blocked));
    assert_eq!(s.escrow.deal(id).state, DealState::Delivered);
    assert_eq!(evil.balance_of(&s.escrow.address()), U256::from(AMOUNT));

    s.env.set_caller(s.seller);
    s.escrow.refund(id);
    assert_eq!(evil.balance_of(&s.buyer), U256::from(AMOUNT));
}

#[test]
fn a_token_that_refuses_the_arbiter_leaves_the_default_split_open() {
    let mut s = setup();
    let mut evil = evil(&s, HONEST);
    let id = New {
        token: Some(evil.address()),
        ..New::token(&s)
    }
    .create(&mut s);
    deliver(&mut s, id);
    dispute(&mut s, id);
    evil.set_blocked(s.arbiter);
    evil.set_mode(BLOCK);

    s.env.set_caller(s.arbiter);
    assert_eq!(
        s.escrow.try_resolve(id, U256::from(600)),
        err(EvilTokenError::Blocked)
    );
    s.env.advance_block_time(DISPUTE);
    s.escrow.settle(id);
    assert_eq!(evil.balance_of(&s.seller), U256::from(300));
    assert_eq!(evil.balance_of(&s.buyer), U256::from(700));
}

#[test]
fn a_misbehaving_token_cannot_touch_the_deposits_of_another() {
    let mut s = setup();
    let honest = New::token(&s).create(&mut s);
    let cspr = New::cspr(&s).create(&mut s);

    // A deal in a token that moves nothing cannot be created, so nothing can be paid out of it.
    let evil = evil(&s, PRETEND);
    let new = New {
        token: Some(evil.address()),
        ..New::token(&s)
    };
    assert!(new.try_create(&mut s).is_err());

    assert_eq!(s.token.balance_of(&s.escrow.address()), U256::from(AMOUNT));
    assert_eq!(s.env.balance_of(&s.escrow), U512::from(AMOUNT));
    assert_eq!(s.escrow.deal(honest).state, DealState::Funded);
    assert_eq!(s.escrow.deal(cspr).state, DealState::Funded);
}

// ---------------------------------------------------------------------------------------------
// What the calls cost
// ---------------------------------------------------------------------------------------------

/// Prints what every call costs. Run it on the Casper VM:
/// `cargo odra test -b casper -s gas_of_every_call -- --nocapture`.
#[test]
fn gas_of_every_call() {
    let mut s = setup();
    for cspr in [false, true] {
        let new = |s: &Setup| if cspr { New::cspr(s) } else { New::token(s) };
        let a = new(&s).create(&mut s);
        s.env.set_caller(s.buyer);
        s.escrow.release(a);
        let b = new(&s).create(&mut s);
        s.env.set_caller(s.seller);
        s.escrow.refund(b);
        let c = new(&s).create(&mut s);
        deliver(&mut s, c);
        dispute(&mut s, c);
        s.env.set_caller(s.arbiter);
        s.escrow.resolve(c, U256::from(600));
        let d = new(&s).create(&mut s);
        deliver(&mut s, d);
        dispute(&mut s, d);
        s.env.advance_block_time(DISPUTE);
        s.escrow.settle(d);
        let e = new(&s).create(&mut s);
        s.env.set_caller(s.seller);
        s.escrow.offer_split(e, Some(U256::from(700)));
        s.env.set_caller(s.buyer);
        s.escrow.offer_split(e, Some(U256::from(700)));
    }
    println!("{}", s.env.gas_report());
}
