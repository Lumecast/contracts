use soroban_sdk::{contracttype, Env};

use crate::types::Market;

/// Persistent storage keys for the market contract.
///
/// Markets and positions are long-lived instance entries; the monotonic
/// counter that hands out market ids lives alongside them.
#[contracttype]
pub enum DataKey {
    /// Monotonic counter for assigning market ids.
    MarketCounter,
    Market(u64),
}

/// Instance TTL bump applied on every write so active markets stay alive
/// between operations.
const INSTANCE_TTL_THRESHOLD: u32 = 5_000;
const INSTANCE_TTL_EXTEND_TO: u32 = 10_000;

/// Keep the contract instance (and everything stored in it) alive.
fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

/// Assign the next monotonic market id.
pub fn next_market_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::MarketCounter)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&DataKey::MarketCounter, &id);
    bump_instance(env);
    id
}

/// Persist a market.
pub fn write_market(env: &Env, market: &Market) {
    env.storage()
        .instance()
        .set(&DataKey::Market(market.id), market);
    bump_instance(env);
}

/// Read a market by id, if it exists.
pub fn get_market(env: &Env, id: u64) -> Option<Market> {
    env.storage().instance().get(&DataKey::Market(id))
}
