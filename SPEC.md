# Escrow contract: specification

This document describes what the `Escrow` contract in `src/escrow.rs` does. The contract is meant
to be deployed once on the Casper mainnet and never changed, so everything here is final once it
is deployed. Read it as the list of things to agree with before that.

## What it is

A buyer deposits CSPR or a CEP-18 token for a seller. The deposit stays in the contract until the
deal is settled:

- the buyer is satisfied and releases it, or says nothing and it goes to the seller on its own;
- the seller gives it back, or does not deliver and it goes back to the buyer on its own;
- the two disagree, and an arbiter they chose splits it, or, if nobody decides, it is split by a
  rule written into the deal when it was created;
- at any point, the two agree on a split themselves and the deal closes with it.

The contract has no owner, no fee, no settings and no pause. It is deployed locked: the package
cannot get a new version. One contract holds every deal.

**No open deal can be stuck.** Each of the three open states has a time limit, and from that
moment anyone can call `settle` to close the deal by the rule of its state.

## A deal

The buyer creates the deal and chooses everything in it. The seller sees it before delivering and
can always refuse by refunding.

| Field | Meaning | Limits |
|---|---|---|
| `seller` | Who the deposit is for. | Not the buyer. |
| `arbiter` | Who may decide a dispute. Optional. | Not the buyer, not the seller. |
| `token` | The CEP-18 token of the deposit. Empty for CSPR. | |
| `deposit` | The amount: motes or the token's smallest units. | More than zero. |
| `arbiter_fee` | What the arbiter takes out of the deposit for a decision. | Less than the deposit. Zero if there is no arbiter. |
| `default_seller_bps` | The seller's part, in hundredths of a percent, if a dispute gets no decision in time. | 0 to 10000. |
| `delivery_deadline` | The seller can mark the deal delivered before this moment. | In the future, at most 365 days away. |
| `review_period` | How long the buyer has to dispute a delivery. | 1 hour to 365 days. |
| `dispute_period` | How long a dispute may stay undecided. | 1 hour to 365 days. |
| `memo` | What the deal is about: a text, a link or a hash of the agreement. Public. | At most 256 bytes. |

Times are block times in milliseconds since the Unix epoch. The one-hour minimum rejects a period
given in seconds by mistake.

In a CSPR deal the buyer, the seller and the arbiter must be accounts. Casper pays CSPR to
accounts only, so a contract as a party could not be paid. A token deal can have contracts as
parties.

## States

```
                 release (buyer)                    refund (seller)
          +--------------------------> Released    +-----------------> Refunded
          |   from every open state                |  from every open state
          |
          |      offer_split (buyer and seller, the same amount)
          +------------------------------------------------------> Settled
          |   from every open state
          |
 Funded --+-- mark_delivered (seller, before the delivery deadline) --> Delivered
          |
          +-- settle (anyone, from the delivery deadline on) ---------> Refunded

 Delivered --+-- dispute (buyer, before the review period ends) -----> Disputed
             |
             +-- settle (anyone, from the end of the review period) --> Released

 Disputed --+-- resolve (arbiter, before the dispute period ends) ---> Resolved
            |
            +-- settle (anyone, from the end of the dispute period) --> Split
```

| State | Open | What it means |
|---|---|---|
| `Funded` | yes | The deposit is held. The seller has not delivered. |
| `Delivered` | yes | The seller said the work is delivered. The review period runs from that moment. |
| `Disputed` | yes | The buyer disputed the delivery. The dispute period runs from that moment. |
| `Released` | no | The whole deposit went to the seller. |
| `Refunded` | no | The whole deposit went back to the buyer. |
| `Resolved` | no | The arbiter decided: fee to the arbiter, a share to the seller, the rest to the buyer. |
| `Split` | no | Nobody decided in time: `default_seller_bps` of the deposit to the seller, the rest to the buyer, nothing to the arbiter. |
| `Settled` | no | The buyer and the seller agreed: the amount they both named to the seller, the rest to the buyer, nothing to the arbiter. |

## Who may do what

With the time limit of the state still ahead:

| | Funded | Delivered | Disputed | closed |
|---|---|---|---|---|
| `mark_delivered` | seller | – | – | – |
| `dispute` | – | buyer | – | – |
| `release` | buyer | buyer | buyer | – |
| `refund` | seller | seller | seller | – |
| `resolve` | – | – | arbiter | – |
| `offer_split` | buyer, seller | buyer, seller | buyer, seller | – |
| `settle` | – | – | – | – |

Once the time limit of the state is reached:

- `mark_delivered`, `dispute` and `resolve` are rejected;
- `settle` works for anyone;
- `release`, `refund` and `offer_split` still work until somebody settles.

The limits are exact: the last millisecond before a limit belongs to the party, the limit itself
to `settle`. The test `every_action_by_every_account_in_every_state` checks the whole first table,
for the buyer, the seller, the arbiter and a stranger.

Three decisions worth a second look:

1. **Only the buyer creates a deal.** A seller's "invoice" is a link that opens the app with the
   form filled in; nothing is stored on chain until the buyer pays.
2. **The arbiter does not accept the role on chain.** An arbiter who never answers is the same as
   one who was never asked: the dispute period runs out and the default split applies.
3. **Only the buyer disputes, and only a delivery.** Before the delivery the buyer needs no
   dispute, the deadline protects them. The seller needs none either: silence after a delivery
   pays the seller.

The delivery deadline is hard: a seller who is late cannot mark the deal delivered any more. A
deal nobody looks at can then only go back to the buyer. A buyer who accepts a late delivery pays
with `release` or a settlement.

## Settling between the two

The buyer and the seller can split the deposit without the arbiter. Each of them has one offer:
the amount the seller would get. `offer_split` sets it, calling again replaces it, and an empty
value withdraws it. The moment the two offers are the same amount, the deal closes with that
split.

- Accepting an offer is making the same offer. If the other party changed theirs a moment before,
  the amounts differ and nothing happens.
- An offer stays until its author changes or withdraws it, so the other party can accept it later.
- An offer does not stop any time limit. A buyer who negotiates during the review must still
  dispute before it ends, or the seller is paid in full.
- The arbiter gets no fee for a settlement, also during a dispute.

## Money

- **Taking the deposit.** For CSPR, the call must carry exactly `deposit` motes. For a token, the
  buyer approves the contract first and the contract pulls `deposit` with `transfer_from`. The
  contract compares its token balance before and after and rejects a deal if it did not receive
  exactly `deposit`.
- **Paying out.** A deal is stored as closed first and paid after. The payouts of a deal always add
  up to its deposit. A share that does not divide evenly is rounded down for the seller.
- **The arbiter's fee** is a fixed amount, part of the deposit, paid only when the arbiter
  decides. The seller's share in `resolve` is at most the deposit less the fee.
- **Every deal is paid from its own token.** A token that misbehaves can only harm the deals made
  in it. No entry point can be entered again while another one runs.
- **Payouts are sent, not claimed.** If a token refuses to pay one of the parties, the paths that
  pay that party fail and change nothing; the other paths stay open. For example, if the seller
  cannot be paid, `refund` still works.

## Entry points

Entry points that change state return nothing. The id of a new deal is in the `DealCreated` event.

| Entry point | Who | What it does |
|---|---|---|
| `create_deal(seller, arbiter, token, deposit, arbiter_fee, default_seller_bps, delivery_deadline, review_period, dispute_period, memo)` | anyone, as the buyer | Opens a deal and takes the deposit. Payable. |
| `mark_delivered(deal_id)` | the seller | `Funded` → `Delivered`. |
| `dispute(deal_id)` | the buyer | `Delivered` → `Disputed`. |
| `release(deal_id)` | the buyer | Pays everything to the seller. |
| `refund(deal_id)` | the seller | Pays everything back to the buyer. |
| `resolve(deal_id, seller_share)` | the arbiter | Decides a dispute. |
| `offer_split(deal_id, seller_share)` | the buyer or the seller | Sets, replaces or withdraws the caller's settlement offer. Closes the deal if both offers are equal. |
| `settle(deal_id)` | anyone | Closes a deal whose time limit has passed. |
| `deal(deal_id)` | view | The deal. |
| `deals_count()` | view | The number of deals. Ids run from `0`. |
| `deals_count_of(account)` | view | The number of deals the account is a party of, in any role. |
| `deal_id_of(account, index)` | view | The id of a deal of the account by position, oldest first. |

The deposit argument is called `deposit`, not `amount`: Odra passes the CSPR attached to a payable
call under the name `amount`, and an argument of the same name would be overwritten by it.

## Storage, for apps that read it

An app reads the contract's state from the node with `odra-storage-reader` and the schema in
`resources/casper_contract_schemas/escrow_schema.json`.

| Path | Key | Value |
|---|---|---|
| `count` | | The number of deals. |
| `deals` | deal id | The deal, with its state, its four times and what each party was paid. |
| `count_of` | account | The number of deals of the account. |
| `ids_of` | `account:index` | The id of the deal at that position. |

A deal keeps `created_at`, `delivered_at`, `disputed_at` and `closed_at` (zero means "not yet") and
`to_seller`, `to_buyer`, `to_arbiter`, and the two current settlement offers, `buyer_offer` and
`seller_offer`, so its page can be drawn from one read, without events.

Anyone can open a deal naming any account as its seller or arbiter, so an account's list can
contain deals it never agreed to. They cost that account nothing; an app should present them as
offers, not as obligations.

## Events

| Event | Fields |
|---|---|
| `DealCreated` | `deal_id` and every field of the deal the buyer chose. |
| `DealDelivered` | `deal_id`, `review_ends_at`. |
| `DealDisputed` | `deal_id`, `dispute_ends_at`. |
| `SplitOffered` | `deal_id`, `by`, `seller_share` (empty when withdrawn). |
| `DealClosed` | `deal_id`, `state`, `to_seller`, `to_buyer`, `to_arbiter`. |

## Errors

| Code | Name | When |
|---|---|---|
| 1 | `ZeroDeposit` | The deposit is zero. |
| 2 | `DepositTooLarge` | The deposit is too large to be split (more than 2^256 / 10000). |
| 3 | `SameParty` | Two of the buyer, the seller and the arbiter are the same account. |
| 4 | `FeeTooLarge` | The arbiter's fee is not less than the deposit. |
| 5 | `FeeWithoutArbiter` | A fee with no arbiter. |
| 6 | `BpsTooLarge` | `default_seller_bps` is more than 10000. |
| 7 | `DeadlineNotInFuture` | The delivery deadline is not in the future. |
| 8 | `DeadlineTooFar` | The delivery deadline is more than 365 days away. |
| 9 | `PeriodTooShort` | A period is shorter than one hour. |
| 10 | `PeriodTooLong` | A period is longer than 365 days. |
| 11 | `MemoTooLong` | The memo is longer than 256 bytes. |
| 12 | `WrongAttachedValue` | A CSPR deal without exactly its deposit attached. |
| 13 | `UnexpectedAttachedValue` | A token deal with CSPR attached. |
| 14 | `DepositMismatch` | The token did not deliver exactly the deposit. |
| 15 | `DealNotFound` | No deal has this id. |
| 16 | `DealClosed` | The deal is closed. |
| 17 | `NotBuyer` | Only the buyer may do this. |
| 18 | `NotSeller` | Only the seller may do this. |
| 19 | `NotArbiter` | Only the arbiter may do this. |
| 20 | `NotFunded` | The deal is not waiting for delivery. |
| 21 | `NotDelivered` | The deal is not marked delivered. |
| 22 | `NotDisputed` | The deal is not disputed. |
| 23 | `DeliveryDeadlinePassed` | Too late to mark the deal delivered. |
| 24 | `ReviewPeriodPassed` | Too late to dispute. |
| 25 | `DisputePeriodPassed` | Too late to decide the dispute. |
| 26 | `TooEarly` | The time limit of the state is still ahead. |
| 27 | `ShareTooLarge` | The seller's share is more than the deposit, or, in the arbiter's decision, more than the deposit less the fee. |
| 28 | `ContractInCsprDeal` | A party of a CSPR deal is a contract. |
| 29 | `NotParty` | Only the buyer or the seller may do this. |

## What a call costs

Gas consumed on the Casper VM by the wasm that `cargo odra build` writes (the `gas_of_every_call`
test). These are not measured on a network yet; a network charges the limit a transaction sets
and refunds part of what it leaves unused.

| Call | Token deal | CSPR deal |
|---|---|---|
| deploying the contract | 340.6 CSPR | |
| `create_deal` | 3.75 | 2.65 |
| `mark_delivered` | 0.75 | 0.71 |
| `dispute` | 0.75 | 0.71 |
| `release`, `refund` | 1.51 | 3.22 |
| `resolve`, three payouts | 3.04 | 8.23 |
| `settle` of a dispute, two payouts | 2.27 | 5.72 |
| `offer_split`, first offer | 0.80 | 0.75 |
| `offer_split` that settles, two payouts | 2.46 | 5.91 |

On a network a CSPR deal is created through a small proxy program that carries the CSPR, which
costs more than the table says: `create_deal` in CSPR consumed 5.3 CSPR on a local NCTL network,
and 3.8 to 4.2 CSPR in a token.

Every CSPR payout costs about 2.5 CSPR, which is why closing a CSPR deal costs more than closing
a token deal. The first CSPR deal ever made costs 8.0 CSPR instead of 2.65, because it creates the
contract's purse; whoever deploys the contract should make that deal.

## What the contract does not do

- **It does not judge.** It cannot know whether anything was delivered. A seller can mark a deal
  delivered without delivering; the buyer must dispute within the review period or the deposit
  goes to the seller.
- **It does not protect from the arbiter.** An arbiter can give everything to either side. The
  parties choose one they both trust, or none.
- **It does not notify.** Nobody is told that a period started. A party that does not look can
  lose by silence. The periods are the buyer's choice and should be long enough for that.
- **It does not vet tokens.** A token whose balances change on their own, or that can freeze
  accounts, can make a deal in that token pay less or fail to pay. Deals in other tokens and in
  CSPR are not affected.
- **It does not hide anything.** The parties, the amount and the memo are public.
- **It does not close a deal by itself.** After a time limit somebody has to send `settle`, and
  pays its gas. Whoever is owed the money has the reason to.

## Tests

`cargo odra test` runs the tests on OdraVM, `cargo odra test -b casper` on the Casper VM with the
built wasm files. Both run the same 43 tests in `src/tests.rs`: every argument check, the whole
table of who may do what, every path in tokens and in CSPR, settlements between the parties, the exact moments of the time limits,
rounding, many deals at once, and tokens that keep a part of a transfer, move nothing, call back
into the contract or refuse to pay a party (`EvilToken` in `src/testing.rs`).

Not done yet: a run on a network, a review by somebody who did not write the contract, and an
audit.
