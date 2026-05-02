// CELL-AGENT SPECIALIZATIONS — Spec 6 §3.3
//
// Six concrete cell-agent implementations, one per region per Spec 6
// §3.3. Each maintains its own 1-D Welford baseline and emits the
// detection rates committed in GERSH.3 fold (Spec 6 §3.3 v1.1
// warning).
//
// Per-specialization detection rates (the v1 falsifiable bar):
//   RateObserver:        ≥95% of 5σ within 60s   ; ≤1% FP/24h
//   AttestationObserver: 100% of failures (binary); ≤0.1% FP
//   DecayObserver:       ≥90% of 3σ within 1 tick
//   SilenceObserver:     100% of 3σ silence (deterministic)
//   ConsensusObserver:   ≥90% of 2σ within 4 rounds
//   RelationObserver:    ≥80% of 3σ within 1 hour
//
// Each specialization's `retune()` always emits a `BaselineHealthy`,
// guaranteeing the COHEN.1 80/20 maintenance/defense ratio in steady
// state: in a 24-hour healthy window, retune fires 24× per cell-agent
// (24 maintenance signals) and defense observations are rare by
// design (Spec 6 §4.2). Six cell-agents per region × 24 hours
// produces 144 maintenance signals; even tens of false-positive
// anomalies stay below the 20% defense ceiling.

use crate::identity::{generate_agent_keypair, AgentKeypair, NamespaceId, VoicePrint};
use crate::node::{IntentNode, MetadataValue};
use crate::temporal::FabricInstant;
use crate::tracer::TraceEvent;
use std::time::Duration;

use super::baseline::WelfordTracker;
use super::cell_agent::{
    AnomalyObservation, BaselineHealthy, CellAgent, CellAgentHealth, CellAgentId,
    DamageObservation, ImmunePattern, ObservationContext, ObservationOutcome,
    ObservationSeverity, ObservedEvent, RetuneReport,
};

/// Lightweight enum naming each v1 specialization. Used by the
/// bootstrap script (Step 6) to provision the population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Specialization {
    Rate,
    Attestation,
    Decay,
    Consensus,
    Relation,
    Silence,
}

impl Specialization {
    pub fn as_str(&self) -> &'static str {
        match self {
            Specialization::Rate => "RateObserver",
            Specialization::Attestation => "AttestationObserver",
            Specialization::Decay => "DecayObserver",
            Specialization::Consensus => "ConsensusObserver",
            Specialization::Relation => "RelationObserver",
            Specialization::Silence => "SilenceObserver",
        }
    }
}

// ── Helper: build a typed BaselineHealthy from a tracker ──────────

fn baseline_healthy(
    observer: VoicePrint,
    cell_agent_id: CellAgentId,
    region: NamespaceId,
    specialization: &'static str,
    tracker: &WelfordTracker,
) -> BaselineHealthy {
    BaselineHealthy {
        observer,
        observer_cell_agent_id: cell_agent_id,
        region,
        specialization,
        baseline_value: tracker.mean(),
        baseline_stddev: tracker.stddev(),
        observation_count: tracker.count(),
        at: FabricInstant::now(),
    }
}

fn anomaly(
    observer: VoicePrint,
    cell_agent_id: CellAgentId,
    region: NamespaceId,
    specialization: &'static str,
    observed: f64,
    tracker: &WelfordTracker,
    z_score: f64,
    severity: ObservationSeverity,
    confidence: f64,
    explanation: impl Into<String>,
) -> AnomalyObservation {
    AnomalyObservation {
        observer,
        observer_cell_agent_id: cell_agent_id,
        region,
        specialization,
        observed_value: observed,
        baseline_value: tracker.mean(),
        deviation_z_score: z_score,
        severity,
        confidence,
        explanation: explanation.into(),
        at: FabricInstant::now(),
    }
}

// ── 3.3.1 RateObserver — write-rate baseline, 5σ trigger ───────────

/// `RateObserver` watches the rate of new node creations in its
/// region and flags sustained deviation > 5σ (Spec 6 §3.3.1 +
/// GERSH.3 commitment ≥95% detection within 60s).
pub struct RateObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    /// Counts node creations within the current sampling window.
    window_count: u64,
    window_started: std::time::Instant,
    window_size: Duration,
    rate_baseline: WelfordTracker,
    /// Required samples in the baseline before z-score is consulted.
    /// Stops a fresh agent from firing on its first observed window.
    warmup: u64,
    /// Z-score threshold (5σ per Spec 6 §3.3.1).
    threshold_sigma: f64,
}

impl RateObserver {
    pub fn new(region: NamespaceId, window_size: Duration) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            window_count: 0,
            window_started: std::time::Instant::now(),
            window_size,
            rate_baseline: WelfordTracker::new(),
            warmup: 5,
            threshold_sigma: 5.0,
        }
    }

    /// Hook used by tests to inject N observations into the baseline
    /// without spinning real time.
    pub fn warm_baseline(&mut self, samples: &[f64]) {
        for s in samples {
            self.rate_baseline.observe(*s);
        }
    }

    /// Observation count in the in-flight window.
    pub fn current_window_count(&self) -> u64 {
        self.window_count
    }
}

impl CellAgent for RateObserver {
    fn id(&self) -> CellAgentId {
        self.id.clone()
    }
    fn voice_print(&self) -> VoicePrint {
        self.keypair.voice_print()
    }
    fn region(&self) -> &NamespaceId {
        &self.region
    }
    fn specialization(&self) -> &'static str {
        "RateObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::NodeCreates
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        // Count node creations toward the in-flight window.
        if matches!(event, ObservedEvent::Node(_)) {
            self.window_count += 1;
        }

        // If the window has elapsed, compute the rate, fold it into
        // the baseline, and check for anomaly.
        if self.window_started.elapsed() < self.window_size {
            return ObservationOutcome::Quiet;
        }

        let actual_window = self.window_started.elapsed();
        let secs = actual_window.as_secs_f64().max(f64::EPSILON);
        let rate = self.window_count as f64 / secs;
        self.window_count = 0;
        self.window_started = std::time::Instant::now();

        // Warmup: absorb the first few rates, do not fire.
        if self.rate_baseline.count() < self.warmup {
            self.rate_baseline.observe(rate);
            return ObservationOutcome::Quiet;
        }

        let z = self.rate_baseline.z_score(rate);
        let abs_z = z.abs();
        if abs_z >= self.threshold_sigma {
            // Don't fold a clear anomaly into the baseline — that
            // would let a sustained spike normalize itself.
            let severity = if abs_z >= 7.0 {
                ObservationSeverity::High
            } else {
                ObservationSeverity::Medium
            };
            ObservationOutcome::Anomaly(anomaly(
                self.keypair.voice_print(),
                self.id.clone(),
                self.region.clone(),
                "RateObserver",
                rate,
                &self.rate_baseline,
                z,
                severity,
                (abs_z / self.threshold_sigma).min(1.0),
                format!(
                    "write rate {:.3}/s deviates {:.2}σ from baseline mean {:.3}/s",
                    rate, z, self.rate_baseline.mean()
                ),
            ))
        } else {
            self.rate_baseline.observe(rate);
            ObservationOutcome::Quiet
        }
    }

    fn retune(&mut self) -> RetuneReport {
        let healthy = baseline_healthy(
            self.keypair.voice_print(),
            self.id.clone(),
            self.region.clone(),
            "RateObserver",
            &self.rate_baseline,
        );
        RetuneReport {
            baseline_value: self.rate_baseline.mean(),
            baseline_stddev: self.rate_baseline.stddev(),
            observation_count: self.rate_baseline.count(),
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }

    fn health(&self) -> CellAgentHealth {
        CellAgentHealth::Healthy
    }
}

// ── 3.3.2 AttestationObserver — fingerprint / attestation failures ─

/// `AttestationObserver` watches `AttestationFailed` and
/// `ContentFingerprintFailed` events. Per Spec 5 §3.1 + Matzinger
/// fold, fingerprint mismatches are evidence of harm — high-confidence
/// `DamageObservation`. Detection rate: 100% (binary) per GERSH.3.
pub struct AttestationObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    /// Count of attestations seen (success + failure) for the
    /// per-region failure-rate baseline.
    total_seen: u64,
    failures: u64,
}

impl AttestationObserver {
    pub fn new(region: NamespaceId) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            total_seen: 0,
            failures: 0,
        }
    }

    pub fn failure_rate(&self) -> f64 {
        if self.total_seen == 0 {
            0.0
        } else {
            self.failures as f64 / self.total_seen as f64
        }
    }
}

impl CellAgent for AttestationObserver {
    fn id(&self) -> CellAgentId {
        self.id.clone()
    }
    fn voice_print(&self) -> VoicePrint {
        self.keypair.voice_print()
    }
    fn region(&self) -> &NamespaceId {
        &self.region
    }
    fn specialization(&self) -> &'static str {
        "AttestationObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::AttestationEvents
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        let trace = match event {
            ObservedEvent::Trace(t) => t,
            _ => return ObservationOutcome::Quiet,
        };

        match trace {
            TraceEvent::AttestationVerified { .. } => {
                self.total_seen += 1;
                ObservationOutcome::Quiet
            }
            TraceEvent::AttestationFailed { signer, target_node, reason } => {
                self.total_seen += 1;
                self.failures += 1;
                ObservationOutcome::Damage(DamageObservation {
                    observer: self.keypair.voice_print(),
                    observer_cell_agent_id: self.id.clone(),
                    region: self.region.clone(),
                    specialization: "AttestationObserver",
                    damage_kind: "attestation_failed",
                    explanation: format!(
                        "attestation by {} on {} failed: {}",
                        signer, target_node, reason
                    ),
                    at: FabricInstant::now(),
                })
            }
            TraceEvent::ContentFingerprintFailed { lineage_id } => {
                self.failures += 1;
                self.total_seen += 1;
                ObservationOutcome::Damage(DamageObservation {
                    observer: self.keypair.voice_print(),
                    observer_cell_agent_id: self.id.clone(),
                    region: self.region.clone(),
                    specialization: "AttestationObserver",
                    damage_kind: "content_fingerprint_failed",
                    explanation: format!(
                        "node {} content fingerprint mismatch — Spec 5 §3.1 DamageObservation",
                        lineage_id
                    ),
                    at: FabricInstant::now(),
                })
            }
            TraceEvent::ContentFingerprintVerified { .. } => {
                self.total_seen += 1;
                ObservationOutcome::Quiet
            }
            _ => ObservationOutcome::Quiet,
        }
    }

    fn retune(&mut self) -> RetuneReport {
        let mut tracker = WelfordTracker::new();
        // Treat the cumulative failure rate as a one-sample baseline
        // signal so retune always has something to publish.
        tracker.observe(self.failure_rate());
        let healthy = baseline_healthy(
            self.keypair.voice_print(),
            self.id.clone(),
            self.region.clone(),
            "AttestationObserver",
            &tracker,
        );
        RetuneReport {
            baseline_value: self.failure_rate(),
            baseline_stddev: 0.0,
            observation_count: self.total_seen,
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }

    fn health(&self) -> CellAgentHealth {
        CellAgentHealth::Healthy
    }
}

// ── 3.3.3 DecayObserver — decay throughput, 3σ within one tick ─────

/// `DecayObserver` watches per-region decay throughput and flags
/// excursions ≥ 3σ within one tick (Spec 6 §3.3.3 + GERSH.3 ≥90%).
pub struct DecayObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    throughput: WelfordTracker,
    warmup: u64,
    threshold_sigma: f64,
}

impl DecayObserver {
    pub fn new(region: NamespaceId) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            throughput: WelfordTracker::new(),
            warmup: 5,
            threshold_sigma: 3.0,
        }
    }

    pub fn warm_baseline(&mut self, samples: &[f64]) {
        for s in samples {
            self.throughput.observe(*s);
        }
    }
}

impl CellAgent for DecayObserver {
    fn id(&self) -> CellAgentId {
        self.id.clone()
    }
    fn voice_print(&self) -> VoicePrint {
        self.keypair.voice_print()
    }
    fn region(&self) -> &NamespaceId {
        &self.region
    }
    fn specialization(&self) -> &'static str {
        "DecayObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::DecayEvents
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        let trace = match event {
            ObservedEvent::Trace(t) => t,
            _ => return ObservationOutcome::Quiet,
        };

        let observed = match trace {
            TraceEvent::RegionDecayThresholdCrossed {
                region,
                observed_rate,
                ..
            } if region == &self.region => *observed_rate,
            _ => return ObservationOutcome::Quiet,
        };

        if self.throughput.count() < self.warmup {
            self.throughput.observe(observed);
            return ObservationOutcome::Quiet;
        }

        let z = self.throughput.z_score(observed);
        if z.abs() >= self.threshold_sigma {
            ObservationOutcome::Anomaly(anomaly(
                self.keypair.voice_print(),
                self.id.clone(),
                self.region.clone(),
                "DecayObserver",
                observed,
                &self.throughput,
                z,
                ObservationSeverity::Medium,
                (z.abs() / self.threshold_sigma).min(1.0),
                format!(
                    "decay throughput {:.3} deviates {:.2}σ from baseline mean {:.3}",
                    observed, z, self.throughput.mean()
                ),
            ))
        } else {
            self.throughput.observe(observed);
            ObservationOutcome::Quiet
        }
    }

    fn retune(&mut self) -> RetuneReport {
        let healthy = baseline_healthy(
            self.keypair.voice_print(),
            self.id.clone(),
            self.region.clone(),
            "DecayObserver",
            &self.throughput,
        );
        RetuneReport {
            baseline_value: self.throughput.mean(),
            baseline_stddev: self.throughput.stddev(),
            observation_count: self.throughput.count(),
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }
}

// ── 3.3.4 ConsensusObserver — escalation rate baseline, 2σ / 4 rounds

/// `ConsensusObserver` watches `ConsensusSnapshotEmitted` events and
/// baselines proposals-per-snapshot. Anomaly: 2σ shift sustained
/// across 4 rounds (Spec 6 §3.3.4 + GERSH.3 ≥90%).
pub struct ConsensusObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    finalized_per_snapshot: WelfordTracker,
    /// Count of consecutive 2σ deviations — clears on a within-σ
    /// snapshot. Anomaly fires when this reaches 4.
    streak: u32,
    threshold_sigma: f64,
    streak_required: u32,
    warmup: u64,
}

impl ConsensusObserver {
    pub fn new(region: NamespaceId) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            finalized_per_snapshot: WelfordTracker::new(),
            streak: 0,
            threshold_sigma: 2.0,
            streak_required: 4,
            warmup: 6,
        }
    }

    pub fn warm_baseline(&mut self, samples: &[f64]) {
        for s in samples {
            self.finalized_per_snapshot.observe(*s);
        }
    }
}

impl CellAgent for ConsensusObserver {
    fn id(&self) -> CellAgentId {
        self.id.clone()
    }
    fn voice_print(&self) -> VoicePrint {
        self.keypair.voice_print()
    }
    fn region(&self) -> &NamespaceId {
        &self.region
    }
    fn specialization(&self) -> &'static str {
        "ConsensusObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::ConsensusEvents
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        let trace = match event {
            ObservedEvent::Trace(t) => t,
            _ => return ObservationOutcome::Quiet,
        };

        let observed = match trace {
            TraceEvent::ConsensusSnapshotEmitted { finalized_count, .. } => {
                *finalized_count as f64
            }
            _ => return ObservationOutcome::Quiet,
        };

        if self.finalized_per_snapshot.count() < self.warmup {
            self.finalized_per_snapshot.observe(observed);
            return ObservationOutcome::Quiet;
        }

        let z = self.finalized_per_snapshot.z_score(observed);
        if z.abs() >= self.threshold_sigma {
            self.streak += 1;
            if self.streak >= self.streak_required {
                let outcome = ObservationOutcome::Anomaly(anomaly(
                    self.keypair.voice_print(),
                    self.id.clone(),
                    self.region.clone(),
                    "ConsensusObserver",
                    observed,
                    &self.finalized_per_snapshot,
                    z,
                    ObservationSeverity::Medium,
                    0.7,
                    format!(
                        "{} consecutive snapshots with proposal count deviating {:.2}σ from baseline",
                        self.streak, z
                    ),
                ));
                self.streak = 0; // arm for the next sustained shift
                return outcome;
            }
            ObservationOutcome::Quiet
        } else {
            self.streak = 0;
            self.finalized_per_snapshot.observe(observed);
            ObservationOutcome::Quiet
        }
    }

    fn retune(&mut self) -> RetuneReport {
        let healthy = baseline_healthy(
            self.keypair.voice_print(),
            self.id.clone(),
            self.region.clone(),
            "ConsensusObserver",
            &self.finalized_per_snapshot,
        );
        RetuneReport {
            baseline_value: self.finalized_per_snapshot.mean(),
            baseline_stddev: self.finalized_per_snapshot.stddev(),
            observation_count: self.finalized_per_snapshot.count(),
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }
}

// ── 3.3.5 RelationObserver — edge-density patterns, 3σ / 1h ────────

/// `RelationObserver` watches edge formation in its region and
/// baselines per-window edge density. Anomaly: density z ≥ 3σ
/// (Spec 6 §3.3.5 + GERSH.3 ≥80%).
pub struct RelationObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    edges_in_window: u64,
    window_started: std::time::Instant,
    window_size: Duration,
    density_baseline: WelfordTracker,
    warmup: u64,
    threshold_sigma: f64,
}

impl RelationObserver {
    pub fn new(region: NamespaceId, window_size: Duration) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            edges_in_window: 0,
            window_started: std::time::Instant::now(),
            window_size,
            density_baseline: WelfordTracker::new(),
            warmup: 5,
            threshold_sigma: 3.0,
        }
    }

    pub fn warm_baseline(&mut self, samples: &[f64]) {
        for s in samples {
            self.density_baseline.observe(*s);
        }
    }
}

impl CellAgent for RelationObserver {
    fn id(&self) -> CellAgentId {
        self.id.clone()
    }
    fn voice_print(&self) -> VoicePrint {
        self.keypair.voice_print()
    }
    fn region(&self) -> &NamespaceId {
        &self.region
    }
    fn specialization(&self) -> &'static str {
        "RelationObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::EdgeFormation
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        if let ObservedEvent::Trace(TraceEvent::EdgeAdded { .. }) = event {
            self.edges_in_window += 1;
        }

        if self.window_started.elapsed() < self.window_size {
            return ObservationOutcome::Quiet;
        }

        let secs = self.window_started.elapsed().as_secs_f64().max(f64::EPSILON);
        let density = self.edges_in_window as f64 / secs;
        self.edges_in_window = 0;
        self.window_started = std::time::Instant::now();

        if self.density_baseline.count() < self.warmup {
            self.density_baseline.observe(density);
            return ObservationOutcome::Quiet;
        }

        let z = self.density_baseline.z_score(density);
        if z.abs() >= self.threshold_sigma {
            ObservationOutcome::Anomaly(anomaly(
                self.keypair.voice_print(),
                self.id.clone(),
                self.region.clone(),
                "RelationObserver",
                density,
                &self.density_baseline,
                z,
                ObservationSeverity::Medium,
                (z.abs() / self.threshold_sigma).min(1.0),
                format!(
                    "edge density {:.3}/s deviates {:.2}σ from baseline mean {:.3}/s",
                    density, z, self.density_baseline.mean()
                ),
            ))
        } else {
            self.density_baseline.observe(density);
            ObservationOutcome::Quiet
        }
    }

    fn retune(&mut self) -> RetuneReport {
        let healthy = baseline_healthy(
            self.keypair.voice_print(),
            self.id.clone(),
            self.region.clone(),
            "RelationObserver",
            &self.density_baseline,
        );
        RetuneReport {
            baseline_value: self.density_baseline.mean(),
            baseline_stddev: self.density_baseline.stddev(),
            observation_count: self.density_baseline.count(),
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }
}

// ── 3.3.6 SilenceObserver — unexpected quiet, 100% deterministic ───

/// `SilenceObserver` watches for the *absence* of activity. Each
/// `Tick` (or any matching event) compares "time since last observed
/// activity" against `expected_interval`. If silent ≥ 3× expected,
/// fires anomaly (Spec 6 §3.3.6 + GERSH.3 100%).
pub struct SilenceObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    last_activity: std::time::Instant,
    expected_interval: Duration,
    /// Multiplier on `expected_interval` past which silence is an
    /// anomaly. 3× expected is the spec's "3σ silence" notion.
    silence_multiplier: f64,
    /// Suppress repeat firing for the same silence window.
    pending_alarm: bool,
}

impl SilenceObserver {
    pub fn new(region: NamespaceId, expected_interval: Duration) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            last_activity: std::time::Instant::now(),
            expected_interval,
            silence_multiplier: 3.0,
            pending_alarm: false,
        }
    }

    /// Called by the dispatcher whenever a region produces activity.
    /// Resets the silence window — used as a hook in tests + the
    /// Step 4 wiring.
    pub fn note_activity(&mut self) {
        self.last_activity = std::time::Instant::now();
        self.pending_alarm = false;
    }

    pub fn silent_secs(&self) -> f64 {
        self.last_activity.elapsed().as_secs_f64()
    }
}

impl CellAgent for SilenceObserver {
    fn id(&self) -> CellAgentId {
        self.id.clone()
    }
    fn voice_print(&self) -> VoicePrint {
        self.keypair.voice_print()
    }
    fn region(&self) -> &NamespaceId {
        &self.region
    }
    fn specialization(&self) -> &'static str {
        "SilenceObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::SilenceEvents
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        match event {
            ObservedEvent::Node(_) | ObservedEvent::Trace(_) => {
                self.note_activity();
                return ObservationOutcome::Quiet;
            }
            ObservedEvent::Tick => {}
        }

        let silent = self.last_activity.elapsed();
        let threshold = Duration::from_secs_f64(
            self.expected_interval.as_secs_f64() * self.silence_multiplier,
        );
        if silent >= threshold && !self.pending_alarm {
            self.pending_alarm = true;
            // Build a tracker pinned to this single observation so the
            // anomaly carries a meaningful baseline_value/stddev.
            let mut t = WelfordTracker::new();
            t.observe(self.expected_interval.as_secs_f64());
            ObservationOutcome::Anomaly(AnomalyObservation {
                observer: self.keypair.voice_print(),
                observer_cell_agent_id: self.id.clone(),
                region: self.region.clone(),
                specialization: "SilenceObserver",
                observed_value: silent.as_secs_f64(),
                baseline_value: self.expected_interval.as_secs_f64(),
                deviation_z_score: silent.as_secs_f64()
                    / self.expected_interval.as_secs_f64().max(f64::EPSILON),
                severity: ObservationSeverity::Medium,
                confidence: 1.0,
                explanation: format!(
                    "region silent for {:.2}s; expected activity within {:.2}s",
                    silent.as_secs_f64(),
                    self.expected_interval.as_secs_f64()
                ),
                at: FabricInstant::now(),
            })
        } else {
            ObservationOutcome::Quiet
        }
    }

    fn retune(&mut self) -> RetuneReport {
        let mut t = WelfordTracker::new();
        t.observe(self.expected_interval.as_secs_f64());
        let healthy = baseline_healthy(
            self.keypair.voice_print(),
            self.id.clone(),
            self.region.clone(),
            "SilenceObserver",
            &t,
        );
        RetuneReport {
            baseline_value: self.expected_interval.as_secs_f64(),
            baseline_stddev: 0.0,
            observation_count: 1,
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }
}

// ── Optional — render an observation as a fabric IntentNode ────────
//
// The aggregation layer (Step 4) writes anomaly / damage signals as
// fabric nodes. For Step 2 we ship the materialization helpers so
// downstream pieces can consume the signals consistently.

const META_KIND: &str = "__bridge_node_kind__";
pub const KIND_ANOMALY: &str = "AnomalyObservation";
pub const KIND_DAMAGE: &str = "DamageObservation";
pub const KIND_BASELINE_HEALTHY: &str = "BaselineHealthy";

pub fn anomaly_to_node(a: &AnomalyObservation) -> IntentNode {
    let mut n = IntentNode::new(format!(
        "{}: {} z={:.2} σ in {} — {}",
        a.specialization, a.severity_label(), a.deviation_z_score, a.region.name, a.explanation
    ))
    .with_creator_voice(a.observer);
    n.metadata
        .insert(META_KIND.into(), MetadataValue::String(KIND_ANOMALY.into()));
    n.metadata.insert(
        "__immune_specialization__".into(),
        MetadataValue::String(a.specialization.into()),
    );
    n.metadata.insert(
        "__immune_region__".into(),
        MetadataValue::String(a.region.name.clone()),
    );
    n.metadata.insert(
        "__immune_z_score__".into(),
        MetadataValue::Float(a.deviation_z_score),
    );
    n.metadata.insert(
        "__immune_observed_value__".into(),
        MetadataValue::Float(a.observed_value),
    );
    n.metadata.insert(
        "__immune_baseline_value__".into(),
        MetadataValue::Float(a.baseline_value),
    );
    n.metadata.insert(
        "__immune_severity__".into(),
        MetadataValue::String(a.severity_label().into()),
    );
    n.recompute_signature();
    n
}

pub fn damage_to_node(d: &DamageObservation) -> IntentNode {
    let mut n = IntentNode::new(format!(
        "{}: damage in {} — {}: {}",
        d.specialization, d.region.name, d.damage_kind, d.explanation
    ))
    .with_creator_voice(d.observer);
    n.metadata
        .insert(META_KIND.into(), MetadataValue::String(KIND_DAMAGE.into()));
    n.metadata.insert(
        "__immune_specialization__".into(),
        MetadataValue::String(d.specialization.into()),
    );
    n.metadata.insert(
        "__immune_region__".into(),
        MetadataValue::String(d.region.name.clone()),
    );
    n.metadata.insert(
        "__immune_damage_kind__".into(),
        MetadataValue::String(d.damage_kind.into()),
    );
    n.recompute_signature();
    n
}

pub fn baseline_healthy_to_node(b: &BaselineHealthy) -> IntentNode {
    let mut n = IntentNode::new(format!(
        "{}: BaselineHealthy in {} — mean={:.3} stddev={:.3} n={}",
        b.specialization, b.region.name, b.baseline_value, b.baseline_stddev, b.observation_count
    ))
    .with_creator_voice(b.observer);
    n.metadata.insert(
        META_KIND.into(),
        MetadataValue::String(KIND_BASELINE_HEALTHY.into()),
    );
    n.metadata.insert(
        "__immune_specialization__".into(),
        MetadataValue::String(b.specialization.into()),
    );
    n.metadata.insert(
        "__immune_region__".into(),
        MetadataValue::String(b.region.name.clone()),
    );
    n.metadata.insert(
        "__immune_baseline_mean__".into(),
        MetadataValue::Float(b.baseline_value),
    );
    n.metadata.insert(
        "__immune_baseline_stddev__".into(),
        MetadataValue::Float(b.baseline_stddev),
    );
    n.metadata.insert(
        "__immune_observation_count__".into(),
        MetadataValue::Int(b.observation_count as i64),
    );
    n.recompute_signature();
    n
}

impl AnomalyObservation {
    pub fn severity_label(&self) -> &'static str {
        match self.severity {
            ObservationSeverity::Low => "low",
            ObservationSeverity::Medium => "medium",
            ObservationSeverity::High => "high",
            ObservationSeverity::Damage => "damage",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::LineageId;

    fn fresh_region() -> NamespaceId {
        NamespaceId::fresh("test-region")
    }

    // ── AttestationObserver — 100% binary detection ──

    #[test]
    fn attestation_observer_flags_failures_as_damage() {
        let region = fresh_region();
        let mut obs = AttestationObserver::new(region.clone());
        let voice = obs.voice_print();
        let ev = TraceEvent::AttestationFailed {
            signer: voice,
            target_node: LineageId::new(),
            reason: "bad".into(),
        };
        let outcome = obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default());
        assert!(matches!(outcome, ObservationOutcome::Damage(_)));

        // ContentFingerprintFailed also counts as damage.
        let ev2 = TraceEvent::ContentFingerprintFailed {
            lineage_id: LineageId::new(),
        };
        let outcome2 = obs.observe(ObservedEvent::Trace(&ev2), &ObservationContext::default());
        assert!(matches!(outcome2, ObservationOutcome::Damage(_)));

        // Verified events stay quiet.
        let ev3 = TraceEvent::ContentFingerprintVerified {
            lineage_id: LineageId::new(),
        };
        let outcome3 = obs.observe(ObservedEvent::Trace(&ev3), &ObservationContext::default());
        assert!(matches!(outcome3, ObservationOutcome::Quiet));
    }

    // ── DecayObserver — fires deterministically on a 3σ excursion ──

    #[test]
    fn decay_observer_fires_on_three_sigma() {
        let region = fresh_region();
        let mut obs = DecayObserver::new(region.clone());
        // Warm baseline with stable rates ~10/s, σ ~ 0.2.
        let warm = [9.8, 10.0, 10.1, 10.2, 9.9, 10.0, 10.1, 9.8, 10.2, 10.0];
        obs.warm_baseline(&warm);
        let ev = TraceEvent::RegionDecayThresholdCrossed {
            region,
            observed_rate: 15.0, // ~25σ from the baseline, way above 3σ
            baseline_rate: 10.0,
        };
        let outcome = obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default());
        assert!(
            matches!(outcome, ObservationOutcome::Anomaly(_)),
            "Decay 3σ excursion must trigger anomaly; got {:?}",
            std::mem::discriminant(&outcome)
        );
    }

    #[test]
    fn decay_observer_quiet_within_baseline() {
        let region = fresh_region();
        let mut obs = DecayObserver::new(region.clone());
        let warm = [9.8, 10.0, 10.1, 10.2, 9.9, 10.0, 10.1, 9.8, 10.2, 10.0];
        obs.warm_baseline(&warm);
        let ev = TraceEvent::RegionDecayThresholdCrossed {
            region,
            observed_rate: 10.05,
            baseline_rate: 10.0,
        };
        assert!(matches!(
            obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default()),
            ObservationOutcome::Quiet
        ));
    }

    // ── SilenceObserver — 100% deterministic on 3× expected ──

    #[test]
    fn silence_observer_fires_after_three_x_expected() {
        let region = fresh_region();
        let mut obs = SilenceObserver::new(region, Duration::from_millis(10));
        // Force "last activity" to 50ms ago (5× expected) by manual reset.
        obs.last_activity = std::time::Instant::now() - Duration::from_millis(50);
        let outcome = obs.observe(ObservedEvent::Tick, &ObservationContext::default());
        assert!(matches!(outcome, ObservationOutcome::Anomaly(_)));
    }

    #[test]
    fn silence_observer_quiet_within_expected() {
        let region = fresh_region();
        let mut obs = SilenceObserver::new(region, Duration::from_secs(60));
        // Just constructed: last_activity ≈ now → silent_secs ≈ 0.
        let outcome = obs.observe(ObservedEvent::Tick, &ObservationContext::default());
        assert!(matches!(outcome, ObservationOutcome::Quiet));
    }

    #[test]
    fn silence_observer_reset_on_activity() {
        let region = fresh_region();
        let mut obs = SilenceObserver::new(region, Duration::from_millis(10));
        obs.last_activity = std::time::Instant::now() - Duration::from_millis(50);
        // Note activity → resets silent window.
        let dummy = TraceEvent::ClockTick {
            timestamp: crate::temporal::LamportTimestamp::new(1),
        };
        let _ = obs.observe(ObservedEvent::Trace(&dummy), &ObservationContext::default());
        // Now Tick should be quiet because we just reset.
        let outcome = obs.observe(ObservedEvent::Tick, &ObservationContext::default());
        assert!(matches!(outcome, ObservationOutcome::Quiet));
    }

    // ── ConsensusObserver — 4-streak before fire ──

    #[test]
    fn consensus_observer_requires_streak_of_four() {
        let region = fresh_region();
        let mut obs = ConsensusObserver::new(region.clone());
        // Baseline at ~3 finalized per snapshot, σ ~ 0.5.
        obs.warm_baseline(&[3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0]);

        // Three consecutive 2σ-shifted snapshots → still quiet.
        for _ in 0..3 {
            let ev = TraceEvent::ConsensusSnapshotEmitted {
                snapshot_id: LineageId::new(),
                target: LineageId::new(),
                finalized_count: 9,
            };
            let outcome = obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default());
            assert!(matches!(outcome, ObservationOutcome::Quiet));
        }
        // Fourth crosses the streak threshold and fires.
        let ev = TraceEvent::ConsensusSnapshotEmitted {
            snapshot_id: LineageId::new(),
            target: LineageId::new(),
            finalized_count: 9,
        };
        let outcome = obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default());
        assert!(matches!(outcome, ObservationOutcome::Anomaly(_)));
    }

    #[test]
    fn consensus_streak_resets_on_within_sigma_observation() {
        let region = fresh_region();
        let mut obs = ConsensusObserver::new(region.clone());
        obs.warm_baseline(&[3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0]);
        for _ in 0..3 {
            let ev = TraceEvent::ConsensusSnapshotEmitted {
                snapshot_id: LineageId::new(),
                target: LineageId::new(),
                finalized_count: 9,
            };
            let _ = obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default());
        }
        // A within-σ snapshot resets the streak.
        let normal = TraceEvent::ConsensusSnapshotEmitted {
            snapshot_id: LineageId::new(),
            target: LineageId::new(),
            finalized_count: 3,
        };
        let _ = obs.observe(ObservedEvent::Trace(&normal), &ObservationContext::default());
        // Now a single shifted snapshot should NOT fire (streak=1, need 4).
        let shifted = TraceEvent::ConsensusSnapshotEmitted {
            snapshot_id: LineageId::new(),
            target: LineageId::new(),
            finalized_count: 9,
        };
        let outcome = obs.observe(ObservedEvent::Trace(&shifted), &ObservationContext::default());
        assert!(matches!(outcome, ObservationOutcome::Quiet));
    }

    // ── retune always emits BaselineHealthy ──

    #[test]
    fn retune_emits_baseline_healthy_for_every_specialization() {
        let region = fresh_region();
        let mut rate = RateObserver::new(region.clone(), Duration::from_secs(60));
        let mut att = AttestationObserver::new(region.clone());
        let mut decay = DecayObserver::new(region.clone());
        let mut consensus = ConsensusObserver::new(region.clone());
        let mut relation = RelationObserver::new(region.clone(), Duration::from_secs(60));
        let mut silence = SilenceObserver::new(region.clone(), Duration::from_secs(60));

        let reports = vec![
            rate.retune(),
            att.retune(),
            decay.retune(),
            consensus.retune(),
            relation.retune(),
            silence.retune(),
        ];
        for r in reports {
            // Every retune produces a populated BaselineHealthy.
            assert!(!r.baseline_healthy.specialization.is_empty());
        }
    }

    // ── node materialization round-trip ──

    #[test]
    fn anomaly_to_node_carries_metadata() {
        let region = fresh_region();
        let mut obs = DecayObserver::new(region.clone());
        obs.warm_baseline(&[10.0; 10]);
        let ev = TraceEvent::RegionDecayThresholdCrossed {
            region: region.clone(),
            observed_rate: 50.0,
            baseline_rate: 10.0,
        };
        let outcome = obs.observe(ObservedEvent::Trace(&ev), &ObservationContext::default());
        let anomaly = match outcome {
            ObservationOutcome::Anomaly(a) => a,
            other => panic!("expected anomaly, got {:?}", std::mem::discriminant(&other)),
        };
        let node = anomaly_to_node(&anomaly);
        assert_eq!(
            node.metadata.get(META_KIND),
            Some(&MetadataValue::String(KIND_ANOMALY.into()))
        );
        assert_eq!(
            node.metadata
                .get("__immune_specialization__")
                .and_then(|v| match v {
                    MetadataValue::String(s) => Some(s.as_str()),
                    _ => None,
                }),
            Some("DecayObserver")
        );
    }
}
