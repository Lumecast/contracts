#![no_std]

//! Lumecast core market contract.
//!
//! M0 (Foundations): repo scaffolding, CI, pinned Soroban SDK, and the core
//! domain model — `Market`, `Position`, `Outcome` — plus a working
//! `create_market` entry point in the local sandbox. `deposit`, `cancel` and
//! `claim` build on this in M1; `claim` also depends on the resolution module
//! (M2).

#[cfg(test)]
extern crate std;

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, String, Vec};

use crate::storage::{get_market, next_market_id, write_market};

mod error;
mod events;
mod storage;
mod types;

pub use error::Error;
pub use events::CreateMarketEvent;
pub use storage::DataKey;
pub use types::{Market, MarketState, Outcome, Position, OUTCOME_COUNT};

#[contract]
pub struct MarketContract;

#[contractimpl]
impl MarketContract {
    /// Create a new binary (YES/NO) market. No funds move yet — USDC enters
    /// via `deposit`.
    pub fn create_market(
        env: Env,
        creator: Address,
        resolver: Address,
        asset: Address,
        question: String,
        close_ts: u64,
        resolution_ts: u64,
    ) -> u64 {
        creator.require_auth();

        if close_ts >= resolution_ts {
            panic_with_error!(&env, Error::InvalidTiming);
        }

        let id = next_market_id(&env);
        let zeros = Vec::from_array(&env, [0i128; OUTCOME_COUNT as usize]);
        let market = Market {
            id,
            question,
            creator: creator.clone(),
            resolver,
            asset,
            close_ts,
            resolution_ts,
            state: MarketState::Open,
            pool: zeros.clone(),
            shares: zeros,
        };
        write_market(&env, &market);

        CreateMarketEvent {
            market_id: market.id,
            creator: market.creator.clone(),
            resolver: market.resolver.clone(),
            question: market.question.clone(),
        }
        .publish(&env);

        id
    }

    /// Read a market by id.
    pub fn market(env: Env, id: u64) -> Option<Market> {
        get_market(&env, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events as _};
    use soroban_sdk::{Env, Event as _};

    fn deploy(env: &Env) -> MarketContractClient<'_> {
        env.mock_all_auths();
        let contract_id = env.register(MarketContract, ());
        MarketContractClient::new(env, &contract_id)
    }

    fn new_market_with(env: &Env, client: &MarketContractClient<'_>, question: &str) -> u64 {
        client.create_market(
            &Address::generate(env),
            &Address::generate(env),
            &Address::generate(env),
            &String::from_str(env, question),
            &1_700_000_000,
            &1_700_086_400,
        )
    }

    #[test]
    fn create_market_persists_market() {
        let env = Env::default();
        let client = deploy(&env);
        let creator = Address::generate(&env);
        let resolver = Address::generate(&env);
        let asset = Address::generate(&env);

        let id = client.create_market(
            &creator,
            &resolver,
            &asset,
            &String::from_str(&env, "Will it rain tomorrow?"),
            &1_700_000_000,
            &1_700_086_400,
        );
        assert_eq!(id, 1);

        let market = client.market(&id).unwrap();
        assert_eq!(market.creator, creator);
        assert_eq!(market.resolver, resolver);
        assert_eq!(market.asset, asset);
        assert_eq!(market.state, MarketState::Open);
        assert_eq!(market.total_pool(), 0);
        assert_eq!(market.shares.len(), 2);
    }

    #[test]
    fn create_market_assigns_increasing_ids() {
        let env = Env::default();
        let client = deploy(&env);

        let first = new_market_with(&env, &client, "Q1");
        let second = new_market_with(&env, &client, "Q2");
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn create_market_rejects_bad_timing() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            let creator = Address::generate(&env);
            client.create_market(
                &creator,
                &Address::generate(&env),
                &Address::generate(&env),
                &String::from_str(&env, "Q"),
                &1_700_000_000,
                &1_700_000_000,
            )
        }));
        assert!(result.is_err());
    }

    #[test]
    fn create_market_emits_event() {
        let env = Env::default();
        let client = deploy(&env);
        let creator = Address::generate(&env);
        let resolver = Address::generate(&env);
        let question = String::from_str(&env, "Q");

        let id = client.create_market(
            &creator,
            &resolver,
            &Address::generate(&env),
            &question,
            &1_700_000_000,
            &1_700_086_400,
        );
        assert_eq!(id, 1);
        assert_eq!(
            env.events().all(),
            [CreateMarketEvent {
                market_id: 1,
                creator,
                resolver,
                question,
            }
            .to_xdr(&env, &client.address)]
        );
    }

    #[test]
    fn market_view_errors_on_unknown_id() {
        let env = Env::default();
        let client = deploy(&env);
        assert_eq!(client.market(&999), None);
    }
}
