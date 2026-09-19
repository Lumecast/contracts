use soroban_sdk::contracterror;

/// Errors surfaced by the market contract and its storage helpers.
#[contracterror]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Error {
    /// The referenced market does not exist.
    MarketNotFound = 1,
    /// The operation requires the market to be in the `Open` state.
    MarketNotOpen = 2,
    /// Trading has closed; no more deposits are accepted.
    AfterClose = 3,
    /// Amounts must be strictly positive.
    ZeroAmount = 4,
    /// `close_ts` must be strictly before `resolution_ts`.
    InvalidTiming = 5,
    /// Outcome index does not map to a known outcome.
    UnknownOutcome = 6,
    /// Caller is not authorized for this operation.
    Unauthorized = 7,
}
