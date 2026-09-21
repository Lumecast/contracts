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
    contract, contractimpl, panic_with_error, token, Address, Env, MuxedAddress, Vec,
};

use lumecast_pricing::{
    lmsr_cost_to_buy, lmsr_seed, lmsr_shares_affordable, pari_mutuel_payout, PricingModel,
    LMSR_B_MAX,
};
use lumecast_resolution::{in_dispute_window, slash_split, GovernanceConfig, Resolution};

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
    BuySharesEvent, CancelMarketEvent, ClaimEvent, CreateMarketEvent, DepositEvent, DisputeEvent,
    FinalizeEvent, ProposeEvent, VoteEvent,
};
pub use storage::DataKey;
pub use types::{CreateMarketParameter, Market, MarketState, Outcome, Position, OUTCOME_COUNT};

/// Read a market by id, panicking with [`Error::MarketNotFound`] if absent.
fn must_get_market(env: &Env, id: u64) -> Market {
    require_market(env, id).unwrap_or_else(|err| panic_with_error!(env, err))
}

/// Decode an inverse outcome, panicking with [`Error::UnknownOutcome`].
fn outcome_from(env: &Env, index: u32) -> Outcome {
    Outcome::from_index(index).unwrap_or_else(|err| panic_with_error!(env, err))
}

/// Send the protocol's share of a forfeited bond to the configured fee
/// receiver, falling back to the contract admin.
fn distribute_protocol_fee(
    env: &Env,
    token: &token::TokenClient,
    governance: &GovernanceConfig,
    protocol_share: i128,
) {
    if protocol_share <= 0 {
        return;
    }
    let receiver = governance
        .protocol_fee_receiver
        .clone()
        .or_else(|| read_admin(env))
        .expect("admin is always set at construction");
    let to = MuxedAddress::from(&receiver);
    token.transfer(&env.current_contract_address(), &to, &protocol_share);
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

    /// Create a new binary (YES/NO) market.
    ///
    /// No funds move for a pari-mutuel market (`params.b == 0` — USDC enters
    /// via `deposit`). For an LMSR market (`params.b > 0`) the creator seeds
    /// `b * ln(2)` of the settlement asset into the pool so the escrow starts
    /// at `C(0)` — that initial capital is what makes the
    /// `escrow >= max(shares)` solvency invariant hold, and it is recovered
    /// from the pool on `finalize`.
    pub fn create_market(env: Env, creator: Address, params: CreateMarketParameter) -> u64 {
        creator.require_auth();

        if params.close_ts >= params.resolution_ts {
            panic_with_error!(&env, Error::InvalidTiming);
        }
        if !(0..=LMSR_B_MAX).contains(&params.b) {
            panic_with_error!(&env, Error::InvalidLiquidity);
        }

        let id = next_market_id(&env);
        let zeros = Vec::from_array(&env, [0i128; OUTCOME_COUNT as usize]);
        let mut market = Market {
            id,
            question: params.question,
            creator: creator.clone(),
            resolver: params.resolver,
            asset: params.asset,
            close_ts: params.close_ts,
            resolution_ts: params.resolution_ts,
            state: MarketState::Open,
            pool: zeros.clone(),
            shares: zeros,
            b: params.b,
        };

        // An LMSR market is seeded with C(0) = b * ln(2) up front: the escrow
        // tracks C(q), and this lift-off capital covers the first marginal
        // purchases before any trader has paid anything in.
        if market.pricing_model() == PricingModel::Lmsr {
            let seed = lmsr_seed(market.b);
            let token = token::TokenClient::new(&env, &market.asset);
            token.transfer_from(
                &env.current_contract_address(),
                &creator,
                &env.current_contract_address(),
                &seed,
            );
            market.pool = Vec::from_array(&env, [seed, 0]);
        }

        write_market(&env, &market);

        CreateMarketEvent {
            market_id: market.id,
            creator: market.creator.clone(),
            resolver: market.resolver.clone(),
            question: market.question.clone(),
            b: market.b,
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
    /// v1 pari-mutuel markets price shares at a fixed 1:1, so one deposited
    /// USDC mints one share. LMSR markets trade through `buy_shares` instead —
    /// this entry point is rejected there. The depositor must have approved
    /// this contract to spend `amount` of the market's settlement asset; the
    /// contract then pulls the tokens into escrow and credits the depositor's
    /// position.
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
        if market.pricing_model() != PricingModel::PariMutuel {
            panic_with_error!(&env, Error::PricingModelMismatch);
        }
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

    /// Buy `outcome` shares in an LMSR market with up to `amount_in` of the
    /// settlement asset.
    ///
    /// The buyer states a *maximum* spend and receives as many shares as have
    /// marginal cost within that budget (`lmsr_shares_affordable`); the
    /// contract pulls exactly `lmsr_cost_to_buy` of the batch, never more, and
    /// everything else stays in the buyer's wallet. The escrow is credited by
    /// exactly the marginal cost of the batch, preserving the
    /// `cash == C(q)` solvency invariant. Only LMSR markets trade here;
    /// pari-mutuel markets keep the fixed 1:1 `deposit`.
    pub fn buy_shares(
        env: Env,
        market_id: u64,
        from: Address,
        outcome: Outcome,
        amount_in: i128,
    ) -> Position {
        from.require_auth();

        if amount_in <= 0 {
            panic_with_error!(&env, Error::ZeroAmount);
        }

        let mut market = must_get_market(&env, market_id);
        if market.pricing_model() != PricingModel::Lmsr {
            panic_with_error!(&env, Error::PricingModelMismatch);
        }
        if market.state != MarketState::Open {
            panic_with_error!(&env, Error::MarketNotOpen);
        }
        if env.ledger().timestamp() > market.close_ts {
            panic_with_error!(&env, Error::AfterClose);
        }

        let q0 = market.shares.get(0).unwrap_or(0);
        let q1 = market.shares.get(1).unwrap_or(0);
        let shares_out = lmsr_shares_affordable(market.b, q0, q1, amount_in);

        let idx = outcome.index();
        let pay = lmsr_cost_to_buy(market.b, q0, q1, shares_out);

        // Pull exactly the marginal cost of the batch into escrow.
        let token = token::TokenClient::new(&env, &market.asset);
        token.transfer_from(
            &env.current_contract_address(),
            &from,
            &env.current_contract_address(),
            &pay,
        );

        // Cash lives in pool[0] for LMSR markets; shares carry the outcome
        // counts q0/q1, so `market.total_pool()` recomputes C(q0, q1).
        market.pool.set(0, market.pool.get(0).unwrap_or(0) + pay);
        market
            .shares
            .set(idx, market.shares.get(idx).unwrap_or(0) + shares_out);
        write_market(&env, &market);

        let mut position = read_position(&env, market_id, &from, outcome);
        position.shares += shares_out;
        write_position(&env, &position);
        add_holder(&env, market_id, &from);

        BuySharesEvent {
            market_id,
            outcome_index: idx,
            from: from.clone(),
            amount_in: pay,
            shares_out,
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

    /// Settle a proposed or disputed market and distribute bonds.
    ///
    /// - **Proposed (uncontested):** once `DISPUTE_WINDOW_LEDGERS` have elapsed
    ///   with no dispute, the proposal becomes the resolution and the proposer's
    ///   bond is returned in full.
    /// - **Disputed:** once committee quorum is reached, the outcome with the
    ///   most votes wins; ties favor the proposal. The losing side's bonds are
    ///   forfeited and split between the winning side (70%) and the protocol
    ///   fee receiver (30%, or the contract admin if unset).
    pub fn finalize(env: Env, market_id: u64) {
        let mut market = must_get_market(&env, market_id);
        let governance = read_governance(&env);
        let resolution =
            read_resolution(&env, market_id).expect("proposed or disputed market has resolution");
        let token = token::TokenClient::new(&env, &market.asset);

        let (winning, uncontested, returned_bond) = match market.state {
            MarketState::Proposed(proposed) => {
                if in_dispute_window(&env, resolution.proposed_ledger) {
                    panic_with_error!(&env, Error::DisputeWindowOpen);
                }
                let to = MuxedAddress::from(&resolution.proposer);
                token.transfer(&env.current_contract_address(), &to, &resolution.bond);
                (proposed, true, Some(resolution.bond))
            }
            MarketState::Disputed(_) => {
                if resolution.votes.len() < governance.quorum {
                    panic_with_error!(&env, Error::QuorumNotReached);
                }

                let mut for_proposed = 0i128;
                let mut for_other = 0i128;
                for i in 0..resolution.votes.len() {
                    if resolution.votes.get(i).unwrap().1 == resolution.proposed {
                        for_proposed += 1;
                    } else {
                        for_other += 1;
                    }
                }
                // Ties favor the proposal (status quo).
                let proposer_wins = for_proposed >= for_other;

                if proposer_wins {
                    // Proposer keeps their own bond, plus a share of every
                    // forfeited challenger counter-bond.
                    let to = MuxedAddress::from(&resolution.proposer);
                    token.transfer(&env.current_contract_address(), &to, &resolution.bond);
                    for i in 0..resolution.challenges.len() {
                        let (_, counter_bond) = resolution.challenges.get(i).unwrap();
                        let (winner_share, protocol_share) = slash_split(counter_bond);
                        let to = MuxedAddress::from(&resolution.proposer);
                        token.transfer(&env.current_contract_address(), &to, &winner_share);
                        distribute_protocol_fee(&env, &token, &governance, protocol_share);
                    }
                    // Proposer's own bond is returned in full.
                    (
                        outcome_from(&env, resolution.proposed),
                        false,
                        Some(resolution.bond),
                    )
                } else {
                    // Challengers recover their counter-bonds and split the
                    // proposer's forfeited bond equally.
                    let n = resolution.challenges.len();
                    for i in 0..n {
                        let (challenger, counter_bond) = resolution.challenges.get(i).unwrap();
                        let to = MuxedAddress::from(challenger);
                        token.transfer(&env.current_contract_address(), &to, &counter_bond);
                    }
                    let (winner_share, protocol_share) = slash_split(resolution.bond);
                    let per_challenger = if n > 0 { winner_share / (n as i128) } else { 0 };
                    for i in 0..n {
                        let (challenger, _) = resolution.challenges.get(i).unwrap();
                        let to = MuxedAddress::from(challenger);
                        token.transfer(&env.current_contract_address(), &to, &per_challenger);
                    }
                    distribute_protocol_fee(&env, &token, &governance, protocol_share);
                    (outcome_from(&env, resolution.proposed), false, Some(0))
                }
            }
            _ => panic_with_error!(&env, Error::NotProposed),
        };

        market.state = MarketState::Resolved(winning);
        write_market(&env, &market);
        remove_resolution(&env, market_id);

        FinalizeEvent {
            market_id,
            outcome_index: winning.index(),
            uncontested,
            returned_bond,
        }
        .publish(&env);
    }

    /// Claim the payout for a resolved market. The claimant must hold shares
    /// in the winning outcome; the payout is their pro-rata share of the total
    /// pool (pari-mutuel). Claiming zeroes the position so double claims pay
    /// nothing.
    pub fn claim(env: Env, market_id: u64, claimant: Address, outcome: Outcome) -> i128 {
        claimant.require_auth();

        let market = must_get_market(&env, market_id);
        let winning = match market.state {
            MarketState::Resolved(winning) => winning,
            _ => panic_with_error!(&env, Error::NotResolved),
        };

        let mut position = read_position(&env, market_id, &claimant, outcome);
        if position.shares <= 0 {
            return 0;
        }

        let total_pool = market.total_pool();
        let winning_shares = market
            .shares
            .get(winning.index())
            .expect("binary market shares");

        let payout = if outcome == winning {
            pari_mutuel_payout(position.shares, total_pool, winning_shares)
        } else {
            0
        };

        if payout > 0 {
            let token = token::TokenClient::new(&env, &market.asset);
            let to = MuxedAddress::from(&claimant);
            token.transfer(&env.current_contract_address(), &to, &payout);
        }

        position.shares = 0;
        write_position(&env, &position);

        if payout > 0 {
            ClaimEvent {
                market_id,
                outcome_index: outcome.index(),
                claimant,
                amount: payout,
            }
            .publish(&env);
        }

        payout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
    use soroban_sdk::{token, Env, Event as _, String, Symbol, TryFromVal};

    fn deploy(env: &Env) -> MarketContractClient<'_> {
        deploy_with_admin(env).0
    }

    /// Deploy the contract and return both the client and its construction-time
    /// admin, so governance fixtures can authenticate `set_governance` calls.
    fn deploy_with_admin(env: &Env) -> (MarketContractClient<'_>, Address) {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let contract_id = env.register(MarketContract, (&admin,));
        (MarketContractClient::new(env, &contract_id), admin)
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
            &CreateMarketParameter {
                resolver: resolver.clone(),
                asset: usdc.clone(),
                question: String::from_str(env, "Will it rain tomorrow?"),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b: 0,
            },
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
            &CreateMarketParameter {
                resolver: resolver.clone(),
                asset: asset.clone(),
                question: String::from_str(&env, "Will it rain tomorrow?"),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b: 0,
            },
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
                &CreateMarketParameter {
                    resolver: Address::generate(&env),
                    asset: Address::generate(&env),
                    question: String::from_str(&env, "Q"),
                    close_ts: 1_700_000_000,
                    resolution_ts: 1_700_000_000,
                    b: 0,
                },
            )
        }));
        assert!(result.is_err());
    }

    #[test]
    fn create_market_rejects_invalid_liquidity() {
        // Negative or out-of-range `b` is refused before any funds move.
        for bad_b in [-1i128, lumecast_pricing::LMSR_B_MAX + 1] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let env = Env::default();
                let client = deploy(&env);
                client.create_market(
                    &Address::generate(&env),
                    &CreateMarketParameter {
                        resolver: Address::generate(&env),
                        asset: Address::generate(&env),
                        question: String::from_str(&env, "Q"),
                        close_ts: 1_700_000_000,
                        resolution_ts: 1_700_086_400,
                        b: bad_b,
                    },
                )
            }));
            assert!(result.is_err(), "accepted b = {bad_b}");
        }
    }

    #[test]
    fn create_market_seeds_lmsr_pool() {
        let env = Env::default();
        let client = deploy(&env);
        let admin = Address::generate(&env);
        let usdc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let usdc_admin = token::StellarAssetClient::new(&env, &usdc);

        let creator = Address::generate(&env);
        usdc_admin.mint(&creator, &1_000_000);
        token::TokenClient::new(&env, &usdc).approve(
            &creator,
            &client.address,
            &i128::MAX,
            &100_000,
        );

        let b = 50_000i128;
        let seed = lmsr_seed(b);
        let before = token::TokenClient::new(&env, &usdc).balance(&creator);

        let market_id = client.create_market(
            &creator,
            &CreateMarketParameter {
                resolver: Address::generate(&env),
                asset: usdc.clone(),
                question: String::from_str(&env, "Q"),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b,
            },
        );

        // The creator funded the C(0) = b * ln(2) seed; the escrow holds it.
        assert_eq!(
            token::TokenClient::new(&env, &usdc).balance(&creator),
            before - seed
        );
        assert_eq!(
            token::TokenClient::new(&env, &usdc).balance(&client.address),
            seed
        );

        let market = client.market(&market_id).unwrap();
        assert_eq!(market.pricing_model(), PricingModel::Lmsr);
        assert_eq!(market.b, b);
        assert_eq!(market.total_pool(), seed);
        assert_eq!(market.shares.get(0).unwrap(), 0);
        assert_eq!(market.shares.get(1).unwrap(), 0);
    }

    /// Deploy an LMSR market (creator seeded) and give `buyer` approved USDC.
    /// Mirrors `create_market_seeds_lmsr_pool`'s setup so the pool starts at
    /// exactly `C(0) = seed`.
    fn lmsr_fixture(
        env: &Env,
        client: &MarketContractClient<'_>,
        b: i128,
    ) -> (u64, Address, Address, i128) {
        let usdc = env
            .register_stellar_asset_contract_v2(Address::generate(env))
            .address();
        let usdc_admin = token::StellarAssetClient::new(env, &usdc);
        let token = token::TokenClient::new(env, &usdc);

        let creator = Address::generate(env);
        usdc_admin.mint(&creator, &1_000_000);
        token.approve(&creator, &client.address, &i128::MAX, &100_000);

        let buyer = Address::generate(env);
        usdc_admin.mint(&buyer, &1_000_000);
        token.approve(&buyer, &client.address, &i128::MAX, &100_000);

        let market_id = client.create_market(
            &creator,
            &CreateMarketParameter {
                resolver: Address::generate(env),
                asset: usdc.clone(),
                question: String::from_str(env, "Q"),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b,
            },
        );
        (market_id, usdc, buyer, lmsr_seed(b))
    }

    #[test]
    fn buy_shares_mints_at_lmsr_marginal_cost() {
        let env = Env::default();
        let client = deploy(&env);
        let b = 10_000i128;
        let (market_id, usdc, alice, seed) = lmsr_fixture(&env, &client, b);
        let token = token::TokenClient::new(&env, &usdc);

        let before = token.balance(&alice);
        let budget = 1_000i128;
        let position = client.buy_shares(&market_id, &alice, &Outcome::Yes, &budget);
        assert!(position.shares > 0);

        let market = client.market(&market_id).unwrap();
        let pay = market.pool.get(0).unwrap() - seed;
        let x = market.shares.get(0).unwrap();

        // Spend is within budget and equals the marginal cost of the batch.
        assert!(pay > 0 && pay <= budget);
        assert_eq!(pay, lmsr_cost_to_buy(b, 0, 0, x));
        assert_eq!(token.balance(&alice), before - pay);
        assert_eq!(position.shares, x);

        // Escrow stays at exactly C(q): cash flows to the creator's seed, not
        // away from it, and never dips below the winning-outcome coverage.
        assert_eq!(market.total_pool(), seed + pay);
        assert_eq!(market.total_pool(), lumecast_pricing::lmsr_cost(b, x, 0));
        assert_eq!(token.balance(&client.address), market.total_pool());
    }

    #[test]
    fn buy_shares_emits_event() {
        let env = Env::default();
        let client = deploy(&env);
        let (market_id, _usdc, alice, seed) = lmsr_fixture(&env, &client, 10_000);

        client.buy_shares(&market_id, &alice, &Outcome::No, &500);

        // Read the observed events before any further invocation, which would
        // replace the captured frame.
        let topics = contract_event_topics(&env);
        assert!(
            topics.contains(&Symbol::new(&env, "buy_shares_event")),
            "expected buy_shares_event in {:?}",
            topics
        );

        let market = client.market(&market_id).unwrap();
        let pay = market.pool.get(0).unwrap() - seed;
        assert!(pay > 0 && pay <= 500);
    }

    #[test]
    fn buy_shares_tiny_budget_still_clears_one_share() {
        // A single whole-token budget always clears at least one share (the
        // marginal price is <= 1.0). At an empty pool the first share rounds to
        // zero cost because the surplus term rounds down to the seed's integer;
        // solvency is preserved since `cost` is unchanged.
        let env = Env::default();
        let client = deploy(&env);
        let (market_id, _usdc, alice, seed) = lmsr_fixture(&env, &client, 10_000);

        let position = client.buy_shares(&market_id, &alice, &Outcome::Yes, &1);
        assert_eq!(position.shares, 1);

        let market = client.market(&market_id).unwrap();
        assert_eq!(market.pool.get(0).unwrap() - seed, 0);
        assert_eq!(
            market.total_pool(),
            lumecast_pricing::lmsr_cost(10_000, 1, 0)
        );
    }

    #[test]
    fn buy_shares_rejects_pari_mutuel_market() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            let funded = funded_market(&env, &client);
            client.buy_shares(&funded.market_id, &funded.alice, &Outcome::Yes, &100)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn buy_shares_after_close_is_rejected() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            let (market_id, _usdc, alice, _seed) = lmsr_fixture(&env, &client, 10_000);
            env.ledger().set_timestamp(1_700_000_001);
            client.buy_shares(&market_id, &alice, &Outcome::Yes, &100)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn deposit_rejects_lmsr_market() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let client = deploy(&env);
            let (market_id, _usdc, alice, _seed) = lmsr_fixture(&env, &client, 10_000);
            client.deposit(&market_id, &alice, &Outcome::Yes, &100)
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
            &CreateMarketParameter {
                resolver: resolver.clone(),
                asset: Address::generate(&env),
                question: question.clone(),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b: 0,
            },
        );
        assert_eq!(id, 1);
        assert_eq!(
            env.events().all(),
            [CreateMarketEvent {
                market_id: 1,
                creator,
                resolver,
                question,
                b: 0,
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
                &CreateMarketParameter {
                    resolver: Address::generate(&env),
                    asset: usdc,
                    question: String::from_str(&env, "Q"),
                    close_ts: 50,
                    resolution_ts: 1_700_086_400,
                    b: 0,
                },
            );
            env.ledger().set_timestamp(51);

            client.deposit(&market_id, &alice, &Outcome::Yes, &100)
        }));
        assert!(result.is_err());
    }

    fn new_market_with(env: &Env, client: &MarketContractClient<'_>, question: &str) -> u64 {
        client.create_market(
            &Address::generate(env),
            &CreateMarketParameter {
                resolver: Address::generate(env),
                asset: Address::generate(env),
                question: String::from_str(env, question),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b: 0,
            },
        )
    }

    struct ResolutionFixture {
        market_id: u64,
        usdc: Address,
        resolver: Address,
        admin: Address,
        committee: Vec<Address>,
        yes_holder: Address,
        no_holder: Address,
    }

    /// A closed market with USDC escrow, a bonded proposal, committee
    /// governance, and (optionally) a challenge. Timestamps are advanced past
    /// `resolution_ts` so proposals are permitted.
    fn resolution_fixture(
        env: &Env,
        client: &MarketContractClient<'_>,
        admin: &Address,
        disputed: bool,
    ) -> ResolutionFixture {
        let usdc = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let usdc_admin = token::StellarAssetClient::new(env, &usdc);

        let alice = Address::generate(env);
        let resolver = Address::generate(env);
        let challenge = Address::generate(env);
        for who in [&alice, &resolver, &challenge] {
            usdc_admin.mint(who, &1_000_000);
            token::TokenClient::new(env, &usdc).approve(who, &client.address, &i128::MAX, &100_000);
        }

        let market_id = client.create_market(
            &alice,
            &CreateMarketParameter {
                resolver: resolver.clone(),
                asset: usdc.clone(),
                question: String::from_str(env, "Will it rain tomorrow?"),
                close_ts: 1_700_000_000,
                resolution_ts: 1_700_086_400,
                b: 0,
            },
        );
        client.deposit(&market_id, &alice, &Outcome::Yes, &4_000);
        client.deposit(&market_id, &challenge, &Outcome::No, &6_000);

        env.ledger().set_timestamp(1_700_086_401);
        client.propose_outcome(&market_id, &resolver, &Outcome::Yes, &10_000);

        let voters = [
            Address::generate(env),
            Address::generate(env),
            Address::generate(env),
        ];
        let committee = Vec::from_array(env, voters);
        client.set_governance(admin, &committee, &3, &None);

        if disputed {
            client.dispute(&market_id, &challenge, &10_000);
        }

        ResolutionFixture {
            market_id,
            usdc,
            resolver,
            admin: admin.clone(),
            committee,
            yes_holder: alice,
            no_holder: challenge,
        }
    }

    fn advance_past_dispute_window(env: &Env) {
        env.ledger().set_sequence_number(
            env.ledger().sequence() + lumecast_resolution::DISPUTE_WINDOW_LEDGERS + 1,
        );
    }

    #[test]
    fn propose_escrows_bond_and_moves_state() {
        let env = Env::default();
        let (client, admin) = deploy_with_admin(&env);
        let f = resolution_fixture(&env, &client, &admin, false);

        let market = client.market(&f.market_id).unwrap();
        assert_eq!(market.state, MarketState::Proposed(Outcome::Yes));
        assert_eq!(
            token::TokenClient::new(&env, &f.usdc).balance(&client.address),
            10_000 + 4_000 + 6_000
        );
        let res = client.resolution(&f.market_id).unwrap();
        assert_eq!(res.proposer, f.resolver);
        assert_eq!(res.bond, 10_000);
    }

    #[test]
    fn propose_requires_resolver() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, false);
            client.propose_outcome(
                &f.market_id,
                &Address::generate(&env),
                &Outcome::Yes,
                &1_000,
            )
        }));
        assert!(result.is_err());
    }

    #[test]
    fn propose_before_resolution_ts_rejected() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, false);
            env.ledger().set_timestamp(1_699_000_000);
            client.propose_outcome(&f.market_id, &f.resolver, &Outcome::Yes, &1_000)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn dispute_requires_sufficient_counter_bond() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, false);
            client.dispute(&f.market_id, &Address::generate(&env), &1_000)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn dispute_expires_after_window() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, false);
            advance_past_dispute_window(&env);
            client.dispute(&f.market_id, &Address::generate(&env), &10_000)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn uncontested_finalize_returns_bond_and_resolves() {
        let env = Env::default();
        let (client, admin) = deploy_with_admin(&env);
        let f = resolution_fixture(&env, &client, &admin, false);
        advance_past_dispute_window(&env);

        let resolver_before = token::TokenClient::new(&env, &f.usdc).balance(&f.resolver);
        client.finalize(&f.market_id);

        let market = client.market(&f.market_id).unwrap();
        assert_eq!(market.state, MarketState::Resolved(Outcome::Yes));
        assert_eq!(
            token::TokenClient::new(&env, &f.usdc).balance(&f.resolver),
            resolver_before + 10_000
        );
        // Payouts payable: Yes holders split the 10,000 pool.
        assert_eq!(
            client.claim(&f.market_id, &Address::generate(&env), &Outcome::No),
            0
        );
    }

    #[test]
    fn finalize_early_when_proposed_is_rejected() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, false);
            client.finalize(&f.market_id)
        }));
        assert!(result.is_err());
    }

    fn vote_both_ways(client: &MarketContractClient<'_>, f: &ResolutionFixture) {
        for (i, voter) in f.committee.iter().enumerate() {
            let outcome = if i == 2 { Outcome::No } else { Outcome::Yes };
            client.vote(&f.market_id, &voter, &outcome);
        }
    }

    #[test]
    fn disputed_finalize_slashes_losing_bond() {
        let env = Env::default();
        let (client, admin) = deploy_with_admin(&env);
        let f = resolution_fixture(&env, &client, &admin, true);

        let admin_before = token::TokenClient::new(&env, &f.usdc).balance(&f.admin);
        let resolver_before = token::TokenClient::new(&env, &f.usdc).balance(&f.resolver);

        vote_both_ways(&client, &f);
        client.finalize(&f.market_id);

        // Yes wins 2-1: proposer takes full bond + 90% of the counter-bond.
        assert_eq!(
            token::TokenClient::new(&env, &f.usdc).balance(&f.resolver),
            resolver_before + 10_000 + (10_000 * 90 / 100)
        );
        assert_eq!(
            token::TokenClient::new(&env, &f.usdc).balance(&f.admin),
            admin_before + (10_000 * 10 / 100)
        );
        assert_eq!(
            client.market(&f.market_id).unwrap().state,
            MarketState::Resolved(Outcome::Yes)
        );
    }

    #[test]
    fn disputed_finalize_requires_quorum() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, true);
            client.vote(&f.market_id, &f.committee.get(0).unwrap(), &Outcome::Yes);
            client.finalize(&f.market_id)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn vote_rejects_non_committee_member() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, true);
            client.vote(&f.market_id, &Address::generate(&env), &Outcome::Yes)
        }));
        assert!(result.is_err());
    }

    #[test]
    fn claim_pays_winning_outcome_pro_rata() {
        let env = Env::default();
        let (client, admin) = deploy_with_admin(&env);
        let f = resolution_fixture(&env, &client, &admin, true);

        vote_both_ways(&client, &f);
        client.finalize(&f.market_id);

        // Losing (No) holder receives nothing.
        let no_claim = client.claim(&f.market_id, &f.no_holder, &Outcome::No);
        assert_eq!(no_claim, 0);

        // Winning (Yes) holder receives the entire pari-mutuel pool, then a
        // double claim pays nothing.
        let first = client.claim(&f.market_id, &f.yes_holder, &Outcome::Yes);
        assert_eq!(first, 10_000);
        let second = client.claim(&f.market_id, &f.yes_holder, &Outcome::Yes);
        assert_eq!(second, 0);
        // The pool is fully drained from contract escrow.
        assert_eq!(
            token::TokenClient::new(&env, &f.usdc).balance(&client.address),
            0
        );
    }

    #[test]
    fn claim_before_resolution_is_rejected() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let env = Env::default();
            let (client, admin) = deploy_with_admin(&env);
            let f = resolution_fixture(&env, &client, &admin, true);
            client.claim(&f.market_id, &f.yes_holder, &Outcome::Yes)
        }));
        assert!(result.is_err());
    }
}
