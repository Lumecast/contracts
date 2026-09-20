#![no_std]

//! Lumecast core market contract.
//!
//! M0 (Foundations): repo scaffolding, CI, pinned Soroban SDK, and the core
//! domain model — `Market`, `Position`, `Outcome` — plus a working
//! `create_market` entry point in the local sandbox. M1 delivers `deposit`,
//! `cancel_market`, and `claim`. M2 adds bonded `propose_outcome` /
//! `dispute` / `finalize` with committee voting and bond slashing.

#[cfg(test)]
extern crate std;

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, Env, MuxedAddress, String, Vec,
};

use lumecast_resolution::{in_dispute_window, GovernanceConfig, Resolution};

use crate::storage::{
    add_holder, get_market, next_market_id, read_admin, read_governance, read_holders,
    read_position, read_resolution, remove_resolution, require_market, write_admin,
    write_governance, write_market, write_position, write_resolution,
};

mod error;
mod events;
mod storage;
mod types;

pub use error::Error;
pub use events::{
    CancelMarketEvent, ClaimEvent, CreateMarketEvent, DepositEvent, DisputeEvent, FinalizeEvent,
    ProposeEvent, VoteEvent,
};
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
    /// Initialize the contract with the platform admin. Only the admin may
    /// mutate governance configuration afterwards.
    pub fn __constructor(env: Env, admin: Address) {
        write_admin(&env, &admin);
    }

    /// Read the contract admin address.
    pub fn admin(env: Env) -> Option<Address> {
        read_admin(&env)
    }

    /// Read the platform governance configuration.
    pub fn governance(env: Env) -> GovernanceConfig {
        read_governance(&env)
    }

    /// Configure the dispute committee, its quorum, and the protocol fee
    /// receiver. Admin-only. Quorum must be `1..=committee.len()`.
    pub fn set_governance(
        env: Env,
        caller: Address,
        committee: Vec<Address>,
        quorum: u32,
        protocol_fee_receiver: Option<Address>,
    ) {
        caller.require_auth();
        if Some(caller) != read_admin(&env) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if committee.is_empty() || quorum == 0 || quorum > committee.len() {
            panic_with_error!(&env, Error::InvalidGovernance);
        }
        write_governance(
            &env,
            &GovernanceConfig {
                committee,
                quorum,
                protocol_fee_receiver,
            },
        );
    }

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

    /// Read the per-market resolution state, if any.
    pub fn resolution(env: Env, market_id: u64) -> Option<Resolution> {
        read_resolution(&env, market_id)
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

    /// Cancel an open market and refund every participant in full.
    ///
    /// Only the market creator or resolver may cancel, and only while the
    /// market is still `Open` (before resolution begins). Each escrow holder
    /// gets their deposited tokens back, the escrow ledger is zeroed, and the
    /// market is locked in the `Cancelled` state.
    pub fn cancel_market(env: Env, market_id: u64, caller: Address) {
        caller.require_auth();

        let mut market = must_get_market(&env, market_id);
        if market.state != MarketState::Open {
            panic_with_error!(&env, Error::MarketNotOpen);
        }
        if caller != market.creator && caller != market.resolver {
            panic_with_error!(&env, Error::Unauthorized);
        }

        // Refund every escrow holder in full, then zero their positions.
        let token = token::TokenClient::new(&env, &market.asset);
        let holders = read_holders(&env, market_id);
        for holder in holders.iter() {
            for outcome in Outcome::ALL {
                let mut position = read_position(&env, market_id, &holder, outcome);
                if position.shares > 0 {
                    let to = MuxedAddress::from(&holder);
                    token.transfer(&env.current_contract_address(), &to, &position.shares);
                    position.shares = 0;
                    write_position(&env, &position);
                }
            }
        }

        // Lock the market and zero the escrow ledger.
        market.state = MarketState::Cancelled;
        let zeros = Vec::from_array(&env, [0i128; OUTCOME_COUNT as usize]);
        market.pool = zeros.clone();
        market.shares = zeros;
        write_market(&env, &market);

        CancelMarketEvent {
            market_id,
            cancelled_by: caller,
        }
        .publish(&env);
    }

    /// Propose a final outcome for `market_id`. Only the market resolver may
    /// call, and only while the market is `Open` (trading closed) and after
    /// `resolution_ts`. The resolver posts a `bond` of the settlement asset,
    /// escrowed until the resolution settles.
    pub fn propose_outcome(
        env: Env,
        market_id: u64,
        resolver: Address,
        outcome: Outcome,
        bond: i128,
    ) {
        resolver.require_auth();

        if bond <= 0 {
            panic_with_error!(&env, Error::ZeroAmount);
        }

        let mut market = must_get_market(&env, market_id);
        if resolver != market.resolver {
            panic_with_error!(&env, Error::NotResolver);
        }
        if market.state != MarketState::Open {
            panic_with_error!(&env, Error::NotProposable);
        }
        if env.ledger().timestamp() < market.resolution_ts {
            panic_with_error!(&env, Error::ResolutionNotReady);
        }

        // Escrow the resolver's bond alongside the pool.
        let token = token::TokenClient::new(&env, &market.asset);
        token.transfer_from(
            &env.current_contract_address(),
            &resolver,
            &env.current_contract_address(),
            &bond,
        );

        let proposed_ledger = env.ledger().sequence();
        let resolution = Resolution {
            proposed: outcome.index(),
            proposer: resolver.clone(),
            bond,
            proposed_ledger,
            challenges: Vec::new(&env),
            votes: Vec::new(&env),
        };
        write_resolution(&env, market_id, &resolution);

        market.state = MarketState::Proposed(outcome);
        write_market(&env, &market);

        ProposeEvent {
            market_id,
            outcome_index: outcome.index(),
            proposer: resolver,
            bond,
        }
        .publish(&env);
    }

    /// Dispute a proposed outcome within the dispute window. Anyone may
    /// challenge, but their `counter_bond` must at least match the proposer's
    /// bond. Escalates the market to `Disputed`, where the committee votes.
    pub fn dispute(env: Env, market_id: u64, challenger: Address, counter_bond: i128) {
        challenger.require_auth();

        if counter_bond <= 0 {
            panic_with_error!(&env, Error::ZeroAmount);
        }

        let mut market = must_get_market(&env, market_id);
        let proposed_outcome = match market.state {
            MarketState::Proposed(o) => o,
            _ => panic_with_error!(&env, Error::NotProposed),
        };

        let mut resolution =
            read_resolution(&env, market_id).expect("proposed market has resolution");
        if !in_dispute_window(&env, resolution.proposed_ledger) {
            panic_with_error!(&env, Error::DisputeWindowClosed);
        }
        if counter_bond < resolution.bond {
            panic_with_error!(&env, Error::BondTooLow);
        }

        // Escrow the challenger's counter-bond.
        let token = token::TokenClient::new(&env, &market.asset);
        token.transfer_from(
            &env.current_contract_address(),
            &challenger,
            &env.current_contract_address(),
            &counter_bond,
        );

        resolution
            .challenges
            .push_back((challenger.clone(), counter_bond));
        write_resolution(&env, market_id, &resolution);

        market.state = MarketState::Disputed(proposed_outcome);
        write_market(&env, &market);

        DisputeEvent {
            market_id,
            outcome_index: proposed_outcome.index(),
            challenger,
            counter_bond,
        }
        .publish(&env);
    }

    /// Cast a committee vote on a disputed market. Only configured committee
    /// members may vote; each member's latest vote replaces any earlier one.
    pub fn vote(env: Env, market_id: u64, voter: Address, outcome: Outcome) {
        voter.require_auth();

        let market = must_get_market(&env, market_id);
        if !matches!(market.state, MarketState::Disputed(_)) {
            panic_with_error!(&env, Error::NotDisputed);
        }

        let governance = read_governance(&env);
        if !governance.committee.contains(&voter) {
            panic_with_error!(&env, Error::NotCommitteeMember);
        }

        let mut resolution =
            read_resolution(&env, market_id).expect("disputed market has resolution");
        let mut replaced = false;
        for i in 0..resolution.votes.len() {
            if resolution.votes.get(i).unwrap().0 == voter {
                resolution.votes.set(i, (voter.clone(), outcome.index()));
                replaced = true;
                break;
            }
        }
        if !replaced {
            resolution.votes.push_back((voter.clone(), outcome.index()));
        }
        write_resolution(&env, market_id, &resolution);

        VoteEvent {
            market_id,
            outcome_index: outcome.index(),
            voter,
        }
        .publish(&env);
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
        resolver: Address,
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

        let resolver = Address::generate(env);
        let market_id = client.create_market(
            &alice,
            &resolver,
            &usdc,
            &String::from_str(env, "Will it rain tomorrow?"),
            &1_700_000_000,
            &1_700_086_400,
        );
        Funded {
            market_id,
            usdc,
            alice,
            resolver,
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
    fn cancel_refunds_all_holders_in_full() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        let usdc = token::TokenClient::new(&env, &funded.usdc);
        let alice_before = usdc.balance(&funded.alice);
        let bob = Address::generate(&env);
        token::StellarAssetClient::new(&env, &funded.usdc).mint(&bob, &1_000_000);
        token::TokenClient::new(&env, &funded.usdc).approve(
            &bob,
            &client.address,
            &i128::MAX,
            &100_000,
        );
        let bob_before = usdc.balance(&bob);

        client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &2_000);
        client.deposit(&funded.market_id, &bob, &Outcome::No, &500);

        client.cancel_market(&funded.market_id, &funded.alice);

        let market = client.market(&funded.market_id).unwrap();
        assert_eq!(market.state, MarketState::Cancelled);
        assert_eq!(market.total_pool(), 0);

        assert_eq!(usdc.balance(&funded.alice), alice_before);
        assert_eq!(usdc.balance(&bob), bob_before);
        assert_eq!(usdc.balance(&client.address), 0);

        assert_eq!(
            client
                .position(&funded.market_id, &funded.alice, &Outcome::Yes)
                .shares,
            0
        );
    }

    #[test]
    fn cancel_rejects_non_authority() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);
        let stranger = Address::generate(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.cancel_market(&funded.market_id, &stranger)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn cancel_twice_is_rejected() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        client.cancel_market(&funded.market_id, &funded.alice);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.cancel_market(&funded.market_id, &funded.alice)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn cancel_unknown_market_is_rejected() {
        let env = Env::default();
        let client = deploy(&env);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let caller = Address::generate(&env);
            client.cancel_market(&42, &caller)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn cancel_emits_event() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        client.cancel_market(&funded.market_id, &funded.resolver);

        let topics = contract_event_topics(&env);
        assert!(topics.contains(&Symbol::new(&env, "cancel_market_event")));
    }

    #[test]
    fn deposit_after_cancel_is_rejected() {
        let env = Env::default();
        let client = deploy(&env);
        let funded = funded_market(&env, &client);

        client.cancel_market(&funded.market_id, &funded.alice);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.deposit(&funded.market_id, &funded.alice, &Outcome::Yes, &100)
        }));
        assert!(result.is_err());
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
