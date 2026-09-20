//! Cross-module lifecycle test: create -> deposit -> cancel, end to end.
//!
//! Spins up the lumecast-market contract next to a sandboxed USDC-like asset
//! and walks three participants through the full money-in / money-back path.

use lumecast_market::{MarketContract, MarketContractClient, MarketState, Outcome};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    Address, Env, String,
};

struct Players {
    client: MarketContractClient<'static>,
    usdc: Address,
    market_id: u64,
    alice: Address,
    bob: Address,
    carol: Address,
    resolver: Address,
    admin: Address,
}

fn setup(env: &Env) -> Players {
    env.mock_all_auths();

    let admin = Address::generate(env);
    let usdc = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let usdc_admin = StellarAssetClient::new(env, &usdc);

    let alice = Address::generate(env);
    let bob = Address::generate(env);
    let carol = Address::generate(env);
    for who in [&alice, &bob, &carol] {
        usdc_admin.mint(who, &1_000_000);
    }

    let market_id = env.register(MarketContract, (&admin,));
    let client = MarketContractClient::new(env, &market_id);
    let token = TokenClient::new(env, &usdc);
    for who in [&alice, &bob, &carol] {
        token.approve(who, &client.address, &i128::MAX, &100_000);
    }

    let resolver = Address::generate(env);
    let market_id = client.create_market(
        &alice,
        &resolver,
        &usdc,
        &String::from_str(env, "Will Lumecast launch its first market on Mainnet?"),
        &1_700_000_000,
        &1_700_086_400,
    );

    Players {
        client,
        usdc,
        market_id,
        alice,
        bob,
        carol,
        resolver,
        admin,
    }
}

#[test]
fn full_lifecycle_round_trips_funds() {
    let env = Env::default();
    let p = setup(&env);
    let token = TokenClient::new(&env, &p.usdc);

    let alice_before = token.balance(&p.alice);
    let bob_before = token.balance(&p.bob);
    let carol_before = token.balance(&p.carol);

    p.client
        .deposit(&p.market_id, &p.alice, &Outcome::Yes, &1_000);
    p.client
        .deposit(&p.market_id, &p.bob, &Outcome::Yes, &2_000);
    p.client
        .deposit(&p.market_id, &p.carol, &Outcome::No, &3_000);

    // Escrowed funds sit on the contract while the market is open.
    assert_eq!(token.balance(&p.client.address), 6_000);
    assert_eq!(token.balance(&p.alice), alice_before - 1_000);

    let market = p.client.market(&p.market_id).unwrap();
    assert_eq!(market.state, MarketState::Open);
    assert_eq!(market.total_pool(), 6_000);
    assert_eq!(market.shares, market.pool);

    // Cancel the market; everyone gets exactly what they put in.
    p.client.cancel_market(&p.market_id, &p.alice);

    let after = p.client.market(&p.market_id).unwrap();
    assert_eq!(after.state, MarketState::Cancelled);
    assert_eq!(after.total_pool(), 0);
    assert_eq!(token.balance(&p.client.address), 0);
    assert_eq!(token.balance(&p.alice), alice_before);
    assert_eq!(token.balance(&p.bob), bob_before);
    assert_eq!(token.balance(&p.carol), carol_before);

    for who in [&p.alice, &p.bob, &p.carol] {
        assert_eq!(
            p.client.position(&p.market_id, who, &Outcome::Yes).shares,
            0
        );
        assert_eq!(p.client.position(&p.market_id, who, &Outcome::No).shares, 0);
    }
}

#[test]
fn resolve_dispute_vote_and_claim_pay_out_winners() {
    let env = Env::default();
    let p = setup(&env);
    let token = TokenClient::new(&env, &p.usdc);

    p.client
        .deposit(&p.market_id, &p.alice, &Outcome::Yes, &4_000);
    p.client.deposit(&p.market_id, &p.bob, &Outcome::No, &6_000);

    // Advance the ledger past resolution_ts so a proposal is permitted.
    env.ledger().set_timestamp(1_700_086_401);

    // The resolver proposes "Yes" and posts a bond of 10,000.
    let proposal_bond = 10_000i128;
    usdc_admin_mint_for(&env, &p.usdc, &p.resolver, proposal_bond, &p.client.address);
    p.client
        .propose_outcome(&p.market_id, &p.resolver, &Outcome::Yes, &proposal_bond);

    // A challenger disputes with an equal counter-bond.
    p.client.dispute(&p.market_id, &p.bob, &proposal_bond);

    // Configure governance and vote: 3-member committee, quorum 3.
    let voters = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let committee = soroban_sdk::Vec::from_array(&env, voters.clone());
    p.client.set_governance(&p.admin, &committee, &3u32, &None);
    for (i, voter) in voters.iter().enumerate() {
        p.client.vote(
            &p.market_id,
            voter,
            if i == 2 { &Outcome::No } else { &Outcome::Yes },
        );
    }

    // Finalize: "Yes" wins 2-1. The challenger's bond (10k) is slashed;
    // 70% to the proposer, 30% to the admin (no explicit fee receiver).
    let admin = p.client.admin().unwrap();
    let admin_before = token.balance(&admin);
    let proposer_before = token.balance(&p.resolver);
    p.client.finalize(&p.market_id);

    let market = p.client.market(&p.market_id).unwrap();
    assert_eq!(market.state, MarketState::Resolved(Outcome::Yes));
    assert_eq!(
        token.balance(&admin),
        admin_before + (proposal_bond * 10 / 100)
    );
    assert_eq!(
        token.balance(&p.resolver),
        proposer_before + proposal_bond + (proposal_bond * 90 / 100)
    );

    // Alice (Yes) gets her full pro-rata share of the 10,000 pool:
    // both outcomes hold 10,000 total, Yes holds 4,000 of it.
    let alice_before = token.balance(&p.alice);
    let payout = p.client.claim(&p.market_id, &p.alice, &Outcome::Yes);
    assert_eq!(payout, 10_000); // pool fully distributed to Yes holders
    assert_eq!(token.balance(&p.alice), alice_before + payout);
    assert_eq!(token.balance(&p.client.address), 0);
}

fn usdc_admin_mint_for(env: &Env, usdc: &Address, who: &Address, amount: i128, spender: &Address) {
    StellarAssetClient::new(env, usdc).mint(who, &amount);
    TokenClient::new(env, usdc).approve(who, spender, &i128::MAX, &100_000);
}
