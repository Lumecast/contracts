use soroban_sdk::{contracttype, Address, Env, Vec};

use crate::error::Error;
use crate::types::{Market, Outcome, Position};
use lumecast_resolution::{GovernanceConfig, Resolution};

/// Persistent storage keys for the market contract.
///
/// Markets and positions are long-lived instance entries; the monotonic
/// counter that hands out market ids lives alongside them.
#[contracttype]
pub enum DataKey {
    /// Monotonic counter for assigning market ids.
    MarketCounter,
    Market(u64),
    /// Full escrow holders per market, used for refund scans on cancel.
    Holders(u64),
    Position(u64, Address, u32),
    /// Contract admin: configures governance (constructor-set).
    Admin,
    /// Platform governance: dispute committee, quorum, protocol fee receiver.
    Governance,
    /// Per-market resolution state (proposal + challenges + votes).
    Resolution(u64),
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

/// Read a market by id, failing with [`Error::MarketNotFound`] if absent.
pub fn require_market(env: &Env, id: u64) -> Result<Market, Error> {
    get_market(env, id).ok_or(Error::MarketNotFound)
}

/// Read a holder's position in `market_id`/`outcome`, defaulting to zero.
pub fn read_position(env: &Env, market_id: u64, owner: &Address, outcome: Outcome) -> Position {
    let key = DataKey::Position(market_id, owner.clone(), outcome.index());
    env.storage()
        .instance()
        .get(&key)
        .unwrap_or_else(|| Position {
            owner: owner.clone(),
            market_id,
            outcome,
            shares: 0,
        })
}

/// Persist a holder's position.
pub fn write_position(env: &Env, position: &Position) {
    let key = DataKey::Position(
        position.market_id,
        position.owner.clone(),
        position.outcome.index(),
    );
    env.storage().instance().set(&key, position);
    bump_instance(env);
}

/// Read the full set of escrow holders for a market.
pub fn read_holders(env: &Env, market_id: u64) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Holders(market_id))
        .unwrap_or_else(|| Vec::new(env))
}

/// Record a holder for a market (idempotent).
pub fn add_holder(env: &Env, market_id: u64, holder: &Address) {
    let mut holders = read_holders(env, market_id);
    for existing in holders.iter() {
        if &existing == holder {
            bump_instance(env);
            return;
        }
    }
    holders.push_back(holder.clone());
    env.storage()
        .instance()
        .set(&DataKey::Holders(market_id), &holders);
    bump_instance(env);
}

/// Persist the contract admin (constructor-only).
pub fn write_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
    bump_instance(env);
}

/// Read the contract admin, if set.
pub fn read_admin(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Admin)
}

/// Persist the platform governance configuration.
pub fn write_governance(env: &Env, config: &GovernanceConfig) {
    env.storage().instance().set(&DataKey::Governance, config);
    bump_instance(env);
}

/// Read the platform governance configuration, defaulting to an unset one.
pub fn read_governance(env: &Env) -> GovernanceConfig {
    env.storage()
        .instance()
        .get(&DataKey::Governance)
        .unwrap_or(GovernanceConfig {
            committee: Vec::new(env),
            quorum: 0,
            protocol_fee_receiver: None,
        })
}

/// Persist the per-market resolution state.
pub fn write_resolution(env: &Env, market_id: u64, resolution: &Resolution) {
    env.storage()
        .instance()
        .set(&DataKey::Resolution(market_id), resolution);
    bump_instance(env);
}

/// Read the per-market resolution state, if any.
pub fn read_resolution(env: &Env, market_id: u64) -> Option<Resolution> {
    env.storage().instance().get(&DataKey::Resolution(market_id))
}

/// Remove the per-market resolution state once finally settled.
pub fn remove_resolution(env: &Env, market_id: u64) {
    env.storage().instance().remove(&DataKey::Resolution(market_id));
}
