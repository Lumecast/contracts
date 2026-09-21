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
    /// Only the market resolver may propose an outcome.
    NotResolver = 8,
    /// Resolution proposed before `resolution_ts`.
    ResolutionNotReady = 9,
    /// Market is already proposed/disputed/resolved; cannot propose again.
    NotProposable = 10,
    /// Resolution requires the market to be in the proposed state.
    NotProposed = 11,
    /// A dispute was raised outside the dispute window (too late).
    DisputeWindowClosed = 12,
    /// Counter-bond must cover (>=) the proposer's bond to dispute.
    BondTooLow = 13,
    /// No dispute committee configured for this contract instance.
    CommitteeNotSet = 14,
    /// Resolution requires the market to be disputed.
    NotDisputed = 15,
    /// Caller is not a member of the dispute committee.
    NotCommitteeMember = 16,
    /// Not enough committee votes have been cast to settle the dispute.
    QuorumNotReached = 17,
    /// Dispute window is still open; cannot finalize an uncontested resolution.
    DisputeWindowOpen = 18,
    /// Claim attempted before the market resolved.
    NotResolved = 19,
    /// Governance configuration is invalid (empty committee, quorum out of range).
    InvalidGovernance = 20,
    /// Liquidity parameter `b` is outside `[0, LMSR_B_MAX]`; zero selects the
    /// pari-mutuel model, anything above selects LMSR.
    InvalidLiquidity = 21,
    /// Operation only applies to the market's pricing model (deposit on an
    /// LMSR market, or buy/sell on a pari-mutuel market).
    PricingModelMismatch = 22,
    /// LMSR markets cannot be cancelled; they exit via resolution, with the
    /// seeded liquidity recovered through `finalize`.
    LmsrNotCancellable = 23,
    /// The holder does not own enough shares to sell.
    InsufficientShares = 24,
    /// The spend is too small to buy even a single share after rounding.
    AmountTooSmall = 25,
}
