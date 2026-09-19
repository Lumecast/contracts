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
