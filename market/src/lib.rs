#![no_std]

//! Lumecast core market contract.
//!
//! M0: scaffolding + working end-to-end contract in the local sandbox.
//! Market lifecycle (create/deposit/cancel) lands over the next commits;
//! claim is introduced together with the resolution module (M2).

use soroban_sdk::contract;

#[contract]
pub struct MarketContract;
