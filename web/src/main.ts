import "./style.css";
import init, {
  AccountInfo,
  Address,
  CsprClickCallbacks,
  EscrowErrors,
  EscrowWasmClient,
  OdraWasmClient,
  setGas,
  TestTokenWasmClient,
  TransactionResult,
  TransactionStatus,
  U256,
  U512,
} from "escrow-wasm-client";
import escrowSchema from "../../resources/casper_contract_schemas/escrow_schema.json";
import testTokenSchema from "../../resources/casper_contract_schemas/test_token_schema.json";
import { CSPR_CLICK_SCRIPT, DEPLOY_GUIDE_URL, DEPLOYED, NETWORK, REPO_URL } from "./config";
import { DEV_ACCOUNTS, installDevWallet } from "./devwallet";
import { looksLikeName, Names } from "./names";
import { ContractStorage, formattedKey, initStorage, Node } from "./storage";

// ---------- Constants ----------

const CSPR = 1_000_000_000n;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;
/** The limits of the contract (src/escrow.rs). */
const MIN_PERIOD = HOUR;
const MAX_PERIOD = 365 * DAY;
const MAX_MEMO_BYTES = 256;
/**
 * How far the app keeps from a time limit. The contract compares it with the time of the block
 * that runs the transaction; a clock that runs ahead of or behind the network would offer an
 * action the contract then refuses, and a failed transaction costs its whole limit.
 */
const CLOCK_MARGIN_MS = 15_000;
/** How many of an account's deals the list loads, newest first. */
const LIST_LIMIT = 40;
const REFRESH_MS = 20_000;

/**
 * Payment limits of the transactions the app sends, in motes, from what the calls consumed on
 * the Casper VM (SPEC.md) with a margin. A payout in CSPR costs about 2.5 CSPR, so closing a CSPR
 * deal needs more than closing a token deal. 2.5 CSPR is the practical minimum of a call.
 */
const GAS = {
  faucet: (5n * CSPR) / 2n,
  approve: (5n * CSPR) / 2n,
  // A CSPR deal goes through a proxy that carries the CSPR: 5.3 CSPR consumed on NCTL, against
  // 3.8 to 4.2 for a token deal.
  create: { token: 6n * CSPR, cspr: 7n * CSPR },
  step: (5n * CSPR) / 2n,
  onePayout: { token: (5n * CSPR) / 2n, cspr: (9n * CSPR) / 2n },
  twoPayouts: { token: (7n * CSPR) / 2n, cspr: 7n * CSPR },
  threePayouts: { token: 4n * CSPR, cspr: 10n * CSPR },
  fourPayouts: { token: (9n * CSPR) / 2n, cspr: (25n * CSPR) / 2n },
};

const ERRORS: Record<number, string> = {
  [EscrowErrors.ZeroDeposit]: "The amount must be greater than zero.",
  [EscrowErrors.DepositTooLarge]: "The amount is too large.",
  [EscrowErrors.SameParty]: "The buyer, the seller and the arbiter must be three different accounts.",
  [EscrowErrors.FeeTooLarge]: "The arbiter's fee must be less than the amount.",
  [EscrowErrors.FeeWithoutArbiter]: "A deal with no arbiter cannot have an arbiter's fee.",
  [EscrowErrors.BpsTooLarge]: "The default split is more than 100%.",
  [EscrowErrors.DeadlineNotInFuture]: "The delivery deadline must be in the future.",
  [EscrowErrors.DeadlineTooFar]: "The delivery deadline is more than a year away.",
  [EscrowErrors.PeriodTooShort]: "A period must be at least one hour.",
  [EscrowErrors.PeriodTooLong]: "A period must be at most 365 days.",
  [EscrowErrors.MemoTooLong]: "The description is too long.",
  [EscrowErrors.WrongAttachedValue]: "The CSPR sent with the deal does not match its amount.",
  [EscrowErrors.UnexpectedAttachedValue]: "A token deal must come with no CSPR.",
  [EscrowErrors.DepositMismatch]: "The token did not deliver the whole amount. This token cannot be used.",
  [EscrowErrors.DealNotFound]: "There is no deal with this number.",
  [EscrowErrors.DealClosed]: "This deal is closed already.",
  [EscrowErrors.NotBuyer]: "Only the buyer of the deal can do this.",
  [EscrowErrors.NotSeller]: "Only the seller of the deal can do this.",
  [EscrowErrors.NotArbiter]: "Only the arbiter of the deal can do this.",
  [EscrowErrors.NotFunded]: "The deal is not waiting for delivery any more.",
  [EscrowErrors.NotDelivered]: "The deal is not marked delivered.",
  [EscrowErrors.NotDisputed]: "The deal is not disputed.",
  [EscrowErrors.DeliveryDeadlinePassed]: "The delivery deadline has passed.",
  [EscrowErrors.ReviewPeriodPassed]: "The review period has passed.",
  [EscrowErrors.DisputePeriodPassed]: "The dispute period has passed.",
  [EscrowErrors.TooEarly]: "The time limit has not been reached yet.",
  [EscrowErrors.ShareTooLarge]: "The seller's part is more than the deal holds.",
  [EscrowErrors.ContractInCsprDeal]: "In a CSPR deal every party must be an account, not a contract.",
  [EscrowErrors.NotParty]: "Only the buyer or the seller can do this.",
};

// ---------- Types ----------

interface TokenMeta {
  symbol: string;
  decimals: number;
}

const CSPR_META: TokenMeta = { symbol: "CSPR", decimals: 9 };

type State = "Funded" | "Delivered" | "Disputed" | "Released" | "Refunded" | "Resolved" | "Split" | "Settled";
type Role = "buyer" | "seller" | "arbiter";

/** A deal as plain values, read once from the contract. */
interface Deal {
  id: bigint;
  buyer: string;
  seller: string;
  arbiter: string | null;
  token: string | null;
  deposit: bigint;
  arbiterFee: bigint;
  bps: number;
  deliveryDeadline: number;
  reviewPeriod: number;
  disputePeriod: number;
  memo: string;
  state: State;
  createdAt: number;
  deliveredAt: number;
  disputedAt: number;
  closedAt: number;
  toSeller: bigint;
  toBuyer: bigint;
  toArbiter: bigint;
  commission: bigint;
  buyerOffer: bigint | null;
  sellerOffer: bigint | null;
  meta: TokenMeta;
}

// ---------- State ----------

const node = new Node(`${NETWORK.nodeUrl}/rpc`);
let client: OdraWasmClient;
let escrow: EscrowWasmClient;
let escrowStorage: ContractStorage;
let escrowHash = "";
let testTokenHash: string | null = null;
/** The commission of this deployment: hundredths of a percent of what a seller is paid. */
let commissionBps = 0;
let commissionRecipient: string | null = null;
let names: Names | null = null;
let account: { publicKey: string; address: string } | null = null;
let busy = false;
/** Whether the contract can be read: the wasm modules are loaded and its storage is open. */
let ready = false;
let renderTicket = 0;
const tokenMetas = new Map<string, Promise<TokenMeta>>();
const tokenStorages = new Map<string, Promise<ContractStorage>>();
/** The development accounts by address, so the local network shows `user-2` instead of a hash. */
const devNames = new Map<string, string>();

// ---------- Small helpers ----------

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const view = $("view");

const ESCAPES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
const esc = (text: string) => text.replace(/[&<>"']/g, (char) => ESCAPES[char]);

function formatAmount(value: bigint, decimals: number): string {
  const base = 10n ** BigInt(decimals);
  const whole = (value / base).toLocaleString("en-US");
  const fraction = (value % base).toString().padStart(decimals, "0").slice(0, 4).replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : whole;
}

const money = (value: bigint, meta: TokenMeta) => `${formatAmount(value, meta.decimals)} ${esc(meta.symbol)}`;

/** Parses a decimal number into the smallest unit; `null` if it is not a valid amount. */
function parseAmount(text: string, decimals: number): bigint | null {
  const match = /^(\d+)(?:[.,](\d+))?$/.exec(text.trim().replace(/[\s_]/g, ""));
  if (!match) return null;
  const fraction = match[2] ?? "";
  if (fraction.length > decimals) return null;
  return BigInt(match[1]) * 10n ** BigInt(decimals) + BigInt(fraction.padEnd(decimals, "0") || "0");
}

const dateFormat = new Intl.DateTimeFormat("en-GB", {
  day: "numeric",
  month: "short",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});
const when = (ms: number) => dateFormat.format(new Date(ms));

/** A length of time in its two largest units: `4 d 6 h`, `3 h 12 min`, `40 s`. */
function span(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3_600);
  const minutes = Math.floor((total % 3_600) / 60);
  if (days > 0) return hours > 0 ? `${days} d ${hours} h` : `${days} d`;
  if (hours > 0) return minutes > 0 ? `${hours} h ${minutes} min` : `${hours} h`;
  if (minutes > 0) return `${minutes} min`;
  return `${total} s`;
}

/** A period the way a person set it: `7 days`, `36 hours`. */
function period(ms: number): string {
  const unit = ms % DAY === 0 ? DAY : HOUR;
  const count = Math.round((ms / unit) * 10) / 10;
  return `${count} ${unit === DAY ? "day" : "hour"}${count === 1 ? "" : "s"}`;
}

/** The value of a `datetime-local` input for a moment, in the local time zone. */
function toInputValue(ms: number): string {
  const date = new Date(ms);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * The formatted key of an address: `account-hash-…` or `hash-…`. `Address.value` prints another
 * form (`entity-account-…`, `contract-package-…`), which `new Address` does not read back.
 */
function key(parsed: Address): string {
  const value = parsed.value;
  const hex = value.slice(-64);
  return value.startsWith("contract-package-") || value.startsWith("hash-") ? `hash-${hex}` : `account-hash-${hex}`;
}

/**
 * Reads an address from a public key (`01…`, `02…`) or a formatted key (`account-hash-…`,
 * `hash-…`) and returns it as a formatted key. Throws if the text is neither.
 *
 * Addresses are kept as strings: a call of the client consumes the `Address` and `U256` objects
 * passed to it, so each call gets fresh ones (`address(text)`).
 */
function parseAddress(text: string): string {
  const value = text.trim();
  return key(/^0[12][0-9a-fA-F]{64,66}$/.test(value) ? Address.fromPublicKey(value) : new Address(value));
}

const address = (formatted: string) => new Address(formatted);
const shortKey = (formatted: string) => `${formatted.slice(-64, -60)}…${formatted.slice(-4)}`;

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : JSON.stringify(error);
}

// ---------- Names of accounts ----------

/**
 * An account as the page shows it: a short hash at first, replaced by its name once
 * `decorate` has looked it up. The whole key is in the title.
 */
function who(formatted: string): string {
  const mine = account?.address === formatted ? ' <span class="role">you</span>' : "";
  const label = devNames.get(formatted) ?? shortKey(formatted);
  const mono = devNames.has(formatted) ? "" : ' class="mono"';
  return `<span data-account="${esc(formatted)}" title="${esc(formatted)}"${mono}>${esc(label)}</span>${mine}`;
}

/** Fills in the CSPR.name names of the accounts shown under `root`. */
function decorate(root: HTMLElement) {
  if (!names) return;
  for (const element of root.querySelectorAll<HTMLElement>("[data-account]")) {
    const formatted = element.dataset.account!;
    if (!formatted.startsWith("account-hash-") || devNames.has(formatted)) continue;
    void names.nameOf(formatted).then((name) => {
      if (!name || !element.isConnected) return;
      element.textContent = name;
      element.classList.remove("mono");
    });
  }
}

/** What a person typed for a party: a name, a public key or a formatted key. */
async function resolveParty(text: string): Promise<{ address: string; note: string }> {
  const value = text.trim();
  if (looksLikeName(value)) {
    if (!names) throw new Error("Names cannot be read on this network. Enter a public key.");
    const found = await names.resolve(value);
    if (!found) throw new Error(`${value.toLowerCase()} does not point at an account.`);
    const from = names.note ? ` (${names.note})` : "";
    return { address: found, note: `${value.toLowerCase()} is ${shortKey(found)}${from}` };
  }
  let parsed: string;
  try {
    parsed = parseAddress(value);
  } catch {
    throw new Error(
      /^0[12][0-9a-fA-F]+$/.test(value)
        ? "This is not a valid public key. Check that it was copied whole."
        : "Enter a .cspr name, a public key or an account hash.",
    );
  }
  return { address: parsed, note: devNames.get(parsed) ?? shortKey(parsed) };
}

// ---------- Reading the contracts ----------

// State is read from the node's storage (see storage.ts). The generated client is used only to
// send transactions.

/**
 * The storage of a token. Every token is read with the schema of `TestToken`, which keeps the
 * state of Odra's `Cep18` module in a field called `token`; a token laid out differently fails
 * to read.
 */
function tokenStorage(token: string): Promise<ContractStorage> {
  let storage = tokenStorages.get(token);
  if (!storage) {
    storage = ContractStorage.open(node, testTokenSchema, token);
    storage.catch(() => tokenStorages.delete(token));
    tokenStorages.set(token, storage);
  }
  return storage;
}

function tokenMeta(token: string | null): Promise<TokenMeta> {
  if (token === null) return Promise.resolve(CSPR_META);
  let meta = tokenMetas.get(token);
  if (!meta) {
    meta = (async () => {
      const storage = await tokenStorage(token);
      const symbol = await storage.read("token.symbol");
      const decimals = await storage.read("token.decimals");
      if (symbol === null || decimals === null) throw new Error("This is not a CEP-18 token the app can read.");
      return { symbol, decimals: Number(decimals) };
    })();
    meta.catch(() => tokenMetas.delete(token));
    tokenMetas.set(token, meta);
  }
  return meta;
}

async function tokenAmount(token: string, path: string, keys: string[]): Promise<bigint> {
  return BigInt((await (await tokenStorage(token)).read(path, keys)) ?? "0");
}

const balanceOf = (token: string | null, owner: string) =>
  token === null ? node.balance(owner) : tokenAmount(token, "token.balances", [owner]);

const optional = (printed: string) => (printed === "None" ? null : printed);

async function loadDeal(id: bigint): Promise<Deal | null> {
  const stored = await escrowStorage.read("deals", [id.toString()]);
  if (stored === null) return null;
  // A struct is printed as a JSON object whose values are all strings; an enum by its variant
  // name; an empty option as `None`.
  const deal: Record<string, string> = JSON.parse(stored);
  const arbiter = optional(deal.arbiter);
  const token = optional(deal.token);
  const buyerOffer = optional(deal.buyer_offer);
  const sellerOffer = optional(deal.seller_offer);
  return {
    id,
    buyer: formattedKey(deal.buyer),
    seller: formattedKey(deal.seller),
    arbiter: arbiter === null ? null : formattedKey(arbiter),
    token: token === null ? null : formattedKey(token),
    deposit: BigInt(deal.deposit),
    arbiterFee: BigInt(deal.arbiter_fee),
    bps: Number(deal.default_seller_bps),
    deliveryDeadline: Number(deal.delivery_deadline),
    reviewPeriod: Number(deal.review_period),
    disputePeriod: Number(deal.dispute_period),
    memo: deal.memo,
    state: deal.state as State,
    createdAt: Number(deal.created_at),
    deliveredAt: Number(deal.delivered_at),
    disputedAt: Number(deal.disputed_at),
    closedAt: Number(deal.closed_at),
    toSeller: BigInt(deal.to_seller),
    toBuyer: BigInt(deal.to_buyer),
    toArbiter: BigInt(deal.to_arbiter),
    commission: BigInt(deal.commission),
    buyerOffer: buyerOffer === null ? null : BigInt(buyerOffer),
    sellerOffer: sellerOffer === null ? null : BigInt(sellerOffer),
    meta: await tokenMeta(token === null ? null : formattedKey(token)).catch(() => ({ symbol: "units", decimals: 0 })),
  };
}

/** The ids of the deals of an account, newest first, at most `LIST_LIMIT`. */
async function dealIdsOf(owner: string): Promise<{ ids: bigint[]; total: number }> {
  const total = Number((await escrowStorage.read("count_of", [owner])) ?? "0");
  const positions = Array.from({ length: Math.min(total, LIST_LIMIT) }, (_, i) => total - 1 - i);
  const ids = await Promise.all(positions.map((position) => escrowStorage.read("ids_of", [`${owner}:${position}`])));
  return { ids: ids.filter((id): id is string => id !== null).map(BigInt), total };
}

// ---------- What a deal means ----------

const isOpen = (deal: Deal) => deal.state === "Funded" || deal.state === "Delivered" || deal.state === "Disputed";

/** The moment the time limit of an open deal's state is reached. */
function limitOf(deal: Deal): number {
  if (deal.state === "Funded") return deal.deliveryDeadline;
  if (deal.state === "Delivered") return deal.deliveredAt + deal.reviewPeriod;
  return deal.disputedAt + deal.disputePeriod;
}

function roleOf(deal: Deal, owner: string | undefined): Role | null {
  if (!owner) return null;
  if (owner === deal.buyer) return "buyer";
  if (owner === deal.seller) return "seller";
  return owner === deal.arbiter ? "arbiter" : null;
}

const commissionPercent = () => `${commissionBps / 100}%`;

function splitText(deal: Deal): string {
  const seller = deal.bps / 100;
  return `${seller}% to the seller and ${100 - seller}% back to the buyer`;
}

function pill(deal: Deal, now: number): string {
  const [tone, text] = ((): [string, string] => {
    const late = isOpen(deal) && now >= limitOf(deal);
    switch (deal.state) {
      case "Funded":
        return late ? ["quiet", "Delivery deadline passed"] : ["held", "Held · waiting for delivery"];
      case "Delivered":
        return late ? ["held", "Review ended"] : ["held", "Held · in review"];
      case "Disputed":
        return late ? ["bad", "Dispute undecided"] : ["bad", "Disputed"];
      case "Released":
        return ["ok", "Paid to seller"];
      case "Refunded":
        return ["quiet", "Returned to buyer"];
      case "Resolved":
        return ["ok", "Decided by arbiter"];
      case "Split":
        return ["quiet", "Split by default"];
      case "Settled":
        return ["ok", "Settled"];
    }
  })();
  return `<span class="pill ${tone}">${text}</span>`;
}

/** Whether the deal is waiting for the account in the given role to do something. */
function waitsFor(deal: Deal, role: Role, now: number): boolean {
  if (!isOpen(deal)) return false;
  const late = now >= limitOf(deal);
  const theirs = role === "buyer" ? deal.sellerOffer : deal.buyerOffer;
  const mine = role === "buyer" ? deal.buyerOffer : deal.sellerOffer;
  if (role !== "arbiter" && theirs !== null && theirs !== mine) return true;
  if (role === "buyer") return (deal.state === "Delivered" && !late) || (deal.state !== "Delivered" && late);
  if (role === "seller") return (deal.state === "Funded" && !late) || (deal.state !== "Funded" && late);
  return deal.state === "Disputed" && !late;
}

/** What comes next in a deal, for a list row. */
function nextText(deal: Deal, role: Role, now: number): string {
  if (!isOpen(deal)) return when(deal.closedAt);
  const limit = limitOf(deal);
  const late = now >= limit;
  const date = when(limit);
  if (deal.state === "Funded") {
    if (late) return role === "buyer" ? "you can take it back" : "can return to the buyer";
    return role === "seller" ? `deliver by ${date}` : `delivery due ${date}`;
  }
  if (deal.state === "Delivered") {
    if (late) return role === "seller" ? "you can collect it" : "the seller can collect it";
    return role === "buyer" ? `release or dispute by ${date}` : `review ends ${date}`;
  }
  if (late) return "the default split can be applied";
  return role === "arbiter" ? `you decide by ${date}` : `arbiter decides by ${date}`;
}

// ---------- Transactions ----------

const toast = $("toast");
const toastText = $("toast-text");
const toastLink = $<HTMLAnchorElement>("toast-link");

function showTx(text: string, options: { hash?: string; error?: boolean } = {}) {
  toastText.textContent = text;
  toast.classList.toggle("error", options.error === true);
  const linked = Boolean(options.hash && NETWORK.explorer);
  if (linked) toastLink.href = `${NETWORK.explorer}/transaction/${options.hash}`;
  toastLink.hidden = !linked;
  toast.hidden = false;
}

function failureText(result: TransactionResult): string {
  if (result.errorCode !== undefined) {
    return ERRORS[result.errorCode] ?? `The contract reported error ${result.errorCode}.`;
  }
  return result.error ?? "The transaction failed.";
}

/** Sends one transaction and waits until it is processed. Returns whether it succeeded. */
async function send(label: string, gas: bigint, call: () => Promise<TransactionResult>): Promise<boolean> {
  showTx(`${label}: confirm it in your wallet…`);
  try {
    setGas(gas);
    const result = await call();
    if (result.isCancelled) {
      showTx(`${label}: you declined to sign.`, { error: true });
      return false;
    }
    if (result.error || result.errorCode !== undefined) {
      showTx(`${label} failed. ${failureText(result)}`, { hash: result.txHash, error: true });
      return false;
    }
    showTx(`${label}: done.`, { hash: result.txHash });
    return true;
  } catch (error) {
    showTx(`${label} failed. ${errorText(error)}`, { error: true });
    return false;
  }
}

/** Runs an action made of transactions with the buttons disabled, then reloads the state. */
async function withBusy(action: () => Promise<void>) {
  if (busy) return;
  busy = true;
  for (const button of view.querySelectorAll<HTMLButtonElement>("button")) button.disabled = true;
  try {
    await action();
  } finally {
    busy = false;
    node.refresh();
    await render();
  }
}

type Payouts = "onePayout" | "twoPayouts" | "threePayouts";
const WITH_COMMISSION = { onePayout: "twoPayouts", twoPayouts: "threePayouts", threePayouts: "fourPayouts" } as const;

/** The limit for closing a deal with so many payouts; a commission is one payout more. */
const payoutGas = (deal: Deal, payouts: Payouts) =>
  GAS[commissionBps > 0 ? WITH_COMMISSION[payouts] : payouts][deal.token === null ? "cspr" : "token"];

// ---------- View: home ----------

function renderHome() {
  const contract = NETWORK.explorer
    ? `<a href="${NETWORK.explorer}/contract-package/${escrowHash.slice(5)}" target="_blank" rel="noopener">view in the explorer</a>`
    : `<span class="muted">${esc(NETWORK.label)}</span>`;
  view.innerHTML = `
    <div class="hero">
      <div>
        <h1>Pay when it’s delivered.</h1>
        <p class="lead">Casper Escrow holds a payment in a smart contract until both sides are done. There is no company in the middle and nobody who can touch the money except the rules you both agreed to.</p>
        <div class="cta">
          <a class="btn primary" href="#/new">Create a deal</a>
          <a class="btn" href="#/deals">My deals</a>
        </div>
        <div class="facts">
          <span id="home-commission"></span>
          <span><b>CSPR</b> and any CEP-18 token</span>
          <span><b>No owner,</b> no upgrades</span>
          <span><b>Open</b> source</span>
        </div>
      </div>
      <form class="sheet mini" id="lookup">
        <h3><label for="lookup-id">Open a deal</label></h3>
        <p class="muted">Anyone can look at a deal by its number. No wallet is needed.</p>
        <div class="lookup">
          <input id="lookup-id" class="num" inputmode="numeric" placeholder="Deal number" autocomplete="off" />
          <button class="btn">Open</button>
        </div>
        <span class="hint" id="lookup-hint"></span>
      </form>
    </div>

    <div class="section">
      <h2>Nobody answers? The deal still closes.</h2>
      <p class="muted" style="max-width:65ch">Every stage has a time limit. When it passes, anyone can close the deal, and the contract pays by the rule written into it on the first day.</p>
      <div class="sheet scroll">
        <table>
          <thead><tr><th>If this happens</th><th>When</th><th>The deposit goes</th></tr></thead>
          <tbody>
            <tr><td>The seller never delivers</td><td class="muted">at the delivery deadline</td><td>back to the buyer</td></tr>
            <tr><td>The buyer says nothing after delivery</td><td class="muted">when the review period ends</td><td>to the seller</td></tr>
            <tr><td>The arbiter never decides a dispute</td><td class="muted">when the dispute period ends</td><td>split the way the deal says, for example 50 / 50</td></tr>
          </tbody>
        </table>
      </div>
    </div>

    <div class="sheet uses">
      <div><h3>Freelance work</h3><p>The client deposits before you start. You get paid when they accept, or once their time to check your delivery has run out.</p></div>
      <div><h3>Selling to a stranger</h3><p>The buyer’s money is locked before you ship, and yours to claim once they have had time to check.</p></div>
      <div><h3>Over-the-counter trades</h3><p>One side deposits, the other sends. An arbiter you both name steps in only if you disagree.</p></div>
    </div>

    <div class="section">
      <h2>Check it yourself</h2>
      <dl class="sheet verify">
        <dt>Contract</dt><dd><span class="mono">${esc(escrowHash)}</span> · ${contract}</dd>
        <dt>Owner or admin keys</dt><dd>None</dd>
        <dt>Upgrades</dt><dd>Not possible. The contract package is locked.</dd>
        <dt>Commission</dt><dd id="home-commission-detail"></dd>
        <dt>Source code</dt><dd><a href="${REPO_URL}" target="_blank" rel="noopener">${REPO_URL.replace("https://", "")}</a></dd>
        <dt>Security audit</dt><dd>Not audited yet.</dd>
      </dl>
    </div>`;

  fillHome();

  $<HTMLFormElement>("lookup").addEventListener("submit", (event) => {
    event.preventDefault();
    const value = $<HTMLInputElement>("lookup-id").value.trim();
    if (/^\d+$/.test(value)) location.hash = `#/deal/${value}`;
    else $("lookup-hint").textContent = "Enter the number of a deal.";
  });
}

/**
 * Fills in what the start page says about the commission, which is read from the contract. The
 * page itself is drawn at once, before anything is loaded.
 */
function fillHome() {
  const fact = document.getElementById("home-commission");
  const detail = document.getElementById("home-commission-detail");
  if (!fact || !detail) return;
  if (!ready) {
    fact.innerHTML = `<b>…</b> commission`;
    detail.textContent = "Reading it from the contract…";
    return;
  }
  fact.innerHTML = `<b>${commissionBps > 0 ? commissionPercent() : "No"}</b> commission`;
  detail.innerHTML =
    commissionBps > 0
      ? `${commissionPercent()} of what a seller is paid, to ${who(commissionRecipient!)}. Nothing is taken from a deposit that goes back to the buyer. It was set when the contract was deployed and cannot be changed.`
      : "None. You pay only the network’s transaction fee.";
  decorate(detail);
}

// ---------- View: a deal ----------

interface Phase {
  name: string;
  start: number;
  end: number;
  live: boolean;
}

/** The stretches of time a deal went through, and the one it is in. */
function phases(deal: Deal): Phase[] {
  const open = isOpen(deal);
  const list: Phase[] = [];
  const deliveryEnd = deal.deliveredAt || (open ? deal.deliveryDeadline : deal.closedAt);
  list.push({ name: "Delivery", start: deal.createdAt, end: deliveryEnd, live: deal.state === "Funded" });
  if (deal.deliveredAt) {
    const end = deal.disputedAt || (open ? deal.deliveredAt + deal.reviewPeriod : deal.closedAt);
    list.push({ name: "Review", start: deal.deliveredAt, end, live: deal.state === "Delivered" });
  }
  if (deal.disputedAt) {
    const end = open ? deal.disputedAt + deal.disputePeriod : deal.closedAt;
    list.push({ name: "Dispute", start: deal.disputedAt, end, live: deal.state === "Disputed" });
  }
  return list;
}

function trackHtml(deal: Deal, now: number): string {
  const list = phases(deal);
  const total = Math.max(1, list[list.length - 1].end - deal.createdAt);
  const segments = list
    .map((phase) => {
      const length = Math.max(0, phase.end - phase.start);
      const width = Math.max(4, (length / total) * 100);
      if (!phase.live) return `<div class="seg done" style="flex:${width} 0 0"><span>${phase.name}</span></div>`;
      const passed = length === 0 ? 100 : Math.min(100, Math.max(0, ((now - phase.start) / length) * 100));
      const tone = phase.name === "Dispute" ? " bad" : "";
      return `<div class="seg live${tone}" style="flex:${width} 0 0"><i style="width:${passed}%"></i><span>${phase.name} · ${period(length)}</span></div>`;
    })
    .join("");

  const marks: string[] = [`<div><b class="num">${when(deal.createdAt)}</b><span class="muted">deposited</span></div>`];
  if (deal.deliveredAt) {
    marks.push(`<div><b class="num">${when(deal.deliveredAt)}</b><span class="muted">marked delivered</span></div>`);
  }
  if (deal.disputedAt) {
    marks.push(`<div><b class="num">${when(deal.disputedAt)}</b><span class="muted">disputed</span></div>`);
  }
  if (isOpen(deal)) {
    const limit = limitOf(deal);
    const what =
      deal.state === "Funded" ? "delivery deadline" : deal.state === "Delivered" ? "review ends" : "dispute period ends";
    const left = now < limit ? `in ${span(limit - now)}` : `${span(now - limit)} ago`;
    marks.push(`<div class="next"><b class="num">${when(limit)}</b><span class="muted">${what} · ${left}</span></div>`);
  } else {
    marks.push(`<div><b class="num">${when(deal.closedAt)}</b><span class="muted">closed</span></div>`);
  }
  return `<div class="bar" role="img" aria-label="The stages of the deal, drawn to scale">${segments}</div><div class="marks">${marks.join("")}</div>`;
}

function historyHtml(deal: Deal): string {
  const rows: [number, string][] = [[deal.createdAt, `${who(deal.buyer)} deposited ${money(deal.deposit, deal.meta)}`]];
  if (deal.deliveredAt) rows.push([deal.deliveredAt, `${who(deal.seller)} marked the deal delivered`]);
  if (deal.disputedAt) rows.push([deal.disputedAt, `${who(deal.buyer)} disputed the delivery`]);
  if (!isOpen(deal)) {
    const how: Record<string, string> = {
      Released: "The deposit was paid to the seller",
      Refunded: "The deposit went back to the buyer",
      Resolved: "The arbiter decided",
      Split: "Nobody decided in time and the default split was applied",
      Settled: "The buyer and the seller settled",
    };
    const parts = [
      `${money(deal.toSeller, deal.meta)} to the seller`,
      `${money(deal.toBuyer, deal.meta)} to the buyer`,
      ...(deal.toArbiter > 0n ? [`${money(deal.toArbiter, deal.meta)} to the arbiter`] : []),
      ...(deal.commission > 0n ? [`${money(deal.commission, deal.meta)} commission`] : []),
    ];
    rows.push([deal.closedAt, `${how[deal.state]}: ${parts.join(", ")}`]);
  }
  return rows
    .reverse()
    .map(([at, text]) => `<li><span class="muted num">${when(at)}</span><span>${text}</span></li>`)
    .join("");
}

/** The panel that says what the viewer can do now, and what happens if nobody does anything. */
function moveHtml(deal: Deal, role: Role | null, now: number): string {
  const amount = money(deal.deposit, deal.meta);
  if (!isOpen(deal)) {
    return `<span class="label">Closed</span><p>This deal is closed. Nothing more can be done with it.</p>`;
  }
  const limit = limitOf(deal);
  const late = now >= limit;
  // Offered only at a safe distance from the limit: see CLOCK_MARGIN_MS.
  const before = now < limit - CLOCK_MARGIN_MS;
  const after = now >= limit + CLOCK_MARGIN_MS;
  const off = busy ? " disabled" : "";
  const button = (action: string, text: string, kind = "") =>
    `<button class="btn ${kind} wide" data-action="${action}"${off}>${text}</button>`;
  const settle =
    deal.state === "Funded"
      ? "Return the deposit to the buyer"
      : deal.state === "Delivered"
        ? "Pay the seller"
        : "Apply the default split";

  if (!account) {
    return `<span class="label">Your move</span><p>Connect a wallet to act on this deal.</p>
      ${after ? `<p class="muted small">Its time limit has passed: anyone can close it.</p>` : ""}`;
  }
  if (role === null) {
    return `<span class="label">You are not a party</span>
      ${after ? `<p>The time limit of this deal has passed. Anyone can close it.</p>${button("settle", settle, "primary")}` : `<p>Only the buyer, the seller and the arbiter can act on this deal.</p>`}`;
  }

  const parts: string[] = [`<span class="label">Your move, as the ${role}</span>`];
  if (role === "buyer") {
    if (deal.state === "Funded") {
      if (after) parts.push(button("settle", `Take ${amount} back`, "primary"));
      parts.push(button("release", `Release ${amount} to the seller`, after ? "" : "primary"));
      parts.push(
        late
          ? `<div class="default">The seller did not deliver in time. You can take the deposit back.</div>`
          : `<div class="default"><b>If the seller does not deliver</b> by ${when(limit)}, the deposit can come back to you.</div>`,
      );
    } else if (deal.state === "Delivered") {
      parts.push(button("release", `Release ${amount} to the seller`, "primary"));
      if (before) parts.push(button("dispute", "Dispute the delivery", "danger"));
      parts.push(
        late
          ? `<div class="default">The review period is over. The seller can collect the whole deposit.</div>`
          : `<div class="default"><b>If you do nothing:</b> from ${when(limit)} the whole deposit can be paid to the seller.</div>`,
      );
      if (before) {
        parts.push(
          deal.arbiter
            ? `<p class="muted small">A dispute hands the decision to the arbiter. This page carries no messages or files: make your case to the arbiter directly.</p>`
            : `<p class="muted small">This deal has no arbiter. After a dispute the deposit is split ${splitText(deal)}, unless the two of you settle first.</p>`,
        );
      }
    } else {
      if (after) parts.push(button("settle", settle, "primary"));
      parts.push(button("release", `Release ${amount} to the seller`));
      parts.push(
        `<div class="default">${late ? "Nobody decided in time." : `${deal.arbiter ? "The arbiter can decide" : "You can settle"} until ${when(limit)}.`} After that the deposit is split ${splitText(deal)}.</div>`,
      );
    }
  } else if (role === "seller") {
    if (deal.state === "Funded") {
      if (before) parts.push(button("deliver", "Mark as delivered", "primary"));
      parts.push(button("refund", `Send ${amount} back to the buyer`));
      parts.push(
        late
          ? `<div class="default">The delivery deadline has passed. The deposit can go back to the buyer.</div>`
          : `<div class="default"><b>If you do not mark it delivered</b> by ${when(limit)}, the deposit goes back to the buyer.</div>`,
      );
    } else if (deal.state === "Delivered") {
      if (after) parts.push(button("settle", `Collect ${amount}`, "primary"));
      parts.push(button("refund", `Send ${amount} back to the buyer`));
      parts.push(
        late
          ? `<div class="default">The review period is over. The deposit is yours to collect.</div>`
          : `<div class="default">The buyer can dispute until ${when(limit)}. After that the deposit is yours to collect.</div>`,
      );
    } else {
      if (after) parts.push(button("settle", settle, "primary"));
      parts.push(button("refund", `Send ${amount} back to the buyer`));
      parts.push(
        `<div class="default">${late ? "Nobody decided in time." : `${deal.arbiter ? "The arbiter can decide" : "You can settle"} until ${when(limit)}.`} After that the deposit is split ${splitText(deal)}.</div>`,
      );
    }
  } else if (deal.state === "Disputed" && before) {
    const most = deal.deposit - deal.arbiterFee;
    parts.push(`
      <div class="field">
        <label for="resolve-share">The seller gets (${esc(deal.meta.symbol)})</label>
        <input id="resolve-share" class="num" inputmode="decimal" placeholder="0 to ${formatAmount(most, deal.meta.decimals)}" autocomplete="off" />
        <span class="hint" id="resolve-hint">You take ${money(deal.arbiterFee, deal.meta)}. The rest goes back to the buyer.</span>
      </div>
      ${button("resolve", "Decide the dispute", "primary")}
      <div class="default"><b>If you do not decide</b> by ${when(limit)}, the deposit is split ${splitText(deal)} and you are not paid.</div>`);
  } else if (deal.state === "Disputed") {
    if (after) parts.push(button("settle", settle, "primary"));
    parts.push(`<p>The time to decide this dispute has passed.</p>`);
  } else {
    parts.push(`<p>There is nothing for you to do unless the buyer disputes the delivery.</p>`);
  }
  return parts.join("");
}

function offersHtml(deal: Deal, role: Role | null): string {
  const line = (label: string, offer: bigint | null) =>
    `<div class="row"><span class="muted">${label}</span><span class="num">${offer === null ? "no offer" : `${money(offer, deal.meta)} to the seller, ${money(deal.deposit - offer, deal.meta)} back`}</span></div>`;
  const rows = line("The buyer offers", deal.buyerOffer) + line("The seller asks", deal.sellerOffer);
  if (!isOpen(deal) || (role !== "buyer" && role !== "seller")) return rows;

  const mine = role === "buyer" ? deal.buyerOffer : deal.sellerOffer;
  const theirs = role === "buyer" ? deal.sellerOffer : deal.buyerOffer;
  const off = busy ? " disabled" : "";
  return `${rows}
    <div class="offerline">
      <input id="offer-share" class="num" inputmode="decimal" placeholder="For the seller, in ${esc(deal.meta.symbol)}" aria-label="Amount for the seller" autocomplete="off" />
      <button class="btn" data-action="offer"${off}>${mine === null ? "Offer" : "Change my offer"}</button>
      ${theirs !== null && theirs !== mine ? `<button class="btn primary" data-action="accept"${off}>Accept ${money(theirs, deal.meta)} for the seller</button>` : ""}
      ${mine !== null ? `<button class="btn" data-action="withdraw"${off}>Withdraw</button>` : ""}
    </div>
    <span class="hint" id="offer-hint">The deal closes the moment both of you name the same amount. An offer does not stop any deadline.</span>`;
}

async function renderDeal(id: bigint, ticket: number) {
  const deal = await loadDeal(id);
  if (ticket !== renderTicket) return;
  if (!deal) {
    view.innerHTML = `<div class="sheet empty">There is no deal ${id}. <a href="#/">Back to the start</a></div>`;
    return;
  }
  const now = Date.now();
  const role = roleOf(deal, account?.address);
  const disputed = deal.arbiter
    ? `the arbiter has ${period(deal.disputePeriod)}; with no decision, ${splitText(deal)}`
    : `no arbiter; after ${period(deal.disputePeriod)}, ${splitText(deal)}`;

  view.innerHTML = `
    <div class="dealhead">
      <div class="row"><span class="label">Deal ${deal.id} · created ${when(deal.createdAt)}</span>${pill(deal, now)}</div>
      <div class="amount num">${money(deal.deposit, deal.meta)}</div>
      <div class="muted memo">${esc(deal.memo) || "No description"}</div>
    </div>
    <div class="dealgrid">
      <div class="stack">
        <div class="sheet pad track"><h3>Where this deal is</h3>${trackHtml(deal, now)}</div>
        <div class="sheet pad stack">
          <h3>The deal</h3>
          <dl class="kv">
            <dt>Buyer</dt><dd>${who(deal.buyer)}</dd>
            <dt>Seller</dt><dd>${who(deal.seller)}</dd>
            <dt>Arbiter</dt><dd>${deal.arbiter ? `${who(deal.arbiter)} · takes ${money(deal.arbiterFee, deal.meta)} for a decision` : "None"}</dd>
            <dt>Deliver by</dt><dd class="num">${when(deal.deliveryDeadline)}</dd>
            <dt>Review</dt><dd>${period(deal.reviewPeriod)} after delivery</dd>
            <dt>If disputed</dt><dd>${disputed}</dd>
            ${commissionBps > 0 ? `<dt>Commission</dt><dd>${commissionPercent()} of what the seller is paid</dd>` : ""}
            ${deal.token ? `<dt>Token</dt><dd class="mono">${esc(deal.token)}</dd>` : ""}
          </dl>
        </div>
        <div class="sheet pad offers"><h3>Settle between the two of you</h3>${offersHtml(deal, role)}</div>
        <div class="sheet pad stack"><h3>History</h3><ul class="hist">${historyHtml(deal)}</ul></div>
      </div>
      <aside class="sheet pad move">
        ${moveHtml(deal, role, now)}
        <div class="row share">
          <span class="mono muted small">${esc(location.href)}</span>
          <button class="btn small" data-action="copy">Copy link</button>
        </div>
      </aside>
    </div>`;
  decorate(view);

  const on = (action: string, handler: () => void) => {
    for (const button of view.querySelectorAll<HTMLButtonElement>(`[data-action="${action}"]`)) {
      button.addEventListener("click", handler);
    }
  };
  const run = (label: string, gas: bigint, call: () => Promise<TransactionResult>) =>
    void withBusy(async () => void (await send(label, gas, call)));

  on("copy", () => void navigator.clipboard?.writeText(location.href).then(() => showTx("Link copied.")));
  on("release", () =>
    run(`Releasing deal ${deal.id} to the seller`, payoutGas(deal, "onePayout"), () => escrow.release(deal.id)),
  );
  on("refund", () =>
    run(`Sending deal ${deal.id} back to the buyer`, payoutGas(deal, "onePayout"), () => escrow.refund(deal.id)),
  );
  on("deliver", () => run(`Marking deal ${deal.id} delivered`, GAS.step, () => escrow.markDelivered(deal.id)));
  on("dispute", () => run(`Disputing deal ${deal.id}`, GAS.step, () => escrow.dispute(deal.id)));
  on("settle", () =>
    run(`Closing deal ${deal.id}`, payoutGas(deal, deal.state === "Disputed" ? "twoPayouts" : "onePayout"), () =>
      escrow.settle(deal.id),
    ),
  );
  on("resolve", () => {
    const share = parseAmount($<HTMLInputElement>("resolve-share").value, deal.meta.decimals);
    if (share === null || share > deal.deposit - deal.arbiterFee) {
      const hint = $("resolve-hint");
      hint.textContent = `Enter an amount from 0 to ${formatAmount(deal.deposit - deal.arbiterFee, deal.meta.decimals)}.`;
      hint.className = "hint bad";
      return;
    }
    run(`Deciding deal ${deal.id}`, payoutGas(deal, "threePayouts"), () =>
      escrow.resolve(deal.id, U256.fromBigInt(share)),
    );
  });

  const offer = (share: bigint | null, label: string) => {
    const theirs = role === "buyer" ? deal.sellerOffer : deal.buyerOffer;
    // An offer equal to the other party's closes the deal and pays out.
    const gas = share !== null && share === theirs ? payoutGas(deal, "twoPayouts") : GAS.step;
    run(label, gas, () => escrow.offerSplit(deal.id, share === null ? null : U256.fromBigInt(share)));
  };
  on("offer", () => {
    const share = parseAmount($<HTMLInputElement>("offer-share").value, deal.meta.decimals);
    if (share === null || share > deal.deposit) {
      const hint = $("offer-hint");
      hint.textContent = `Enter what the seller would get: from 0 to ${formatAmount(deal.deposit, deal.meta.decimals)}.`;
      hint.className = "hint bad";
      return;
    }
    offer(share, `Offering a split of deal ${deal.id}`);
  });
  on("accept", () => offer(role === "buyer" ? deal.sellerOffer : deal.buyerOffer, `Settling deal ${deal.id}`));
  on("withdraw", () => offer(null, `Withdrawing the offer on deal ${deal.id}`));
}

// ---------- View: my deals ----------

async function renderDeals(ticket: number) {
  if (!account) {
    view.innerHTML = `<div class="row"><h2>My deals</h2></div><div class="sheet empty">Connect a wallet to see the deals you are a party of.</div>`;
    return;
  }
  const owner = account.address;
  const { ids, total } = await dealIdsOf(owner);
  const deals = (await Promise.all(ids.map(loadDeal))).filter((deal): deal is Deal => deal !== null);
  if (ticket !== renderTicket) return;
  const now = Date.now();

  const row = (deal: Deal) => {
    const role = roleOf(deal, owner)!;
    const other =
      role === "buyer"
        ? `to ${who(deal.seller)}`
        : role === "seller"
          ? `from ${who(deal.buyer)}`
          : `between ${who(deal.buyer)} and ${who(deal.seller)}`;
    return `<li>
      <span class="what"><a href="#/deal/${deal.id}">${esc(deal.memo) || `Deal ${deal.id}`}</a><span class="muted">${deal.id} · ${other} <span class="role">${role}</span></span></span>
      <span class="amt num">${money(deal.deposit, deal.meta)}</span>
      <span class="st">${pill(deal, now)}<span class="muted num">${nextText(deal, role, now)}</span></span>
    </li>`;
  };
  const group = (title: string, list: Deal[]) =>
    list.length === 0
      ? ""
      : `<div class="group"><span class="label">${title} · ${list.length}</span><ul class="sheet list">${list.map(row).join("")}</ul></div>`;

  const mine = deals.filter((deal) => waitsFor(deal, roleOf(deal, owner)!, now));
  const others = deals.filter((deal) => isOpen(deal) && !mine.includes(deal));
  const closed = deals.filter((deal) => !isOpen(deal));
  view.innerHTML = `
    <div class="row"><h2>My deals</h2><a class="btn primary" href="#/new">Create a deal</a></div>
    ${deals.length === 0 ? `<div class="sheet empty">You are not a party of any deal yet.</div>` : ""}
    ${group("Waiting for you", mine)}
    ${group("Waiting for others", others)}
    ${group("Closed", closed)}
    ${total > deals.length ? `<p class="muted small">Showing your ${deals.length} newest deals of ${total}. Open an older one by its number from the start page.</p>` : ""}`;
  decorate(view);
}

// ---------- View: new deal ----------

interface NewDeal {
  seller: string;
  arbiter: string | null;
  token: string | null;
  meta: TokenMeta;
  deposit: bigint;
  fee: bigint;
  bps: number;
  deadline: number;
  review: number;
  dispute: number;
  memo: string;
}

function renderNew() {
  const tokens = [
    `<option value="cspr">CSPR</option>`,
    ...(testTokenHash ? [`<option value="${esc(testTokenHash)}">TEST (test token)</option>`] : []),
    `<option value="other">Other token…</option>`,
  ].join("");
  const units = `<option value="${DAY}">days</option><option value="${HOUR}">hours</option>`;
  view.innerHTML = `
    <div><h2>New deal</h2><p class="muted">You are the buyer. The seller sees all of this before delivering and can send the money back at any time.</p></div>
    <div class="dealgrid">
      <form class="form" id="newdeal" novalidate>
        <div class="field">
          <label for="f-seller">Who are you paying?</label>
          <input id="f-seller" autocomplete="off" placeholder="name.cspr, public key or account hash" />
          <span class="hint" id="h-seller"></span>
        </div>
        <div class="field">
          <label for="f-amount">How much?</label>
          <div class="inline">
            <input id="f-amount" class="num" inputmode="decimal" autocomplete="off" />
            <select id="f-token" aria-label="Currency">${tokens}</select>
          </div>
          <input id="f-token-hash" class="mono" placeholder="hash-… of a CEP-18 token" autocomplete="off" hidden />
          <span class="hint" id="h-amount"></span>
        </div>
        <div class="field">
          <label for="f-memo">What is it for?</label>
          <input id="f-memo" autocomplete="off" />
          <span class="hint">Public and permanent. A short description, a link or a hash of your agreement.</span>
        </div>
        <div class="two">
          <div class="field">
            <label for="f-deadline">Deliver by</label>
            <input id="f-deadline" type="datetime-local" value="${toInputValue(Date.now() + 7 * DAY)}" />
          </div>
          <div class="field">
            <label for="f-review">Time to check the work</label>
            <div class="inline"><input id="f-review" class="num" type="number" min="1" step="1" value="7" /><select id="f-review-unit" aria-label="Unit">${units}</select></div>
          </div>
        </div>
        <fieldset>
          <legend>If you disagree</legend>
          <div class="two">
            <div class="field">
              <label for="f-arbiter">Arbiter (optional)</label>
              <input id="f-arbiter" autocomplete="off" placeholder="name.cspr, public key or account hash" />
              <span class="hint" id="h-arbiter">Agree with them first. A dispute is argued outside this app.</span>
            </div>
            <div class="field">
              <label for="f-fee">Arbiter’s fee</label>
              <input id="f-fee" class="num" inputmode="decimal" value="0" autocomplete="off" />
              <span class="hint">Taken from the deposit, only for a decision.</span>
            </div>
          </div>
          <div class="two">
            <div class="field">
              <label for="f-dispute">Time to decide a dispute</label>
              <div class="inline"><input id="f-dispute" class="num" type="number" min="1" step="1" value="14" /><select id="f-dispute-unit" aria-label="Unit">${units}</select></div>
            </div>
            <div class="field">
              <label for="f-split">If nobody decides in that time</label>
              <input id="f-split" type="range" min="0" max="100" step="5" value="50" />
              <span class="hint num" id="h-split"></span>
            </div>
          </div>
        </fieldset>
      </form>
      <aside class="sheet pad agree">
        <span class="label">What you are agreeing to</span>
        <ol id="summary"></ol>
        <button class="btn primary wide" id="submit" form="newdeal"></button>
        <span class="hint" id="h-submit">This cannot be changed after you sign.</span>
      </aside>
    </div>`;

  const input = (id: string) => $<HTMLInputElement>(id);
  const form = $<HTMLFormElement>("newdeal");
  let meta: TokenMeta = CSPR_META;
  const resolved: { seller: string | null; arbiter: string | null } = { seller: null, arbiter: null };

  const chosenToken = (): string | null | undefined => {
    const value = input("f-token").value;
    if (value === "cspr") return null;
    if (value !== "other") return value;
    try {
      return parseAddress(input("f-token-hash").value);
    } catch {
      return undefined;
    }
  };

  /** A period of the form in words: `1 hour`, `7 days`. */
  const lengthText = (name: string) => {
    const count = input(`f-${name}`).value || "?";
    const unit = input(`f-${name}-unit`).value === String(DAY) ? "day" : "hour";
    return `${count} ${unit}${count === "1" ? "" : "s"}`;
  };
  /** A party of the form by the name it resolved to, or as typed. */
  const partyText = (field: "seller" | "arbiter") => {
    const typed = input(`f-${field}`).value.trim();
    if (looksLikeName(typed)) return typed;
    // Something unreadable is shown shortened, so a long key does not fill the panel.
    if (!resolved[field]) return typed.length > 24 ? `${typed.slice(0, 10)}…${typed.slice(-6)}` : typed;
    return devNames.get(resolved[field]!) ?? shortKey(resolved[field]!);
  };

  const summary = () => {
    const amount = parseAmount(input("f-amount").value, meta.decimals);
    const amountText = amount === null ? "the amount" : money(amount, meta);
    const split = Number(input("f-split").value);
    const rule = `${split}% to the seller, ${100 - split}% back to you`;
    const review = lengthText("review");
    const dispute = lengthText("dispute");
    const deadline = Date.parse(input("f-deadline").value);
    const arbiter = partyText("arbiter");
    $("h-split").textContent = rule;
    $("summary").innerHTML = `
      <li>You deposit <b class="num">${amountText}</b> now. It leaves your wallet.</li>
      <li><b>${esc(partyText("seller") || "The seller")}</b> has until <b>${Number.isNaN(deadline) ? "the deadline" : when(deadline)}</b> to deliver. If they don’t, you get everything back.</li>
      <li>After delivery you have <b>${esc(review)}</b> to object. If you say nothing, the seller is paid in full.</li>
      ${commissionBps > 0 ? `<li>Of whatever the seller is paid, <b>${commissionPercent()}</b> is kept as a commission. You pay nothing on top.</li>` : ""}
      <li>${arbiter ? `If you object, <b>${esc(arbiter)}</b> decides within ${esc(dispute)}. With no decision: ${rule}.` : `If you object and do not settle within ${esc(dispute)}: ${rule}.`}</li>`;
    $("submit").textContent = account ? `Deposit ${amount === null ? "" : money(amount, meta)}`.trim() : "Connect a wallet first";
    $<HTMLButtonElement>("submit").disabled = busy || !account;
  };

  const describeToken = async () => {
    const token = chosenToken();
    input("f-token-hash").hidden = input("f-token").value !== "other";
    const hint = $("h-amount");
    hint.className = "hint";
    hint.textContent = "";
    if (token === undefined) {
      if (input("f-token-hash").value.trim()) {
        hint.textContent = "Enter the package hash of a CEP-18 token: hash-…";
        hint.className = "hint bad";
      }
      return;
    }
    try {
      meta = await tokenMeta(token);
      const balance = account ? await balanceOf(token, account.address) : null;
      const faucet =
        token !== null && token === testTokenHash && account
          ? ` <button type="button" class="btn small" id="faucet">Get 1,000 TEST</button>`
          : "";
      hint.innerHTML = `${balance === null ? "" : `Your balance: ${money(balance, meta)}.`}${faucet}`;
      $("faucet")?.addEventListener("click", () => {
        void (async () => {
          await send("Test tokens", GAS.faucet, () => new TestTokenWasmClient(client, address(token!)).faucet());
          node.refresh();
          await describeToken();
        })();
      });
    } catch (error) {
      hint.textContent = errorText(error);
      hint.className = "hint bad";
    }
    summary();
  };

  /** Reads a party field into `resolved`. Returns what is wrong with it, or `null`. */
  const lookUp = async (field: "seller" | "arbiter"): Promise<string | null> => {
    const text = input(`f-${field}`).value;
    const hint = $(`h-${field}`);
    resolved[field] = null;
    if (!text.trim()) {
      hint.className = "hint";
      hint.textContent = field === "arbiter" ? "Agree with them first. A dispute is argued outside this app." : "";
      summary();
      return null;
    }
    try {
      const party = await resolveParty(text);
      if (input(`f-${field}`).value !== text) return "The field changed while it was being read.";
      resolved[field] = party.address;
      hint.textContent = `✓ ${party.note}`;
      hint.className = "hint good";
      summary();
      return null;
    } catch (error) {
      if (input(`f-${field}`).value !== text) return "The field changed while it was being read.";
      hint.textContent = errorText(error);
      hint.className = "hint bad";
      summary();
      return errorText(error);
    }
  };

  /** Reads and checks the form. Throws a sentence for the person who filled it in. */
  const read = async (): Promise<NewDeal> => {
    if (!account) throw new Error("Connect a wallet first.");
    const [sellerProblem, arbiterProblem] = await Promise.all([lookUp("seller"), lookUp("arbiter")]);
    if (sellerProblem) throw new Error(`The seller: ${sellerProblem}`);
    if (!resolved.seller) throw new Error("Say who you are paying.");
    const hasArbiter = input("f-arbiter").value.trim() !== "";
    if (arbiterProblem) throw new Error(`The arbiter: ${arbiterProblem}`);
    if (hasArbiter && !resolved.arbiter) throw new Error("The arbiter could not be read.");
    const arbiter = hasArbiter ? resolved.arbiter : null;
    const parties = [account.address, resolved.seller, ...(arbiter ? [arbiter] : [])];
    if (new Set(parties).size !== parties.length) {
      throw new Error("You, the seller and the arbiter must be three different accounts.");
    }
    const token = chosenToken();
    if (token === undefined) throw new Error("Enter the package hash of the token.");
    if (token === null && parties.some((party) => !party.startsWith("account-hash-"))) {
      throw new Error("In a CSPR deal every party must be an account, not a contract.");
    }
    const tokenMetaNow = await tokenMeta(token);
    const deposit = parseAmount(input("f-amount").value, tokenMetaNow.decimals);
    if (deposit === null || deposit === 0n) throw new Error("Enter the amount.");
    const fee = parseAmount(input("f-fee").value || "0", tokenMetaNow.decimals);
    if (fee === null) throw new Error("The arbiter's fee is not a number.");
    if (!arbiter && fee > 0n) throw new Error("A deal with no arbiter cannot have an arbiter's fee.");
    if (fee >= deposit) throw new Error("The arbiter's fee must be less than the amount.");
    const balance = await balanceOf(token, account.address);
    if (balance < deposit) throw new Error(`Your balance is ${money(balance, tokenMetaNow)}: not enough for this deposit.`);

    const deadline = Date.parse(input("f-deadline").value);
    const now = Date.now();
    if (Number.isNaN(deadline) || deadline <= now + 60_000) throw new Error("The delivery deadline must be in the future.");
    if (deadline - now > MAX_PERIOD - HOUR) throw new Error("The delivery deadline can be at most a year away.");
    const length = (name: string, label: string) => {
      const value = Number(input(`f-${name}`).value) * Number(input(`f-${name}-unit`).value);
      if (!Number.isFinite(value) || value < MIN_PERIOD) throw new Error(`${label} must be at least one hour.`);
      if (value > MAX_PERIOD) throw new Error(`${label} can be at most 365 days.`);
      return Math.round(value);
    };
    const memo = input("f-memo").value.trim();
    if (new TextEncoder().encode(memo).length > MAX_MEMO_BYTES) throw new Error("The description is too long.");
    return {
      seller: resolved.seller,
      arbiter,
      token,
      meta: tokenMetaNow,
      deposit,
      fee,
      bps: Number(input("f-split").value) * 100,
      deadline,
      review: length("review", "The time to check the work"),
      dispute: length("dispute", "The time to decide a dispute"),
      memo,
    };
  };

  const submit = async () => {
    const hint = $("h-submit");
    let deal: NewDeal;
    try {
      deal = await read();
    } catch (error) {
      hint.textContent = errorText(error);
      hint.className = "hint bad";
      return;
    }
    const buyer = account!.address;
    let created: string | null = null;
    await withBusy(async () => {
      if (deal.token !== null) {
        const token = deal.token;
        const allowance = await tokenAmount(token, "token.allowances", [`${buyer}:${escrowHash}`]);
        if (allowance < deal.deposit) {
          const allowed = await send("Allowing the contract to take the deposit", GAS.approve, () =>
            new TestTokenWasmClient(client, address(token)).approve(address(escrowHash), U256.fromBigInt(deal.deposit)),
          );
          if (!allowed) return;
        }
      }
      const done = await send("Creating the deal", GAS.create[deal.token === null ? "cspr" : "token"], () =>
        escrow.createDeal(
          address(deal.seller),
          deal.arbiter ? address(deal.arbiter) : null,
          deal.token ? address(deal.token) : null,
          U256.fromBigInt(deal.deposit),
          U256.fromBigInt(deal.fee),
          deal.bps,
          BigInt(deal.deadline),
          BigInt(deal.review),
          BigInt(deal.dispute),
          deal.memo,
          U512.fromBigInt(deal.token === null ? deal.deposit : 0n),
        ),
      );
      if (!done) return;
      // The newest deal of the buyer is the one just made.
      node.refresh();
      const { ids } = await dealIdsOf(buyer);
      created = ids.length > 0 ? ids[0].toString() : null;
    });
    if (created !== null) location.hash = `#/deal/${created}`;
  };

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    void submit();
  });
  form.addEventListener("input", summary);
  input("f-token").addEventListener("change", () => void describeToken());
  input("f-token-hash").addEventListener("change", () => void describeToken());
  input("f-seller").addEventListener("change", () => void lookUp("seller"));
  input("f-arbiter").addEventListener("change", () => void lookUp("arbiter"));
  summary();
  void describeToken();
}

// ---------- Routing ----------

function route(): { name: "home" | "deals" | "new" } | { name: "deal"; id: bigint } {
  const hash = location.hash.replace(/^#\/?/, "");
  const deal = /^deal\/(\d+)$/.exec(hash);
  if (deal) return { name: "deal", id: BigInt(deal[1]) };
  if (hash === "deals" || hash === "new") return { name: hash };
  return { name: "home" };
}

async function render() {
  const ticket = ++renderTicket;
  const current = route();
  for (const link of document.querySelectorAll<HTMLElement>("[data-nav]")) {
    if (link.dataset.nav === current.name) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  try {
    if (current.name === "home") renderHome();
    else if (!ready) view.innerHTML = `<div class="sheet empty">Loading…</div>`;
    else if (current.name === "new") renderNew();
    else if (current.name === "deals") await renderDeals(ticket);
    else if (current.name === "deal") await renderDeal(current.id, ticket);
  } catch (error) {
    if (ticket !== renderTicket) return;
    console.error(error);
    view.innerHTML = `<div class="notice">${esc(errorText(error))}</div>`;
  }
}

// ---------- Wallet ----------

const connectBtn = $<HTMLButtonElement>("connect");
const devSelect = $<HTMLSelectElement>("dev-account");

async function onAccount(info: AccountInfo) {
  account = { publicKey: info.publicKey, address: key(info.address) };
  const formatted = account.address;
  const label = devNames.get(formatted) ?? shortKey(formatted);
  $("account-name").textContent = label;
  $("account").title = info.publicKey;
  connectBtn.hidden = true;
  $("account").hidden = NETWORK.devWallet;
  $("disconnect").hidden = false;
  if (NETWORK.devWallet) devSelect.value = devNames.get(formatted) ?? "";
  else void names?.nameOf(formatted).then((name) => name && ($("account-name").textContent = name));
  await render();
}

async function onSignedOut() {
  account = null;
  connectBtn.hidden = NETWORK.devWallet;
  $("account").hidden = true;
  $("disconnect").hidden = true;
  if (NETWORK.devWallet) devSelect.value = "";
  await render();
}

function loadCsprClick(): Promise<void> {
  return new Promise((resolve, reject) => {
    window.addEventListener("csprclick:loaded", () => resolve(), { once: true });
    const script = document.createElement("script");
    script.src = CSPR_CLICK_SCRIPT;
    script.onerror = () => reject(new Error("Could not load CSPR.click."));
    document.head.append(script);
  });
}

// ---------- Start ----------

async function start() {
  $("network").textContent = NETWORK.label;
  if (!DEPLOYED) throw new Error(`The contract is not deployed on ${NETWORK.label} yet.`);
  const deployed = (name: string) => DEPLOYED.contracts.find((contract) => contract.name === name)?.package_hash;
  escrowHash = deployed("Escrow") ?? "";
  if (!escrowHash) throw new Error(`The contract is not deployed on ${NETWORK.label} yet.`);
  testTokenHash = NETWORK.testToken ? (deployed("TestToken") ?? null) : null;

  if (NETWORK.demo) {
    $("demo").innerHTML = `This is a demo on ${esc(NETWORK.label)}: the contract is not audited and nothing here has value. <a href="${REPO_URL}" target="_blank" rel="noopener">Source on GitHub</a> · <a href="${DEPLOY_GUIDE_URL}" target="_blank" rel="noopener">Run your own on the mainnet</a>`;
    $("demo").hidden = false;
  }
  $("footer-contract").innerHTML = `Contract on ${esc(NETWORK.label)}: <span class="mono">${esc(escrowHash)}</span>`;

  // The start page is text: it is on the screen before the wasm modules and the contract's
  // state, a few megabytes and a few requests, have arrived.
  window.addEventListener("hashchange", () => void render());
  $("toast-close").addEventListener("click", () => (toast.hidden = true));
  void render();

  await Promise.all([init(), initStorage()]);
  client = new OdraWasmClient(NETWORK.nodeUrl, NETWORK.speculativeUrl, NETWORK.chainName);
  escrow = new EscrowWasmClient(client, address(escrowHash));
  escrowStorage = await ContractStorage.open(node, escrowSchema, escrowHash);
  const [bps, recipient] = await Promise.all([
    escrowStorage.read("commission_bps"),
    escrowStorage.read("commission_recipient"),
  ]);
  commissionBps = Number(bps ?? "0");
  commissionRecipient = recipient === null ? null : formattedKey(recipient);

  // Names are a convenience: the app works without them.
  if (NETWORK.names) {
    void Names.open(NETWORK.names)
      .then((opened) => {
        names = opened;
        decorate(view);
      })
      .catch((error) => console.warn("CSPR.name is not available:", error));
  }

  CsprClickCallbacks.onSignedIn((info: AccountInfo) => void onAccount(info));
  CsprClickCallbacks.onSwitchedAccount((info: AccountInfo) => void onAccount(info));
  CsprClickCallbacks.onSignedOut(() => void onSignedOut());
  CsprClickCallbacks.onDisconnected(() => void onSignedOut());
  CsprClickCallbacks.onTransactionStatusUpdate((status: TransactionStatus, data: TransactionResult) => {
    if (status === TransactionStatus.SENT && data.txHash) {
      showTx("Sent. Waiting for the network to process it…", { hash: data.txHash });
    }
  });

  connectBtn.addEventListener("click", () => client.signIn().catch(console.error));
  $("disconnect").addEventListener("click", () => client.signOut().catch(console.error));

  if (NETWORK.devWallet) {
    for (const dev of DEV_ACCOUNTS) devNames.set(key(Address.fromPublicKey(dev.publicKey)), dev.name);
  }

  ready = true;
  // The start page is on the screen already, perhaps with something typed into it: only its
  // commission is filled in. Any other view was waiting for this.
  if (route().name === "home") fillHome();
  else await render();

  // Deadlines pass and other parties act: the views that show deals are read again now and then,
  // unless something is being typed or sent.
  setInterval(() => {
    const name = route().name;
    const typing = view.contains(document.activeElement) && document.activeElement instanceof HTMLInputElement;
    if (!busy && !typing && !document.hidden && (name === "deal" || name === "deals")) void render();
  }, REFRESH_MS);

  if (NETWORK.devWallet) {
    const wallet = installDevWallet(node);
    devSelect.innerHTML =
      `<option value="">Signed out</option>` +
      DEV_ACCOUNTS.map((dev) => `<option value="${esc(dev.name)}">${esc(dev.name)}</option>`).join("");
    devSelect.hidden = false;
    connectBtn.hidden = true;
    devSelect.addEventListener("change", () => {
      if (devSelect.value) wallet.switchTo(devSelect.value);
      else client.signOut().catch(console.error);
    });
  } else {
    // Loaded last: the client listens for the events the script sends once it is ready.
    await loadCsprClick();
    connectBtn.disabled = false;
  }
}

start().catch((error) => {
  console.error(error);
  view.innerHTML = `<div class="notice">The app could not start: ${esc(errorText(error))}</div>`;
});
