# Lumecast Market Contract Interface

This document describes the complete interface of the Lumecast Market Contract, including all entry points, events, errors, and data types.

## Contract Address
Deployed on Stellar/Soroban. The contract is a single instance that manages multiple markets keyed by `market_id`.

## Entry Points

### Constructor
```rust
__constructor(env: Env, admin: Address)
```
Initializes the contract with the platform admin. Only callable once at deployment.

### Admin Functions

#### `admin(env: Env) -> Option<Address>`
Returns the contract admin address.

#### `set_governance(env: Env, caller: Address, committee: Vec<Address>, quorum: u32, protocol_fee_receiver: Option<Address>)`
Configures the dispute committee, quorum, and protocol fee receiver. Admin-only.
- `committee`: List of committee member addresses (must not be empty)
- `quorum`: Minimum votes required (1..=committee.len())
- `protocol_fee_receiver`: Optional address to receive protocol share of slashed bonds

#### `pause(env: Env, caller: Address)`
Pauses the contract. Admin-only. All state-changing operations are rejected while paused.
Emits `PauseEvent`.

#### `unpause(env: Env, caller: Address)`
Unpauses the contract. Admin-only.
Emits `UnpauseEvent`.

#### `paused(env: Env) -> bool`
Returns the current pause state.

#### `governance(env: Env) -> GovernanceConfig`
Returns the current governance configuration.

### Market Lifecycle

#### `create_market(env: Env, creator: Address, params: CreateMarketParameter) -> u64`
Creates a new binary (YES/NO) prediction market.
- `params.resolver`: Address responsible for proposing resolution
- `params.asset`: Settlement asset (USDC Stellar Asset Contract)
- `params.question`: The prediction question
- `params.close_ts`: Timestamp after which deposits are rejected
- `params.resolution_ts`: Earliest timestamp for resolution proposal
- `params.b`: LMSR liquidity parameter (0 = pari-mutuel, >0 = LMSR)
Returns the assigned `market_id`.

Emits `CreateMarketEvent`.

#### `market(env: Env, id: u64) -> Option<Market>`
Returns the market by ID, or `None` if not found.

#### `resolution(env: Env, market_id: u64) -> Option<Resolution>`
Returns the per-market resolution state, if any.

### Trading (Pari-mutuel)

#### `deposit(env: Env, market_id: u64, from: Address, outcome: Outcome, amount: i128) -> Position`
Buys shares in a pari-mutuel market at fixed 1:1 price.
- Pulls `amount` of settlement asset from `from` into escrow
- Mints `amount` shares of `outcome` to `from`
- Only allowed while market is `Open` and before `close_ts`
Emits `DepositEvent`.

### Trading (LMSR)

#### `buy_shares(env: Env, market_id: u64, from: Address, outcome: Outcome, amount_in: i128) -> Position`
Buys shares in an LMSR market with a spend budget.
- `amount_in`: Maximum tokens willing to spend
- Receives `shares_out` shares at marginal price
- Contract pulls exactly `lmsr_cost_to_buy` tokens
- Only allowed while market is `Open` and before `close_ts`
Emits `BuySharesEvent`.

#### `sell_shares(env: Env, market_id: u64, from: Address, outcome: Outcome, shares_in: i128) -> i128`
Sells shares back to an LMSR market.
- `shares_in`: Shares to redeem (must be <= position)
- Receives `proceeds` = `C(q) - C(q - shares_in)`
- Allowed while market is `Open` (including after `close_ts`)
Emits `SellSharesEvent`.

#### `price(env: Env, market_id: u64, outcome: Outcome) -> i128`
Returns the marginal price of `outcome` in fixed point (10,000 = 1.0).
Only available for LMSR markets.

### Cancellation

#### `cancel_market(env: Env, market_id: u64, caller: Address)`
Cancels an open pari-mutuel market and refunds all participants.
- Caller must be market creator or resolver
- Only allowed while market is `Open`
- Refunds all deposits 1:1, zeros positions and pool
Emits `CancelMarketEvent`.

Not available for LMSR markets (use resolution flow instead).

### Resolution

#### `propose_outcome(env: Env, market_id: u64, resolver: Address, outcome: Outcome, bond: i128)`
Resolver proposes the final outcome with a bond.
- Only resolver may call
- Only while market is `Open` and after `resolution_ts`
- Escrows `bond` tokens from resolver
- Moves market to `Proposed` state, starts dispute window
Emits `ProposeEvent`.

#### `dispute(env: Env, market_id: u64, challenger: Address, counter_bond: i128)`
Challenges a proposed outcome within the dispute window.
- `counter_bond` must be >= proposer's bond
- Escrows `counter_bond` tokens from challenger
- Moves market to `Disputed` state
Emits `DisputeEvent`.

#### `vote(env: Env, market_id: u64, voter: Address, outcome: Outcome)`
Committee member casts a vote on a disputed market.
- Only configured committee members may vote
- Latest vote replaces earlier ones
- Only allowed while market is `Disputed`
Emits `VoteEvent`.

#### `finalize(env: Env, market_id: u64)`
Settles a proposed or disputed market.
- **Uncontested**: After dispute window expires, returns proposer's bond, locks outcome
- **Disputed**: Once quorum reached, majority wins; losing bonds slashed (70% to winners, 30% to protocol)
- LMSR: Sweeps surplus liquidity to creator, leaves 1:1 coverage for winners
- Pari-mutuel: Keeps full pool for pro-rata claims
Emits `FinalizeEvent`.

### Claims

#### `claim(env: Env, market_id: u64, claimant: Address, outcome: Outcome) -> i128`
Claims payout for a resolved market.
- Only winning outcome holders receive payout
- LMSR: 1:1 per share (capped by escrow)
- Pari-mutuel: Pro-rata share of total pool
- Position zeroed after claim (double-claim protection)
Emits `ClaimEvent`.

#### `position(env: Env, market_id: u64, owner: Address, outcome: Outcome) -> Position`
Returns a holder's position (shares) in a market/outcome.

## Events

All events are emitted as Soroban contract events with topics for indexing.

| Event | Topics | Data |
|-------|--------|------|
| `CreateMarketEvent` | `market_id` | `creator`, `resolver`, `question`, `b` |
| `DepositEvent` | `market_id`, `outcome_index` | `from`, `amount` |
| `BuySharesEvent` | `market_id`, `outcome_index` | `from`, `amount_in`, `shares_out` |
| `SellSharesEvent` | `market_id`, `outcome_index` | `from`, `amount_out`, `shares_in` |
| `CancelMarketEvent` | `market_id` | `cancelled_by` |
| `ProposeEvent` | `market_id`, `outcome_index` | `proposer`, `bond` |
| `DisputeEvent` | `market_id`, `outcome_index` | `challenger`, `counter_bond` |
| `VoteEvent` | `market_id`, `outcome_index` | `voter` |
| `FinalizeEvent` | `market_id`, `outcome_index` | `uncontested`, `returned_bond` |
| `ClaimEvent` | `market_id`, `outcome_index` | `claimant`, `amount` |
| `PauseEvent` | `paused_by` | — |
| `UnpauseEvent` | `unpaused_by` | — |

## Errors

| Code | Error | Description |
|------|-------|-------------|
| 1 | `MarketNotFound` | Referenced market does not exist |
| 2 | `MarketNotOpen` | Operation requires market to be in `Open` state |
| 3 | `AfterClose` | Trading has closed; no more deposits accepted |
| 4 | `ZeroAmount` | Amounts must be strictly positive |
| 5 | `InvalidTiming` | `close_ts` must be strictly before `resolution_ts` |
| 6 | `UnknownOutcome` | Outcome index does not map to a known outcome |
| 7 | `Unauthorized` | Caller is not authorized for this operation |
| 8 | `NotResolver` | Only the market resolver may propose an outcome |
| 9 | `ResolutionNotReady` | Resolution proposed before `resolution_ts` |
| 10 | `NotProposable` | Market is already proposed/disputed/resolved |
| 11 | `NotProposed` | Resolution requires market to be in proposed state |
| 12 | `DisputeWindowClosed` | Dispute raised outside dispute window |
| 13 | `BondTooLow` | Counter-bond must cover proposer's bond |
| 14 | `CommitteeNotSet` | No dispute committee configured |
| 15 | `NotDisputed` | Resolution requires market to be disputed |
| 16 | `NotCommitteeMember` | Caller is not a committee member |
| 17 | `QuorumNotReached` | Not enough committee votes cast |
| 18 | `DisputeWindowOpen` | Cannot finalize uncontested while window open |
| 19 | `NotResolved` | Claim attempted before market resolved |
| 20 | `InvalidGovernance` | Invalid governance config (empty committee, bad quorum) |
| 21 | `InvalidLiquidity` | Liquidity parameter `b` outside `[0, LMSR_B_MAX]` |
| 22 | `PricingModelMismatch` | Operation only applies to market's pricing model |
| 23 | `LmsrNotCancellable` | LMSR markets cannot be cancelled |
| 24 | `InsufficientShares` | Holder does not own enough shares to sell |
| 25 | `ReentrancyDetected` | Reentrancy detected during external call |
| 26 | `ContractPaused` | Contract is paused; no state-changing operations allowed |

## Data Types

### `Outcome`
```rust
enum Outcome {
    Yes,  // index 0
    No,   // index 1
}
```

### `MarketState`
```rust
enum MarketState {
    Open,
    Proposed(Outcome),
    Disputed(Outcome),
    Resolved(Outcome),
    Cancelled,
}
```

### `CreateMarketParameter`
```rust
struct CreateMarketParameter {
    resolver: Address,
    asset: Address,
    question: String,
    close_ts: u64,
    resolution_ts: u64,
    b: i128,  // 0 = pari-mutuel, >0 = LMSR liquidity
}
```

### `Market`
```rust
struct Market {
    id: u64,
    question: String,
    creator: Address,
    resolver: Address,
    asset: Address,
    close_ts: u64,
    resolution_ts: u64,
    state: MarketState,
    pool: Vec<i128>,      // escrowed tokens per outcome (LMSR: pool[0] = C(q))
    shares: Vec<i128>,    // outstanding shares per outcome
    b: i128,              // LMSR liquidity parameter
}
```

### `Position`
```rust
struct Position {
    owner: Address,
    market_id: u64,
    outcome: Outcome,
    shares: i128,
}
```

### `GovernanceConfig`
```rust
struct GovernanceConfig {
    committee: Vec<Address>,
    quorum: u32,
    protocol_fee_receiver: Option<Address>,
}
```

### `Resolution`
```rust
struct Resolution {
    proposed: u32,              // outcome index
    proposer: Address,
    bond: i128,
    proposed_ledger: u32,       // anchors dispute window
    challenges: Vec<(Address, i128)>,  // (challenger, counter_bond)
    votes: Vec<(Address, u32)>,        // (voter, outcome_index)
}
```

## Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `OUTCOME_COUNT` | 2 | Number of outcomes (YES/NO) |
| `LMSR_B_MIN` | 1 | Minimum LMSR liquidity parameter |
| `LMSR_B_MAX` | 1_000_000_000_000 | Maximum LMSR liquidity parameter |
| `DISPUTE_WINDOW_LEDGERS` | 34_560 | Dispute window duration (~48h at 5s/ledger) |
| `PROTOCOL_SLASH_BPS` | 1_000 | Protocol fee from slashed bonds (10%) |
| `ONE` | 10_000 | Fixed-point basis (1.0 = 10,000) |

## Security Features

### Reentrancy Guard
All external token calls (`transfer`, `transfer_from`) are wrapped in a reentrancy guard. If a reentrant call is detected, the transaction panics with `Error::ReentrancyDetected`.

### Circuit Breaker
Admin can pause the contract at any time via `pause()`. While paused, all state-changing operations are rejected with `Error::ContractPaused`. Read-only views remain available.

### Authorization
All state-changing functions require authentication from the relevant party (creator, resolver, depositor, committee member, admin).

### Solvency Invariant
LMSR markets maintain `escrow = C(q) >= max(q0, q1)` at all times, guaranteeing all winning shares can be paid 1:1.

## Deployment

```bash
# Build
soroban contract build

# Deploy to testnet
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/market.wasm \
  --source <identity> \
  --network testnet

# Initialize (constructor called automatically with --init)
soroban contract invoke \
  --id <contract-id> \
  --source <identity> \
  --network testnet \
  -- __constructor --admin <admin-address>
```