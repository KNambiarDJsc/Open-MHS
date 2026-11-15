//! Intervals for rates and differences of rates (ADR-0032).

/// z for a two-sided 95 % interval.
pub const Z95: f64 = 1.959_963_984_540_054;

/// Wilson score interval for `k` events in `n` trials. `None` when `n` is zero.
pub fn wilson(k: u64, n: u64) -> Option<(f64, f64)> {
    if n == 0 || k > n {
        return None;
    }
    let (k, n) = (k as f64, n as f64);
    let p = k / n;
    let z2 = Z95 * Z95;
    let denom = 1.0 + z2 / n;
    let centre = (p + z2 / (2.0 * n)) / denom;
    let half = Z95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denom;
    Some(((centre - half).max(0.0), (centre + half).min(1.0)))
}

/// Difference `p_b - p_a` with Newcombe's hybrid score interval (method 10), built from the two Wilson intervals.
pub fn newcombe(k_a: u64, n_a: u64, k_b: u64, n_b: u64) -> Option<(f64, f64, f64)> {
    let (la, ua) = wilson(k_a, n_a)?;
    let (lb, ub) = wilson(k_b, n_b)?;
    let pa = k_a as f64 / n_a as f64;
    let pb = k_b as f64 / n_b as f64;
    let d = pb - pa;
    let lo = d - ((pb - lb).powi(2) + (ua - pa).powi(2)).sqrt();
    let hi = d + ((ub - pb).powi(2) + (pa - la).powi(2)).sqrt();
    Some((d, lo, hi))
}

/// One stratum: `k_a` events in `n_a` trials in arm a, and `k_b` in `n_b` in arm b.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StratumCounts {
    pub k_a: u64,
    pub n_a: u64,
    pub k_b: u64,
    pub n_b: u64,
}

/// Mantel-Haenszel risk difference `b - a` over strata, with a Wald interval from the large-strata variance
/// `Σ w² (p_a q_a / n_a + p_b q_b / n_b) / (Σ w)²`, `w = n_a n_b / (n_a + n_b)`. Strata missing an arm are skipped.
/// Not suited to many sparse strata.
pub fn mantel_haenszel_rd(strata: &[StratumCounts]) -> Option<(f64, f64, f64)> {
    let mut sum_w = 0.0;
    let mut sum_wd = 0.0;
    let mut sum_var = 0.0;
    for s in strata.iter().filter(|s| s.n_a > 0 && s.n_b > 0) {
        let (na, nb) = (s.n_a as f64, s.n_b as f64);
        let (pa, pb) = (s.k_a as f64 / na, s.k_b as f64 / nb);
        let w = na * nb / (na + nb);
        sum_w += w;
        sum_wd += w * (pb - pa);
        sum_var += w * w * (pa * (1.0 - pa) / na + pb * (1.0 - pb) / nb);
    }
    if sum_w == 0.0 {
        return None;
    }
    let d = sum_wd / sum_w;
    let se = sum_var.sqrt() / sum_w;
    Some((d, d - Z95 * se, d + Z95 * se))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 5e-5
    }

    #[test]
    fn wilson_matches_reference_values() {
        // Reference values computed independently (Python, closed form).
        let (lo, hi) = wilson(31, 1200).unwrap_or_default();
        assert!(close(lo, 0.0183) && close(hi, 0.0364), "{lo} {hi}");
        let (lo, hi) = wilson(19, 379).unwrap_or_default();
        assert!(close(lo, 0.0323) && close(hi, 0.0770), "{lo} {hi}");
        let (lo, hi) = wilson(0, 30).unwrap_or_default();
        assert!(close(lo, 0.0) && close(hi, 0.1135), "{lo} {hi}");
        assert_eq!(wilson(1, 0), None);
    }

    #[test]
    fn newcombe_matches_reference_values() {
        let (d, lo, hi) = newcombe(31, 1200, 19, 379).unwrap_or_default();
        assert!(
            close(d, 0.0243) && close(lo, 0.0036) && close(hi, 0.0522),
            "{d} {lo} {hi}"
        );
    }

    #[test]
    fn mantel_haenszel_with_one_stratum_is_the_plain_difference() {
        let one = [StratumCounts {
            k_a: 10,
            n_a: 100,
            k_b: 20,
            n_b: 100,
        }];
        let (d, lo, hi) = mantel_haenszel_rd(&one).unwrap_or_default();
        assert!(close(d, 0.1));
        // Wald: 0.1 ± 1.96 sqrt(0.09/100 + 0.16/100) = 0.1 ± 0.098
        assert!(
            close(lo, 0.1 - Z95 * 0.05) && close(hi, 0.1 + Z95 * 0.05),
            "{lo} {hi}"
        );
    }

    #[test]
    fn mantel_haenszel_removes_a_confound_that_pooling_hides() {
        // Release b ran mostly at the hard site. Within each site the rates are equal.
        let strata = [
            StratumCounts {
                k_a: 10,
                n_a: 500,
                k_b: 2,
                n_b: 100,
            }, // easy site: 2 % vs 2 %
            StratumCounts {
                k_a: 10,
                n_a: 100,
                k_b: 50,
                n_b: 500,
            }, // hard site: 10 % vs 10 %
        ];
        let (d, _, _) = mantel_haenszel_rd(&strata).unwrap_or_default();
        assert!(d.abs() < 1e-12, "{d}");
        let pooled = newcombe(20, 600, 52, 600).unwrap_or_default().0;
        assert!(
            pooled > 0.05,
            "pooled data shows a false 5 point regression: {pooled}"
        );
    }
}
