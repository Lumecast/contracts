# ADR 0006: Comprehensive LMSR Property Testing

## Status
Accepted

## Context
The LMSR (Logarithmic Market Scoring Rule) pricing module is the core of Lumecast's v2 automated market maker. It handles real value and must maintain strict invariants:
- **Solvency**: `C(q0, q1) >= max(q0, q1)` — escrow always covers all winning shares
- **Price bounds**: `0 <= p0, p1 <= 1` and `p0 + p1 = 1` (within fixed-point rounding)
- **Monotonicity**: Cost never decreases when shares are added
- **Budget respect**: `cost_to_buy(budget) <= budget`

The original test suite covered basic functionality but lacked systematic property-based testing for edge cases and extreme parameters.

## Decision
Expand the LMSR test suite with comprehensive property tests covering:

### 1. Boundary Parameter Tests
- Minimum liquidity (`b = LMSR_B_MIN = 1`)
- Maximum liquidity (`b = LMSR_B_MAX = 1e12`)
- Zero and negative budgets
- Extreme share counts (up to 50M shares)

### 2. Invariant Tests
- Solvency invariant under all tested parameters
- Price complementarity (`p0 + p1 ≈ ONE`) for all test cases
- Cost monotonicity in each outcome
- Round-trip property (buy then sell returns to original state)

### 3. Symmetry and Consistency Tests
- Buy/sell cost symmetry (buy cost ≈ sell proceeds)
- Cost function matches integrated marginal price
- Price bounds under extreme imbalance

### 4. Edge Case Tests
- Zero/negative budgets for `shares_affordable`
- Very small budgets at high prices
- Large share counts stress test
- Pari-mutuel payout edge cases (zero pool, zero winning shares, etc.)

## Consequences

### Positive
- **High confidence**: 12 new property tests covering critical invariants
- **Regression protection**: Any change breaking invariants will be caught
- **Parameter validation**: Confirms LMSR works across full parameter range
- **Documentation**: Tests serve as executable specifications of expected behavior

### Negative
- **Test runtime**: Additional ~0.1s test time (acceptable)
- **Maintenance burden**: More tests to update if API changes (mitigated by stable API)

## Implementation Notes
- Tests use deterministic pseudo-random walks for reproducibility
- No external dependencies (pure Rust, no `proptest` crate needed)
- Tests added to `pricing/src/lib.rs` in the existing test module
- Existing fuzz walk test (`lmsr_walk_never_breaks_solvency_bound`) retained and complemented

## Alternatives Considered
1. **Property-based testing crate (proptest/quickcheck)**: More powerful but adds dependency and complexity. Current deterministic approach is sufficient for core invariants.
2. **Fuzzing with cargo-fuzz**: Excellent for finding crashes but requires separate setup. Unit property tests run in standard `cargo test`.
3. **Formal verification**: High assurance but very high effort. Deferred to audit phase (M5).

## Testing
All new tests pass:
- `lmsr_min_liquidity_works`
- `lmsr_max_liquidity_works`
- `lmsr_round_trip_buy_sell`
- `lmsr_price_bounds_extreme_imbalance`
- `lmsr_cost_monotonicity_large_numbers`
- `lmsr_shares_affordable_edge_cases`
- `lmsr_buy_sell_symmetry`
- `lmsr_solvency_invariant_comprehensive`
- `lmsr_price_complementarity_comprehensive`
- `lmsr_cost_to_buy_respects_budget`
- `lmsr_large_share_counts`
- `pari_mutuel_payout_edge_cases`