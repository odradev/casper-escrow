# Running Casper Escrow on the mainnet

This repository is a demo. Nobody runs this contract on the Casper mainnet, and its authors do not
plan to. This page is for someone who wants to: it says what you take on and what to do.

## What you take on

- **The contract has not been audited.** It has tests (see [SPEC.md](SPEC.md)), and that is all.
  Other people's money will sit in what you deploy. Have it reviewed by someone who did not write
  it before you ask anyone to use it.
- **It cannot be fixed.** A deployment is locked: there is no upgrade, no pause and no owner. If a
  flaw is found, all you can do is tell people to stop using it.
- **You may be running a regulated service.** Deploying code is one thing; running a site that
  takes a commission on other people's payments is another. In the EU this can make you a
  crypto-asset service provider under MiCA, and other places have their own rules. Ask a lawyer
  before you set a commission above zero.
- **The commission is final.** You choose it once, at deployment. It cannot be raised, lowered or
  redirected afterwards, by you or by anyone.

## 1. Build and test

You need Rust, `cargo-odra`, `wasm-opt`, `wasm-strip` and Node.js; the README lists them.

```sh
cargo odra test              # on OdraVM
cargo odra test -b casper    # on the Casper VM, with the wasm that will be deployed
```

`Cargo.lock` pins the exact commit of Odra the contract was tested with. Do not run
`cargo update` before deploying.

## 2. Choose the commission

Two values, both fixed for good:

- `ESCROW_COMMISSION_BPS`: hundredths of a percent of what sellers are paid. `50` is 0.5%; `0` is
  no commission; the contract accepts at most `1000`, which is 10%.
- `ESCROW_COMMISSION_RECIPIENT`: the account that receives it, as `account-hash-…`. It must be an
  account, not a contract. If you leave it out, it is the account that deploys.

The commission is taken only from what a seller is paid. A deposit that goes back to the buyer
pays nothing.

## 3. Deploy

Copy `.env.sample` to `.env` and point it at the mainnet:

```sh
ODRA_CASPER_LIVENET_SECRET_KEY_PATH=<path to your secret_key.pem>
ODRA_CASPER_LIVENET_NODE_ADDRESS=https://node.mainnet.casper.network
ODRA_CASPER_LIVENET_EVENTS_URL=https://node.mainnet.casper.network/events
ODRA_CASPER_LIVENET_CHAIN_NAME=casper
```

```sh
cargo odra build
ESCROW_COMMISSION_BPS=50 ESCROW_COMMISSION_RECIPIENT=account-hash-… cargo run --bin escrow_cli -- deploy
```

Deploying consumed 358.7 CSPR of gas on the Casper VM. The script sets a limit of 395 CSPR; the
network charges the limit and gives back part of what is left, so have about 400 CSPR on the
account. On the mainnet the script deploys the contract only, without the test token.

The package hash is written to `resources/casper-contracts.toml`. Check what you deployed:

```sh
cargo run --bin escrow_cli -- contract Escrow commission_bps
cargo run --bin escrow_cli -- contract Escrow commission_recipient
```

and look the package up in an explorer: its lock status must be `Locked`.

## 4. Build the app

```sh
cargo odra generate-client
cd web
npm run reader && npm install
npm run contracts casper
VITE_NETWORK=mainnet VITE_CSPR_CLICK_APP_ID=<your id> npm run build
```

- **CSPR.click.** The app signs through [CSPR.click](https://cspr.click). Register your domain
  there and pass the id you get as `VITE_CSPR_CLICK_APP_ID`.
- **The node.** A browser cannot call a Casper node directly: nodes send no CORS headers. The app
  is set to `https://mainnet-rpc.odra.dev`, a proxy of the Odra team that comes with no promise of
  staying up. For anything people rely on, run your own proxy and put its address in
  `web/src/config.ts`.
- **Names.** `.cspr` names are read from the storage of the CSPR.name contracts, whose addresses
  are in `web/src/config.ts`. If those contracts are upgraded, the reading in `web/src/names.ts`
  may need to follow.
- **Links.** `REPO_URL` in `web/src/config.ts` is where the app's "Source code" links point.
  Point it at the source you actually deployed.

`web/dist` is the whole site: static files for any host. There is no backend.

## 5. Before you tell anyone

- Make one deal in CSPR and one in a token with two accounts of your own, and close them every
  way you can: release, refund, a dispute with a decision, a settlement.
- Publish the package hash, the commit you built from and the commission, so that users can check
  the site against the chain.
- Say plainly on your site who runs it and that the contract is not audited, if it is not.
