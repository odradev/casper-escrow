//! Tokens for trying and testing the escrow contract. They have no value: never use them outside
//! of tests and test networks.

use odra::casper_types::U256;
use odra::prelude::*;
use odra::ContractRef;
use odra_modules::cep18_token::Cep18;

use crate::escrow::EscrowContractRef;

/// Number of decimals of the test token.
pub const DECIMALS: u8 = 9;
/// Whole tokens a single `faucet` call mints.
pub const FAUCET_TOKENS: u64 = 1_000;

/// A CEP-18 token with a public faucet.
#[odra::module]
pub struct TestToken {
    token: SubModule<Cep18>,
}

#[odra::module]
impl TestToken {
    /// Sets the token metadata. Nothing is minted up front.
    pub fn init(&mut self) {
        self.token.init(
            String::from("TEST"),
            String::from("Escrow Test Token"),
            DECIMALS,
            U256::zero(),
        );
    }

    /// Mints `FAUCET_TOKENS` whole tokens to the caller.
    pub fn faucet(&mut self) {
        let caller = self.env().caller();
        self.token.raw_mint(&caller, &Self::faucet_amount());
    }

    delegate! {
        to self.token {
            fn name(&self) -> String;
            fn symbol(&self) -> String;
            fn decimals(&self) -> u8;
            fn total_supply(&self) -> U256;
            fn balance_of(&self, address: &Address) -> U256;
            fn allowance(&self, owner: &Address, spender: &Address) -> U256;
            fn approve(&mut self, spender: &Address, amount: &U256);
            fn decrease_allowance(&mut self, spender: &Address, decr_by: &U256);
            fn increase_allowance(&mut self, spender: &Address, inc_by: &U256);
            fn transfer(&mut self, recipient: &Address, amount: &U256);
            fn transfer_from(&mut self, owner: &Address, recipient: &Address, amount: &U256);
        }
    }
}

impl TestToken {
    /// The amount a single `faucet` call mints, in the smallest unit.
    pub fn faucet_amount() -> U256 {
        U256::from(FAUCET_TOKENS) * U256::from(10u64.pow(DECIMALS as u32))
    }
}

/// `EvilToken` behaves like an honest token.
pub const HONEST: u8 = 0;
/// `EvilToken` keeps one percent of every amount it moves.
pub const SKIM: u8 = 1;
/// `EvilToken` moves nothing and reports success.
pub const PRETEND: u8 = 2;
/// `EvilToken` calls back into the escrow contract while it is being moved with `transfer_from`.
pub const REENTER: u8 = 3;
/// `EvilToken` refuses to pay the blocked account.
pub const BLOCK: u8 = 4;

/// A token with the entry points the escrow contract calls and a switch for misbehaving. It
/// checks no allowances.
#[odra::module(errors = EvilTokenError)]
pub struct EvilToken {
    balances: Mapping<Address, U256>,
    mode: Var<u8>,
    /// The escrow contract to call back in the `REENTER` mode.
    target: Var<Address>,
    /// The account that cannot be paid in the `BLOCK` mode.
    blocked: Var<Address>,
}

/// Errors of `EvilToken`.
#[odra::odra_error]
pub enum EvilTokenError {
    /// The sender has too few tokens.
    InsufficientBalance = 900,
    /// The recipient is blocked.
    Blocked = 901,
}

#[odra::module]
impl EvilToken {
    /// Gives `amount` to `to`.
    pub fn mint(&mut self, to: Address, amount: U256) {
        self.balances.add(&to, amount);
    }

    /// Chooses how the token misbehaves.
    pub fn set_mode(&mut self, mode: u8) {
        self.mode.set(mode);
    }

    /// Sets the escrow contract the `REENTER` mode calls.
    pub fn set_target(&mut self, target: Address) {
        self.target.set(target);
    }

    /// Sets the account the `BLOCK` mode refuses to pay.
    pub fn set_blocked(&mut self, blocked: Address) {
        self.blocked.set(blocked);
    }

    pub fn balance_of(&self, address: &Address) -> U256 {
        self.balances.get_or_default(address)
    }

    pub fn transfer(&mut self, recipient: &Address, amount: &U256) {
        let owner = self.env().caller();
        self.move_tokens(&owner, recipient, amount);
    }

    pub fn transfer_from(&mut self, owner: &Address, recipient: &Address, amount: &U256) {
        if self.mode.get_or_default() == REENTER {
            let target = self.target.get().unwrap_or_revert(self);
            EscrowContractRef::new(self.env(), target).settle(0);
        }
        self.move_tokens(owner, recipient, amount);
    }
}

impl EvilToken {
    fn move_tokens(&mut self, owner: &Address, recipient: &Address, amount: &U256) {
        let mode = self.mode.get_or_default();
        if mode == PRETEND {
            return;
        }
        if mode == BLOCK && self.blocked.get() == Some(*recipient) {
            self.env().revert(EvilTokenError::Blocked);
        }
        if self.balances.get_or_default(owner) < *amount {
            self.env().revert(EvilTokenError::InsufficientBalance);
        }
        let received = if mode == SKIM {
            *amount - *amount / U256::from(100)
        } else {
            *amount
        };
        self.balances.subtract(owner, *amount);
        self.balances.add(recipient, received);
    }
}
