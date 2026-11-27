# 0003: The sandbox, a paper node with a simulated account

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-15 |
| Builds on | [Simulated venue](../concepts/simulated_venue.md), paper mode |

## Context

A paper node already runs every order through the live code and fills it on the simulated venue
against the market the node reads. What it does not have is an account: nothing it buys costs
anything, nothing it fills pays a fee, and nothing refuses an order the account could not pay for.
A strategy that would be refused for funds, or that only wins before fees, looks the same in
paper as one that works. A client evaluating a strategy before funding a venue account needs
exactly that difference, and so does a client whose account at a venue does not yet exist.

## Decision

The paper node becomes the sandbox: the simulated venue keeps a simulated account per venue.

1. **Starting balances are declared**, per venue and currency, in the node's configuration. A
   paper node without them is refused at start, never given a default: an assumed balance is an
   invented fact about capital.
2. **Cash moves with every fill.** A buy pays price times quantity in the quote currency and a
   sell receives it; a perpetual's fill moves margin by the same rule until a margin model is
   declared.
3. **An order the account cannot pay for is rejected** by the simulated venue, as a live venue
   would (`InsufficientMargin`), and the rejection reaches the record and the strategy like any
   other.
4. **Fees are charged from the venue's declared fees**, the taker rate on the part of an order
   that takes liquidity and the maker rate on the part that rests and is filled. A venue with no
   declared fee charges none and the run's model says so; it is never assumed.
5. **The model is stated**: fill, latency, fees and starting balances are written once at start
   and named in every paper result, so a result states what it assumed.
6. **Trades move the simulated market** where the node records them: a resting order fills on a
   recorded trade through its price, not only on a book that crosses it.

## Consequences

- A paper run's daily loss includes fees and its exposure is bounded by what the account holds.
- A strategy can be developed and shown to a client for a venue whose account does not exist yet,
  with the refusals that account would give.
- Margin, leverage and liquidation of perpetuals are not simulated; a perpetual's cash moves as a
  spot purchase would. That is stated in the model until a margin model is decided.

## Alternatives not taken

- **Use the live venue's account read in paper.** It ties paper to a funded account, which is the
  thing a client may not have yet, and the simulated fills would never be reflected in it.
- **Skip fees until the live run.** A strategy whose edge is smaller than its fees would pass paper
  and fail live, the worst place to find out.
