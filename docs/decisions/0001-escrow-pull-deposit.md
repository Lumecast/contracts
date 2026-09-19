# ADR-0001: Deposit uses escrow-pull with a pre-authorized allowance

## Status

Accepted

## Context

`lumecast-market` escrows the settlement asset while a market is open. The
first buy-side entry point, `deposit`, must move USDC from the depositor's
wallet into the contract. There are two canonical ways to do this with the
Stellar Asset Contract (SAC):

1. **Escrow-pull**: the depositor pre-approves the market contract, and
   `deposit` calls `transfer_from(depositor -> market)`.
2. **Transfer-then-forward**: the depositor sends tokens to the contract and
   the frontend registers the deposit separately (prone to misattribution,
   requires nonce bookkeeping, needs a relayer or callback to know the sender).

Because contract auth permits the *depositor* to authorize approved spend
in-band (`deposit` is a single user-initiated call), the pull model attributes
funds to the caller unambiguously with a single transaction. A
transfer-and-forward design has no trustworthy in-contract caller signal.

## Decision

`deposit(market_id, caller, outcome, amount)` requires `caller.require_auth()`,
then calls `transfer_from(caller -> contract, amount)` against the market's
settlement asset. No refund allowance is needed on cancel: the contract owns
the escrow balance and can `transfer` it out of its own balance without auth.

## Consequences

- One transaction per purchase; no relayer or frontend reconciliation step.
- Frontends must send one `approve(contract, max_uint, live_until)` beforehand.
- `transfer_from` re-checks the allowance at spend time, so a tightened or
  revoked allowance is respected instantly.
- Escrow balances are always exactly backed by `pool`; see ADR-0002.