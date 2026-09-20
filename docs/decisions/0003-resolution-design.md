# ADR-0003: Bonded resolution with dispute window, committee vote, and slashing

## Status

Accepted

## Context

Once a prediction market closes, someone must declare the winning outcome and
deposits must become payable. A resolver who is free to pick any outcome (and
paid regardless) could settle markets dishonestly — front-running the real-world
result, or colluding with traders. We need a settlement path that gives the
resolver an incentive to be correct and lets other participants force an appeal
when the proposal is wrong, without requiring a trusted oracle or multisig for
every market.

Options considered:

1. **Trusted oracle only.** Admissions committee signs every resolution. Simple,
   but a single point of failure and a permanent gas/voting cost per market.
2. **Resolver-only, no bond.** Cheapest, but gives the resolver a free corner —
   they could push an outcome that profits their own position.
3. **Optimistic with bonded challenge + committee appeal (chosen).** The resolver
   proposes an outcome and escrows a bond. Anyone may dispute within a fixed
   window by posting a counter-bond at least as large. If a dispute happens, the
   platform committee votes; the losing side's bond is forfeited and split
   between the winning side and the protocol. Most markets settle uncontested.

## Decision

Resolution lifecycle (all state on the market contract):

1. **Propose** — after `resolution_ts`, the market `resolver` calls
   `propose_outcome(market_id, resolver, outcome, bond)`. The bond (in the
   settlement asset) is pulled from the resolver into escrow. State moves
   `Open -> Proposed(outcome)`.
2. **Dispute** — while inside `DISPUTE_WINDOW_LEDGERS` (34,560 ledgers, ~48 h)
   of the proposal, anyone may call `dispute(market_id, challenger,
   counter_bond)` with `counter_bond >= bond`. The counter-bond is escrowed and
   state moves `Proposed -> Disputed`. A dispute requires a configured committee
   (`CommitteeNotSet` otherwise).
3. **Vote** — committee members call `vote(market_id, voter, outcome)`; each
   member's latest vote replaces any prior vote. Membership and quorum come from
   `GovernanceConfig`, set by the contract admin via `set_governance`.
4. **Finalize** — `finalize(market_id)` (permissionless):
   - Uncontested `Proposed`: once the dispute window has elapsed, the proposed
     outcome wins and the proposer's full bond is returned.
   - `Disputed`: once `votes.len() >= quorum`, the outcome with the majority of
     votes wins; ties favor the proposal. The losing side's bonds are forfeited
     and split with `slash_split` — 90% to the winning side, 10% to the protocol
     fee receiver (or the contract admin when unset). Challengers recover their
     own counter-bonds in full if the challenger side wins.
   - State moves to `Resolved(outcome)` and the resolution record is removed.
5. **Claim** — any winner calls `claim(market_id, claimant, outcome)`. The payout
   is `pari_mutuel_payout(shares, total_pool, winning_shares)`; the position is
   zeroed so double claims pay nothing. Losing-outcome positions pay 0.

Bond-splitting invariants: the total of bond funds leaves escrow at finalize
(no funds stranded), and the protocol's 10% cut is only taken on forfeited
bonds, never on the winning side's own stake.

## Consequences

- Most markets settle with a single transfer (uncontested), keeping resolution
  cheap; appeals are rare and funded by the disputer's counter-bond.
- An honest proposer bears zero cost; a dishonest one is burned for the
  committee's benefit and the protocol's.
- The bond is paid in the settlement asset, so it cannot be dumped back into
  the prediction contract — `deposit` rejects non-`Open` markets and
  non-winning claims pay nothing.
- Quorum and committee size are protocol policy; a small committee means a
  faster narrative but a larger trust surface. Locking that into
  `GovernanceConfig` admin-settable keeps M2 shipping without a governance
  token.
- `Outcome::from_index` guarantees the majority index maps to a real outcome, so
  finalize cannot lock an invalid state. Tie-favoring-proposal is a deliberate
  bias towards the resolver unless the committee definitively disagrees.