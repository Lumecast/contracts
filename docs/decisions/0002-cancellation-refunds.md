# ADR-0002: Cancellation refunds every holder in full from contract escrow

## Status

Accepted

## Context

A cancelled market must never leave funds stranded. Before resolution begins,
the market's escrow holds USDC from depositors, and the outstanding-share
ledger (`Market.shares` + per-holder `Position`s) is the only record of who
owns what. Refund options:

1. **Back to gas originator or market creator**, on the assumption the frontend
   handles payouts. Loses money for depositors if the app disappears.
2. **A refund claim contract** (holder withdraws later). Adds a new contract and
   a second trust surface for the first milestone.
3. **Scan the holder registry and push refunds** (chosen): the contract itself
   returns each depositor's full principal directly from its escrow balance.

## Decision

`cancel_market` may be called by the market `creator` or `resolver` while the
market is `Open`. It scans the per-market holder registry, refunds each holder
their full deposited principal on every outcome with a nonzero position, zeroes
those positions plus the shared `pool`/`shares` ledger, sets state to
`Cancelled`, and emits `CancelMarketEvent`. A `MarketNotFound` / `MarketNotOpen`
/ `Unauthorized` error guards each invalid invocation.

## Consequences

- The escrow is provably drained at cancel time; `pool == 0` implies the
  contract holds no market funds.
- Refund cost is bounded by the number of holders (one token transfer per
  non-empty outcome-holder pair), which is fine while a single market is small.
- A cancelled market is terminal: `deposit` and (later) `resolve` are rejected.
- Invariant assumed by future resolution: `sum(positions) == pool` must hold on
  both outcomes, so resolution can pay out shares 1:1 without a pool scan.