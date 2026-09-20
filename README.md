# Lumecast Contracts

Soroban smart contracts powering **Lumecast**, a prediction market platform built on Stellar. This repo is the on-chain core: market creation, escrow, share accounting, pricing, and resolution.

## Table of Contents
- [Overview](#overview)
- [Architecture](#architecture)
- [Repo Structure](#repo-structure)
- [Prerequisites](#prerequisites)
- [Getting Started](#getting-started)
- [Testing](#testing)
- [Deploying](#deploying)
- [Contract Interface](#contract-interface)
- [Security](#security)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

## Overview

Lumecast lets users create and trade on binary (YES/NO) prediction markets, settled in USDC on Stellar. This repo contains:

- **`market` contract** — market lifecycle, escrow, and share accounting
- **`resolution` module** — bonded propose/dispute/finalize flow for determining outcomes
- **`pricing` module** — pari-mutuel pool logic (v1) and LMSR automated market maker (v2)

Related repos: [`lumecast/frontend`](https://github.com/lumecast/frontend) (web app) and [`lumecast/indexer-api`](https://github.com/lumecast/indexer-api) (off-chain indexing/query layer).

## Architecture

```
┌─────────────────────────────────────────────┐
│                Market Contract               │
│  ┌───────────┐  ┌───────────┐  ┌──────────┐  │
│  │  Escrow   │  │  Pricing  │  │Resolution│  │
│  │  (USDC)   │  │ (pari-mut/│  │ (propose/│  │
│  │           │  │   LMSR)   │  │ dispute) │  │
│  └───────────┘  └───────────┘  └──────────┘  │
└─────────────────────────────────────────────┘
         │                              │
         ▼                              ▼
  Stellar Asset Contract         Events (indexed by
  (USDC token client)            lumecast/indexer-api)
```

Each market is an isolated instance of contract state (not a separate deployed contract per market — markets are keyed by ID within a single deployed contract instance, to keep deployment and upgrade management simple).

## Repo Structure

```
contracts/
├── market/           # Core market contract: create, deposit, claim, cancel
│   ├── src/
│   └── Cargo.toml
├── resolution/        # Propose/dispute/finalize logic, shared by market contract
│   ├── src/
│   └── Cargo.toml
├── pricing/            # Pari-mutuel + LMSR pricing modules
│   ├── src/
│   └── Cargo.toml
├── tests/              # Integration tests across modules
├── docs/
│   └── decisions/     # Design decision notes (ADR-style)
├── scripts/            # Deploy + interaction scripts (soroban CLI wrappers)
├── PLAN.md
└── README.md
```

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable, `wasm32-unknown-unknown` target)
- [Soroban CLI](https://developers.stellar.org/docs/tools/developer-tools/cli/install-cli)
- [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/cli) for testnet interaction

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked soroban-cli
```

## Getting Started

```bash
git clone https://github.com/lumecast/contracts.git
cd contracts

# Build all contracts to WASM
soroban contract build

# Run the local sandbox
soroban network start local
```

## Testing

```bash
# Unit + integration tests
cargo test --workspace

# With coverage
cargo tarpaulin --workspace --out Html
```

CI runs `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` on every PR. All three must pass before merge.

## Deploying

```bash
# Deploy to testnet
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/market.wasm \
  --source <your-identity> \
  --network testnet

# Invoke a function
soroban contract invoke \
  --id <contract-id> \
  --source <your-identity> \
  --network testnet \
  -- create_market --question "..." --close_ts ... --resolver ...
```

See `scripts/` for wrapped versions of common deploy/invoke flows.

## Contract Interface

| Function | Description |
|---|---|
| `create_market(question, close_ts, resolution_ts, resolver, asset)` | Opens a new market |
| `deposit(market_id, outcome, amount)` | Buy shares in an outcome |
| `cancel_market(market_id)` | Refund all participants if a market can't be resolved fairly |
| `propose_outcome(market_id, outcome, bond)` | Resolver proposes the final outcome with an escrowed bond |
| `dispute(market_id, counter_bond)` | Challenge a proposed outcome within the dispute window |
| `vote(market_id, outcome)` | Committee member casts (or replaces) a vote on a disputed market |
| `finalize(market_id)` | Locks in the outcome and settles / slashes bonds |
| `claim(market_id, outcome)` | Withdraw winnings post-resolution (pari-mutuel) |

Events: `CreateMarketEvent`, `DepositEvent`, `CancelMarketEvent`,
`ProposeEvent`, `DisputeEvent`, `VoteEvent`, `FinalizeEvent`, `ClaimEvent`.

Full parameter types and events are documented in `docs/interface.md` (generated from contract doc comments).

## Security

- This code has **not yet been audited**. See [PLAN.md](./PLAN.md) for the audit milestone and timeline.
- Found a vulnerability? Please **do not open a public issue**. Email `security@lumecast.io` (or the org's designated security contact) instead.
- A bug bounty will be announced ahead of mainnet launch.

## Roadmap

See [PLAN.md](./PLAN.md) for the full milestone breakdown (M0 through M7, foundations through mainnet launch).

## Contributing

1. Fork and branch from `main`
2. Write tests for any new logic — PRs without tests will not be merged
3. Run `cargo fmt` and `cargo clippy` before opening a PR
4. Open a PR against `main`, describing the change and linking any relevant design decision doc

## License

MIT
