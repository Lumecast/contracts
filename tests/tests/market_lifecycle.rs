//! Cross-module lifecycle test: create -> deposit -> cancel, end to end.
//!
//! Spins up the lumecast-market contract next to a sandboxed USDC-like asset
//! and walks three participants through the full money-in / money-back path.

use lumecast_market::{
    CreateMarketParameter, MarketContract, MarketContractClient, MarketState, Outcome,
};
use lumecast_pricing::lmsr_marginal_price;
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
        &CreateMarketParameter {
            resolver: resolver.clone(),
            asset: usdc.clone(),
            question: String::from_str(env, "Will Lumecast launch its first market on Mainnet?"),
            close_ts: 1_700_000_000,
            resolution_ts: 1_700_086_400,
            b: 0,
        },
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

fn setup_lmsr(env: &Env, b: i128) -> Players {
    let mut p = setup(env);
    p.market_id = p.client.create_market(
        &p.alice,
        &CreateMarketParameter {
            resolver: p.resolver.clone(),
            asset: p.usdc.clone(),
            question: String::from_str(env, "Will Lumecast launch its first market on Mainnet?"),
            close_ts: 1_700_000_000,
            resolution_ts: 1_700_086_400,
            b,
        },
    );
    p
}

/// End-to-end LMSR lifecycle across the whole stack: seed liquidity buys a
/// curve, marginal pricing is consistent, resolution sweeps surplus to the
/// creator, and winning claims are strictly one-for-one.
#[test]
fn lmsr_lifecycle_seeds_buys_resolves_and_claims_one_to_one() {
    let env = Env::default();
    let p = setup_lmsr(&env, 10_000);
    let token = TokenClient::new(&env, &p.usdc);

    // Both sides trade against the seeded curve; the escrow exactly matches
    // the LMSR cost function and prices complement to 1.0 (within one unit of
    // fixed-point floor).
    p.client
        .buy_shares(&p.market_id, &p.bob, &Outcome::Yes, &100_000);
    let yes_after_buy = token.balance(&p.bob);
    assert!(yes_after_buy < 1_000_000); // bob actually paid for shares
    p.client
        .buy_shares(&p.market_id, &p.carol, &Outcome::No, &50_000);
    let market = p.client.market(&p.market_id).unwrap();
    let q_yes = market.shares.get(0).unwrap();
    let q_no = market.shares.get(1).unwrap();
    assert_eq!(
        market.total_pool(),
        lumecast_pricing::lmsr_cost(10_000, q_yes, q_no)
    );
    let p_yes = p.client.price(&p.market_id, &Outcome::Yes);
    let p_no = p.client.price(&p.market_id, &Outcome::No);
    assert_eq!(p_yes, lmsr_marginal_price(10_000, q_yes, q_no));
    assert_eq!(p_no, lmsr_marginal_price(10_000, q_no, q_yes));
    assert!(p_yes + p_no >= 9_999 && p_yes + p_no <= 10_000);
    assert!(p_yes > p_no);

    // Baselines recorded after trading so deltas only measure resolution.
    let creator_after_buys = token.balance(&p.alice);
    let no_after_buys = token.balance(&p.carol);
    let pool_after_buys = market.total_pool();

    // Resolve uncontested beyond the dispute window; YES wins its share count.
    env.ledger().set_timestamp(1_700_086_401);
    usdc_admin_mint_for(&env, &p.usdc, &p.resolver, 10_000, &p.client.address);
    p.client
        .propose_outcome(&p.market_id, &p.resolver, &Outcome::Yes, &10_000);
    env.ledger().set_sequence_number(
        env.ledger().sequence() + lumecast_resolution::DISPUTE_WINDOW_LEDGERS + 1,
    );
    p.client.finalize(&p.market_id);

    let market = p.client.market(&p.market_id).unwrap();
    // The final market's q_yes is exactly the shares bob bought (carol only
    // ever holds No), and the pool has been trimmed to that coverage.
    assert_eq!(market.shares.get(0).unwrap(), q_yes);
    assert_eq!(market.state, MarketState::Resolved(Outcome::Yes));
    assert_eq!(market.total_pool(), q_yes);

    // Winner claims exactly the winning coverage at 1:1; creator's sweep is
    // the seeded surplus; the losing side is untouched; escrow empties.
    let yes_payout = p.client.claim(&p.market_id, &p.bob, &Outcome::Yes);
    assert_eq!(yes_payout, q_yes);
    assert_eq!(token.balance(&p.bob), yes_after_buy + q_yes);
    assert_eq!(
        token.balance(&p.alice) - creator_after_buys,
        pool_after_buys - q_yes
    );
    assert_eq!(token.balance(&p.carol), no_after_buys);
    assert_eq!(p.client.claim(&p.market_id, &p.carol, &Outcome::No), 0);
    assert_eq!(token.balance(&p.client.address), 0);
}
