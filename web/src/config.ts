// The networks the app can be built for. `VITE_NETWORK` chooses one at build time; a plain
// `npm run dev` runs against a local NCTL network.

/** The contracts of CSPR.name on a network: package hashes. */
export interface NameService {
  /** The JSON-RPC endpoint of a node of the network the names live on. */
  rpcUrl: string;
  nameToken: string;
  resolver: string;
  reverseResolver: string;
  /** Shown next to resolved names when they come from another network than the deals. */
  note?: string;
}

export interface Network {
  /** What the header shows. */
  label: string;
  chainName: string;
  /** The node, without the `/rpc` the clients add. */
  nodeUrl: string;
  /** The generated client wants a node for simulated calls too. The app makes none. */
  speculativeUrl: string;
  /** The block explorer, or `null` where there is none. */
  explorer: string | null;
  /** Whether transactions are signed by the development wallet instead of CSPR.click. */
  devWallet: boolean;
  /** Whether this is a place to try the contract, not to trust it with money. */
  demo: boolean;
  /** Whether the test token and its faucet are offered. */
  testToken: boolean;
  names: NameService | null;
}

const MAINNET_NAMES: NameService = {
  rpcUrl: "https://mainnet-rpc.odra.dev/rpc",
  nameToken: "hash-50d4d8cfff221e0084c461de81adfec06f04c6601cf4419ea56b9dc823f45488",
  resolver: "hash-2c41427f79cc69456a9ad38e8cb379cb0f1084f17c467b17ccb389346fd2c139",
  reverseResolver: "hash-925a459eeaa286736dd1f1d3887f0e5c5e73c0329ec664ce55043374dca20e85",
};

const NETWORKS: Record<string, Network> = {
  nctl: {
    label: "Local NCTL",
    chainName: "casper-net-1",
    // Served by the dev server, which passes the requests on to the node (see vite.config.ts).
    nodeUrl: `${window.location.origin}/nctl`,
    speculativeUrl: `${window.location.origin}/nctl`,
    explorer: null,
    devWallet: true,
    demo: true,
    testToken: true,
    // A local network has no names. The mainnet's are read instead, so the name fields can be
    // tried: the accounts they point at are the same keys on every network.
    names: { ...MAINNET_NAMES, note: "name read from the mainnet" },
  },
  testnet: {
    label: "Casper Testnet",
    chainName: "casper-test",
    nodeUrl: "https://testnet-rpc.odra.dev",
    speculativeUrl: "https://testnet-speculative-rpc.odra.dev",
    explorer: "https://testnet.cspr.live",
    devWallet: false,
    demo: true,
    testToken: true,
    // The testnet has no names of its own to show the feature with: the mainnet's are read.
    names: { ...MAINNET_NAMES, note: "name read from the mainnet" },
  },
  mainnet: {
    label: "Casper Mainnet",
    chainName: "casper",
    nodeUrl: "https://mainnet-rpc.odra.dev",
    speculativeUrl: "https://mainnet-rpc.odra.dev",
    explorer: "https://cspr.live",
    devWallet: false,
    demo: false,
    testToken: false,
    names: MAINNET_NAMES,
  },
};

const chosen = (import.meta.env.VITE_NETWORK as string | undefined) ?? "nctl";
if (!(chosen in NETWORKS)) throw new Error(`Unknown network "${chosen}" in VITE_NETWORK.`);

export const NETWORK: Network = NETWORKS[chosen];

/** The addresses the CLI wrote when it deployed to the chosen network. */
const files = import.meta.glob<{ contracts: { name: string; package_hash: string }[] }>(
  "./contracts/*.json",
  { eager: true, import: "default" },
);
export const DEPLOYED = files[`./contracts/${NETWORK.chainName}.json`] ?? null;

/** Where the source lives, and the guide to running a deployment of one's own. */
export const REPO_URL = "https://github.com/odradev/casper-escrow";
export const DEPLOY_GUIDE_URL = `${REPO_URL}/blob/main/DEPLOY.md`;

export const CSPR_CLICK_SCRIPT = "https://cdn.cspr.click/ui/v1.10.1/csprclick-client-1.10.1.js";
