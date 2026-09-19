#![no_std]

//! Lumecast core market contract.
//!
//! M0 (Foundations): repo scaffolding, CI, pinned Soroban SDK, and the core
//! domain model — `Market`, `Position`, `Outcome`. The market lifecycle
//! (`create_market` → `deposit` → `cancel`/`claim`) is built on top of this
//! model in the M1 milestone; `claim` ships with the resolution module (M2).

use soroban_sdk::contract;

mod error;
mod types;

pub use error::Error;
pub use types::{Market, MarketState, Outcome, Position, OUTCOME_COUNT};

#[contract]
pub struct MarketContract;
