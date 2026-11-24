//! Escrow: a buyer's deposit, in CSPR or a CEP-18 token, waits in the contract until the deal
//! between the buyer and the seller is settled. Every open deal can always be closed by somebody:
//! each state has a time limit after which anyone may apply the rule the deal was created with.
//!
//! The contract has no owner, no fee and no configuration, and cannot be upgraded.

#![allow(clippy::too_many_arguments)]

use odra::casper_types::{U256, U512};
use odra::prelude::*;
use odra::ContractRef;
use odra_modules::cep18_token::Cep18ContractRef;

const HOUR: u64 = 60 * 60 * 1000;
/// The shortest review period and dispute period, in milliseconds. A period given in seconds by
/// mistake is a thousand times too short; this limit rejects it.
pub const MIN_PERIOD: u64 = HOUR;
/// The longest review period, dispute period and time until the delivery deadline, in
/// milliseconds.
pub const MAX_PERIOD: u64 = 365 * 24 * HOUR;
/// The longest memo, in bytes.
pub const MAX_MEMO_LEN: usize = 256;
/// `default_seller_bps` of a deal that gives the seller everything.
pub const BPS: u32 = 10_000;

/// Where a deal stands. `Funded`, `Delivered` and `Disputed` are open: the deposit is in the
/// contract. The other five are closed, and say how the deposit left it.
#[odra::odra_type]
#[derive(Copy)]
pub enum DealState {
    /// The deposit is held and the seller has not delivered yet.
    Funded,
    /// The seller said the work is delivered; the buyer's review period runs.
    Delivered,
    /// The buyer disputed the delivery; the dispute period runs.
    Disputed,
    /// The whole deposit went to the seller.
    Released,
    /// The whole deposit went back to the buyer.
    Refunded,
    /// The arbiter decided the dispute.
    Resolved,
    /// The dispute period ran out with no decision; the deposit was split by `default_seller_bps`.
    Split,
    /// The buyer and the seller agreed on a split themselves.
    Settled,
}

/// A deal. Times are block times: milliseconds since the Unix epoch. A time of zero means "not
/// yet".
#[odra::odra_type]
pub struct Deal {
    /// The account that deposited.
    pub buyer: Address,
    /// The account the deposit is for.
    pub seller: Address,
    /// The account that may decide a dispute, if the deal has one.
    pub arbiter: Option<Address>,
    /// The CEP-18 token deposited; `None` for CSPR.
    pub token: Option<Address>,
    /// The amount deposited: motes of CSPR or the smallest units of the token.
    pub deposit: U256,
    /// What the arbiter takes out of the deposit for deciding a dispute. Paid only for a decision.
    pub arbiter_fee: U256,
    /// The seller's part of the deposit, in hundredths of a percent, if a dispute gets no
    /// decision in time. The buyer gets the rest.
    pub default_seller_bps: u32,
    /// The seller can mark the deal delivered before this moment. From it on, a deal still
    /// `Funded` can be settled back to the buyer.
    pub delivery_deadline: u64,
    /// How long the buyer has to dispute a delivery, counted from `delivered_at`.
    pub review_period: u64,
    /// How long a dispute may stay undecided, counted from `disputed_at`.
    pub dispute_period: u64,
    /// What the deal is about: a free text, a link or a hash of the agreement.
    pub memo: String,
    /// Where the deal stands.
    pub state: DealState,
    /// When the deal was created.
    pub created_at: u64,
    /// When the seller marked it delivered.
    pub delivered_at: u64,
    /// When the buyer disputed it.
    pub disputed_at: u64,
    /// When it closed.
    pub closed_at: u64,
    /// What the seller received when the deal closed.
    pub to_seller: U256,
    /// What the buyer received when the deal closed.
    pub to_buyer: U256,
    /// What the arbiter received when the deal closed.
    pub to_arbiter: U256,
    /// What the buyer currently offers the seller in a settlement, if anything.
    pub buyer_offer: Option<U256>,
    /// What the seller currently asks for in a settlement, if anything.
    pub seller_offer: Option<U256>,
}

/// Errors of the escrow contract.
#[odra::odra_error]
pub enum Error {
    /// The deposit of a new deal is zero.
    ZeroDeposit = 1,
    /// The deposit of a new deal is too large to be split.
    DepositTooLarge = 2,
    /// The buyer, the seller and the arbiter must be three different accounts.
    SameParty = 3,
    /// The arbiter's fee is not less than the deposit.
    FeeTooLarge = 4,
    /// A deal with no arbiter cannot have an arbiter's fee.
    FeeWithoutArbiter = 5,
    /// `default_seller_bps` is more than 10000.
    BpsTooLarge = 6,
    /// The delivery deadline is not in the future.
    DeadlineNotInFuture = 7,
    /// The delivery deadline is more than `MAX_PERIOD` away.
    DeadlineTooFar = 8,
    /// A period is shorter than `MIN_PERIOD`.
    PeriodTooShort = 9,
    /// A period is longer than `MAX_PERIOD`.
    PeriodTooLong = 10,
    /// The memo is longer than `MAX_MEMO_LEN` bytes.
    MemoTooLong = 11,
    /// A CSPR deal must come with exactly its deposit attached.
    WrongAttachedValue = 12,
    /// A token deal must come with no CSPR attached.
    UnexpectedAttachedValue = 13,
    /// The contract did not receive exactly the tokens it asked for.
    DepositMismatch = 14,
    /// No deal has this id.
    DealNotFound = 15,
    /// The deal is closed: its deposit has been paid out.
    DealClosed = 16,
    /// Only the buyer of the deal may do this.
    NotBuyer = 17,
    /// Only the seller of the deal may do this.
    NotSeller = 18,
    /// Only the arbiter of the deal may do this.
    NotArbiter = 19,
    /// The deal is not waiting for delivery.
    NotFunded = 20,
    /// The deal is not marked delivered.
    NotDelivered = 21,
    /// The deal is not disputed.
    NotDisputed = 22,
    /// The delivery deadline has passed.
    DeliveryDeadlinePassed = 23,
    /// The review period has passed.
    ReviewPeriodPassed = 24,
    /// The dispute period has passed.
    DisputePeriodPassed = 25,
    /// The time limit of the deal's state has not been reached yet.
    TooEarly = 26,
    /// The seller's share is more than the deposit, or, in a decision of the arbiter, more than
    /// the deposit less the arbiter's fee.
    ShareTooLarge = 27,
    /// A party of a CSPR deal is a contract. Casper pays CSPR to accounts only.
    ContractInCsprDeal = 28,
    /// Only the buyer or the seller of the deal may do this.
    NotParty = 29,
}

/// A buyer deposited for a seller.
#[odra::event]
pub struct DealCreated {
    pub deal_id: u64,
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Option<Address>,
    pub token: Option<Address>,
    pub deposit: U256,
    pub arbiter_fee: U256,
    pub default_seller_bps: u32,
    pub delivery_deadline: u64,
    pub review_period: u64,
    pub dispute_period: u64,
    pub memo: String,
}

/// The seller marked a deal delivered.
#[odra::event]
pub struct DealDelivered {
    pub deal_id: u64,
    /// The buyer can dispute before this moment.
    pub review_ends_at: u64,
}

/// The buyer disputed a delivery.
#[odra::event]
pub struct DealDisputed {
    pub deal_id: u64,
    /// The arbiter can decide before this moment.
    pub dispute_ends_at: u64,
}

/// The buyer or the seller set, changed or withdrew a settlement offer.
#[odra::event]
pub struct SplitOffered {
    pub deal_id: u64,
    /// The party whose offer this is.
    pub by: Address,
    /// What the seller would get; `None` withdraws the offer.
    pub seller_share: Option<U256>,
}

/// A deal closed and its deposit was paid out.
#[odra::event]
pub struct DealClosed {
    pub deal_id: u64,
    /// How it closed: `Released`, `Refunded`, `Resolved`, `Split` or `Settled`.
    pub state: DealState,
    pub to_seller: U256,
    pub to_buyer: U256,
    pub to_arbiter: U256,
}

/// Holds the deposits of buyers until each deal is settled.
#[odra::module(
    events = [DealCreated, DealDelivered, DealDisputed, SplitOffered, DealClosed],
    errors = Error
)]
pub struct Escrow {
    /// Deals by id.
    deals: Mapping<u64, Deal>,
    /// The number of deals created; also the id of the next one.
    count: Var<u64>,
    /// The number of deals an account is a party of, in any role.
    count_of: Mapping<Address, u64>,
    /// The ids of the deals of an account, by their position: `0` to `count_of - 1`.
    ids_of: Mapping<(Address, u64), u64>,
}

#[odra::module]
impl Escrow {
    /// Opens a deal with the caller as the buyer and takes the deposit.
    ///
    /// For a CSPR deal `token` is `None`, exactly `deposit` motes must be attached and every
    /// party must be an account. For a token deal nothing is attached and the caller must have
    /// approved this contract for `deposit` of `token`. The id of the new deal is the value `deals_count` returned before the call; the
    /// `DealCreated` event carries it.
    ///
    /// The argument cannot be called `amount`: Odra passes the attached CSPR under that name.
    ///
    /// `delivery_deadline` is a moment in milliseconds since the Unix epoch; `review_period` and
    /// `dispute_period` are lengths in milliseconds.
    #[odra(payable, non_reentrant)]
    pub fn create_deal(
        &mut self,
        seller: Address,
        arbiter: Option<Address>,
        token: Option<Address>,
        deposit: U256,
        arbiter_fee: U256,
        default_seller_bps: u32,
        delivery_deadline: u64,
        review_period: u64,
        dispute_period: u64,
        memo: String,
    ) {
        let env = self.env();
        let now = env.get_block_time();
        let buyer = env.caller();

        if deposit.is_zero() {
            env.revert(Error::ZeroDeposit);
        }
        if deposit > U256::MAX / U256::from(BPS) {
            env.revert(Error::DepositTooLarge);
        }
        if seller == buyer || arbiter == Some(buyer) || arbiter == Some(seller) {
            env.revert(Error::SameParty);
        }
        if arbiter.is_none() && !arbiter_fee.is_zero() {
            env.revert(Error::FeeWithoutArbiter);
        }
        if arbiter_fee >= deposit {
            env.revert(Error::FeeTooLarge);
        }
        if default_seller_bps > BPS {
            env.revert(Error::BpsTooLarge);
        }
        if delivery_deadline <= now {
            env.revert(Error::DeadlineNotInFuture);
        }
        if delivery_deadline - now > MAX_PERIOD {
            env.revert(Error::DeadlineTooFar);
        }
        if review_period < MIN_PERIOD || dispute_period < MIN_PERIOD {
            env.revert(Error::PeriodTooShort);
        }
        if review_period > MAX_PERIOD || dispute_period > MAX_PERIOD {
            env.revert(Error::PeriodTooLong);
        }
        if memo.len() > MAX_MEMO_LEN {
            env.revert(Error::MemoTooLong);
        }

        let attached = env.attached_value();
        match token {
            None => {
                // A payout to a contract would fail, and the deal could not be closed that way.
                if buyer.is_contract()
                    || seller.is_contract()
                    || arbiter.is_some_and(|arbiter| arbiter.is_contract())
                {
                    env.revert(Error::ContractInCsprDeal);
                }
                if attached != to_u512(deposit) {
                    env.revert(Error::WrongAttachedValue);
                }
            }
            Some(token) => {
                if !attached.is_zero() {
                    env.revert(Error::UnexpectedAttachedValue);
                }
                // A token may keep a part of a transfer or move nothing at all. The deposit must
                // be whole, or one deal would be paid out with the tokens of another.
                let this = env.self_address();
                let mut token = Cep18ContractRef::new(self.env(), token);
                let before = token.balance_of(&this);
                token.transfer_from(&buyer, &this, &deposit);
                if token.balance_of(&this).checked_sub(before) != Some(deposit) {
                    env.revert(Error::DepositMismatch);
                }
            }
        }

        let deal_id = self.count.get_or_default();
        self.count.set(deal_id + 1);
        self.deals.set(
            &deal_id,
            Deal {
                buyer,
                seller,
                arbiter,
                token,
                deposit,
                arbiter_fee,
                default_seller_bps,
                delivery_deadline,
                review_period,
                dispute_period,
                memo: memo.clone(),
                state: DealState::Funded,
                created_at: now,
                delivered_at: 0,
                disputed_at: 0,
                closed_at: 0,
                to_seller: U256::zero(),
                to_buyer: U256::zero(),
                to_arbiter: U256::zero(),
                buyer_offer: None,
                seller_offer: None,
            },
        );
        self.add_to_index(&buyer, deal_id);
        self.add_to_index(&seller, deal_id);
        if let Some(arbiter) = arbiter {
            self.add_to_index(&arbiter, deal_id);
        }

        env.emit_event(DealCreated {
            deal_id,
            buyer,
            seller,
            arbiter,
            token,
            deposit,
            arbiter_fee,
            default_seller_bps,
            delivery_deadline,
            review_period,
            dispute_period,
            memo,
        });
    }

    /// Says the work is delivered and starts the buyer's review period. Only the seller may call
    /// it, on a `Funded` deal, before the delivery deadline.
    #[odra(non_reentrant)]
    pub fn mark_delivered(&mut self, deal_id: u64) {
        let mut deal = self.open_deal(deal_id);
        let now = self.env().get_block_time();
        if self.env().caller() != deal.seller {
            self.env().revert(Error::NotSeller);
        }
        if deal.state != DealState::Funded {
            self.env().revert(Error::NotFunded);
        }
        if now >= deal.delivery_deadline {
            self.env().revert(Error::DeliveryDeadlinePassed);
        }
        deal.state = DealState::Delivered;
        deal.delivered_at = now;
        let review_ends_at = now + deal.review_period;
        self.deals.set(&deal_id, deal);
        self.env().emit_event(DealDelivered {
            deal_id,
            review_ends_at,
        });
    }

    /// Disputes the delivery and starts the dispute period. Only the buyer may call it, on a
    /// `Delivered` deal, before the review period ends.
    #[odra(non_reentrant)]
    pub fn dispute(&mut self, deal_id: u64) {
        let mut deal = self.open_deal(deal_id);
        let now = self.env().get_block_time();
        if self.env().caller() != deal.buyer {
            self.env().revert(Error::NotBuyer);
        }
        if deal.state != DealState::Delivered {
            self.env().revert(Error::NotDelivered);
        }
        if now >= deal.delivered_at + deal.review_period {
            self.env().revert(Error::ReviewPeriodPassed);
        }
        deal.state = DealState::Disputed;
        deal.disputed_at = now;
        let dispute_ends_at = now + deal.dispute_period;
        self.deals.set(&deal_id, deal);
        self.env().emit_event(DealDisputed {
            deal_id,
            dispute_ends_at,
        });
    }

    /// Pays the whole deposit to the seller. Only the buyer may call it, on any open deal.
    #[odra(non_reentrant)]
    pub fn release(&mut self, deal_id: u64) {
        let deal = self.open_deal(deal_id);
        if self.env().caller() != deal.buyer {
            self.env().revert(Error::NotBuyer);
        }
        let deposit = deal.deposit;
        self.close(deal_id, deal, DealState::Released, deposit, U256::zero());
    }

    /// Sends the whole deposit back to the buyer. Only the seller may call it, on any open deal.
    #[odra(non_reentrant)]
    pub fn refund(&mut self, deal_id: u64) {
        let deal = self.open_deal(deal_id);
        if self.env().caller() != deal.seller {
            self.env().revert(Error::NotSeller);
        }
        self.close(
            deal_id,
            deal,
            DealState::Refunded,
            U256::zero(),
            U256::zero(),
        );
    }

    /// Decides a dispute: the arbiter takes the fee, `seller_share` goes to the seller and the
    /// rest back to the buyer. Only the arbiter may call it, on a `Disputed` deal, before the
    /// dispute period ends.
    #[odra(non_reentrant)]
    pub fn resolve(&mut self, deal_id: u64, seller_share: U256) {
        let deal = self.open_deal(deal_id);
        if Some(self.env().caller()) != deal.arbiter {
            self.env().revert(Error::NotArbiter);
        }
        if deal.state != DealState::Disputed {
            self.env().revert(Error::NotDisputed);
        }
        if self.env().get_block_time() >= deal.disputed_at + deal.dispute_period {
            self.env().revert(Error::DisputePeriodPassed);
        }
        let fee = deal.arbiter_fee;
        if seller_share > deal.deposit - fee {
            self.env().revert(Error::ShareTooLarge);
        }
        self.close(deal_id, deal, DealState::Resolved, seller_share, fee);
    }

    /// Offers a settlement: `seller_share` of the deposit to the seller, the rest back to the
    /// buyer. Only the buyer or the seller may call it, on any open deal.
    ///
    /// Each party has one offer; calling again replaces it and `None` withdraws it. When the
    /// offers of the two parties are the same amount, the deal closes with that split and the
    /// arbiter gets nothing. An offer does not stop any time limit of the deal.
    #[odra(non_reentrant)]
    pub fn offer_split(&mut self, deal_id: u64, seller_share: Option<U256>) {
        let mut deal = self.open_deal(deal_id);
        let caller = self.env().caller();
        let is_buyer = caller == deal.buyer;
        if !is_buyer && caller != deal.seller {
            self.env().revert(Error::NotParty);
        }
        if seller_share.is_some_and(|share| share > deal.deposit) {
            self.env().revert(Error::ShareTooLarge);
        }
        if is_buyer {
            deal.buyer_offer = seller_share;
        } else {
            deal.seller_offer = seller_share;
        }
        self.env().emit_event(SplitOffered {
            deal_id,
            by: caller,
            seller_share,
        });
        match seller_share {
            Some(share) if deal.buyer_offer == deal.seller_offer => {
                self.close(deal_id, deal, DealState::Settled, share, U256::zero());
            }
            _ => self.deals.set(&deal_id, deal),
        }
    }

    /// Closes a deal whose time limit has passed, by the rule of its state. Anyone may call it.
    ///
    /// - `Funded`, from the delivery deadline on: the deposit goes back to the buyer.
    /// - `Delivered`, from the end of the review period on: the deposit goes to the seller.
    /// - `Disputed`, from the end of the dispute period on: the deposit is split by
    ///   `default_seller_bps`, and the arbiter gets nothing.
    #[odra(non_reentrant)]
    pub fn settle(&mut self, deal_id: u64) {
        let deal = self.open_deal(deal_id);
        let now = self.env().get_block_time();
        let (limit, state, to_seller) = match deal.state {
            DealState::Funded => (deal.delivery_deadline, DealState::Refunded, U256::zero()),
            DealState::Delivered => (
                deal.delivered_at + deal.review_period,
                DealState::Released,
                deal.deposit,
            ),
            _ => (
                deal.disputed_at + deal.dispute_period,
                DealState::Split,
                deal.deposit * U256::from(deal.default_seller_bps) / U256::from(BPS),
            ),
        };
        if now < limit {
            self.env().revert(Error::TooEarly);
        }
        self.close(deal_id, deal, state, to_seller, U256::zero());
    }

    /// Returns the deal with the given id.
    pub fn deal(&self, deal_id: u64) -> Deal {
        self.get_deal(deal_id)
    }

    /// Returns the number of deals created. Ids run from `0` to `count - 1`.
    pub fn deals_count(&self) -> u64 {
        self.count.get_or_default()
    }

    /// Returns the number of deals `account` is a party of, in any role.
    pub fn deals_count_of(&self, account: Address) -> u64 {
        self.count_of.get_or_default(&account)
    }

    /// Returns the id of a deal of `account` by its position, `0` to `deals_count_of - 1`, in
    /// the order the deals were created.
    pub fn deal_id_of(&self, account: Address, index: u64) -> u64 {
        self.ids_of
            .get(&(account, index))
            .unwrap_or_revert_with(self, Error::DealNotFound)
    }
}

impl Escrow {
    fn get_deal(&self, deal_id: u64) -> Deal {
        self.deals
            .get(&deal_id)
            .unwrap_or_revert_with(self, Error::DealNotFound)
    }

    /// The deal, if its deposit is still in the contract.
    fn open_deal(&self, deal_id: u64) -> Deal {
        let deal = self.get_deal(deal_id);
        if !matches!(
            deal.state,
            DealState::Funded | DealState::Delivered | DealState::Disputed
        ) {
            self.env().revert(Error::DealClosed);
        }
        deal
    }

    fn add_to_index(&mut self, account: &Address, deal_id: u64) {
        let position = self.count_of.get_or_default(account);
        self.ids_of.set(&(*account, position), deal_id);
        self.count_of.set(account, position + 1);
    }

    /// Closes the deal and pays the deposit out: `to_seller` to the seller, `to_arbiter` to the
    /// arbiter and the rest to the buyer. The deal is stored as closed before anything is paid.
    fn close(
        &mut self,
        deal_id: u64,
        mut deal: Deal,
        state: DealState,
        to_seller: U256,
        to_arbiter: U256,
    ) {
        let to_buyer = deal.deposit - to_seller - to_arbiter;
        deal.state = state;
        deal.closed_at = self.env().get_block_time();
        deal.to_seller = to_seller;
        deal.to_buyer = to_buyer;
        deal.to_arbiter = to_arbiter;
        self.deals.set(&deal_id, deal.clone());

        self.pay(&deal.token, &deal.seller, to_seller);
        self.pay(&deal.token, &deal.buyer, to_buyer);
        if let Some(arbiter) = deal.arbiter {
            self.pay(&deal.token, &arbiter, to_arbiter);
        }

        self.env().emit_event(DealClosed {
            deal_id,
            state,
            to_seller,
            to_buyer,
            to_arbiter,
        });
    }

    fn pay(&self, token: &Option<Address>, to: &Address, amount: U256) {
        if amount.is_zero() {
            return;
        }
        match token {
            None => self.env().transfer_tokens(to, &to_u512(amount)),
            Some(token) => Cep18ContractRef::new(self.env(), *token).transfer(to, &amount),
        }
    }
}

fn to_u512(value: U256) -> U512 {
    let mut bytes = [0u8; 32];
    value.to_little_endian(&mut bytes);
    U512::from_little_endian(&bytes)
}
