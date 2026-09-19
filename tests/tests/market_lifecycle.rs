//! Cross-module lifecycle test: create -> deposit -> cancel, end to end.
//!
//! Spins up the lumecast-market contract next to a sandboxed USDC-like asset
//! and walks three participants through the full money-in / money-back path.

use lumecast_market::{MarketContract, MarketContractClient, MarketState, Outcome};
use soroban_sdk::{
    testutils::Address as _,
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
}

fn setup(env: &Env) -> Players {
    env.mock_all_auths();

    let admin = Address::generate(env);
    let usdc = env.register_stellar_asset_contract_v2(admin).address();
    let usdc_admin = StellarAssetClient::new(env, &usdc);

    let alice = Address::generate(env);
    let bob = Address::generate(env);
    let carol = Address::generate(env);
    for who in [&alice, &bob, &carol] {
        usdc_admin.mint(who, &1_000_000);
    }

    let market_id = env.register(MarketContract, ());
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
