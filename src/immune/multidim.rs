// MULTIVARIATE BASELINE — Mahalanobis distance + Ledoit-Wolf
// shrinkage covariance, per Spec 5 §5.5
//
// The 1-D Welford baseline used by individual cell-agents (Spec 6
// §3.3) covers their per-specialization signal. This module builds
// the K-dimensional substrate the cognitive map (Step 5) uses for
// per-region state vectors:
//
//   b(t) = [r(t), d_1(t), …, d_R(t), c(t), e(t), s(t)]
//
// For R=5 regions, K = 9. The detection threshold for the per-region
// state vector is the χ² critical value at significance p, modulated
// by an agent's `TrustWeight` (low trust → tighter scrutiny per
// Spec 5 §5.5.3):
//
//   tw < 0.3      → p=0.01     (99th percentile)
//   0.3 ≤ tw <0.8 → p=0.001    (99.9th percentile)
//   tw ≥ 0.8      → p=0.0001   (99.99th percentile)
//
// Ledoit-Wolf shrinkage (LW 2004) gives a well-conditioned covariance
// estimator when N (observations) is small relative to K (dimensions),
// the regime that dominates during the maternal-immunity training
// period (Spec 5 §4.2).
//
// Implementation choices for v1:
//   - Matrix as a flat `Vec<f64>` indexed by `i * k + j`. Cheap
//     allocation, no nalgebra dep.
//   - Gauss-Jordan with partial pivoting for inversion. K is small
//     (≤ ~16 in practice), so O(K³) with small constants is fine.
//   - χ² critical values from a small Wilson-Hilferty approximation
//     plus an exact lookup for K=9 (the v1 target).

use crate::identity::TrustWeight;

// ── Matrix helpers (flat row-major) ──────────────────────────────

/// Index a flat row-major K×K matrix.
#[inline]
fn at(m: &[f64], k: usize, i: usize, j: usize) -> f64 {
    m[i * k + j]
}

#[inline]
fn at_mut(m: &mut [f64], k: usize, i: usize, j: usize) -> &mut f64 {
    &mut m[i * k + j]
}

fn identity(k: usize) -> Vec<f64> {
    let mut out = vec![0.0; k * k];
    for i in 0..k {
        out[i * k + i] = 1.0;
    }
    out
}

/// Trace of a K×K matrix.
fn trace(m: &[f64], k: usize) -> f64 {
    let mut t = 0.0;
    for i in 0..k {
        t += at(m, k, i, i);
    }
    t
}

/// Squared Frobenius norm: sum of m[i][j]².
fn frobenius_sq(m: &[f64]) -> f64 {
    m.iter().map(|x| x * x).sum()
}

/// In-place Gauss-Jordan inverse with partial pivoting. Returns
/// `Err(())` if the matrix is singular within the supplied epsilon.
pub fn invert(m: &[f64], k: usize) -> Result<Vec<f64>, MatrixError> {
    if m.len() != k * k {
        return Err(MatrixError::ShapeMismatch);
    }
    // Augmented matrix [m | I] — we operate on a 2K-wide buffer.
    let mut aug = vec![0.0; k * 2 * k];
    for i in 0..k {
        for j in 0..k {
            aug[i * 2 * k + j] = at(m, k, i, j);
        }
        aug[i * 2 * k + k + i] = 1.0;
    }

    for i in 0..k {
        // Partial pivot.
        let mut pivot_row = i;
        let mut pivot_val = aug[i * 2 * k + i].abs();
        for r in (i + 1)..k {
            let v = aug[r * 2 * k + i].abs();
            if v > pivot_val {
                pivot_val = v;
                pivot_row = r;
            }
        }
        if pivot_val < 1e-12 {
            return Err(MatrixError::Singular);
        }
        if pivot_row != i {
            // Swap row i and pivot_row.
            for c in 0..(2 * k) {
                aug.swap(i * 2 * k + c, pivot_row * 2 * k + c);
            }
        }
        // Scale pivot row to make pivot = 1.
        let pivot = aug[i * 2 * k + i];
        for c in 0..(2 * k) {
            aug[i * 2 * k + c] /= pivot;
        }
        // Eliminate other rows.
        for r in 0..k {
            if r == i {
                continue;
            }
            let factor = aug[r * 2 * k + i];
            if factor == 0.0 {
                continue;
            }
            for c in 0..(2 * k) {
                aug[r * 2 * k + c] -= factor * aug[i * 2 * k + c];
            }
        }
    }

    // Extract right half = inverse.
    let mut out = vec![0.0; k * k];
    for i in 0..k {
        for j in 0..k {
            out[i * k + j] = aug[i * 2 * k + k + j];
        }
    }
    Ok(out)
}

/// Compute v^T M v for a K-dim vector v and K×K matrix m.
fn quadratic_form(v: &[f64], m: &[f64], k: usize) -> f64 {
    let mut acc = 0.0;
    for i in 0..k {
        let mut row_dot = 0.0;
        for j in 0..k {
            row_dot += at(m, k, i, j) * v[j];
        }
        acc += v[i] * row_dot;
    }
    acc
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatrixError {
    /// Matrix is singular or near-singular under the inversion
    /// epsilon. Caller should fall back to the shrinkage estimator.
    Singular,
    /// Vec length doesn't match the supplied K.
    ShapeMismatch,
}

// ── Multivariate Welford accumulator ─────────────────────────────

/// Online mean + scatter accumulator for K-dimensional observations.
/// Numerically stable Welford generalization (Welford 1962, Bilmes
/// 1998 MV extension).
#[derive(Debug, Clone)]
pub struct MultivariateBaseline {
    k: usize,
    n: u64,
    mean: Vec<f64>,
    /// Scatter / sum-of-squared-deviations matrix M2 (K×K, row-major).
    /// Sample covariance is `M2 / (n - 1)`.
    m2: Vec<f64>,
}

impl MultivariateBaseline {
    pub fn new(k: usize) -> Self {
        Self {
            k,
            n: 0,
            mean: vec![0.0; k],
            m2: vec![0.0; k * k],
        }
    }

    pub fn dim(&self) -> usize {
        self.k
    }
    pub fn count(&self) -> u64 {
        self.n
    }
    pub fn mean(&self) -> &[f64] {
        &self.mean
    }

    /// Sample covariance (`M2 / (n-1)`). Returns a K×K row-major
    /// matrix of zeros if `n < 2`.
    pub fn sample_covariance(&self) -> Vec<f64> {
        if self.n < 2 {
            return vec![0.0; self.k * self.k];
        }
        let scale = 1.0 / (self.n - 1) as f64;
        self.m2.iter().map(|x| x * scale).collect()
    }

    /// Fold one K-dim observation into the running mean + scatter.
    pub fn observe(&mut self, x: &[f64]) {
        assert_eq!(x.len(), self.k, "observation dim mismatch");
        self.n += 1;
        let n = self.n as f64;
        // delta = x - mean
        let mut delta = vec![0.0; self.k];
        for i in 0..self.k {
            delta[i] = x[i] - self.mean[i];
        }
        for i in 0..self.k {
            self.mean[i] += delta[i] / n;
        }
        // m2 += (x - mean_new)(x - mean_old)^T  (Welford MV update)
        for i in 0..self.k {
            let dxi = x[i] - self.mean[i];
            for j in 0..self.k {
                let dxj_old = delta[j];
                let entry = at_mut(&mut self.m2, self.k, i, j);
                *entry += dxi * dxj_old;
            }
        }
    }

    /// Mahalanobis distance squared of `x` against the running
    /// baseline using Ledoit-Wolf-shrunk sample covariance.
    /// Returns `Err(Singular)` only if the shrunk covariance is
    /// itself singular (extremely rare, would mean every observation
    /// is identical AND k == 0).
    pub fn mahalanobis_squared(&self, x: &[f64]) -> Result<f64, MatrixError> {
        assert_eq!(x.len(), self.k, "observation dim mismatch");
        if self.n < 2 {
            return Ok(0.0);
        }
        let s = self.sample_covariance();
        let shrunk = ledoit_wolf_shrink(&s, self.k, self.n);
        let inv = invert(&shrunk, self.k)?;
        let mut diff = vec![0.0; self.k];
        for i in 0..self.k {
            diff[i] = x[i] - self.mean[i];
        }
        Ok(quadratic_form(&diff, &inv, self.k))
    }

    /// Convenience: square root of `mahalanobis_squared`.
    pub fn mahalanobis_distance(&self, x: &[f64]) -> Result<f64, MatrixError> {
        Ok(self.mahalanobis_squared(x)?.max(0.0).sqrt())
    }
}

// ── Ledoit-Wolf shrinkage ────────────────────────────────────────

/// Ledoit-Wolf shrinkage of a sample covariance matrix `s` toward
/// the identity-scaled target `μ I`, where `μ = trace(s) / k`.
///
/// Returns `Σ_shrunk = (1 - α) * s + α * μ * I` with α chosen
/// analytically to minimize MSE in the LW (2004) sense. v1 uses the
/// simplified closed form
///
/// ```text
///     α = min(d² / (d² + b²), 1)
/// ```
///
/// where `d² = ||s - μ I||²_F` and `b²` is bounded above by `||s||²_F /
/// n`. With small N this drives α toward 1 (heavily shrunk → identity-
/// like → invertible). With large N and a well-conditioned `s`, α drops
/// toward 0 (no shrinkage needed). This is the LW behavior the spec
/// calls for; the exact LW estimator additionally averages
/// per-observation cross-product variance, which v1 omits because we
/// don't keep the raw observation history (Welford is online-only).
pub fn ledoit_wolf_shrink(s: &[f64], k: usize, n: u64) -> Vec<f64> {
    if k == 0 {
        return Vec::new();
    }
    let mu = trace(s, k) / k as f64;
    // Build target μ * I.
    let mut target = identity(k);
    for entry in target.iter_mut() {
        *entry *= mu;
    }
    if n < 2 {
        // No samples → return target outright; the spec calls for a
        // heavily-shrunk estimator under the maternal-immunity period.
        return target;
    }
    // d² = ||s - target||²_F
    let mut d2 = 0.0;
    for i in 0..(k * k) {
        let diff = s[i] - target[i];
        d2 += diff * diff;
    }
    // b² ≤ ||s||²_F / n  — v1 upper-bound proxy that matches LW's
    // "average cross-product variance" intuition without retaining
    // the observation history.
    let b2 = frobenius_sq(s) / n as f64;
    // α = min(d² / (d² + b²), 1)
    let denom = d2 + b2;
    let alpha = if denom <= 0.0 {
        1.0
    } else {
        (d2 / denom).max(0.0).min(1.0)
    };
    // For small N relative to K, force α toward 1 — LW's
    // well-conditioned-when-N≤K guarantee:
    let alpha = if (n as usize) <= k { alpha.max(0.5) } else { alpha };

    let mut out = vec![0.0; k * k];
    for i in 0..(k * k) {
        out[i] = (1.0 - alpha) * s[i] + alpha * target[i];
    }
    out
}

// ── χ² critical values + trust-modulated thresholds ──────────────

/// χ²_p(k) — critical value of the chi-squared distribution at upper
/// tail probability `p` with `k` degrees of freedom.
///
/// v1 uses the Wilson-Hilferty approximation
///
/// ```text
///     χ²_p(k) ≈ k * (1 - 2/(9k) + z_p * sqrt(2/(9k)))³
/// ```
///
/// where `z_p` is the standard normal critical value at upper tail
/// `p`. Accuracy: <0.5% error for k ≥ 5 and 1e-5 ≤ p ≤ 0.5.
///
/// Exact entries for K=9 (the Spec 5 §5.5.3 target dimensionality):
///
/// ```text
///     p=0.01    → χ² = 21.666
///     p=0.001   → χ² = 27.877
///     p=0.0001  → χ² = 33.720
/// ```
pub fn chi_squared_critical(p: f64, k: usize) -> f64 {
    // Exact lookup for K=9 + the three Spec 5 §5.5.3 thresholds.
    if k == 9 {
        if (p - 0.01).abs() < 1e-9 {
            return 21.666;
        } else if (p - 0.001).abs() < 1e-9 {
            return 27.877;
        } else if (p - 0.0001).abs() < 1e-9 {
            return 33.720;
        }
    }
    let kf = k as f64;
    let z = inverse_normal_cdf(1.0 - p);
    let frac = 2.0 / (9.0 * kf);
    let inner = 1.0 - frac + z * frac.sqrt();
    kf * inner.powi(3)
}

/// Inverse normal CDF Φ⁻¹(q) via Abramowitz-Stegun (1964) 26.2.23
/// rational approximation. Accuracy: |error| < 4.5e-4 for 0 < q < 1.
/// This is the version popularized by Press et al. (Numerical
/// Recipes) and is plenty for our χ² approximation accuracy target
/// (well under the 0.5% error band the spec calls for).
fn inverse_normal_cdf(q: f64) -> f64 {
    // The approximation is one-sided (works on tail probability ≤ 0.5
    // and reflects). Pick the smaller-tail probability and remember
    // which tail to flip back to.
    let (p_tail, sign) = if q <= 0.5 { (q, -1.0) } else { (1.0 - q, 1.0) };
    let p_tail = p_tail.max(1e-300);
    let t = (-2.0 * p_tail.ln()).sqrt();
    let c0 = 2.515517;
    let c1 = 0.802853;
    let c2 = 0.010328;
    let d1 = 1.432788;
    let d2 = 0.189269;
    let d3 = 0.001308;
    let num = c0 + c1 * t + c2 * t * t;
    let den = 1.0 + d1 * t + d2 * t * t + d3 * t * t * t;
    sign * (t - num / den)
}

/// χ² critical value for a K-dim observation, modulated by an agent's
/// `TrustWeight` per Spec 5 §5.5.3.
pub fn trust_modulated_threshold(trust: &TrustWeight, k: usize) -> f64 {
    let p = if trust.value < 0.3 {
        0.01
    } else if trust.value < 0.8 {
        0.001
    } else {
        0.0001
    };
    chi_squared_critical(p, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn invert_identity_is_identity() {
        let i = identity(4);
        let inv = invert(&i, 4).unwrap();
        for r in 0..4 {
            for c in 0..4 {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!(approx(inv[r * 4 + c], want, 1e-12));
            }
        }
    }

    #[test]
    fn invert_known_2x2() {
        // A = [[4, 7], [2, 6]] → A^-1 = [[0.6, -0.7], [-0.2, 0.4]]
        let a = vec![4.0, 7.0, 2.0, 6.0];
        let inv = invert(&a, 2).unwrap();
        assert!(approx(inv[0], 0.6, 1e-9));
        assert!(approx(inv[1], -0.7, 1e-9));
        assert!(approx(inv[2], -0.2, 1e-9));
        assert!(approx(inv[3], 0.4, 1e-9));
    }

    #[test]
    fn invert_singular_matrix_errors() {
        // Rank-1 matrix → singular.
        let a = vec![1.0, 2.0, 2.0, 4.0];
        assert_eq!(invert(&a, 2), Err(MatrixError::Singular));
    }

    #[test]
    fn multivariate_welford_matches_textbook_means() {
        let mut b = MultivariateBaseline::new(2);
        // Two observations: (1, 2), (3, 4) → mean = (2, 3).
        b.observe(&[1.0, 2.0]);
        b.observe(&[3.0, 4.0]);
        assert!(approx(b.mean()[0], 2.0, 1e-12));
        assert!(approx(b.mean()[1], 3.0, 1e-12));
        // Sample covariance:
        //   var(x) = ((1-2)² + (3-2)²)/(n-1) = 2
        //   cov(x,y) = ((1-2)(2-3) + (3-2)(4-3))/(n-1) = 2
        //   var(y) = 2
        let cov = b.sample_covariance();
        assert!(approx(cov[0], 2.0, 1e-12));
        assert!(approx(cov[1], 2.0, 1e-12));
        assert!(approx(cov[2], 2.0, 1e-12));
        assert!(approx(cov[3], 2.0, 1e-12));
    }

    #[test]
    fn mahalanobis_one_dim_matches_z_squared() {
        // For K=1 with samples [10, 12, 14, 16, 18], sample variance
        // is 10. Mahalanobis² of x=20 from mean 14 is (6)² / 10 = 3.6.
        let mut b = MultivariateBaseline::new(1);
        for x in [10.0, 12.0, 14.0, 16.0, 18.0] {
            b.observe(&[x]);
        }
        let m2 = b.mahalanobis_squared(&[20.0]).unwrap();
        // Allow some slack for the LW shrinkage in the K=1 case
        // (with N=5 > K=1, α should be small).
        assert!(approx(m2, 3.6, 0.5), "m2 = {}", m2);
    }

    #[test]
    fn ledoit_wolf_shrinks_singular_sample_to_invertible() {
        // Build a degenerate sample covariance: rank 1 in 2D.
        let s = vec![4.0, 2.0, 2.0, 1.0]; // singular
        let shrunk = ledoit_wolf_shrink(&s, 2, 1);
        // After shrinkage with N <= K forced α ≥ 0.5, the matrix
        // becomes invertible.
        assert!(invert(&shrunk, 2).is_ok());
    }

    #[test]
    fn chi_squared_critical_matches_spec_5_table_for_k_9() {
        // Spec 5 §5.5.3 gives:
        //   p=0.01   → 21.67
        //   p=0.001  → 27.88
        //   p=0.0001 → 33.72
        assert!(approx(chi_squared_critical(0.01, 9), 21.67, 0.05));
        assert!(approx(chi_squared_critical(0.001, 9), 27.88, 0.05));
        assert!(approx(chi_squared_critical(0.0001, 9), 33.72, 0.05));
    }

    #[test]
    fn chi_squared_wilson_hilferty_approximation_reasonable() {
        // For K=5, p=0.05, the textbook χ²_0.05(5) = 11.07.
        let v = chi_squared_critical(0.05, 5);
        assert!(approx(v, 11.07, 0.5), "WH approximation gave {}", v);
        // For K=10, p=0.01, textbook χ²_0.01(10) = 23.21.
        let v = chi_squared_critical(0.01, 10);
        assert!(approx(v, 23.21, 0.5), "WH approximation gave {}", v);
    }

    #[test]
    fn trust_modulated_threshold_uses_correct_p_band() {
        let mut tw = TrustWeight::operator_provisioned();
        // tw.value starts at 0.1 — low trust → p=0.01.
        tw.value = 0.1;
        let low = trust_modulated_threshold(&tw, 9);
        assert!(approx(low, 21.67, 0.05));
        tw.value = 0.5;
        let mid = trust_modulated_threshold(&tw, 9);
        assert!(approx(mid, 27.88, 0.05));
        tw.value = 0.9;
        let high = trust_modulated_threshold(&tw, 9);
        assert!(approx(high, 33.72, 0.05));
        // Higher trust → higher threshold → fewer false positives.
        assert!(low < mid && mid < high);
    }

    #[test]
    fn mahalanobis_warmup_returns_zero() {
        // n < 2 short-circuits to 0 — no baseline yet.
        let b = MultivariateBaseline::new(3);
        assert_eq!(b.mahalanobis_squared(&[1.0, 2.0, 3.0]).unwrap(), 0.0);
    }
}
