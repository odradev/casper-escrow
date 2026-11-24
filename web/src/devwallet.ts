// A development wallet for a local NCTL network.
//
// The generated client sends transactions through `window.csprclick`, and CSPR.click knows only
// the public networks. On a local network this module stands in for it: it signs with the test
// keys of NCTL (src/dev-keys.json, written by `npm run devkeys`), sends the transaction to the
// node and reports the result in the shape CSPR.click uses. It is never loaded on a public
// network.
import type { Node } from "./storage";

export interface DevAccount {
  name: string;
  publicKey: string;
  pkcs8: string;
}

const keyFiles = import.meta.glob<DevAccount[]>("./dev-keys.json", { eager: true, import: "default" });
export const DEV_ACCOUNTS: DevAccount[] = keyFiles["./dev-keys.json"] ?? [];

const REMEMBERED = "casper-escrow-dev-account";
const POLL_MS = 1_000;
const GIVE_UP_MS = 180_000;

type Listener = (event: unknown) => void;
type StatusUpdate = (status: string, result: object) => void;

const fromBase64 = (text: string) => Uint8Array.from(atob(text), (char) => char.charCodeAt(0));
const fromHex = (text: string) => Uint8Array.from(text.match(/../g) ?? [], (byte) => parseInt(byte, 16));
const toHex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");

function accountInfo(account: DevAccount) {
  return {
    provider: "dev-wallet",
    providerSupports: [],
    cspr_name: null,
    public_key: account.publicKey,
    connected_at: Date.now(),
    token: null,
    custom: null,
    balance: null,
    liquid_balance: null,
    logo: null,
  };
}

interface ExecutionInfo {
  execution_info?: { execution_result?: { Version2?: { error_message: string | null } } };
}

/** Puts the development wallet at `window.csprclick` and tells the client it is there. */
export function installDevWallet(node: Node): { switchTo: (name: string) => void; active: () => DevAccount | null } {
  if (DEV_ACCOUNTS.length === 0) {
    throw new Error("No development accounts: run `npm run devkeys` with the NCTL container up.");
  }
  const listeners = new Map<string, Listener[]>();
  let active: DevAccount | null = null;

  const emit = (event: string, payload: unknown) => {
    for (const listener of listeners.get(event) ?? []) listener(payload);
  };

  const signIn = (account: DevAccount, event: string) => {
    active = account;
    try {
      localStorage.setItem(REMEMBERED, account.name);
    } catch {
      // Nothing to remember it in: the next visit starts signed out.
    }
    emit(event, { account: accountInfo(account) });
  };

  const sign = async (account: DevAccount, hashHex: string): Promise<string> => {
    const key = await crypto.subtle.importKey("pkcs8", fromBase64(account.pkcs8), { name: "Ed25519" }, false, [
      "sign",
    ]);
    const signature = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, key, fromHex(hashHex)));
    // `01` marks an ed25519 signature.
    return `01${toHex(signature)}`;
  };

  const send = async (transactionJson: string, publicKey: string, onStatus: StatusUpdate) => {
    const account = DEV_ACCOUNTS.find((candidate) => candidate.publicKey === publicKey);
    if (!account) throw new Error(`The development wallet has no key for ${publicKey}.`);
    const transaction = JSON.parse(transactionJson);
    const body = transaction.Version1;
    const hash: string = body.hash;
    body.approvals = [{ signer: account.publicKey, signature: await sign(account, hash) }];

    const result = (status: string, error: string | null) => ({
      status,
      cancelled: false,
      deployHash: null,
      transactionHash: hash,
      error,
      errorData: null,
      csprCloudTransaction: null,
    });

    await node.rpc("account_put_transaction", { transaction });
    onStatus("sent", result("sent", null));

    const started = Date.now();
    while (Date.now() - started < GIVE_UP_MS) {
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
      const info = await node.rpc<ExecutionInfo>("info_get_transaction", {
        transaction_hash: { Version1: hash },
      });
      const executed = info.execution_info?.execution_result?.Version2;
      if (executed) {
        console.info(`dev wallet: ${hash} ran`, executed);
        const final = result("processed", executed.error_message);
        onStatus("processed", final);
        return final;
      }
    }
    return result("timeout", "The local node did not run the transaction in time.");
  };

  const wallet = {
    on: (event: string, listener: Listener) => {
      listeners.set(event, [...(listeners.get(event) ?? []), listener]);
    },
    signIn: () => signIn(active ?? DEV_ACCOUNTS[1] ?? DEV_ACCOUNTS[0], "csprclick:signed_in"),
    signOut: () => {
      active = null;
      try {
        localStorage.removeItem(REMEMBERED);
      } catch {
        // Nothing was remembered.
      }
      emit("csprclick:signed_out", {});
    },
    disconnect: async () => wallet.signOut(),
    switchAccount: async () => {
      const next = DEV_ACCOUNTS[(DEV_ACCOUNTS.indexOf(active ?? DEV_ACCOUNTS[0]) + 1) % DEV_ACCOUNTS.length];
      signIn(next, "csprclick:switched_account");
    },
    getActivePublicKey: async () => active?.publicKey ?? "",
    getActiveAccountAsync: async () => (active ? { account: accountInfo(active) } : null),
    isUnlocked: async () => true,
    send,
  };
  (window as unknown as { csprclick: typeof wallet }).csprclick = wallet;
  window.dispatchEvent(new Event("csprclick:loaded"));

  // Back as the account of the last visit, once the client has attached its listeners.
  let remembered: string | null = null;
  try {
    remembered = localStorage.getItem(REMEMBERED);
  } catch {
    // Start signed out.
  }
  const previous = DEV_ACCOUNTS.find((account) => account.name === remembered);
  if (previous) setTimeout(() => signIn(previous, "csprclick:signed_in"), 0);

  return {
    switchTo: (name) => {
      const account = DEV_ACCOUNTS.find((candidate) => candidate.name === name);
      if (account) signIn(account, active ? "csprclick:switched_account" : "csprclick:signed_in");
    },
    active: () => active,
  };
}
