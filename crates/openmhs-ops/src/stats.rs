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

/// Paired outcomes of one failure/success question asked twice about the same scenarios.
///
/// `a` and `b` are the arms; `both` fail in both, `only_a` fail in `a` alone, `only_b` in `b` alone, `neither` in neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairedCounts {
    pub both: u64,
    pub only_a: u64,
    pub only_b: u64,
    pub neither: u64,
}

impl PairedCounts {
    pub fn pairs(&self) -> u64 {
        self.both + self.only_a + self.only_b + self.neither
    }
}

/// Difference `p_b - p_a` of two correlated failure rates, with Newcombe's hybrid score interval for paired proportions
/// (method 10: the two Wilson intervals, combined through the correlation the pairs show). With no correlation it is
/// the unpaired [`newcombe`] interval. `None` when there are no pairs.
pub fn newcombe_paired(c: PairedCounts) -> Option<(f64, f64, f64)> {
    let n = c.pairs();
    if n == 0 {
        return None;
    }
    let (a, b, cc, d) = (
        c.both as f64,
        c.only_b as f64,
        c.only_a as f64,
        c.neither as f64,
    );
    let nf = n as f64;
    let (p1, p2) = ((a + b) / nf, (a + cc) / nf);
    let (l1, u1) = wilson(c.both + c.only_b, n)?;
    let (l2, u2) = wilson(c.both + c.only_a, n)?;
    let big_a = (a + b) * (cc + d) * (a + cc) * (b + d);
    let big_b = a * d - b * cc;
    let adjusted = if big_b > nf / 2.0 {
        big_b - nf / 2.0
    } else if big_b >= 0.0 {
        0.0
    } else {
        big_b
    };
    let phi = if big_a > 0.0 {
        adjusted / big_a.sqrt()
    } else {
        0.0
    };
    let diff = p1 - p2;
    let lower = diff
        - ((p1 - l1).powi(2) - 2.0 * phi * (p1 - l1) * (u2 - p2) + (u2 - p2).powi(2))
            .max(0.0)
            .sqrt();
    let upper = diff
        + ((u1 - p1).powi(2) - 2.0 * phi * (u1 - p1) * (p2 - l2) + (p2 - l2).powi(2))
            .max(0.0)
            .sqrt();
    Some((diff, lower.max(-1.0), upper.min(1.0)))
}

/// Exact two-sided McNemar test: under "no difference" the pairs that differ split evenly. `None` without discordant pairs.
pub fn mcnemar_exact(only_a: u64, only_b: u64) -> Option<f64> {
    let m = only_a + only_b;
    if m == 0 {
        return None;
    }
    let k = only_a.min(only_b);
    let mut term = 0.5_f64.powi(i32::try_from(m).unwrap_or(i32::MAX));
    let mut tail = 0.0;
    for i in 0..=k {
        tail += term;
        term *= (m - i) as f64 / (i + 1) as f64;
    }
    Some((2.0 * tail).min(1.0))
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

    #[test]
    fn mcnemar_matches_hand_computed_values() {
        // m = 10 discordant pairs: P(0 of 10) = 1/1024; P(<= 2 of 10) = 56/1024.
        assert!(close(
            mcnemar_exact(0, 10).unwrap_or_default(),
            2.0 / 1024.0
        ));
        assert!(close(
            mcnemar_exact(2, 8).unwrap_or_default(),
            112.0 / 1024.0
        ));
        assert!(close(mcnemar_exact(5, 5).unwrap_or_default(), 1.0));
        assert_eq!(mcnemar_exact(0, 0), None);
        // 5 discordant pairs all one way: 2 / 32.
        assert!(close(mcnemar_exact(5, 0).unwrap_or_default(), 0.0625));
    }

    #[test]
    fn with_no_correlation_the_paired_interval_is_the_unpaired_one() {
        // Independence: a*d == b*c gives B = 0, so phi = 0.
        let c = PairedCounts {
            both: 4,
            only_a: 16,
            only_b: 16,
            neither: 64,
        };
        let (d, lo, hi) = newcombe_paired(c).unwrap_or_default();
        let (ud, ulo, uhi) = newcombe(20, 100, 20, 100).unwrap_or_default();
        assert!(
            close(d, ud) && close(lo, ulo) && close(hi, uhi),
            "{lo} {hi} vs {ulo} {uhi}"
        );
    }

    #[test]
    fn pairing_narrows_the_interval_when_the_arms_agree_scenario_by_scenario() {
        // The arms fail on mostly the same scenarios: strongly correlated.
        let paired = PairedCounts {
            both: 18,
            only_a: 7,
            only_b: 2,
            neither: 73,
        };
        let (d, lo, hi) = newcombe_paired(paired).unwrap_or_default();
        let (ud, ulo, uhi) = newcombe(25, 100, 20, 100).unwrap_or_default();
        assert!(close(d, ud), "same point estimate");
        assert!(
            hi - lo < uhi - ulo,
            "paired {lo}..{hi} is narrower than unpaired {ulo}..{uhi}"
        );
    }

    /// Coverage by simulation: draw correlated pairs from a known joint distribution many times; the 95 % interval should
    /// contain the true difference close to 95 % of the time. Deterministic (a fixed linear congruential generator).
    #[test]
    fn the_paired_interval_covers_the_true_difference_about_95_percent_of_the_time() {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64) / ((1u64 << 53) as f64)
        };
        // Joint: both fail 0.20, only a fails 0.10, only b fails 0.05, neither 0.65. True p_b - p_a = -0.05.
        let (pb, pa_only, pb_only) = (0.20, 0.10, 0.05);
        let truth = pb_only - pa_only;
        for n in [30u64, 60] {
            let (mut covered, trials) = (0u32, 2000u32);
            for _ in 0..trials {
                let mut c = PairedCounts {
                    both: 0,
                    only_a: 0,
                    only_b: 0,
                    neither: 0,
                };
                for _ in 0..n {
                    let u = next();
                    if u < pb {
                        c.both += 1
                    } else if u < pb + pa_only {
                        c.only_a += 1
                    } else if u < pb + pa_only + pb_only {
                        c.only_b += 1
                    } else {
                        c.neither += 1
                    }
                }
                if let Some((_, lo, hi)) = newcombe_paired(c)
                    && lo <= truth
                    && truth <= hi
                {
                    covered += 1
                }
            }
            let rate = f64::from(covered) / f64::from(trials);
            assert!((0.92..=0.985).contains(&rate), "n = {n}: coverage {rate}");
        }
    }
}
