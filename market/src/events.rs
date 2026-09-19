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
