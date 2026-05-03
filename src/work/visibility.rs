// Visibility query (Spec 9 §3.2) — status from topology, no status field
//
// Eight categories per the v1.1 fold + Cantrill C.1 / Kingsbury K.1
// folds. The query takes a `BridgeFabric` (so SuccessCheck predicates
// can be evaluated through the same trait surface comms uses) and a
// `checks_for` closure that returns the SuccessCheck list for a given
// intent lineage. Callers maintain the check registry however they
// want — Nabu uses an in-memory `Mutex<HashMap>` populated when
// intents are written; tests pass a plain `HashMap`.
//
// v1 implements 7 of the 8 categories. `Contested` is left empty —
// it requires per-agent SuccessCheck evaluation (the same check
// passing for one creator and failing for another), which needs a
// per-agent fabric view that's out of scope for the minimum visible
// substrate. Stub kept in the response shape so the MCP tool's JSON
// contract is stable from v1 onward.

use std::collections::{HashMap, HashSet};

use crate::bridge::BridgeFabric;
use crate::comms::handoff::CheckOutcome;
use crate::comms::message::{SuccessCheck, Urgency};
use crate::context::RelationshipKind;
use crate::node::IntentNode;
use crate::signature::LineageId;

use super::{
    claim::WorkClaim, KIND_WORK_INTENT, META_INTENT_CADENCE_SECS, META_INTENT_CHECK_COUNT,
    META_INTENT_DEADLINE_NS, META_INTENT_DURATION_KIND, META_INTENT_URGENCY, META_KIND,
};

/// Per-intent scratch state collected during the fabric scan, then
/// passed to `classify` once SuccessChecks have been evaluated.
struct ScanRow {
    lineage_id: LineageId,
    description: String,
    urgency: Option<Urgency>,
    #[allow(dead_code)]
    duration_kind: Option<String>,
    cadence_secs: Option<f64>,
    #[allow(dead_code)]
    deadline_ns: Option<i64>,
    check_count: usize,
    intent_age_secs: f64,
    weight: f64,
}

/// One of the eight Spec 9 §3.2 + §3.2 fatal-fold categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkStatus {
    Active,
    Stalled,
    Completed,
    Unstarted,
    Abandoned,
    Contested,
    Overloaded,
    Orphaned,
}

impl WorkStatus {
    pub fn label(self) -> &'static str {
        match self {
            WorkStatus::Active => "active",
            WorkStatus::Stalled => "stalled",
            WorkStatus::Completed => "completed",
            WorkStatus::Unstarted => "unstarted",
            WorkStatus::Abandoned => "abandoned",
            WorkStatus::Contested => "contested",
            WorkStatus::Overloaded => "overloaded",
            WorkStatus::Orphaned => "orphaned",
        }
    }
}

/// A single intent's projected state. The query produces one of these
/// per WorkIntent in the fabric.
#[derive(Debug, Clone)]
pub struct IntentSummary {
    pub lineage_id: LineageId,
    pub description: String,
    pub urgency: Option<Urgency>,
    pub status: WorkStatus,
    pub claim_count: usize,
    pub evidence_count: usize,
    pub last_evidence_age_secs: Option<f64>,
    pub passing_checks: usize,
    pub total_checks: usize,
    pub agents: Vec<String>,
}

/// All intents in the fabric, bucketed by status. The shape mirrors
/// the JSON contract the `fabric_work_status` MCP tool returns
/// (Cantrill C.1 fold).
#[derive(Debug, Clone, Default)]
pub struct VisibilitySnapshot {
    pub active: Vec<IntentSummary>,
    pub stalled: Vec<IntentSummary>,
    pub completed: Vec<IntentSummary>,
    pub unstarted: Vec<IntentSummary>,
    pub abandoned: Vec<IntentSummary>,
    pub contested: Vec<IntentSummary>,
    pub overloaded: Vec<IntentSummary>,
    pub orphaned: Vec<IntentSummary>,
}

impl VisibilitySnapshot {
    pub fn total_intents(&self) -> usize {
        self.active.len()
            + self.stalled.len()
            + self.completed.len()
            + self.unstarted.len()
            + self.abandoned.len()
            + self.contested.len()
            + self.overloaded.len()
            + self.orphaned.len()
    }

    fn push(&mut self, summary: IntentSummary) {
        match summary.status {
            WorkStatus::Active => self.active.push(summary),
            WorkStatus::Stalled => self.stalled.push(summary),
            WorkStatus::Completed => self.completed.push(summary),
            WorkStatus::Unstarted => self.unstarted.push(summary),
            WorkStatus::Abandoned => self.abandoned.push(summary),
            WorkStatus::Contested => self.contested.push(summary),
            WorkStatus::Overloaded => self.overloaded.push(summary),
            WorkStatus::Orphaned => self.orphaned.push(summary),
        }
    }
}

/// Tunable thresholds for the visibility query. Defaults match the
/// spec text; tests override to exercise the boundaries without
/// faking wall-clock time.
#[derive(Debug, Clone, Copy)]
pub struct VisibilityConfig {
    /// Stall fires when `last_evidence_age > stall_factor * cadence`.
    /// Default: 2.0 (per §3.3 — "2× the calibrated cadence").
    pub stall_factor: f64,
    /// Default cadence for discrete intents that don't carry one in
    /// metadata. 24h.
    pub default_discrete_cadence_secs: f64,
    /// Orphan fires for Prompt+ urgency intents with zero claims that
    /// are older than this. Default 4h per Cantrill C.S1.
    pub orphan_age_secs: f64,
    /// Abandoned fires when the node weight (decay product) is below
    /// this. Default 0.1 — well into the long tail of the exponential.
    pub abandoned_weight_threshold: f64,
}

impl Default for VisibilityConfig {
    fn default() -> Self {
        Self {
            stall_factor: 2.0,
            default_discrete_cadence_secs: 86_400.0,
            orphan_age_secs: 4.0 * 3600.0,
            abandoned_weight_threshold: 0.1,
        }
    }
}

/// Run the visibility query.
///
/// `checks_for(lineage_id)` returns the SuccessCheck list for the
/// intent identified by `lineage_id`. For intents not in the registry
/// (e.g. checks lost to a crash), an empty list is acceptable —
/// the intent will fall through to a topology-only category.
pub fn query_visibility<F>(
    bridge: &BridgeFabric,
    config: &VisibilityConfig,
    checks_for: F,
) -> VisibilitySnapshot
where
    F: Fn(&LineageId) -> Vec<SuccessCheck>,
{
    // Phase 1: scan fabric for intents, claims, evidence under the
    // read lock. Phase 2: evaluate SuccessChecks (which take the
    // bridge via `evaluate`) outside the closure.

    // intent fingerprint hex -> claims pointing at it
    let mut claims_by_fp: HashMap<String, Vec<String>> = HashMap::new();
    let mut intent_rows: Vec<ScanRow> = Vec::new();
    // intent fingerprint hex -> last evidence age secs + count
    let mut evidence_by_fp: HashMap<String, (f64, usize)> = HashMap::new();

    // First pass: collect intents and claims
    let intent_fingerprints: HashMap<LineageId, String> = bridge.read_inner(|inner| {
        let mut by_id = HashMap::new();
        for (id, node) in inner.nodes() {
            let kind = node
                .metadata
                .get(META_KIND)
                .map(|v| v.as_str_repr())
                .unwrap_or_default();
            if kind == KIND_WORK_INTENT {
                let fp = node.content_fingerprint().to_hex();
                by_id.insert(id.clone(), fp.clone());

                let urgency = parse_urgency(node);
                let duration_kind = node
                    .metadata
                    .get(META_INTENT_DURATION_KIND)
                    .map(|v| v.as_str_repr());
                let cadence_secs = node.metadata.get(META_INTENT_CADENCE_SECS).and_then(|v| {
                    if let crate::node::MetadataValue::Int(n) = v {
                        Some(*n as f64)
                    } else {
                        None
                    }
                });
                let deadline_ns = node.metadata.get(META_INTENT_DEADLINE_NS).and_then(|v| {
                    if let crate::node::MetadataValue::Int(n) = v {
                        Some(*n)
                    } else {
                        None
                    }
                });
                let check_count = node
                    .metadata
                    .get(META_INTENT_CHECK_COUNT)
                    .and_then(|v| {
                        if let crate::node::MetadataValue::Int(n) = v {
                            Some(*n as usize)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                let intent_age_secs = inner.node_age_secs(id).unwrap_or(0.0);
                let weight = inner.activation_weight(id).unwrap_or(0.0);

                intent_rows.push(ScanRow {
                    lineage_id: id.clone(),
                    description: node.want.description.clone(),
                    urgency,
                    duration_kind,
                    cadence_secs,
                    deadline_ns,
                    check_count,
                    intent_age_secs,
                    weight,
                });
            } else if WorkClaim::is_claim_node(node) {
                if let Some(intent_fp) = WorkClaim::intent_fingerprint_from_node(node) {
                    if let Some(agent) = WorkClaim::agent_hex_from_node(node) {
                        claims_by_fp.entry(intent_fp).or_default().push(agent);
                    }
                }
            }
        }
        by_id
    });

    // Second pass: count incoming Fulfills edges per intent. Done in
    // a separate read_inner so we can use the fingerprint map.
    bridge.read_inner(|inner| {
        for (intent_id, fp) in &intent_fingerprints {
            let mut count = 0usize;
            let mut youngest_age: Option<f64> = None;
            for source_id in inner.edges_to(intent_id) {
                let edges = inner.edges_from(source_id);
                let is_fulfills = edges
                    .iter()
                    .any(|e| e.target == *intent_id && matches!(e.kind, RelationshipKind::Fulfills));
                if !is_fulfills {
                    continue;
                }
                count += 1;
                if let Some(age) = inner.node_age_secs(source_id) {
                    youngest_age = Some(match youngest_age {
                        Some(prev) => prev.min(age),
                        None => age,
                    });
                }
            }
            if count > 0 {
                let last_age = youngest_age.unwrap_or(f64::INFINITY);
                evidence_by_fp.insert(fp.clone(), (last_age, count));
            }
        }
    });

    // Phase 2: evaluate SuccessChecks per intent (unlocked).
    let mut snapshot = VisibilitySnapshot::default();
    for row in intent_rows {
        let fp = intent_fingerprints
            .get(&row.lineage_id)
            .cloned()
            .unwrap_or_default();
        let claims = claims_by_fp.remove(&fp).unwrap_or_default();
        let claim_count = claims.len();
        let unique_agents: HashSet<String> = claims.iter().cloned().collect();

        let (last_evidence_age_secs, evidence_count) = evidence_by_fp
            .get(&fp)
            .map(|(age, n)| (Some(*age), *n))
            .unwrap_or((None, 0));

        let checks = checks_for(&row.lineage_id);
        let (passing_checks, total_checks) = if checks.is_empty() {
            (0, row.check_count)
        } else {
            let passing = checks
                .iter()
                .map(|c| c.evaluate(bridge))
                .filter(|o| matches!(o, CheckOutcome::Pass))
                .count();
            (passing, checks.len())
        };

        let status = classify(
            &row,
            claim_count,
            evidence_count,
            last_evidence_age_secs,
            passing_checks,
            total_checks,
            config,
        );

        snapshot.push(IntentSummary {
            lineage_id: row.lineage_id,
            description: row.description,
            urgency: row.urgency,
            status,
            claim_count,
            evidence_count,
            last_evidence_age_secs,
            passing_checks,
            total_checks,
            agents: unique_agents.into_iter().collect(),
        });
    }

    snapshot
}

fn parse_urgency(node: &IntentNode) -> Option<Urgency> {
    let label = node.metadata.get(META_INTENT_URGENCY)?.as_str_repr();
    match label.as_str() {
        "Background" => Some(Urgency::Background),
        "Normal" => Some(Urgency::Normal),
        "Prompt" => Some(Urgency::Prompt),
        "Immediate" => Some(Urgency::Immediate),
        _ => None,
    }
}

fn classify(
    row: &ScanRow,
    claim_count: usize,
    evidence_count: usize,
    last_evidence_age_secs: Option<f64>,
    passing_checks: usize,
    total_checks: usize,
    config: &VisibilityConfig,
) -> WorkStatus {
    // Decision order matters: first match wins. Ordering captures the
    // priority the spec implies — abandonment is a final state, then
    // operator-attention categories (orphaned/overloaded), then the
    // success/failure categories, then the topology-only ones.

    if row.weight < config.abandoned_weight_threshold {
        return WorkStatus::Abandoned;
    }

    let urgency = row.urgency.unwrap_or(Urgency::Normal);
    let rank = urgency_rank(urgency);

    if rank >= urgency_rank(Urgency::Prompt)
        && claim_count == 0
        && row.intent_age_secs > config.orphan_age_secs
    {
        return WorkStatus::Orphaned;
    }

    if claim_count > 2 && rank < urgency_rank(Urgency::Immediate) {
        return WorkStatus::Overloaded;
    }

    if total_checks > 0 && passing_checks == total_checks {
        return WorkStatus::Completed;
    }

    if claim_count == 0 && evidence_count == 0 {
        return WorkStatus::Unstarted;
    }

    if let Some(age) = last_evidence_age_secs {
        let cadence = row
            .cadence_secs
            .unwrap_or(config.default_discrete_cadence_secs);
        if age > config.stall_factor * cadence {
            return WorkStatus::Stalled;
        }
    } else if claim_count > 0 {
        // Has a claim but no evidence yet — treat as Active until
        // the cadence-from-claim threshold is exceeded. For v1 we
        // call this "active" since the agent has signed up but not
        // yet produced. Cadence-from-claim stall is Step 6 work.
        return WorkStatus::Active;
    }

    WorkStatus::Active
}

fn urgency_rank(u: Urgency) -> u8 {
    match u {
        Urgency::Background => 0,
        Urgency::Normal => 1,
        Urgency::Prompt => 2,
        Urgency::Immediate => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeFabric;
    use crate::fabric::Fabric;
    use crate::identity::{generate_agent_keypair, NamespaceId};
    use crate::node::IntentNode;
    use crate::work::{
        claim::WorkClaim,
        hotash_work,
        intent::{IntentDuration, WorkIntent},
    };
    use std::time::Duration;

    fn fresh_intent(description: &str, urgency: Urgency) -> WorkIntent {
        WorkIntent {
            description: description.into(),
            intended_outcome: "ok".into(),
            constraints: vec![],
            success_checks: vec![],
            requested_by: generate_agent_keypair().voice_print(),
            urgency,
            context: vec![],
            duration: IntentDuration::Discrete { deadline: None },
        }
    }

    /// Build a populated fabric (writes via `Fabric::create` which
    /// accepts an explicit namespace), then wrap it as a BridgeFabric
    /// for the visibility query — the bridge wraps the fabric under a
    /// RwLock so live writes after wrap aren't possible. All test
    /// scenarios materialize into a single fabric pre-wrap.
    fn wrap(fabric: Fabric) -> BridgeFabric {
        BridgeFabric::wrap(fabric)
    }

    #[test]
    fn unstarted_intent_has_no_claims_no_evidence() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent = fresh_intent("Spec 9 step 3 unstarted", Urgency::Normal);
        fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();

        let bridge = wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.unstarted.len(), 1);
        assert_eq!(snap.total_intents(), 1);
    }

    #[test]
    fn intent_with_claim_only_is_active() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent = fresh_intent("active intent", Urgency::Normal);
        let intent_id = fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();

        let claim = WorkClaim {
            intent: intent_identity,
            agent: creator.voice_print(),
            approach: "do it".into(),
            estimated_evidence_cadence: Duration::from_secs(900),
        };
        fabric
            .create(claim.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();

        let bridge = wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.active.len(), 1, "snapshot: {:?}", snap);
    }

    #[test]
    fn overloaded_when_three_claims_and_normal_urgency() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent = fresh_intent("overloaded", Urgency::Normal);
        let intent_id = fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();

        for i in 0..3 {
            let agent_kp = generate_agent_keypair();
            let claim = WorkClaim {
                intent: intent_identity.clone(),
                agent: agent_kp.voice_print(),
                approach: format!("agent {} approach", i),
                estimated_evidence_cadence: Duration::from_secs(900),
            };
            fabric
                .create(
                    claim.to_intent_node(agent_kp.voice_print()),
                    &hotash_work(),
                    None,
                )
                .unwrap();
        }

        let bridge = wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.overloaded.len(), 1);
        assert_eq!(snap.overloaded[0].claim_count, 3);
    }

    #[test]
    fn overloaded_does_not_fire_for_immediate_urgency() {
        // Per Cantrill C.S1: overloaded triggers on urgency <
        // Immediate. Immediate-urgency intents accumulating claims
        // are an expected biological over-response (Kingsbury K.2).
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent = fresh_intent("on fire", Urgency::Immediate);
        let intent_id = fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();

        for i in 0..4 {
            let agent_kp = generate_agent_keypair();
            let claim = WorkClaim {
                intent: intent_identity.clone(),
                agent: agent_kp.voice_print(),
                approach: format!("agent {} approach", i),
                estimated_evidence_cadence: Duration::from_secs(60),
            };
            fabric
                .create(
                    claim.to_intent_node(agent_kp.voice_print()),
                    &hotash_work(),
                    None,
                )
                .unwrap();
        }

        let bridge = wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.overloaded.len(), 0);
        // Falls into active (claims, no evidence yet).
        assert_eq!(snap.active.len(), 1);
    }

    #[test]
    fn orphaned_when_prompt_urgency_unclaimed_after_threshold() {
        // Use config orphan_age = 0.0 so any unclaimed Prompt-urgency
        // intent counts immediately — avoids needing fake clocks.
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent = fresh_intent("needs a hand", Urgency::Prompt);
        fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();

        let bridge = wrap(fabric);
        let mut config = VisibilityConfig::default();
        config.orphan_age_secs = 0.0;

        let snap = query_visibility(&bridge, &config, |_| Vec::new());
        assert_eq!(snap.orphaned.len(), 1);
    }

    #[test]
    fn completed_when_all_checks_pass() {
        // SuccessCheck::NodeExists evaluates true when the referenced
        // LineageId is in the fabric. Provide a registry returning a
        // single NodeExists check pointing at a node we just wrote.
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();

        let evidence_node = IntentNode::new("the work was done")
            .with_creator_voice(creator.voice_print());
        let nisaba = NamespaceId::fresh("nisaba");
        let evidence_id = fabric.create(evidence_node, &nisaba, None).unwrap();

        let mut intent = fresh_intent("complete this", Urgency::Normal);
        intent.success_checks = vec![SuccessCheck::NodeExists {
            reference: evidence_id.as_uuid().to_string(),
        }];
        let intent_id = fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();

        let bridge = wrap(fabric);
        let intent_id_for_lookup = intent_id.clone();
        let checks = intent.success_checks.clone();
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), move |id| {
            if *id == intent_id_for_lookup {
                checks.clone()
            } else {
                Vec::new()
            }
        });
        assert_eq!(snap.completed.len(), 1, "snapshot: {:?}", snap);
        assert_eq!(snap.completed[0].passing_checks, 1);
        assert_eq!(snap.completed[0].total_checks, 1);
    }

    #[test]
    fn stalled_when_evidence_old_relative_to_cadence() {
        // Use a very small cadence so existing evidence (just-written)
        // already exceeds 2× cadence. A direct topology-only test of
        // the stall classifier — wall-clock-faking would otherwise
        // require either ScanRow injection or Fabric mutation.
        use std::thread::sleep;
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();

        let mut intent = fresh_intent("standing journal", Urgency::Normal);
        intent.duration = IntentDuration::Standing {
            expected_cadence: Duration::from_millis(1),
        };
        let intent_id = fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();

        // Claim so the intent isn't unstarted.
        let claim = WorkClaim {
            intent: intent_identity,
            agent: creator.voice_print(),
            approach: "tend".into(),
            estimated_evidence_cadence: Duration::from_millis(1),
        };
        fabric
            .create(claim.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();

        // Evidence node — just one, in nisaba — linked back via Fulfills.
        let nisaba = NamespaceId::fresh("nisaba");
        let evidence_id = fabric
            .create(
                IntentNode::new("yesterday's entry").with_creator_voice(creator.voice_print()),
                &nisaba,
                None,
            )
            .unwrap();
        fabric
            .add_edge(&evidence_id, &intent_id, 1.0, RelationshipKind::Fulfills)
            .unwrap();

        // Sleep ~30ms so the 1ms cadence × 2 stall threshold (2ms) is
        // exceeded by the evidence node's age.
        sleep(Duration::from_millis(30));

        let bridge = wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.stalled.len(), 1, "snapshot: {:?}", snap);
        assert_eq!(snap.stalled[0].evidence_count, 1);
    }

    #[test]
    fn snapshot_total_matches_intent_count() {
        // Sanity test — every intent appears in exactly one bucket.
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        for i in 0..5 {
            let intent = fresh_intent(&format!("intent {}", i), Urgency::Normal);
            fabric
                .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
                .unwrap();
        }
        let bridge = wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.total_intents(), 5);
    }
}
