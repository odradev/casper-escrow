# escrow

An escrow contract for the [Casper Network](https://casper.network), written in
[Odra](https://odra.dev). A buyer's deposit, in CSPR or a CEP-18 token, waits in the contract until
the deal with the seller is settled: released, refunded, decided by an arbiter the two chose, or
closed by a time limit. No owner and no upgrades; a deployment may take a commission, fixed when
it is deployed.

This repository is a demo. The contract has not been audited and is not deployed on the mainnet.
[DEPLOY.md](DEPLOY.md) is for whoever wants to run it there.

[SPEC.md](SPEC.md) says exactly what the contract does. The contract is `src/escrow.rs`, its tests
are `src/tests.rs`, and `src/testing.rs` holds tokens for tests and test networks.

## The web app

`web/` is a static app: it reads deals from a node's storage with
[odra-storage-reader](https://github.com/odradev/odra/tree/release/2.11.0/odra-storage-reader) and
sends transactions with the client `cargo odra generate-client` writes. There is no backend.

To run it against a local NCTL network:

```sh
docker run --rm --name mynctl -d -p 11101:11101 -p 14101:14101 -p 18101:18101 makesoftware/casper-nctl
mkdir -p .node-keys      # then copy the users' keys out of the container, see .env.sample
cargo odra build && cargo run --bin escrow_cli -- deploy
cargo odra generate-client
cd web
npm run reader && npm install
npm run contracts casper-net-1 && npm run devkeys
npm run dev
```

On a local network the app signs with NCTL's test accounts, chosen in the header, instead of a
wallet: CSPR.click knows only the public networks. `VITE_NETWORK=testnet` or `mainnet` builds the
app for a public network, once the contract is deployed there and its addresses are written with
`npm run contracts <chain>`.

## Setup

This project needs three things on your machine:

- Rust toolchain (see [rustup.rs](https://rustup.rs/)) — the pinned nightly from `rust-toolchain.toml`
  is installed automatically on the first build
- The `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [`cargo-odra`](https://github.com/odradev/cargo-odra): `cargo install cargo-odra --locked`

`wasm-strip` ([wabt](https://github.com/WebAssembly/wabt)) and `wasm-opt`
([binaryen](https://github.com/WebAssembly/binaryen), **version 121 or newer**) are needed to build
wasm files. See the [Installation guide](https://odra.dev/docs/getting-started/installation) for
details, or [Ubuntu / WSL setup](https://odra.dev/docs/getting-started/ubuntu-wsl-setup) for a
verified command-by-command walkthrough.

Run `rustup target add wasm32-unknown-unknown` **from inside this directory** — the toolchain is
pinned in `rust-toolchain.toml`, and running it elsewhere adds the target to the wrong toolchain.

## Usage

### Build

Compiles every contract listed in `Odra.toml` to wasm. The result files are placed in the
`${project-root}/wasm` directory.

```
$ cargo odra build
```

To build a single contract, pass its name from `Odra.toml` with `-c`:

```
$ cargo odra build -c my_contract
```

### Test
To run test on your local machine, you can basically execute the command:

```
$ cargo odra test
```

To test actual wasm files against a backend, 
you need to specify the backend passing -b argument to `cargo-odra`.

```
$ cargo odra test -b casper
```

## Documentation and tooling

- [Odra Docs and Tutorials](https://odra.dev/docs) — start here
- [Odra framework](https://github.com/odradev/odra) — source and
  [examples](https://github.com/odradev/odra/tree/HEAD/examples)
- [API reference](https://docs.rs/odra/latest/odra/)
- [Cargo Odra](https://github.com/odradev/cargo-odra) — `cargo odra` commands
- [Discord](https://discord.com/invite/Mm5ABc9P8k) — questions and help

### Working with an AI agent

If you use Claude Code, install the [Odra plugin](https://github.com/odradev/odradev-plugins)
so the agent knows Odra's APIs, project layout and tooling:

```
/plugin marketplace add odradev/odradev-plugins
/plugin install odra-plugin@odradev-plugins
```

Agents without the plugin should read [https://odra.dev/llms.txt](https://odra.dev/llms.txt)
first — it indexes the whole documentation set.
