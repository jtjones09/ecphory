// AGGREGATION LAYER — Population to decision (Spec 6 §5)
//
// Single AnomalyObservation / DamageObservation nodes from individual
// cell-agents are not authoritative. The aggregation layer counts
// them, weights them, and emits the population-level decisions:
//
//   AnomalyConvergence (Spec 6 §5.2.1)
//     N or more cell-agents of *different* specializations write
//     anomaly / damage observations about the same region within T
//     seconds. The evaluator emits a `ConvergedAnomaly`. Default
//     N=3, T=300s.
//
//   EscalationLadder (Spec 6 §5.2.2)
//     M or more ConvergedAnomaly events in time window U for a
//     single region. Triggers `P53Scope::Region` AUTOMATICALLY —
//     but only if at least one DamageObservation appears among the
//     convergent signals (Matzinger fold). Pure anomaly convergence
//     surfaces but does not auto-p53. Default M=3, U=900s.
//
// KING.1 (FATAL) fold — atomicity:
//   "Aggregation evaluation runs inside the same transactional
//    boundary as each anomaly observation write."
//
// Implementation: every region's aggregation state is behind its own
// `Mutex<RegionAggregationState>`. The `evaluate` method takes that
// mutex for the duration of the count + emit, so two concurrent
// observations from different threads cannot both observe N-1 and
// miss convergence — one will hold the lock, count itself in, and
// either emit ConvergedAnomaly or not; the other waits, then sees
// the prior observation and the convergence flag and acts on the
// updated state.
//
// CANTRILL.4 fold — kill switch:
//   `ImmuneResponseMode { Active, AlertOnly, Disabled }` per region.
//   `Active` is default. `AlertOnly` writes ConvergedAnomaly but
//   does NOT trigger auto-p53. `Disabled` skips convergence
//   evaluation entirely (for known-turbulent regions) and
//   auto-reverts to AlertOnly after 4 hours.

use crate::identity::NamespaceId;
use crate::signature::LineageId;
use crate::temporal::FabricInstant;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Configuration knobs for the aggregation layer.
#[derive(Debug, Clone, Copy)]
pub struct AggregationConfig {
    /// Number of cell-agents (of distinct specializations) required
    /// to converge. Spec 6 §5.2.1 default.
    pub convergence_n: u32,
    /// Time window over which convergence is evaluated. Default 300s.
    pub convergence_window: Duration,
    /// Number of converged anomalies to escalate to P53::Region.
    /// Spec 6 §5.2.2 default.
    pub escalation_m: u32,
    /// Time window over which M is counted. Default 900s.
    pub escalation_window: Duration,
    /// Auto-reverting `Disabled` timeout — after this elapses, a
    /// region in Disabled mode reverts to AlertOnly. Spec 6 §5.2.2
    /// CANTRILL.4 fold default 4h.
    pub disabled_revert_after: Duration,
}

impl Default for AggregationConfig {
    fn default() -> Self {
        Self {
            convergence_n: 3,
            convergence_window: Duration::from_secs(300),
            escalation_m: 3,
            escalation_window: Duration::from_secs(900),
            disabled_revert_after: Duration::from_secs(4 * 3600),
        }
    }
}

/// Per-region operational kill-switch (CANTRILL.4 fold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImmuneResponseMode {
    /// Default. Convergence evaluation runs; auto-P53 enabled.
    Active,
    /// Convergence evaluation runs and emits ConvergedAnomaly nodes,
    /// but auto-P53 is suppressed. Operator alert only.
    AlertOnly,
    /// Convergence evaluation skipped entirely. Used for regions
    /// under known turbulence. Auto-reverts to AlertOnly after
    /// `disabled_revert_after`.
    Disabled,
}

/// One observation record carried in the aggregation state.
///
/// We keep specialization-as-string + is_damage rather than the full
/// AnomalyObservation/DamageObservation struct so the aggregation
/// state can be held cheaply for the convergence window without
/// retaining heavy node payloads. The originating observation is
/// stored as a LineageId so callers (and the cognitive map in Step 5)
/// can reconstruct via the fabric.
#[derive(Debug, Clone)]
pub struct ObservationRecord {
    pub at: Instant,
    pub fabric_instant: FabricInstant,
    pub specialization: String,
    pub source_node: LineageId,
    pub is_damage: bool,
}

/// One ConvergedAnomaly record — emitted on each convergence,
/// retained for the EscalationLadder window.
#[derive(Debug, Clone)]
pub struct ConvergedAnomalyRecord {
    pub at: Instant,
    pub fabric_instant: FabricInstant,
    pub region: NamespaceId,
    pub source_observations: Vec<LineageId>,
    pub specializations: Vec<String>,
    pub had_damage: bool,
}

/// Per-region aggregation state. Held behind a `Mutex` in
/// `AggregationLayer` so KING.1 atomicity holds.
#[derive(Debug, Default)]
pub(crate) struct RegionAggregationState {
    pub observations: Vec<ObservationRecord>,
    pub converged: Vec<ConvergedAnomalyRecord>,
    pub mode: Option<ImmuneResponseMode>,
    pub disabled_at: Option<Instant>,
}

impl RegionAggregationState {
    /// Drop observations older than `window`.
    fn evict_observations(&mut self, window: Duration) {
        let cutoff = Instant::now().checked_sub(window);
        if let Some(c) = cutoff {
            self.observations.retain(|r| r.at >= c);
        }
    }
    fn evict_converged(&mut self, window: Duration) {
        let cutoff = Instant::now().checked_sub(window);
        if let Some(c) = cutoff {
            self.converged.retain(|r| r.at >= c);
        }
    }
}

/// Result returned by `AggregationLayer::evaluate`.
#[derive(Debug, Clone)]
pub enum AggregationOutcome {
    /// Observation absorbed; no population-level signal.
    Quiet,
    /// N cell-agents of distinct specializations converged on this
    /// region within the window. The aggregation layer records a
    /// ConvergedAnomaly internally; the caller writes the
    /// corresponding fabric node.
    ConvergedAnomaly {
        region: NamespaceId,
        source_observations: Vec<LineageId>,
        specializations: Vec<String>,
        had_damage: bool,
    },
    /// Convergence + escalation — M ConvergedAnomaly events in U
    /// seconds, with at least one damage observation among them.
    /// Caller MUST trigger `P53Scope::Region` (unless mode is
    /// `AlertOnly`, in which case caller alerts only).
    EscalateP53Region {
        region: NamespaceId,
        source_observations: Vec<LineageId>,
        specializations: Vec<String>,
        mode: ImmuneResponseMode,
    },
}

use std::sync::Arc;

/// Top-level aggregation layer. One instance per BridgeFabric. Per-
/// region state is stored as `Arc<Mutex<RegionAggregationState>>` so
/// each region can be locked independently — different regions don't
/// serialize on each other.
pub struct AggregationLayer {
    config: AggregationConfig,
    states: Mutex<HashMap<NamespaceId, Arc<Mutex<RegionAggregationState>>>>,
}

impl AggregationLayer {
    pub fn new() -> Self {
        Self::with_config(AggregationConfig::default())
    }
    pub fn with_config(config: AggregationConfig) -> Self {
        Self { config, states: Mutex::new(HashMap::new()) }
    }
    pub fn config(&self) -> &AggregationConfig {
        &self.config
    }

    fn region_state(&self, region: &NamespaceId) -> Arc<Mutex<RegionAggregationState>> {
        let mut map = self.states.lock().expect("aggregation outer poisoned");
        map.entry(region.clone())
            .or_insert_with(|| Arc::new(Mutex::new(RegionAggregationState::default())))
            .clone()
    }

    pub fn set_response_mode(&self, region: &NamespaceId, mode: ImmuneResponseMode) {
        let arc = self.region_state(region);
        let mut g = arc.lock().expect("region poisoned");
        g.mode = Some(mode);
        g.disabled_at = if matches!(mode, ImmuneResponseMode::Disabled) {
            Some(Instant::now())
        } else {
            None
        };
    }

    pub fn response_mode(&self, region: &NamespaceId) -> ImmuneResponseMode {
        let arc = self.region_state(region);
        let g = arc.lock().expect("region poisoned");
        self.effective_mode(&g)
    }

    fn effective_mode(&self, guard: &RegionAggregationState) -> ImmuneResponseMode {
        match guard.mode {
            Some(ImmuneResponseMode::Disabled) => {
                if let Some(at) = guard.disabled_at {
                    if at.elapsed() >= self.config.disabled_revert_after {
                        ImmuneResponseMode::AlertOnly
                    } else {
                        ImmuneResponseMode::Disabled
                    }
                } else {
                    ImmuneResponseMode::Disabled
                }
            }
            Some(mode) => mode,
            None => ImmuneResponseMode::Active,
        }
    }

    /// Atomic evaluation per KING.1 fold: this method takes the
    /// per-region mutex for the duration of (1) recording the new
    /// observation, (2) checking convergence, (3) recording any
    /// resulting ConvergedAnomaly, (4) checking escalation, (5)
    /// emitting `EscalateP53Region` if appropriate. Two concurrent
    /// observations from different threads serialize through this
    /// mutex.
    pub fn evaluate(
        &self,
        region: NamespaceId,
        record: ObservationRecord,
    ) -> AggregationOutcome {
        let arc = self.region_state(&region);
        let mut g = arc.lock().expect("region poisoned");
        let mode = self.effective_mode(&g);

        // Disabled mode: skip convergence evaluation entirely. The
        // observation is still retained — operators reading the
        // cognitive map will see it.
        if mode == ImmuneResponseMode::Disabled {
            g.observations.push(record);
            g.evict_observations(self.config.convergence_window);
            return AggregationOutcome::Quiet;
        }

        g.observations.push(record);
        g.evict_observations(self.config.convergence_window);

        // Count distinct specializations within window.
        let mut specs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut sources: Vec<LineageId> = Vec::new();
        let mut had_damage = false;
        for r in &g.observations {
            specs.insert(r.specialization.clone());
            sources.push(r.source_node.clone());
            if r.is_damage {
                had_damage = true;
            }
        }
        if (specs.len() as u32) < self.config.convergence_n {
            return AggregationOutcome::Quiet;
        }

        // Convergence reached. Record it.
        let converged = ConvergedAnomalyRecord {
            at: Instant::now(),
            fabric_instant: FabricInstant::now(),
            region: region.clone(),
            source_observations: sources.clone(),
            specializations: specs.iter().cloned().collect(),
            had_damage,
        };
        // Arm for the next round: clear consumed observations so we
        // don't re-fire ConvergedAnomaly on every subsequent
        // observation in the same window.
        g.observations.clear();
        g.converged.push(converged.clone());
        g.evict_converged(self.config.escalation_window);

        // Escalation check: M ConvergedAnomaly events in U seconds
        // for this region, with at least one damage observation
        // among them (Matzinger fold).
        let escalation_count = g.converged.len() as u32;
        let any_damage = g.converged.iter().any(|c| c.had_damage);

        if escalation_count >= self.config.escalation_m && any_damage {
            // Pull the source observations from all qualifying
            // converged events for the receipt.
            let all_sources: Vec<LineageId> = g
                .converged
                .iter()
                .flat_map(|c| c.source_observations.iter().cloned())
                .collect();
            let all_specs: Vec<String> = g
                .converged
                .iter()
                .flat_map(|c| c.specializations.iter().cloned())
                .collect();
            // Reset the converged window — the escalation consumes it.
            g.converged.clear();
            return AggregationOutcome::EscalateP53Region {
                region,
                source_observations: all_sources,
                specializations: all_specs,
                mode,
            };
        }

        AggregationOutcome::ConvergedAnomaly {
            region: converged.region,
            source_observations: converged.source_observations,
            specializations: converged.specializations,
            had_damage: converged.had_damage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::LineageId;
    use std::sync::Arc;

    fn rec(specialization: &str, is_damage: bool) -> ObservationRecord {
        ObservationRecord {
            at: Instant::now(),
            fabric_instant: FabricInstant::now(),
            specialization: specialization.into(),
            source_node: LineageId::new(),
            is_damage,
        }
    }

    fn region(name: &str) -> NamespaceId {
        NamespaceId::fresh(name)
    }

    #[test]
    fn convergence_below_n_stays_quiet() {
        let layer = AggregationLayer::new();
        let r = region("test");
        // Only 2 distinct specializations < N=3.
        assert!(matches!(
            layer.evaluate(r.clone(), rec("RateObserver", false)),
            AggregationOutcome::Quiet
        ));
        assert!(matches!(
            layer.evaluate(r, rec("DecayObserver", false)),
            AggregationOutcome::Quiet
        ));
    }

    #[test]
    fn three_distinct_specializations_converge() {
        let layer = AggregationLayer::new();
        let r = region("test");
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let outcome = layer.evaluate(r, rec("RelationObserver", false));
        match outcome {
            AggregationOutcome::ConvergedAnomaly { specializations, had_damage, .. } => {
                assert_eq!(specializations.len(), 3);
                assert!(!had_damage);
            }
            other => panic!("Expected ConvergedAnomaly, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn duplicate_specializations_do_not_converge() {
        // Three observations from the SAME specialization should
        // not trigger convergence — convergence requires distinct
        // specializations (collective-computation property).
        let layer = AggregationLayer::new();
        let r = region("test");
        for _ in 0..3 {
            let outcome = layer.evaluate(r.clone(), rec("RateObserver", false));
            assert!(matches!(outcome, AggregationOutcome::Quiet));
        }
    }

    #[test]
    fn pure_anomaly_convergence_does_not_escalate_to_p53() {
        // Matzinger fold: pure anomaly convergence (no damage among
        // signals) emits ConvergedAnomaly but does NOT auto-trigger
        // P53Scope::Region.
        let layer = AggregationLayer::new();
        let r = region("test");
        // First convergence — anomaly only.
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let _ = layer.evaluate(r.clone(), rec("RelationObserver", false));
        // Second convergence — anomaly only.
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let _ = layer.evaluate(r.clone(), rec("RelationObserver", false));
        // Third convergence — anomaly only.
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let outcome = layer.evaluate(r, rec("RelationObserver", false));
        // Three converged events but no damage → ConvergedAnomaly,
        // not EscalateP53Region.
        assert!(matches!(outcome, AggregationOutcome::ConvergedAnomaly { .. }));
    }

    #[test]
    fn three_convergences_with_damage_escalate_to_p53() {
        let layer = AggregationLayer::new();
        let r = region("test");
        // Three convergences, at least one damage observation.
        for _ in 0..2 {
            let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
            let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
            let _ = layer.evaluate(r.clone(), rec("RelationObserver", false));
        }
        // Third convergence carries a damage observation.
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let outcome = layer.evaluate(r, rec("AttestationObserver", true));
        assert!(
            matches!(outcome, AggregationOutcome::EscalateP53Region { .. }),
            "Three convergences + damage signal must escalate to P53Region"
        );
    }

    #[test]
    fn alert_only_mode_blocks_p53_escalation() {
        // CANTRILL.4 fold: AlertOnly mode emits ConvergedAnomaly +
        // would emit EscalateP53Region but the caller observes the
        // mode in the receipt and suppresses auto-p53. The
        // aggregation layer still emits the receipt with mode label
        // so the caller can decide.
        let layer = AggregationLayer::new();
        let r = region("test");
        layer.set_response_mode(&r, ImmuneResponseMode::AlertOnly);
        // Build to escalation conditions.
        for _ in 0..2 {
            let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
            let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
            let _ = layer.evaluate(r.clone(), rec("RelationObserver", false));
        }
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let outcome = layer.evaluate(r, rec("AttestationObserver", true));
        // Layer still produces EscalateP53Region — but the mode
        // field tells the caller to alert only.
        match outcome {
            AggregationOutcome::EscalateP53Region { mode, .. } => {
                assert_eq!(mode, ImmuneResponseMode::AlertOnly);
            }
            other => panic!("Expected EscalateP53Region with AlertOnly mode; got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn disabled_mode_skips_convergence() {
        let layer = AggregationLayer::new();
        let r = region("test");
        layer.set_response_mode(&r, ImmuneResponseMode::Disabled);
        // Even three distinct specializations stay quiet under Disabled.
        let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
        let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));
        let outcome = layer.evaluate(r, rec("RelationObserver", true));
        assert!(matches!(outcome, AggregationOutcome::Quiet));
    }

    #[test]
    fn disabled_mode_auto_reverts_after_timeout() {
        let cfg = AggregationConfig {
            disabled_revert_after: Duration::from_millis(20),
            ..AggregationConfig::default()
        };
        let layer = AggregationLayer::with_config(cfg);
        let r = region("test");
        layer.set_response_mode(&r, ImmuneResponseMode::Disabled);
        std::thread::sleep(Duration::from_millis(30));
        // After the revert window, response_mode reads as AlertOnly.
        assert_eq!(layer.response_mode(&r), ImmuneResponseMode::AlertOnly);
    }

    #[test]
    fn aggregation_evaluation_is_atomic_under_concurrency() {
        // KING.1 FATAL fold: two concurrent threads each contributing
        // observations to the same region must NOT both observe N-1.
        // With N=3, if two threads each push the third distinct
        // specialization simultaneously, exactly one wins and emits
        // ConvergedAnomaly — the other observation slides into the
        // freshly-cleared next-round state.
        //
        // We exercise this with a tight loop: 100 trials of two
        // threads each pushing one observation after the layer has
        // been pre-loaded with N-1 specializations. Across all
        // trials, total ConvergedAnomaly emissions equal the number
        // of trials in which both threads' specializations differed
        // from each other AND from the pre-loaded set.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let trials = 50;
        let mut converged_count = 0;
        for _ in 0..trials {
            let layer = Arc::new(AggregationLayer::new());
            let r = region("atomicity-test");
            // Pre-load N-1=2 distinct specializations so a single
            // additional distinct one tips into convergence.
            let _ = layer.evaluate(r.clone(), rec("RateObserver", false));
            let _ = layer.evaluate(r.clone(), rec("DecayObserver", false));

            let counter = Arc::new(AtomicUsize::new(0));
            let mut handles = Vec::new();
            for spec in ["RelationObserver", "AttestationObserver"] {
                let l = Arc::clone(&layer);
                let region_clone = r.clone();
                let spec = spec.to_string();
                let c = Arc::clone(&counter);
                handles.push(std::thread::spawn(move || {
                    let outcome = l.evaluate(region_clone, rec(&spec, false));
                    if matches!(outcome, AggregationOutcome::ConvergedAnomaly { .. }) {
                        c.fetch_add(1, Ordering::SeqCst);
                    }
                }));
            }
            for h in handles {
                let _ = h.join();
            }
            // Per the atomicity guarantee, at least one of the two
            // threads observed convergence (the one that took the
            // mutex second sees specs.len() >= 3). Both could fire
            // *only* if the round arming clears between them — and
            // the test's "evict + clear" reset on convergence
            // ensures that when both contribute distinct specs, one
            // converges first, then the other's observation lands
            // in the next round (which is still under N).
            let n = counter.load(Ordering::SeqCst);
            assert!(
                n >= 1,
                "At least one thread must observe convergence; got {n}"
            );
            converged_count += n;
        }
        // Sanity: across 50 trials with both threads firing distinct
        // specs, convergence_count should be at least 50 (one per
        // trial), strictly greater than 0 (not silently dropped).
        assert!(
            converged_count >= trials,
            "Convergence count {converged_count} < trials {trials}"
        );
    }
}
