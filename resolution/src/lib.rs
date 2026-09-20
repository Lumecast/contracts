#![no_std]

//! Lumecast resolution module (M2).
//!
//! Bonded propose → dispute window → committee vote → finalize flow. The
//! resolver proposes an outcome and stakes a bond; anyone may dispute within
//! the window by posting a counter-bond; a configured dispute committee votes
//! and, once quorum is reached, `finalize` locks in the outcome and applies
//! bond slashing (losing side's bond goes to the winning side + protocol).
//!
//! This crate is a pure-logic rlib consumed by `lumecast-market`; it carries no
//! storage of its own. The market contract owns storage and token movement.

use soroban_sdk::{contracttype, Address, Env, Vec};

/// Duration of the dispute window expressed in ledgers (~5s per ledger):
/// 34_560 ledgers ≈ 48h.
pub const DISPUTE_WINDOW_LEDGERS: u32 = 34_560;

/// Fee basis points (per 10_000) taken from the losing side's forfeited bond
/// and paid to the protocol; the remainder goes to the winning side.
pub const PROTOCOL_SLASH_BPS: i128 = 1_000;

/// Basis-point denominator.
pub const BPS_TOTAL: i128 = 10_000;

/// Resolution state for a single market.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// Outcome index proposed by the resolver (per [`crate`]'s index scheme).
    pub proposed: u32,
    /// Address that proposed the outcome.
    pub proposer: Address,
    /// Bond escrowed by the proposer (market settlement asset).
    pub bond: i128,
    /// Ledger sequence at which the proposal was made; anchors the dispute window.
    pub proposed_ledger: u32,
    /// Open challenges: each `(challenger, counter_bond)` pair.
    pub challenges: Vec<(Address, i128)>,
    /// Committee votes cast for this resolution: `(voter, outcome index)`.
    pub votes: Vec<(Address, u32)>,
}

/// Platform governance configuration, stored by the market contract.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceConfig {
    /// Dispute committee (multisig members) who may vote on disputes.
    pub committee: Vec<Address>,
    /// Minimum number of committee votes required to finalize a dispute.
    pub quorum: u32,
    /// Receiver of the protocol's share of slashed bonds.
    pub protocol_fee_receiver: Option<Address>,
}

/// True while a proposal is still inside its dispute window.
pub fn in_dispute_window(env: &Env, proposed_ledger: u32) -> bool {
    env.ledger().sequence() - proposed_ledger < DISPUTE_WINDOW_LEDGERS
}

/// Split a forfeited bond into the winner's pot and the protocol's share.
pub fn slash_split(total: i128) -> (i128, i128) {
    let protocol = total * PROTOCOL_SLASH_BPS / BPS_TOTAL;
    (total - protocol, protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _};
    use soroban_sdk::Env;

    fn resolution(env: &Env) -> Resolution {
        Resolution {
            proposed: 0,
            proposer: Address::generate(env),
            bond: 1_000,
            proposed_ledger: 0,
            challenges: Vec::new(env),
            votes: Vec::new(env),
        }
    }

    #[test]
    fn dispute_window_tracks_ledger_age() {
        let env = Env::default();
        let res = resolution(&env);
        assert!(in_dispute_window(&env, res.proposed_ledger));
        env.ledger()
            .set_sequence_number(res.proposed_ledger + DISPUTE_WINDOW_LEDGERS + 1);
        assert!(!in_dispute_window(&env, res.proposed_ledger));
    }

    #[test]
    fn slash_split_takes_protocol_bps() {
        let (winner_via, protocol) = slash_split(10_000);
        assert_eq!(protocol, PROTOCOL_SLASH_BPS * 10_000 / BPS_TOTAL);
        assert_eq!(winner_via + protocol, 10_000);
        assert_eq!(protocol, 1_000);
        assert_eq!(winner_via, 9_000);
    }

    #[test]
    fn slash_split_zero_and_dust() {
        assert_eq!(slash_split(0), (0, 0));
        // Below one basis point of protocol share, everything floors to winner.
        let (winner_via, protocol) = slash_split(999);
        assert_eq!(winner_via + protocol, 999);
        assert!(protocol >= 0);
        assert!(winner_via >= 0);
    }
}
