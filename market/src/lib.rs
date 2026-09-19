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

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, String, Vec};

use crate::storage::{
    add_holder, get_market, next_market_id, read_position, require_market, write_market,
    write_position,
};

mod error;
mod events;
mod storage;
mod types;

pub use error::Error;
pub use events::{CreateMarketEvent, DepositEvent};
pub use storage::DataKey;
pub use types::{Market, MarketState, Outcome, Position, OUTCOME_COUNT};

/// Read a market by id, panicking with [`Error::MarketNotFound`] if absent.
fn must_get_market(env: &Env, id: u64) -> Market {
    require_market(env, id).unwrap_or_else(|err| panic_with_error!(env, err))
}

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

    /// Buy `amount` shares of `outcome` in `market_id`.
    ///
    /// v1 is pari-mutuel at a fixed 1:1 price, so one deposited USDC mints one
    /// share. The depositor must have approved this contract to spend `amount`
    /// of the market's settlement asset; the contract then pulls the tokens
    /// into escrow and credits the depositor's position.
    pub fn deposit(
        env: Env,
        market_id: u64,
        from: Address,
        outcome: Outcome,
        amount: i128,
    ) -> Position {
        from.require_auth();

        if amount <= 0 {
            panic_with_error!(&env, Error::ZeroAmount);
        }

        let mut market = must_get_market(&env, market_id);
        if market.state != MarketState::Open {
            panic_with_error!(&env, Error::MarketNotOpen);
        }
        if env.ledger().timestamp() > market.close_ts {
            panic_with_error!(&env, Error::AfterClose);
        }

        // Pull the settlement tokens out of the depositor's wallet into escrow.
        let token = token::TokenClient::new(&env, &market.asset);
        token.transfer_from(
            &env.current_contract_address(),
            &from,
            &env.current_contract_address(),
            &amount,
        );

        // Credit the pool and the depositor's position.
        let idx = outcome.index();
        market.pool.set(idx, market.pool.get(idx).unwrap() + amount);
        market
            .shares
            .set(idx, market.shares.get(idx).unwrap() + amount);
        write_market(&env, &market);

        let mut position = read_position(&env, market_id, &from, outcome);
        position.shares += amount;
        write_position(&env, &position);
        add_holder(&env, market_id, &from);

        DepositEvent {
            market_id,
            outcome_index: idx,
            from: from.clone(),
            amount,
        }
        .publish(&env);

        position
    }

    /// Read a holder's position in a market/outcome.
    pub fn position(env: Env, market_id: u64, owner: Address, outcome: Outcome) -> Position {
        read_position(&env, market_id, &owner, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
    use soroban_sdk::{token, Env, Event as _, Symbol, TryFromVal};

    fn deploy(env: &Env) -> MarketContractClient<'_> {
        env.mock_all_auths();
        let contract_id = env.register(MarketContract, ());
        MarketContractClient::new(env, &contract_id)
    }

    struct Funded {
        market_id: u64,
        usdc: Address,
        alice: Address,
    }

    /// Deploy a funded USDC-backed market owned by `alice` with a far-future
    /// close timestamp.
    fn funded_market(env: &Env, client: &MarketContractClient<'_>) -> Funded {
        let admin = Address::generate(env);
        let usdc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        token::StellarAssetClient::new(env, &usdc).mint(&Address::generate(env), &10_000_000);

        let alice = Address::generate(env);
        token::StellarAssetClient::new(env, &usdc).mint(&alice, &1_000_000);

        // Pre-approve the contract to pull settlement tokens on deposit.
        token::TokenClient::new(env, &usdc).approve(&alice, &client.address, &i128::MAX, &100_000);

        let market_id = client.create_market(
            &alice,
            &Address::generate(env),
            &usdc,
            &String::from_str(env, "Will it rain tomorrow?"),
            &1_700_000_000,
            &1_700_086_400,
        );
        Funded {
            market_id,
            usdc,
            alice,
        }
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

    #[test]
    fn deposit_mints_shares_and_escrows_tokens() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        let usdc = token::TokenClient::new(&env, &funded.usdc);
        let alice_before = usdc.balance(&funded.alice);

        let position = client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &1_000);
        assert_eq!(position.shares, 1_000);
        assert_eq!(position.outcome, Outcome::Yes);
        assert_eq!(position.market_id, funded.market_id);

        let market = client.market(&funded.market_id).unwrap();
        assert_eq!(market.pool.get(0).unwrap(), 1_000);
        assert_eq!(market.shares.get(0).unwrap(), 1_000);
        assert_eq!(market.pool.get(1).unwrap(), 0);
        assert_eq!(market.total_pool(), 1_000);

        assert_eq!(usdc.balance(&funded.alice), alice_before - 1_000);
        assert_eq!(usdc.balance(&client.address), 1_000);
    }

    #[test]
    fn deposit_accumulates_across_deposits() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &250);
        client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &750);

        let position = client.position(&funded.market_id, &funded.alice, &Outcome::Yes);
        assert_eq!(position.shares, 1_000);

        let market = client.market(&funded.market_id).unwrap();
        assert_eq!(market.shares.get(0).unwrap(), 1_000);
    }

    fn contract_event_topics(env: &Env) -> std::vec::Vec<Symbol> {
        env.events()
            .all()
            .events()
            .iter()
            .filter_map(|e| match &e.body {
                soroban_sdk::xdr::ContractEventBody::V0(v0) => v0
                    .topics
                    .first()
                    .map(|t| Symbol::try_from_val(env, t).unwrap()),
            })
            .collect()
    }

    #[test]
    fn deposit_emits_event() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &100);

        let topics = contract_event_topics(&env);
        assert!(
            topics.contains(&Symbol::new(&env, "deposit_event")),
            "expected deposit_event in {:?}",
            topics
        );
    }

    #[test]
    fn deposit_rejects_zero_amount() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            let funded = funded_market(&env, &client);
            client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &0)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn deposit_rejects_unknown_market() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            client.deposit(&42, &Address::generate(&env), &Outcome::No, &100)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn deposit_after_close_panics() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            let alice = Address::generate(&env);
            let usdc = env
                .register_stellar_asset_contract_v2(Address::generate(&env))
                .address();
            token::StellarAssetClient::new(&env, &usdc).mint(&alice, &1_000_000);

            env.ledger().set_timestamp(100);
            let market_id = client.create_market(
                &alice,
                &Address::generate(&env),
                &usdc,
                &String::from_str(&env, "Q"),
                &50,
                &1_700_086_400,
            );
            env.ledger().set_timestamp(51);

            client.deposit(&market_id, &alice, &Outcome::Yes, &100)
        }));
        assert!(result.is_err());
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
}
