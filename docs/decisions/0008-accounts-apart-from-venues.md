# 0008: An account is named apart from its venue

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-19 |
| Extends | The node configuration's `venues` and `venue_access` |

## Context

A node named each venue access by the venue itself: `"name": "bybit"` chose the adapter, the
credential shape, the market stream, the ledger the orders are kept in and the `venue:` limit scope.
One name was both the dialect the platform speaks and the account it trades for, so a node could
hold one account per venue. Two accounts at one venue (a main account and a sub-account, or two
clients' accounts) could not be traded by one node, and a second account's identical instruments
were refused as an identity declared twice.

## Decision

1. **A venue access names its account and, where they differ, its venue.** `name` is the account:
   the ledger, the reconciliation, the `venue:` limit scope and a desk's venue list all use it.
   `venue` is the registered venue whose adapter, credential shape, market stream and fees the
   account uses. Absent, it is the name, so every existing configuration means what it meant. The
   profile must state the same venue, or the account is not opened.
2. **Market data is the venue's, not the account's.** The node reads each venue's market and facts
   stream once, under the venue's name, whatever number of accounts trade there. A strategy sees
   markets, never accounts.
3. **One instrument, several accounts.** An identity may be listed by several accounts of the same
   venue when every listing is the same instrument. Positions stay per account; an `instrument:`
   or `underlying:` limit binds the sum across them. An identity listed by two different venues is
   still refused, as before.
4. **A proposal that names a venue is routed among the desk's accounts at that venue**, in the
   desk's order, as an unnamed proposal is among all of the desk's accounts. A proposal may also
   name one account, which then is the only candidate. Accounts are allocated by desks: a desk that
   should trade one client's account lists that account alone.

## Consequences

- A node trades any number of accounts at one venue, each reconciled and limited on its own, with
  one market stream per venue.
- Configurations without `venue` are unchanged.
- A strategy that named a venue keeps working when a second account is added; which account takes
  its order is the desk's declaration, an operator decision rather than a strategy's.
