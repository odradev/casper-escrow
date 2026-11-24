//! Command line tool of the escrow project: deploys the contract and calls it on a network.

use std::str::FromStr;

use escrow::escrow::{Escrow, EscrowInitArgs, MAX_COMMISSION_BPS};
use escrow::testing::TestToken;
use odra::host::{HostEnv, NoArgs};
use odra::prelude::*;
use odra_cli::{deploy::DeployScript, DeployedContractsContainer, DeployerExt, OdraCli};

const CSPR: u64 = 1_000_000_000;

/// Deploying `Escrow` consumed 358.7 CSPR on the Casper VM (the `gas_of_every_call` test, on the
/// wasm `cargo odra build` writes). The network refunds three quarters of what a limit leaves
/// unused; a failed transaction is charged its whole limit.
const DEPLOY_GAS: u64 = 395 * CSPR;

/// Deploying `TestToken` consumed 274.6 CSPR on the Casper VM.
const TEST_TOKEN_DEPLOY_GAS: u64 = 310 * CSPR;

/// Deploys `Escrow`, or loads it if it is deployed already. The contract is deployed locked: its
/// package cannot get a new version.
pub struct EscrowDeployScript;

impl DeployScript for EscrowDeployScript {
    fn deploy(
        &self,
        env: &HostEnv,
        container: &mut DeployedContractsContainer,
    ) -> Result<(), odra_cli::deploy::Error> {
        let args = EscrowInitArgs {
            commission_bps: commission_bps(),
            commission_recipient: commission_recipient(env),
        };
        Escrow::load_or_deploy(env, args, container, DEPLOY_GAS)?;
        // A token to try the contract with, everywhere but on the mainnet.
        if std::env::var("ODRA_CASPER_LIVENET_CHAIN_NAME").as_deref() != Ok("casper") {
            TestToken::load_or_deploy(env, NoArgs, container, TEST_TOKEN_DEPLOY_GAS)?;
        }
        Ok(())
    }
}

/// The commission of the deployment, from `ESCROW_COMMISSION_BPS`: hundredths of a percent of what
/// sellers are paid. No commission if the variable is not set.
fn commission_bps() -> u32 {
    let bps = match std::env::var("ESCROW_COMMISSION_BPS") {
        Ok(text) => text
            .parse()
            .expect("ESCROW_COMMISSION_BPS is not a whole number"),
        Err(_) => 0,
    };
    assert!(
        bps <= MAX_COMMISSION_BPS,
        "ESCROW_COMMISSION_BPS is more than {MAX_COMMISSION_BPS}, the largest commission the contract accepts"
    );
    bps
}

/// The account the commission is paid to, from `ESCROW_COMMISSION_RECIPIENT` (`account-hash-…`).
/// The deploying account if the variable is not set.
fn commission_recipient(env: &HostEnv) -> Address {
    match std::env::var("ESCROW_COMMISSION_RECIPIENT") {
        Ok(text) => Address::from_str(&text)
            .expect("ESCROW_COMMISSION_RECIPIENT is not an address: account-hash-…"),
        Err(_) => env.caller(),
    }
}

/// Runs the command line tool.
pub fn main() {
    OdraCli::new()
        .about("CLI tool for the escrow contract")
        .deploy(EscrowDeployScript)
        .contract::<Escrow>()
        .contract::<TestToken>()
        .build()
        .run();
}
