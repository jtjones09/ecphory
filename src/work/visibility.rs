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
    /// Step 5: this intent has at least one child linked via
    /// `SplitFrom` — it's the parent of a split.
    has_split_children: bool,
}

/// Spec 9 §3.2 categories (eight) plus Step 5's `Split` for parent
/// intents that have been broken into children. Spec text lists
/// "split — see children" as a distinct status; v1.1 adds it as a
/// proper variant rather than overloading another category.
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
    /// Step 5 / K.S2 fold: this intent has at least one child linked
    /// via `RelationshipKind::SplitFrom`. The parent is preserved as
    /// provenance; the live work happens on the children.
    Split,
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
            WorkStatus::Split => "split",
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
    /// Step 6: effective stall threshold actually used. `None` if the
    /// intent hasn't accumulated enough evidence to calibrate
    /// (`< calibration_min_observations`); falls back to the declared
    /// `expected_cadence` or `default_discrete_cadence_secs`.
    pub calibrated_cadence_secs: Option<f64>,
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
    pub split: Vec<IntentSummary>,
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
            + self.split.len()
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
            WorkStatus::Split => self.split.push(summary),
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
    /// Step 6 / Gershman G.2 fold: minimum observed-evidence count
    /// before the calibrated cadence replaces the agent's self-
    /// estimate. Default 10.
    pub calibration_min_observations: usize,
}

impl Default for VisibilityConfig {
    fn default() -> Self {
        Self {
            stall_factor: 2.0,
            default_discrete_cadence_secs: 86_400.0,
            orphan_age_secs: 4.0 * 3600.0,
            abandoned_weight_threshold: 0.1,
            calibration_min_observations: 10,
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
                    has_split_children: false,
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

    // Second pass: walk incoming edges to each intent. Collect all
    // evidence ages (Fulfills source-node ages) per intent so Step 6
    // can compute calibrated cadence; flag split parents when any
    // SplitFrom edge points at the intent.
    let mut split_parents: std::collections::HashSet<LineageId> =
        std::collections::HashSet::new();
    let mut evidence_ages_by_fp: HashMap<String, Vec<f64>> = HashMap::new();
    bridge.read_inner(|inner| {
        for (intent_id, fp) in &intent_fingerprints {
            let mut ages: Vec<f64> = Vec::new();
            for source_id in inner.edges_to(intent_id) {
                let edges = inner.edges_from(source_id);
                for edge in edges {
                    if edge.target != *intent_id {
                        continue;
                    }
                    match edge.kind {
                        RelationshipKind::Fulfills => {
                            if let Some(age) = inner.node_age_secs(source_id) {
                                ages.push(age);
                            }
                        }
                        RelationshipKind::SplitFrom => {
                            split_parents.insert(intent_id.clone());
                        }
                        _ => {}
                    }
                }
            }
            if !ages.is_empty() {
                let count = ages.len();
                let last_age = ages.iter().cloned().fold(f64::INFINITY, f64::min);
                evidence_by_fp.insert(fp.clone(), (last_age, count));
                evidence_ages_by_fp.insert(fp.clone(), ages);
            }
        }
    });

    // Stamp split-parent flag onto the rows.
    for row in intent_rows.iter_mut() {
        if split_parents.contains(&row.lineage_id) {
            row.has_split_children = true;
        }
    }

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

        // Step 6 / G.2 fold: compute calibrated cadence from observed
        // intervals once we have enough evidence. Below the threshold
        // the declared cadence (or default) is used.
        let calibrated_cadence_secs = evidence_ages_by_fp
            .get(&fp)
            .and_then(|ages| calibrated_cadence(ages, config.calibration_min_observations));

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
            calibrated_cadence_secs,
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
            calibrated_cadence_secs,
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

/// Mean of consecutive intervals between evidence write-times.
/// `ages` is the list of evidence-node ages (seconds since creation)
/// — sorting them descending gives oldest-first. The differences
/// between consecutive entries are the gaps between writes.
/// Returns `None` if `ages.len() < min_observations`.
pub fn calibrated_cadence(ages: &[f64], min_observations: usize) -> Option<f64> {
    if ages.len() < min_observations || ages.len() < 2 {
        return None;
    }
    let mut sorted = ages.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let mut sum = 0.0;
    for w in sorted.windows(2) {
        sum += w[0] - w[1];
    }
    let n_intervals = (sorted.len() - 1) as f64;
    if n_intervals == 0.0 {
        None
    } else {
        Some(sum / n_intervals)
    }
}

fn classify(
    row: &ScanRow,
    claim_count: usize,
    evidence_count: usize,
    last_evidence_age_secs: Option<f64>,
    passing_checks: usize,
    total_checks: usize,
    calibrated_cadence_secs: Option<f64>,
    config: &VisibilityConfig,
) -> WorkStatus {
    // Decision order matters: first match wins. Ordering captures the
    // priority the spec implies — split (parent of children) is a
    // structural marker that overrides everything else; then
    // abandonment as a final state; then operator-attention
    // categories (orphaned/overloaded); then success/failure; then
    // topology-only.

    if row.has_split_children {
        return WorkStatus::Split;
    }

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
        // Step 6 / G.2 fold: prefer the calibrated cadence. Falls
        // back to the declared cadence, then to the default.
        let cadence = calibrated_cadence_secs
            .or(row.cadence_secs)
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
    fn calibrated_cadence_returns_none_below_threshold() {
        // 9 observations < 10 (default min) → None
        let ages: Vec<f64> = (1..=9).map(|i| i as f64 * 30.0).collect();
        assert!(super::calibrated_cadence(&ages, 10).is_none());
    }

    #[test]
    fn calibrated_cadence_computes_mean_interval_at_threshold() {
        // 10 evenly-spaced ages → mean interval = 30.
        // Ages: [30, 60, 90, ..., 300], sorted descending give
        // intervals of 30 each.
        let ages: Vec<f64> = (1..=10).map(|i| i as f64 * 30.0).collect();
        let cadence = super::calibrated_cadence(&ages, 10).unwrap();
        assert!((cadence - 30.0).abs() < 0.001);
    }

    #[test]
    fn calibrated_cadence_overrides_declared_cadence_in_stall_check() {
        // Standing intent declared at 1h cadence. 10 evidence nodes
        // at much faster intervals. Stall threshold should track the
        // calibrated cadence, not the declared one.
        //
        // Use stall_factor=2 (default) and calibration_min=2 so the
        // test doesn't have to accumulate 10 evidence nodes.
        use std::thread::sleep;

        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();

        let mut intent = fresh_intent("calibrated stall", Urgency::Normal);
        intent.duration = IntentDuration::Standing {
            expected_cadence: Duration::from_secs(3600), // declared 1h
        };
        let intent_id = fabric
            .create(intent.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();

        // Claim it.
        let claim = WorkClaim {
            intent: intent_identity,
            agent: creator.voice_print(),
            approach: "tend".into(),
            estimated_evidence_cadence: Duration::from_secs(60),
        };
        fabric
            .create(claim.to_intent_node(creator.voice_print()), &hotash_work(), None)
            .unwrap();

        // Two evidence nodes ~10ms apart so calibrated cadence ≈ 0.01s.
        let nisaba = NamespaceId::fresh("nisaba");
        let e1 = fabric
            .create(
                IntentNode::new("ev1").with_creator_voice(creator.voice_print()),
                &nisaba,
                None,
            )
            .unwrap();
        sleep(Duration::from_millis(10));
        let e2 = fabric
            .create(
                IntentNode::new("ev2").with_creator_voice(creator.voice_print()),
                &nisaba,
                None,
            )
            .unwrap();
        fabric.add_edge(&e1, &intent_id, 1.0, RelationshipKind::Fulfills).unwrap();
        fabric.add_edge(&e2, &intent_id, 1.0, RelationshipKind::Fulfills).unwrap();

        // Wait long enough that 2× calibrated (≈0.02s) is exceeded
        // but 2× declared (7200s) is nowhere near.
        sleep(Duration::from_millis(50));

        let bridge = wrap(fabric);
        let mut config = VisibilityConfig::default();
        config.calibration_min_observations = 2; // make calibration kick in with 2

        let snap = query_visibility(&bridge, &config, |_| Vec::new());
        assert_eq!(snap.stalled.len(), 1, "snapshot: {:?}", snap);
        let s = &snap.stalled[0];
        assert!(s.calibrated_cadence_secs.is_some());
        // The declared cadence was 3600s; calibrated should be tiny.
        assert!(s.calibrated_cadence_secs.unwrap() < 1.0);
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
