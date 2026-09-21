use lumecast_pricing::PricingModel;
use soroban_sdk::{contracttype, Address, String, Vec};

use crate::error::Error;

/// Number of outcomes in a v1 (binary) market.
pub const OUTCOME_COUNT: u32 = 2;

/// Outcome of a binary (YES/NO) prediction market.
///
/// Multi-outcome markets are a planned upgrade; the stable index mapping keeps
/// outcome-arrayed state (`pool`/`shares`) forward-compatible.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    Yes,
    No,
}

impl Outcome {
    /// Canonical ordering of all outcomes.
    pub const ALL: [Outcome; 2] = [Outcome::Yes, Outcome::No];

    /// Stable index into outcome-arrayed state (`pool`, `shares`).
    pub const fn index(self) -> u32 {
        match self {
            Outcome::Yes => 0,
            Outcome::No => 1,
        }
    }

    /// Inverse of [`Outcome::index`].
    pub const fn from_index(index: u32) -> Result<Self, Error> {
        match index {
            0 => Ok(Outcome::Yes),
            1 => Ok(Outcome::No),
            _ => Err(Error::UnknownOutcome),
        }
    }
}

/// Lifecycle state of a market.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketState {
    /// Accepting deposits until `close_ts`; cancellable by creator/resolver.
    Open,
    /// Outcome proposed, inside the dispute window (M2).
    Proposed(Outcome),
    /// Outcome proposed and disputed; awaiting committee vote (M2).
    Disputed(Outcome),
    /// Outcome locked in; claims payable (M2).
    Resolved(Outcome),
    /// Cancelled before resolution; all deposits refunded.
    Cancelled,
}

/// Arguments bundled for `create_market`, so the creation interface can grow
/// (liquidity parameter `b`, future fee hooks) without churning the flat
/// signature. The authoritative caller identity stays a separate `creator` arg.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateMarketParameter {
    /// Address responsible for proposing resolution (M2).
    pub resolver: Address,
    /// Settlement asset (USDC Stellar Asset Contract).
    pub asset: Address,
    /// The prediction being traded.
    pub question: String,
    /// Ledger-chain timestamp after which deposits are rejected.
    pub close_ts: u64,
    /// Earliest timestamp at which resolution may be proposed (M2).
    pub resolution_ts: u64,
    /// LMSR liquidity parameter; `0` selects the v1 pari-mutuel model.
    pub b: i128,
}

/// A single prediction market and the unit of escrow for its question.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Market {
    /// Stable identifier within the contract instance.
    pub id: u64,
    /// The prediction being traded.
    pub question: String,
    /// Address that created the market.
    pub creator: Address,
    /// Address responsible for proposing resolution (M2).
    pub resolver: Address,
    /// Settlement asset (USDC Stellar Asset Contract).
    pub asset: Address,
    /// Ledger-chain timestamp after which deposits are rejected.
    pub close_ts: u64,
    /// Earliest timestamp at which resolution may be proposed (M2).
    pub resolution_ts: u64,
    /// Current lifecycle state.
    pub state: MarketState,
    /// Escrowed settlement tokens per outcome index.
    pub pool: Vec<i128>,
    /// Outstanding shares per outcome index.
    pub shares: Vec<i128>,
    /// LMSR liquidity parameter. `b == 0` selects the v1 pari-mutuel model;
    /// `b > 0` opens an LMSR market with that liquidity. For LMSR markets
    /// `pool[0]` holds the single cash balance `C(q)` (see
    /// [`lumecast_pricing::lmsr_cost`]) and `pool[1]` is unused.
    pub b: i128,
}

impl Market {
    /// The pricing strategy attached to this market.
    pub fn pricing_model(&self) -> PricingModel {
        if self.b > 0 {
            PricingModel::Lmsr
        } else {
            PricingModel::PariMutuel
        }
    }

    /// Total settlement tokens escrowed across all outcomes.
    pub fn total_pool(&self) -> i128 {
        match self.pricing_model() {
            PricingModel::Lmsr => self.pool.get(0).unwrap_or(0),
            PricingModel::PariMutuel => self.pool.iter().sum(),
        }
    }
}

/// A holder's stake in a single outcome of a single market.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub owner: Address,
    pub market_id: u64,
    pub outcome: Outcome,
    pub shares: i128,
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::Env;

    #[test]
    fn outcome_index_mapping_is_stable() {
        assert_eq!(Outcome::Yes.index(), 0);
        assert_eq!(Outcome::No.index(), 1);
        assert_eq!(Outcome::ALL, [Outcome::Yes, Outcome::No]);
    }

    #[test]
    fn outcome_from_index_round_trips() {
        assert_eq!(Outcome::from_index(0), Ok(Outcome::Yes));
        assert_eq!(Outcome::from_index(1), Ok(Outcome::No));
        assert_eq!(Outcome::from_index(2), Err(Error::UnknownOutcome));
    }

    #[test]
    fn market_state_derives_eq_and_copy() {
        let open = MarketState::Open;
        let copied = open;
        assert_eq!(open, copied);
        assert_eq!(
            MarketState::Proposed(Outcome::No),
            MarketState::Proposed(Outcome::No)
        );
        assert_ne!(
            MarketState::Proposed(Outcome::Yes),
            MarketState::Proposed(Outcome::No)
        );
    }

    #[test]
    fn market_constructs_with_balanced_zero_pools() {
        let env = Env::default();
        let market = Market {
            id: 1,
            question: String::from_str(&env, "Will Lumecast ship M1 on time?"),
            creator: Address::generate(&env),
            resolver: Address::generate(&env),
            asset: Address::generate(&env),
            close_ts: 1_700_000_000,
            resolution_ts: 1_700_086_400,
            state: MarketState::Open,
            pool: Vec::from_array(&env, [0i128; OUTCOME_COUNT as usize]),
            shares: Vec::from_array(&env, [0i128; OUTCOME_COUNT as usize]),
            b: 0,
        };
        assert_eq!(market.id, 1);
        assert_eq!(market.pool.len(), market.shares.len());
        assert_eq!(market.total_pool(), 0);
        assert_eq!(market.state, MarketState::Open);
    }

    #[test]
    fn market_pricing_model_maps_b() {
        let env = Env::default();
        let zero = market_with(&env, 0);
        assert_eq!(zero.pricing_model(), PricingModel::PariMutuel);
        assert_eq!(zero.total_pool(), 0);

        let lmsr = market_with(&env, 1_000);
        assert_eq!(lmsr.pricing_model(), PricingModel::Lmsr);
    }

    fn market_with(env: &Env, b: i128) -> Market {
        Market {
            id: 1,
            question: String::from_str(env, "Q"),
            creator: Address::generate(env),
            resolver: Address::generate(env),
            asset: Address::generate(env),
            close_ts: 1_700_000_000,
            resolution_ts: 1_700_086_400,
            state: MarketState::Open,
            pool: Vec::from_array(env, [0i128; OUTCOME_COUNT as usize]),
            shares: Vec::from_array(env, [0i128; OUTCOME_COUNT as usize]),
            b,
        }
    }
}
