# ADR 0005: Security Hardening — Reentrancy Guard and Circuit Breaker

## Status
Accepted

## Context
As the Lumecast contracts approach testnet launch (M6) and mainnet (M7), security hardening is critical. The contracts handle real USDC value and interact with external token contracts (Stellar Asset Contracts) via `transfer` and `transfer_from` calls. These external calls introduce reentrancy risk: a malicious token contract could call back into the market contract before the first call completes, potentially draining funds or corrupting state.

Additionally, a circuit breaker / pause mechanism is needed for emergency stops if a vulnerability is discovered post-deployment.

## Decision
Implement two complementary security mechanisms:

### 1. Reentrancy Guard
- A boolean flag `ReentrancyGuard` in persistent storage
- Set to `true` before any external token call (`transfer`, `transfer_from`)
- Set to `false` after the call completes
- If a call is attempted while the guard is already `true`, panic with `Error::ReentrancyDetected`
- Applied to all functions making external calls: `create_market` (LMSR seed), `deposit`, `buy_shares`, `sell_shares`, `cancel_market`, `propose_outcome`, `dispute`, `finalize`, `claim`

### 2. Circuit Breaker / Pause
- A boolean flag `Paused` in persistent storage
- Admin-only `pause()` and `unpause()` functions
- `paused()` view function to read state
- All state-changing entry points check `is_paused()` at the start and panic with `Error::ContractPaused` if true
- Read-only views (`market`, `resolution`, `position`, `price`, `admin`, `governance`, `paused`) remain available

## Consequences

### Positive
- **Reentrancy protection**: Prevents the most common class of smart contract vulnerabilities
- **Emergency stop**: Admin can pause the contract instantly if a bug is found
- **Minimal overhead**: Single storage read/write per external call
- **Composable**: Works with existing authorization and state checks

### Negative
- **Slightly higher gas costs**: Extra storage operations for guard enter/exit
- **Admin trust assumption**: Pause power is centralized; mitigated by multi-sig admin in production
- **Not a silver bullet**: Does not protect against all vulnerability classes (e.g., logic errors, oracle manipulation)

## Alternatives Considered
1. **No reentrancy guard**: Relies on Soroban's single-threaded execution model. Rejected because malicious token contracts can still reenter via cross-contract calls.
2. **Mutex pattern with try_lock**: More complex, same security properties. Simple boolean flag is sufficient for single-threaded Soroban.
3. **Checks-effects-interactions pattern**: Already followed, but guard adds defense-in-depth.
4. **Upgradeable contracts for emergency fixes**: Adds significant complexity and trust assumptions. Pause is simpler and faster to activate.

## Implementation Notes
- Guard uses instance storage (same as other contract state) for atomicity with state changes
- Pause state persists across contract upgrades (if upgradeable)
- Error codes 25 (`ReentrancyDetected`) and 26 (`ContractPaused`) added to `Error` enum
- All 9 state-changing functions wrapped with `with_reentrancy_guard` helper
- `check_not_paused` helper called at start of all 9 state-changing functions

## Testing
- Unit tests verify pause blocks all state-changing operations
- Unit tests verify read-only operations work while paused
- Unit tests verify reentrancy guard doesn't interfere with normal operation
- Unit tests verify only admin can pause/unpause