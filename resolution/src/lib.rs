#![no_std]

//! Lumecast resolution module.
//!
//! Bonded propose → dispute window → finalize flow (M2). Stubbed for M0 so the
//! workspace assembles; the market contract will consume this once resolution
//! lands.

use soroban_sdk::{contracttype, Address, Env, Vec};

/// State of a resolution in progress.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// Proposed winning outcome index.
    pub proposed: u32,
    /// Resolver who posted the proposing bond.
    pub proposer: Address,
    /// Bond escrowed by the proposer.
    pub bond: i128,
    /// Ledger sequence at which the proposal was made.
    pub proposed_ledger: u32,
    /// Open challenges: (challenger, counter_bond) pairs.
    pub challenges: Vec<(Address, i128)>,
}

/// Duration of the dispute window expressed in ledgers (~5s per ledger).
pub const DISPUTE_WINDOW_LEDGERS: u32 = 34_560;

/// True while the proposal is still inside its dispute window.
pub fn in_dispute_window(env: &Env, proposed_ledger: u32) -> bool {
    env.ledger().sequence() - proposed_ledger < DISPUTE_WINDOW_LEDGERS
}
