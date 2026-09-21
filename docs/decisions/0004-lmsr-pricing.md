# ADR-0004: LMSR automated market maker for Lumen markets

## Status

Accepted

## Context

The v1 market is pari-mutuel: deposits pool by outcome and pay out pro-rata on
resolution. That settles soundly, but a pari-mutuel pool has no price formation
while open — you cannot buy or sell at a market price, there is no way to exit an
early position, and liquidity is whatever traders happen to bring. For a live
prediction market we want continuous pricing, two-sided tradability, and an
incentive for a market creator to seed liquidity. A logarithmic market scoring
rule (LMSR) provides all three with a single tractable pricing formula, so we
need to decide how to graft it onto the existing market contract without
breaking the pari-mutuel v1.

## Decision

Markets are created with a per-market liquidity parameter `b: i128`.
`b == 0` selects the legacy pari-mutuel behavior; `b > 0` selects LMSR. The two
models share storage, lifecycle, and resolution primitives but branch on the
pricing/claim/cancel logic:

- **Pool bookkeeping.** LMSR escrows the cost function value as cash:
  `pool = [C(q), 0]`, with the curve state in `market.shares`, so solvency is
  `C(q) >= max(q_no, q_yes)` by construction and the escrow holds exactly
  `C(q)`.
- **Pricing.** The marginal price of outcome 0 given `(q_0, q_1)` is

  `p = ONE / (ONE + t)`, `t = exp(-D / b)`, `D = |q_0 - q_1|`

  computed in fixed point with `ONE = 10_000` and an internal scale of `1e12`
  for the `exp` term. The complement is `1 - p`, bounded in `[0, ONE]`. Because
  the function always prices *outcome 0 of the pair*, buy/sell/price orient
  `q_self`/`q_other` around the requested outcome (`1 - idx`).
- **Trading.** `buy_shares` takes a spend budget, caps the share-to-clear within
  it at the marginal cost, and credits the actual spend to escrow.
  `sell_shares` pays the marginal value of the shares sold and reflects the
  proceeds for instant re-margining. Both are rejected on pari-mutuel markets,
  and no trade touches `pool` except through `C` (whole-token rounding may leave
  a share on the floor at `pay == 0`, which is acceptable for granular budgets).
- **Resolution.** On finalize the LMSR branch pays the creator
  `sweep = C(q) - q_winning` (the seeded liquidity above the winning side's
  coverage) and trims the escrow ledger to `q_winning`. Claims are then strictly
  one-for-one, settled against the winning coverage, with the claim draining the
  escrow ledger in lockstep with transfers.
- **Cancellation.** LMSR markets cannot be cancelled once created — the curve is
  already priced in and sellers could unwind at the seed's expense. The only
  exit is resolution (`LmsrNotCancellable`).
- **Exposure.** `price(market_id, outcome)` returns the current marginal price in
  the same fixed-point units.

The `exp` term at `b` in whole tokens is approximated in integer arithmetic such
that `exp(-D/b) == 1/2^ceil(ln2 * D / b)`, truncated to an internal scale; the
rounding cost is at most one unit of price complementarity, which we assert in
tests rather than fight.

## Consequences

- Two pricing models coexist behind one `create_market` signature, so all
  existing pari-mutuel tests, snapshots, and integration flows are unchanged.
- LMSR markets get continuous marginal pricing, buy/sell entry and exit, and a
  creator liquidity subsidy that is clawed back at resolution only if the
  creator's side loses.
- `deposit` and `cancel_market` are pari-mutuel-only; `buy_shares`,
  `sell_shares`, and `price` are LMSR-only — model misuse panics with
  `PricingModelMismatch` rather than silently misbehaving.
- Fixed-point flooring means very small trades can round to `pay == 0` for a
  single share; acceptable for whole-token accounting and covered by a
  regression test (`buy_shares_tiny_budget_still_clears_one_share`).
- Claimed up-front sinks: on `i128` balances, the exact-repayment invariant
  `pool == C(q)` holds throughout, so rounding dust at the end of life resolves
  to at most a few whole tokens paid to the creator or left unclaimed.