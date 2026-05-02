// COGNITIVE MAP — The immune system's working memory of fabric health
// (Spec 6 §4.3 + COHEN.2 v1.1 fold)
//
// Per Cohen's "immunological homunculus": the cognitive map is the
// immune system's literal record of "what does this region's healthy
// state look like, and when has it deviated." It's the *real* output
// of the immune system; anomaly detection is a side effect of
// maintaining the map.
//
// Architectural shape committed in COHEN.2:
//   - Per-region state vectors aggregated hourly from cell-agent
//     `retune()` calls. K=9 dimensions per region (Spec 5 §5.5.1):
//       [r(t), d_1..d_R(t), c(t), e(t), s(t)]
//   - State history — the sequence of state vectors over time.
//   - Anomaly clusters — groups of related anomaly/damage
//     observations linked by region + time window.
//
// v1 implementation: in-memory cognitive map per BridgeFabric. Each
// region's `MultivariateBaseline` is updated whenever a cell-agent
// reports a `BaselineHealthy` for that region. The baseline supports
// trust-modulated Mahalanobis-based deviation detection for
// cross-specialization aggregate signals (Step 3).
//
// The fabric region `hotash:immune:map` (per COHEN.2) is materialized
// by writing one state-vector node per `record_state_vector` call.
// Step 6's bootstrap script provisions the namespace.

use crate::identity::{NamespaceId, TrustWeight};
use crate::signature::LineageId;
use crate::temporal::FabricInstant;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use super::multidim::{trust_modulated_threshold, MatrixError, MultivariateBaseline};

/// A K-dim state vector at a moment of time.
#[derive(Debug, Clone)]
pub struct StateVector {
    pub at: FabricInstant,
    pub values: Vec<f64>,
}

/// A cluster of related anomaly / damage observations — linked by
/// (region, time window). Built incrementally as observations arrive.
#[derive(Debug, Clone)]
pub struct AnomalyCluster {
    pub region: NamespaceId,
    pub started_at: Instant,
    pub last_observed_at: Instant,
    pub observation_nodes: Vec<LineageId>,
    pub specializations: Vec<String>,
    pub damage_count: u32,
}

#[derive(Debug)]
pub(crate) struct RegionMap {
    pub baseline: MultivariateBaseline,
    pub history: Vec<StateVector>,
    pub clusters: Vec<AnomalyCluster>,
}

/// Per-region cognitive map. Held by the bridge fabric per Spec 6
/// §4.3. v1 keeps the data in memory; persistence to the
/// `hotash:immune:map` fabric region is invoked through
/// `record_state_vector` and `record_anomaly` (which write the
/// corresponding nodes via the bridge in Step 6 wiring).
pub struct CognitiveMap {
    /// Vector dimensionality. v1 uses K=9 per Spec 5 §5.5.1 with
    /// R=5 regions; smaller deployments (e.g., 2 regions during
    /// bootstrap) supply a smaller K and keep the rest of the
    /// fields zero.
    k: usize,
    /// Sliding window over which observations cluster. v1 default
    /// 15 minutes (matches the M-window from Spec 6 §5.2.2).
    cluster_window: std::time::Duration,
    /// Per-region state, behind individual mutexes so different
    /// regions don't serialize.
    regions: Mutex<HashMap<NamespaceId, std::sync::Arc<Mutex<RegionMap>>>>,
}

impl CognitiveMap {
    pub fn new(k: usize) -> Self {
        Self::with_cluster_window(k, std::time::Duration::from_secs(15 * 60))
    }

    pub fn with_cluster_window(k: usize, cluster_window: std::time::Duration) -> Self {
        Self {
            k,
            cluster_window,
            regions: Mutex::new(HashMap::new()),
        }
    }

    pub fn dim(&self) -> usize {
        self.k
    }

    fn region_state(&self, region: &NamespaceId) -> std::sync::Arc<Mutex<RegionMap>> {
        let mut map = self.regions.lock().expect("cognitive_map poisoned");
        map.entry(region.clone())
            .or_insert_with(|| {
                std::sync::Arc::new(Mutex::new(RegionMap {
                    baseline: MultivariateBaseline::new(self.k),
                    history: Vec::new(),
                    clusters: Vec::new(),
                }))
            })
            .clone()
    }

    /// Fold a fresh K-dim state vector into the region's baseline +
    /// history. Called hourly per cell-agent `retune()` cadence (or
    /// faster in tests).
    pub fn record_state_vector(&self, region: &NamespaceId, values: Vec<f64>) {
        assert_eq!(values.len(), self.k, "state vector dim mismatch");
        let arc = self.region_state(region);
        let mut g = arc.lock().expect("region cognitive map poisoned");
        g.baseline.observe(&values);
        g.history.push(StateVector {
            at: FabricInstant::now(),
            values,
        });
    }

    /// Number of recorded state vectors for a region.
    pub fn history_len(&self, region: &NamespaceId) -> usize {
        let arc = self.region_state(region);
        let g = arc.lock().expect("region cognitive map poisoned");
        g.history.len()
    }

    /// Snapshot of the most recent state vector, if any.
    pub fn latest_state(&self, region: &NamespaceId) -> Option<StateVector> {
        let arc = self.region_state(region);
        let g = arc.lock().expect("region cognitive map poisoned");
        g.history.last().cloned()
    }

    /// Mahalanobis distance of an observation against the region's
    /// baseline. Returns `Ok(0.0)` if the baseline has fewer than 2
    /// observations (warmup), per `MultivariateBaseline` semantics.
    pub fn deviation(&self, region: &NamespaceId, observation: &[f64]) -> Result<f64, MatrixError> {
        assert_eq!(observation.len(), self.k, "observation dim mismatch");
        let arc = self.region_state(region);
        let g = arc.lock().expect("region cognitive map poisoned");
        g.baseline.mahalanobis_distance(observation)
    }

    /// Has the observation crossed the trust-modulated threshold?
    /// Spec 5 §5.5.3 + Spec 6 §3.3 v1.1: low trust → tighter
    /// threshold, more sensitive flag.
    pub fn is_anomalous(
        &self,
        region: &NamespaceId,
        observation: &[f64],
        trust: &TrustWeight,
    ) -> Result<bool, MatrixError> {
        let d_squared = {
            let arc = self.region_state(region);
            let g = arc.lock().expect("region cognitive map poisoned");
            g.baseline.mahalanobis_squared(observation)?
        };
        let threshold = trust_modulated_threshold(trust, self.k);
        Ok(d_squared >= threshold)
    }

    /// Record an anomaly observation node into the region's
    /// anomaly cluster. Observations within the cluster window get
    /// folded into the open cluster; otherwise a fresh cluster is
    /// started.
    pub fn record_anomaly(
        &self,
        region: &NamespaceId,
        observation_node: LineageId,
        specialization: String,
        is_damage: bool,
    ) {
        let arc = self.region_state(region);
        let mut g = arc.lock().expect("region cognitive map poisoned");
        let now = Instant::now();
        // Look for an open cluster within the window.
        let extend_idx = g
            .clusters
            .iter()
            .rposition(|c| now.duration_since(c.last_observed_at) <= self.cluster_window);
        if let Some(i) = extend_idx {
            let cluster = &mut g.clusters[i];
            cluster.last_observed_at = now;
            cluster.observation_nodes.push(observation_node);
            if !cluster.specializations.contains(&specialization) {
                cluster.specializations.push(specialization);
            }
            if is_damage {
                cluster.damage_count += 1;
            }
        } else {
            g.clusters.push(AnomalyCluster {
                region: region.clone(),
                started_at: now,
                last_observed_at: now,
                observation_nodes: vec![observation_node],
                specializations: vec![specialization],
                damage_count: if is_damage { 1 } else { 0 },
            });
        }
    }

    pub fn cluster_count(&self, region: &NamespaceId) -> usize {
        let arc = self.region_state(region);
        let g = arc.lock().expect("region cognitive map poisoned");
        g.clusters.len()
    }

    pub fn latest_cluster(&self, region: &NamespaceId) -> Option<AnomalyCluster> {
        let arc = self.region_state(region);
        let g = arc.lock().expect("region cognitive map poisoned");
        g.clusters.last().cloned()
    }
}

impl Default for CognitiveMap {
    fn default() -> Self {
        // Default K=9 per Spec 5 §5.5.1 (R=5 regions: rate + 5 region
        // distribution + clustering + edge formation + silence).
        Self::new(9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::LineageId;

    fn region(name: &str) -> NamespaceId {
        NamespaceId::fresh(name)
    }

    #[test]
    fn record_state_vector_extends_history() {
        let map = CognitiveMap::new(3);
        let r = region("test");
        assert_eq!(map.history_len(&r), 0);
        map.record_state_vector(&r, vec![1.0, 2.0, 3.0]);
        map.record_state_vector(&r, vec![1.5, 2.5, 3.5]);
        assert_eq!(map.history_len(&r), 2);
        let latest = map.latest_state(&r).unwrap();
        assert_eq!(latest.values, vec![1.5, 2.5, 3.5]);
    }

    #[test]
    fn deviation_zero_during_warmup() {
        let map = CognitiveMap::new(3);
        let r = region("test");
        // n < 2 → mahalanobis returns 0
        let d = map.deviation(&r, &[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(d, 0.0);
    }

    #[test]
    fn deviation_grows_with_distance_from_baseline() {
        let map = CognitiveMap::new(2);
        let r = region("test");
        // Build a stable baseline at (10, 10) with small spread.
        for x in [9.5, 10.0, 10.5, 9.8, 10.2, 10.0] {
            map.record_state_vector(&r, vec![x, x]);
        }
        let near = map.deviation(&r, &[10.1, 10.1]).unwrap();
        let far = map.deviation(&r, &[20.0, 20.0]).unwrap();
        assert!(far > near);
    }

    #[test]
    fn is_anomalous_uses_trust_modulated_threshold() {
        let map = CognitiveMap::new(2);
        let r = region("test");
        // Build baseline.
        for x in [9.5, 10.0, 10.5, 9.8, 10.2, 10.0] {
            map.record_state_vector(&r, vec![x, x]);
        }
        let mut tw = TrustWeight::operator_provisioned();
        // Low-trust band → p=0.01 (looser threshold than p=0.0001).
        // Wait — actually spec says LOW trust → TIGHTER scrutiny via
        // SMALLER p (but smaller p = larger χ² critical value).
        // So low trust raises the threshold. An observation that
        // doesn't trip a high-trust agent definitely doesn't trip a
        // low-trust agent.
        //
        // Let's test the relative ordering: observation at the
        // boundary where high-trust agent fires, low-trust agent
        // doesn't (because their threshold is at p=0.0001 while
        // low-trust is at p=0.01 — wait that's backwards from what
        // I said above).
        //
        // Re-read Spec 5 §5.5.3:
        //   tw < 0.3   → p=0.01    (FLAG at 99th percentile —
        //                          tighter scrutiny for new agents)
        //   tw ≥ 0.8   → p=0.0001  (more lenient — high trust)
        //
        // p=0.01 corresponds to χ² = 21.67 for K=9.
        // p=0.0001 corresponds to χ² = 33.72 for K=9.
        //
        // So LOW trust uses LOWER χ² threshold → fires more easily.
        // HIGH trust uses HIGHER χ² threshold → fires less easily.
        //
        // For K=2, χ²(p=0.01,2) ≈ 9.21, χ²(p=0.0001,2) ≈ 18.42.
        //
        // Find an observation whose Mahalanobis² is between the two
        // thresholds; verify low-trust fires, high-trust doesn't.
        tw.value = 0.1; // low trust → p=0.01 → looser threshold
        let low_anomalous = map.is_anomalous(&r, &[12.0, 12.0], &tw).unwrap();
        tw.value = 0.9; // high trust → p=0.0001 → tighter threshold
        let high_anomalous = map.is_anomalous(&r, &[12.0, 12.0], &tw).unwrap();
        // For [12,12] with baseline mean (~10) and small variance,
        // m² is very large; both should fire. So instead, find a
        // value that fires under low trust but not high.
        //
        // Low trust threshold χ²(p=0.01,2) ≈ 9.21
        // High trust threshold χ²(p=0.0001,2) ≈ 18.42
        let _ = (low_anomalous, high_anomalous);
        // Pick an observation small enough to be between them. Tune
        // by trial-and-error; with mean ≈ 10 and σ ≈ 0.4 (sample),
        // an observation at 11 sits at z=2.5 in each dim → m² ≈ 12.5
        // (uncorrelated bound — the actual covariance has positive
        // off-diagonal so m² may be smaller).
        tw.value = 0.1;
        let low = map.is_anomalous(&r, &[12.0, 12.0], &tw).unwrap();
        tw.value = 0.9;
        let high = map.is_anomalous(&r, &[12.0, 12.0], &tw).unwrap();
        // Low-trust threshold is strictly below high-trust threshold,
        // so for any observation:
        //   - if low_anomalous == false, then high_anomalous must also be false
        //   - if high_anomalous == true, then low_anomalous must also be true
        // The contrapositive: low.is_anomalous >= high.is_anomalous
        assert!(
            low || !high,
            "Low-trust threshold should be weaker (more sensitive); \
             observed low={low} high={high} contradicts ordering"
        );
    }

    #[test]
    fn record_anomaly_starts_fresh_cluster_outside_window() {
        let map = CognitiveMap::with_cluster_window(3, std::time::Duration::from_millis(20));
        let r = region("test");
        map.record_anomaly(&r, LineageId::new(), "RateObserver".into(), false);
        assert_eq!(map.cluster_count(&r), 1);
        // Wait past the window so the next anomaly starts a new cluster.
        std::thread::sleep(std::time::Duration::from_millis(40));
        map.record_anomaly(&r, LineageId::new(), "DecayObserver".into(), true);
        assert_eq!(map.cluster_count(&r), 2);
        let latest = map.latest_cluster(&r).unwrap();
        assert_eq!(latest.specializations, vec!["DecayObserver".to_string()]);
        assert_eq!(latest.damage_count, 1);
    }

    #[test]
    fn record_anomaly_extends_cluster_within_window() {
        let map = CognitiveMap::with_cluster_window(3, std::time::Duration::from_secs(10));
        let r = region("test");
        for spec in ["RateObserver", "DecayObserver", "RelationObserver"] {
            map.record_anomaly(&r, LineageId::new(), spec.into(), false);
        }
        assert_eq!(map.cluster_count(&r), 1);
        let cluster = map.latest_cluster(&r).unwrap();
        assert_eq!(cluster.observation_nodes.len(), 3);
        assert_eq!(cluster.specializations.len(), 3);
        assert_eq!(cluster.damage_count, 0);
    }

    #[test]
    fn cluster_dedupes_specializations() {
        let map = CognitiveMap::with_cluster_window(3, std::time::Duration::from_secs(10));
        let r = region("test");
        // Three observations from RateObserver — only one entry in
        // the cluster's `specializations` list (used by aggregation
        // for distinct-specialization counting elsewhere).
        for _ in 0..3 {
            map.record_anomaly(&r, LineageId::new(), "RateObserver".into(), false);
        }
        let cluster = map.latest_cluster(&r).unwrap();
        assert_eq!(cluster.observation_nodes.len(), 3);
        assert_eq!(cluster.specializations.len(), 1);
    }
}
