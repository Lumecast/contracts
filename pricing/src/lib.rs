#![no_std]

//! Lumecast pricing module.
//!
//! v1 uses a simple pari-mutuel pool; v2 upgrades to an LMSR automated market
//! maker. This module is stubbed out for M0 (foundations) and wired into the
//! market contract in M1/M3.

use soroban_sdk::{contracttype, Vec};

/// Fixed-point basis: `ONE` == 1.0 in normalized pricing math.
pub const ONE: i128 = 10_000;

/// Pricing strategy attached to a pool.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricingModel {
    PariMutuel,
    Lmsr,
}

/// Pool math inputs (outstanding shares and escrowed tokens per outcome).
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub model: PricingModel,
    /// Outstanding shares per outcome index.
    pub shares: Vec<i128>,
    /// Escrowed settlement tokens per outcome index.
    pub pool: Vec<i128>,
}

/// Total settlement tokens escrowed across all outcomes of a pool.
pub fn total_escrowed(pool: &Pool) -> i128 {
    pool.pool.iter().sum()
}

/// Pari-mutuel payout to a holder of `holder_shares` of the winning outcome
/// given the whole `total_pool` (both outcomes) and the outstanding
/// `winning_shares` of the winning outcome.
///
/// Winners split the entire pool proportional to their winning shares:
/// `holder_shares * total_pool / winning_shares`.
///
/// Edge case — zero-liquidity winning side (nobody held shares of the outcome
/// that won): there is no one to pay a proportional share to, so every holder
/// is refunded their principal instead (1:1 pari-mutuel, shares == deposit).
pub fn pari_mutuel_payout(holder_shares: i128, total_pool: i128, winning_shares: i128) -> i128 {
    if winning_shares <= 0 {
        return holder_shares;
    }
    holder_shares * total_pool / winning_shares
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winners_split_entire_pool_proportionally() {
        // 1000 YES, 500 NO → 1500 total pool. A 500-share YES holder takes 1000.
        assert_eq!(pari_mutuel_payout(500, 1_500, 1_000), 750);
        assert_eq!(pari_mutuel_payout(1_000, 1_500, 1_000), 1_500);
    }

    #[test]
    fn one_side_only_pays_back_principal() {
        assert_eq!(pari_mutuel_payout(700, 700, 700), 700);
    }

    #[test]
    fn zero_liquidity_winning_side_refunds_principal() {
        assert_eq!(pari_mutuel_payout(700, 700, 0), 700);
        assert_eq!(pari_mutuel_payout(0, 700, 0), 0);
    }

    #[test]
    fn no_position_pays_nothing() {
        assert_eq!(pari_mutuel_payout(0, 1_500, 1_000), 0);
    }
}
