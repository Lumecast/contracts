# Lumecast Contracts — Project Plan

## 1. Purpose
This repo holds the on-chain logic for Lumecast: a prediction market platform built on Soroban (Stellar's smart contract platform). It is the source of truth for market creation, share issuance, fund custody, and outcome resolution. Everything else in the Lumecast org (frontend, indexer) is downstream of what this repo defines.

## 2. Scope
In scope:
- Market lifecycle: create, trade, close, resolve, claim, cancel
- Escrow of USDC (Stellar Asset Contract token) per market
- Share/outcome token accounting (YES/NO to start; multi-outcome later)
- Pricing engine (pari-mutuel for v1, LMSR as v2 upgrade)
- Resolution flow: propose → dispute window → finalize
- Fee mechanism (protocol fee on winnings or trade spread)

Out of scope (lives in other repos):
- Indexing historical trades/markets for fast queries → `indexer-api`
- UI/wallet connection → `frontend`
- Off-chain data feeds / oracle infrastructure beyond the on-chain interface → future `oracle-adapter` repo if needed

## 3. Milestones

### M0 — Foundations (Week 1-2)
- Repo scaffolding, CI (build + test on every PR), Soroban SDK pinned version
- Core data structures: `Market`, `Position`, `Outcome`
- Local sandbox environment working end-to-end with a dummy contract

### M1 — Market Contract v1 (Pari-mutuel) (Week 3-5)
- [x] `create_market(question, close_ts, resolution_ts, resolver, asset)`
- [x] `deposit(market_id, outcome, amount)` — pulls USDC via token client, mints position
- [x] `claim(market_id, outcome)` — pays out pro-rata share of pool to winners post-resolution (M2 wiring)
- [x] `cancel_market(market_id)` — creator/resolver path if a market can't be resolved fairly, refunds all participants
- [x] Unit tests covering: happy path, double-claim prevention, deposits after close, claim before resolution, zero-liquidity edge cases

### M2 — Resolution Module (Week 5-7)
- [x] `propose_outcome(market_id, outcome, bond)` — resolver stakes a bond
- [x] Dispute window (34,560 ledgers ≈ 48h)
- [x] `dispute(market_id, counter_bond)` — opens a challenge, escalates to multisig vote
- [x] `vote(market_id, outcome)` — committee member votes; quorum-gated finalize
- [x] `finalize(market_id)` — locks outcome after window closes uncontested, or after multisig vote resolves a dispute
- [x] Slashing logic: losing side of a dispute forfeits bond to the winning side + protocol (10%)
- [x] Unit + integration tests for propose/dispute/vote/finalize/claim lifecycle

### M3 — LMSR Pricing Upgrade (Week 8-11)
- Implement LMSR cost function in fixed-point (`i128`) arithmetic
- `buy_shares(market_id, outcome, amount)` / `sell_shares(...)` at LMSR-computed price
- Liquidity parameter (`b`) tuning per market, admin-configurable at creation
- Extensive fuzz/property tests: no negative payouts, no way to drain the liquidity pool below solvency, price always in [0,1]

### M4 — Security Hardening (Week 11-13)
- Internal review pass against common Soroban pitfalls (reentrancy-equivalent patterns, authorization checks, integer overflow, storage bloat/rent)
- Fix findings, write regression tests for each
- Freeze contract interface for audit

### M5 — External Audit (Week 13-17, calendar time depends on auditor availability)
- Engage a Soroban/Rust-experienced auditing firm
- Triage and fix findings
- Publish audit report (transparency matters for a real-money product)

### M6 — Testnet Launch (Week 17-19)
- Deploy to Stellar testnet
- Run public "fake money" markets for real users via the frontend
- Monitor for economic exploits (edge trading near resolution, oracle front-running, LMSR liquidity attacks)

### M7 — Mainnet Launch (Week 19+, gated on legal sign-off)
- Deploy narrow scope first (one market category, one resolver committee)
- Circuit breaker / pause mechanism live from day one
- Post-launch monitoring dashboards wired to `indexer-api`

## 4. Key Design Decisions (and why)

| Decision | Choice | Rationale |
|---|---|---|
| Settlement asset | USDC (Stellar SAC) | Stable unit of account; XLM volatility is bad UX for a betting product |
| v1 pricing | Pari-mutuel | Simple, auditable, no liquidity-attack surface; ships fast |
| v2 pricing | LMSR | Gives live-moving odds, the actual "market" behavior people expect |
| Resolution | Bonded propose + dispute + multisig fallback | Fully centralized resolution doesn't scale trust; full decentralization (pure oracle) is overkill for v1 and hard to get right |
| Storage | Minimize on-chain storage, emit events for everything indexable | Soroban storage has rent costs; push history-keeping to the indexer |

## 5. Risks & Open Questions
- **Regulatory**: Real-money markets may require licensing depending on jurisdiction — legal sign-off is a hard gate before M7, not a formality.
- **Oracle risk**: Who sits on the multisig dispute committee, and how are they compensated/held accountable? Needs a governance doc before mainnet.
- **LMSR liquidity risk**: Needs careful modeling before M3 ships — a poorly tuned `b` parameter can make markets either too illiquid or too easy to manipulate near resolution.
- **Upgradability**: Soroban contracts can be upgradeable via WASM hash swap with admin auth — decide now whether Lumecast wants upgradeable contracts (faster iteration, more trust assumptions) or immutable-per-deployment (slower, more trustworthy) contracts.

## 6. Definition of Done (per milestone)
A milestone is done when: code merged to `main`, test coverage for new logic ≥ 90%, CI green, and a short design note added to `/docs/decisions/` explaining any non-obvious tradeoff made.