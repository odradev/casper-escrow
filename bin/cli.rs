//! Command line tool of the escrow project: deploys the contract and calls it on a network.

use escrow::escrow::Escrow;
use escrow::testing::TestToken;
use odra::host::{HostEnv, NoArgs};
use odra_cli::{deploy::DeployScript, DeployedContractsContainer, DeployerExt, OdraCli};

const CSPR: u64 = 1_000_000_000;

/// Deploying `Escrow` consumed 340.6 CSPR on the Casper VM (the `gas_of_every_call` test, on the
/// wasm `cargo odra build` writes). The network refunds three quarters of what a limit leaves
/// unused; a failed transaction is charged its whole limit.
const DEPLOY_GAS: u64 = 375 * CSPR;

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
        Escrow::load_or_deploy(env, NoArgs, container, DEPLOY_GAS)?;
        // A token to try the contract with, everywhere but on the mainnet.
        if std::env::var("ODRA_CASPER_LIVENET_CHAIN_NAME").as_deref() != Ok("casper") {
            TestToken::load_or_deploy(env, NoArgs, container, TEST_TOKEN_DEPLOY_GAS)?;
        }
        Ok(())
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
