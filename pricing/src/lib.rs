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
