use soroban_sdk::{contractevent, Address, String};

/// Emitted when a market is created.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateMarketEvent {
    #[topic]
    pub market_id: u64,
    pub creator: Address,
    pub resolver: Address,
    pub question: String,
}

/// Emitted when a depositor buys shares in an outcome (token enters escrow).
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositEvent {
    #[topic]
    pub market_id: u64,
    #[topic]
    pub outcome_index: u32,
    pub from: Address,
    pub amount: i128,
}

/// Emitted when a market is cancelled and every deposit is refunded.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelMarketEvent {
    #[topic]
    pub market_id: u64,
    pub cancelled_by: Address,
}

/// Emitted when the resolver proposes an outcome and posts a bond.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposeEvent {
    #[topic]
    pub market_id: u64,
    #[topic]
    pub outcome_index: u32,
    pub proposer: Address,
    pub bond: i128,
}

/// Emitted when a challenger disputes a proposal by posting a counter-bond.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeEvent {
    #[topic]
    pub market_id: u64,
    #[topic]
    pub outcome_index: u32,
    pub challenger: Address,
    pub counter_bond: i128,
}

/// Emitted when a committee member casts a vote on a dispute.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoteEvent {
    #[topic]
    pub market_id: u64,
    #[topic]
    pub outcome_index: u32,
    pub voter: Address,
}

/// Emitted when a market finalizes: outcome locked, bonds settled.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizeEvent {
    #[topic]
    pub market_id: u64,
    #[topic]
    pub outcome_index: u32,
    /// True when the resolution was uncontested (window expired silently).
    pub uncontested: bool,
    /// Some(bond) returned to the proposer when uncontested.
    pub returned_bond: Option<i128>,
}

/// Emitted when a holder claims winnings (or a zero-liquidity refund).
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimEvent {
    #[topic]
    pub market_id: u64,
    #[topic]
    pub outcome_index: u32,
    pub claimant: Address,
    pub amount: i128,
}