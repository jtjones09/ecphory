// OPACITY OBSERVER — Spec 7 §6.1 / Step 7
//
// Per Spec 7 §4.4 v1.1 fold (Cantrill C.2 + Gershman G.2): the full
// LLM-backed translator + interpretability assessment is deferred to
// v1.5. The v1 metric is concrete and testable without LLM reasoning:
//
// > OpacityObserver flags messages containing >3 node references that
// > the operator has not observed in the last 7 days.
//
// This is a seventh cell-agent specialization beyond the six in Spec 6,
// scoped to the comms region. It implements the standard `CellAgent`
// trait so it slots into `BridgeFabric::register_cell_agent` alongside
// the other observers.
//
// "Operator observed in the last 7 days" is a runtime claim the fabric
// cannot answer alone — subscriptions don't record per-node observation
// history at v1. The observer therefore consults an
// `Arc<RwLock<HashSet<LineageId>>>` shared with the operator's
// subscription callback (or with nisaba-the-agent in production). When
// that set is empty, every reference counts as unobserved.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::comms::message::{
    CommsMessage, KIND_COMMS_MESSAGE, META_KIND,
};
use crate::identity::{generate_agent_keypair, AgentKeypair, NamespaceId, VoicePrint};
use crate::immune::cell_agent::{
    AnomalyObservation, BaselineHealthy, CellAgent, CellAgentHealth, CellAgentId,
    ImmunePattern, ObservationContext, ObservationOutcome, ObservationSeverity, ObservedEvent,
    RetuneReport,
};
use crate::signature::LineageId;
use crate::temporal::FabricInstant;

/// Default opacity threshold per Spec 7 §4.4 v1.1: >3 unobserved refs.
pub const DEFAULT_OPACITY_THRESHOLD: usize = 3;
/// Default observation window per Spec 7 §4.4 v1.1.
pub const DEFAULT_OBSERVATION_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Shared, mutable record of which fabric nodes the operator's
/// subscription has observed. Updated by the operator's comms callback
/// (in production: nisaba-the-agent's subscription) and read by the
/// OpacityObserver.
pub type OperatorObservedSet = Arc<RwLock<HashSet<LineageId>>>;

/// `OpacityObserver` — Spec 7 §6.1 cell-agent specialization for the
/// comms region. Flags messages whose `references` field carries more
/// than `threshold` unobserved-by-operator LineageIds.
pub struct OpacityObserver {
    id: CellAgentId,
    region: NamespaceId,
    keypair: AgentKeypair,
    /// Voice print of the operator whose observation log is consulted.
    /// Recorded for the AnomalyObservation explanation; the actual
    /// log lives in `operator_observed`.
    operator_voice: VoicePrint,
    /// Shared observation set. Empty by default — the operator hasn't
    /// observed anything yet.
    operator_observed: OperatorObservedSet,
    /// Number of unobserved references that triggers the flag.
    /// Default per Spec 7 §4.4 v1.1: 3 (i.e., flag at 4 or more).
    threshold: usize,
    /// Cumulative number of comms-message creations seen (for retune).
    messages_seen: u64,
    /// Cumulative number of flag firings (for retune).
    flags_emitted: u64,
    /// Window per spec; recorded for surfacing in the explanation,
    /// not consulted in v1's `observe()` directly.
    #[allow(dead_code)]
    window: Duration,
}

impl OpacityObserver {
    pub fn new(
        region: NamespaceId,
        operator_voice: VoicePrint,
        operator_observed: OperatorObservedSet,
    ) -> Self {
        Self {
            id: CellAgentId::new(),
            region,
            keypair: generate_agent_keypair(),
            operator_voice,
            operator_observed,
            threshold: DEFAULT_OPACITY_THRESHOLD,
            messages_seen: 0,
            flags_emitted: 0,
            window: DEFAULT_OBSERVATION_WINDOW,
        }
    }

    /// Override the unobserved-reference threshold (default 3).
    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.threshold = threshold;
        self
    }
}

impl CellAgent for OpacityObserver {
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
        "OpacityObserver"
    }
    fn pattern(&self) -> ImmunePattern {
        ImmunePattern::NodeCreates
    }

    fn observe(
        &mut self,
        event: ObservedEvent<'_>,
        _ctx: &ObservationContext,
    ) -> ObservationOutcome {
        let node = match event {
            ObservedEvent::Node(n) => n,
            _ => return ObservationOutcome::Quiet,
        };
        // Only comms messages — thread nodes, conflict markers, and
        // immune-system signals are not in scope.
        let is_comms = node
            .metadata
            .get(META_KIND)
            .map(|v| v.as_str_repr() == KIND_COMMS_MESSAGE)
            .unwrap_or(false);
        if !is_comms {
            return ObservationOutcome::Quiet;
        }

        self.messages_seen += 1;

        let refs = CommsMessage::references_from_node(node);
        if refs.is_empty() {
            return ObservationOutcome::Quiet;
        }

        let observed = self.operator_observed.read().expect("operator-observed poisoned");
        let unobserved = refs.iter().filter(|id| !observed.contains(id)).count();
        drop(observed);

        if unobserved <= self.threshold {
            return ObservationOutcome::Quiet;
        }

        self.flags_emitted += 1;
        ObservationOutcome::Anomaly(AnomalyObservation {
            observer: self.keypair.voice_print(),
            observer_cell_agent_id: self.id.clone(),
            region: self.region.clone(),
            specialization: "OpacityObserver",
            observed_value: unobserved as f64,
            baseline_value: self.threshold as f64,
            deviation_z_score: (unobserved as f64 - self.threshold as f64).max(0.0),
            severity: ObservationSeverity::Medium,
            confidence: 1.0,
            explanation: format!(
                "comms message references {} fabric nodes the operator ({}) has not observed; threshold {}",
                unobserved, self.operator_voice, self.threshold
            ),
            at: FabricInstant::now(),
        })
    }

    fn retune(&mut self) -> RetuneReport {
        // OpacityObserver doesn't carry a Welford baseline — its
        // metric is structural ("count of unobserved refs"), not
        // statistical. Synthesize a BaselineHealthy from current
        // counters so the COHEN.1 maintenance/defense ratio stays
        // truthful.
        let healthy = BaselineHealthy {
            observer: self.keypair.voice_print(),
            observer_cell_agent_id: self.id.clone(),
            region: self.region.clone(),
            specialization: "OpacityObserver",
            baseline_value: self.threshold as f64,
            baseline_stddev: 0.0,
            observation_count: self.messages_seen,
            at: FabricInstant::now(),
        };
        RetuneReport {
            baseline_value: self.threshold as f64,
            baseline_stddev: 0.0,
            observation_count: self.messages_seen,
            baseline_shifted: false,
            baseline_healthy: healthy,
        }
    }

    fn health(&self) -> CellAgentHealth {
        CellAgentHealth::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::{CommsMessage, MessageContent, MessageIntent, Sensitivity, Urgency};
    use crate::node::IntentNode;

    fn fresh_observer() -> (OpacityObserver, OperatorObservedSet, AgentKeypair) {
        let operator = generate_agent_keypair();
        let observed: OperatorObservedSet = Arc::new(RwLock::new(HashSet::new()));
        let observer = OpacityObserver::new(
            NamespaceId::hotash_comms(),
            operator.voice_print(),
            Arc::clone(&observed),
        );
        (observer, observed, operator)
    }

    fn message_with_references(speaker: VoicePrint, refs: Vec<LineageId>) -> IntentNode {
        let msg = CommsMessage {
            content: MessageContent::Text("here are the nodes".into()),
            thread: None,
            mentions: vec![],
            intent: MessageIntent::Inform,
            urgency: Urgency::Normal,
            sensitivity: Sensitivity::Normal,
            references: refs,
        };
        msg.to_intent_node(speaker)
    }

    #[test]
    fn quiet_when_references_under_threshold() {
        let (mut observer, _observed, _operator) = fresh_observer();
        let speaker = generate_agent_keypair();
        let refs = vec![LineageId::new(), LineageId::new(), LineageId::new()];
        let node = message_with_references(speaker.voice_print(), refs);
        let outcome = observer.observe(
            ObservedEvent::Node(&node),
            &ObservationContext::default(),
        );
        assert!(matches!(outcome, ObservationOutcome::Quiet));
    }

    #[test]
    fn anomaly_when_more_than_threshold_unobserved_refs() {
        let (mut observer, _observed, _operator) = fresh_observer();
        let speaker = generate_agent_keypair();
        let refs = vec![
            LineageId::new(),
            LineageId::new(),
            LineageId::new(),
            LineageId::new(),
            LineageId::new(),
        ];
        let node = message_with_references(speaker.voice_print(), refs);
        let outcome = observer.observe(
            ObservedEvent::Node(&node),
            &ObservationContext::default(),
        );
        match outcome {
            ObservationOutcome::Anomaly(a) => {
                assert_eq!(a.observed_value, 5.0);
                assert_eq!(a.specialization, "OpacityObserver");
            }
            other => panic!("expected anomaly, got {:?}", other),
        }
    }

    #[test]
    fn quiet_when_operator_has_observed_enough_refs() {
        let (mut observer, observed, _operator) = fresh_observer();
        let speaker = generate_agent_keypair();
        let observed_ids: Vec<LineageId> = (0..3).map(|_| LineageId::new()).collect();
        observed.write().unwrap().extend(observed_ids.iter().cloned());

        // 3 already-observed + 2 new — only 2 unobserved → under threshold (3).
        let refs: Vec<LineageId> = observed_ids
            .into_iter()
            .chain([LineageId::new(), LineageId::new()])
            .collect();
        let node = message_with_references(speaker.voice_print(), refs);
        let outcome = observer.observe(
            ObservedEvent::Node(&node),
            &ObservationContext::default(),
        );
        assert!(matches!(outcome, ObservationOutcome::Quiet));
    }

    #[test]
    fn skips_non_comms_nodes() {
        let (mut observer, _observed, _operator) = fresh_observer();
        let plain = IntentNode::new("not a comms message");
        let outcome = observer.observe(
            ObservedEvent::Node(&plain),
            &ObservationContext::default(),
        );
        assert!(matches!(outcome, ObservationOutcome::Quiet));
    }
}
