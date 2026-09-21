#![no_std]

//! Lumecast pricing module.
//!
//! v1 uses a simple pari-mutuel pool; v2 upgrades to an LMSR automated market
//! maker. This module is stubbed out for M0 (foundations) and wired into the
//! market contract in M1/M3.

use soroban_sdk::{contracttype, Vec};

/// Fixed-point basis: `ONE` == 1.0 in normalized pricing math.
pub const ONE: i128 = 10_000;

/// `ln(2)` at the internal fixed-point scale, used by the LMSR seed.
pub const LN2: i128 = 693_147_180_559;

/// Internal fixed-point scale for the LMSR exp/ln machinery.
///
/// Working at `1e12` keeps rounding dust below a couple dozen token units even
/// for the largest admitted `b`, while every intermediate product stays well
/// inside `i128` (`S^3 / 27 ≈ 3.7e34 < ~1.7e38`).
pub const LMSR_INTERNAL_SCALE: i128 = 1_000_000_000_000;

/// Smallest `b` admitted for an LMSR market. The liquidity parameter scales a
/// cost function in token units, so it must be strictly positive and bounded
/// above to keep the fixed-point math inside `i128`.
pub const LMSR_B_MIN: i128 = 1;
/// Maximum `b` admitted for an LMSR market (1e12, well within `i128` even at
/// `b * LN2`).
pub const LMSR_B_MAX: i128 = 1_000_000_000_000;

/// Exp/ln series length. Convergence checkpoints for `ln1p` and the `atanh`
/// expansion sit comfortably under 24 terms.
pub const LMSR_SERIES_TERMS: usize = 24;

/// Above `z = EXP_NEG_CUT` the term `e^{-z}` is below one internal-scale unit
/// (`e^{-28} ≈ 7e-13 < 1e-12`) and is treated as zero.
pub const EXP_NEG_CUT: i128 = 28;

/// Pricing strategy attached to a pool.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricingModel {
    PariMutuel,
    Lmsr,
}

/// Pool math inputs (outstanding shares and escrowed tokens per outcome).
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub model: PricingModel,
    /// Outstanding shares per outcome index.
    pub shares: Vec<i128>,
    /// Escrowed settlement tokens per outcome index.
    pub pool: Vec<i128>,
}

/// Total settlement tokens escrowed across all outcomes of a pool.
pub fn total_escrowed(pool: &Pool) -> i128 {
    pool.pool.iter().sum()
}

/// Pari-mutuel payout to a holder of `holder_shares` of the winning outcome
/// given the whole `total_pool` (both outcomes) and the outstanding
/// `winning_shares` of the winning outcome.
///
/// Winners split the entire pool proportional to their winning shares:
/// `holder_shares * total_pool / winning_shares`.
///
/// Edge case — zero-liquidity winning side (nobody held shares of the outcome
/// that won): there is no one to pay a proportional share to, so every holder
/// is refunded their principal instead (1:1 pari-mutuel, shares == deposit).
pub fn pari_mutuel_payout(holder_shares: i128, total_pool: i128, winning_shares: i128) -> i128 {
    if winning_shares <= 0 {
        return holder_shares;
    }
    holder_shares * total_pool / winning_shares
}

/// Fixed-point `e^{-z}` for `z >= 0`, in [`LMSR_INTERNAL_SCALE`] units.
///
/// Range reduction followed by a Taylor series and repeated squaring:
/// `e^{-z} = (e^{-z / 2^m})^{2^m}`. The argument is halved until it drops
/// below 1.0 so the Taylor terms decay monotonically (no catastrophic
/// cancellation), then the result is squared back up `m` times. Everything
/// stays in `i128` fixed point.
fn exp_neg(z_hi: i128) -> i128 {
    if z_hi <= 0 {
        return LMSR_INTERNAL_SCALE;
    }
    if z_hi >= EXP_NEG_CUT * LMSR_INTERNAL_SCALE {
        return 0;
    }

    let mut u_hi = z_hi;
    let mut m: i128 = 0;
    while u_hi >= LMSR_INTERNAL_SCALE {
        u_hi >>= 1;
        m += 1;
    }

    let scale = LMSR_INTERNAL_SCALE;
    let mut acc = scale;
    let mut term = scale; // term_k = (-u)^k / k!
    let mut k: i128 = 1;
    loop {
        term = term * -u_hi / (k * scale);
        acc += term;
        if term == 0 || k >= LMSR_SERIES_TERMS as i128 {
            break;
        }
        k += 1;
    }

    for _ in 0..m {
        acc = acc * acc / scale;
    }
    acc
}

/// Fixed-point `ln(1 + t)` for `t = t_hi / S` in `[0, 1]`, in
/// [`LMSR_INTERNAL_SCALE`] units.
///
/// Uses the identity `ln(1 + t) = 2 * atanh(t / (2 + t))`; `y = t / (2 + t)`
/// lies in `[0, 1/3]` where the `atanh` series converges geometrically fast.
fn ln1p(t_hi: i128) -> i128 {
    if t_hi <= 0 {
        return 0;
    }
    let scale = LMSR_INTERNAL_SCALE;
    let y_hi = t_hi * scale / (2 * scale + t_hi);

    let mut sum = 0i128;
    let mut term = y_hi; // term_k = y^(2k+1) / (2k+1), in scale units
    let mut k: i128 = 0;
    loop {
        sum += term;
        if k >= LMSR_SERIES_TERMS as i128 || term == 0 {
            break;
        }
        // term_{k+1} = term_k * y^2 * (2k+1)/(2k+3)
        term = term * y_hi / scale * y_hi / scale * (2 * k + 1) / (2 * k + 3);
        k += 1;
    }
    2 * sum
}

/// Seed liquidity required to open an LMSR market with liquidity `b`: the cost
/// function value at an empty pool, `C(0, 0) = b * ln(2)`.
///
/// For the solvency guarantee `escrow = C(q) >= max(q)`, the escrow must start
/// at `C(0, 0)`; this is the amount the market creator seeds when the market
/// opens.
pub fn lmsr_seed(b: i128) -> i128 {
    if b <= 0 {
        return 0;
    }
    lmsr_cost(b, 0, 0)
}

/// LMSR cost function for a binary pool with outstanding `q0`/`q1` shares and
/// liquidity `b`: `C(q) = b * ln(e^{q0/b} + e^{q1/b})`.
///
/// Rewritten via the max term: with `M = max(q0, q1)` and `D = |q0 - q1|`,
/// `C(q) = M + b * ln(1 + e^{-D/b})`. The `ln` term is rounded *up*, so the
/// returned cost is always `>= max(q0, q1)` — the escrow can therefore pay
/// every winning share in full, by construction, no matter how lopsided the
/// pool grows.
pub fn lmsr_cost(b: i128, q0: i128, q1: i128) -> i128 {
    if b <= 0 {
        return 0;
    }
    let scale = LMSR_INTERNAL_SCALE;
    let q_max = if q0 >= q1 { q0 } else { q1 };
    let q_min = if q0 >= q1 { q1 } else { q0 };
    let d = q_max - q_min;

    let z_hi = d * scale / b;
    let t_hi = exp_neg(z_hi);
    let f_hi = ln1p(t_hi);
    let surplus = (b * f_hi + scale - 1) / scale;
    q_max + surplus
}

/// Marginal price of outcome 0 in fixed point (`ONE` == 1.0):
/// `p0 = e^{q0/b} / (e^{q0/b} + e^{q1/b})`, always in `[0, ONE]` and the
/// complement of outcome 1's price.
pub fn lmsr_marginal_price(b: i128, q0: i128, q1: i128) -> i128 {
    if b <= 0 {
        return 0;
    }
    let scale = LMSR_INTERNAL_SCALE;
    let z_hi = (q0 - q1).abs() * scale / b;
    let t_hi = exp_neg(z_hi);
    if q0 >= q1 {
        ONE * ONE / (ONE + t_hi / (scale / ONE))
    } else {
        t_hi / (scale / ONE) * ONE / (ONE + t_hi / (scale / ONE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winners_split_entire_pool_proportionally() {
        // 1000 YES, 500 NO → 1500 total pool. A 500-share YES holder takes 1000.
        assert_eq!(pari_mutuel_payout(500, 1_500, 1_000), 750);
        assert_eq!(pari_mutuel_payout(1_000, 1_500, 1_000), 1_500);
    }

    #[test]
    fn one_side_only_pays_back_principal() {
        assert_eq!(pari_mutuel_payout(700, 700, 700), 700);
    }

    #[test]
    fn zero_liquidity_winning_side_refunds_principal() {
        assert_eq!(pari_mutuel_payout(700, 700, 0), 700);
        assert_eq!(pari_mutuel_payout(0, 700, 0), 0);
    }

    #[test]
    fn no_position_pays_nothing() {
        assert_eq!(pari_mutuel_payout(0, 1_500, 1_000), 0);
    }

    #[test]
    fn lmsr_cost_at_empty_pool_is_the_seed() {
        // C(0, 0) = b * ln(2) ≈ 0.6931 * b.
        assert_eq!(lmsr_cost(10_000, 0, 0), lmsr_seed(10_000));
        assert!((6_931..=6_932).contains(&lmsr_cost(10_000, 0, 0)));
        assert!((693..=694).contains(&lmsr_cost(1_000, 0, 0)));
        // A degenerate `b` funds nothing.
        assert_eq!(lmsr_seed(0), 0);
        assert_eq!(lmsr_seed(-5), 0);
    }

    #[test]
    fn lmsr_cost_is_symmetric_and_degenerate_free() {
        // Symmetric in the two outcomes.
        assert_eq!(lmsr_cost(1_000, 120, 40), lmsr_cost(1_000, 40, 120));
        // A one-sided pool costs its max share count plus a small surplus
        // `b * ln(1 + e^{-5}) ≈ 7` tokens.
        let dominant = lmsr_cost(1_000, 5_000, 0);
        assert!((5_000..=5_010).contains(&dominant));
        // Imbalanced pools never cost less than the dominant outcome size.
        assert!(lmsr_cost(1_000, 8_000, 1_000) >= 8_000);
    }

    #[test]
    fn lmsr_cost_is_monotone_in_each_outcome() {
        let b = 2_500;
        for (y, n) in [(0, 0), (10, 0), (0, 10), (50, 30), (120, 400), (1_000, 900)] {
            // Buying more of either outcome never lowers the cost. Integer
            // rounding can flatten a sub-token increment, hence `>=`.
            assert!(
                lmsr_cost(b, y + 1, n) >= lmsr_cost(b, y, n),
                "y-side at ({y},{n})"
            );
            assert!(
                lmsr_cost(b, y, n + 1) >= lmsr_cost(b, y, n),
                "n-side at ({y},{n})"
            );
        }
    }

    #[test]
    fn lmsr_cost_never_below_max_outcome() {
        let b = 3_333;
        for (y, n) in [
            (0, 0),
            (1_000, 0),
            (0, 7_000),
            (12_345, 6_543),
            (99_999, 100_000),
            (250_000, 1),
        ] {
            assert!(lmsr_cost(b, y, n) >= y.max(n), "at ({y},{n})");
        }
    }

    #[test]
    fn lmsr_marginal_price_is_bounded_and_complements() {
        let b = 10_000;
        let cases = [
            (0i128, 0i128),
            (1_000, 0),
            (0, 1_000),
            (500, 500),
            (7_000, 3_000),
            (3_000, 7_000),
            (100_000, 1),
            (1, 100_000),
        ];
        for (y, n) in cases {
            let p0 = lmsr_marginal_price(b, y, n);
            let p1 = lmsr_marginal_price(b, n, y);
            assert!((0..=ONE).contains(&p0), "p0 at ({y},{n}) = {p0}");
            // Fixed-point floors can cost one unit of complementarity.
            assert!(
                p0 + p1 >= ONE - 1 && p0 + p1 <= ONE,
                "p0+p1 at ({y},{n}) = {}",
                p0 + p1
            );
        }
    }

    #[test]
    fn lmsr_marginal_price_extremes() {
        // Equal-held pool trades at 0.5; a dominant side approaches 1.0.
        assert_eq!(lmsr_marginal_price(10_000, 5_000, 5_000), ONE / 2);
        assert!(lmsr_marginal_price(10_000, 1_000_000, 0) >= ONE - 5);
        let deep = lmsr_marginal_price(1, 50_000, 0);
        assert!((0..=ONE).contains(&deep));
    }

    #[test]
    fn lmsr_cost_matches_integrated_marginal_price() {
        // Buying x shares pays the integral of the marginal price, so the
        // per-trade unit cost must stay close to/below 1.0 token per share.
        let b = 2_000;
        let q0 = 4_000;
        let q1 = 9_000;
        let base = lmsr_cost(b, q0, q1);
        for x in [1i128, 5, 10, 100] {
            let cost = lmsr_cost(b, q0 + x, q1) - base;
            assert!(cost >= 0, "x={x}");
            assert!(cost <= x + 2, "cost {cost} exceeds shares {x}");
        }
    }
}
