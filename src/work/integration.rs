// Comms integration + work-region degradation (Spec 9 Steps 8 + 9)
//
// Step 8 — when a HandoffContext (Spec 7 §4.2) is received, the
// receiving agent writes a WorkClaim linked to the work intent the
// handoff references. `claim_from_handoff` is the helper. Decision
// provenance (Cohen I.1 from Spec 7) applies through the same comms
// machinery — the v1 helper just creates the claim and links it.
//
// Step 9 — work-region degradation mirrors Spec 7's comms degradation:
//
//   - `WorkHealth { Healthy | Degraded { reason } }` is the caller's
//     authoritative signal (immune-system flag, P53 trigger, region
//     unreachable).
//   - `submit_evidence_or_fallback` writes a Fulfills-linked evidence
//     node when the work region is healthy; otherwise writes a marked
//     `work_degraded` node into the agent's operational region.
//   - `replay_degraded_into_work` rewires the markers as proper
//     Fulfills edges once the work region is healthy again.

use std::time::Duration;

use crate::bridge::BridgeFabric;
use crate::comms::message::HandoffContext;
use crate::context::RelationshipKind;
use crate::fabric::FabricError;
use crate::identity::{AgentKeypair, EditMode, NamespaceId, NodeIdentity, VoicePrint, WriteError};
use crate::node::{IntentNode, MetadataValue};
use crate::signature::LineageId;

use super::claim::WorkClaim;
use super::hotash_work;

/// Step 8 default cadence assumption when the handoff carries no
/// deadline. 1h is short enough to feel responsive, long enough that
/// most reasonable agents won't trip the stall threshold by accident.
const DEFAULT_HANDOFF_CADENCE: Duration = Duration::from_secs(3600);

/// Build (and write) a WorkClaim from a HandoffContext that references
/// the work intent at `intent`. The receiving agent is the claim's
/// agent; `creator_voice` (typically the same agent's voice print)
/// signs the claim node.
///
/// Returns the claim's LineageId. Cadence is derived from the
/// handoff's deadline (`deadline - now`) when present, else
/// `DEFAULT_HANDOFF_CADENCE`.
pub fn claim_from_handoff(
    bridge: &BridgeFabric,
    handoff: &HandoffContext,
    intent: NodeIdentity,
    receiving_agent: VoicePrint,
    signer: &AgentKeypair,
) -> Result<LineageId, WriteError> {
    let cadence = handoff
        .deadline
        .as_ref()
        .map(|d| {
            let elapsed = d.elapsed_secs();
            if elapsed >= 0.0 {
                Duration::from_secs(elapsed as u64)
            } else {
                Duration::from_secs((-elapsed) as u64)
            }
        })
        .unwrap_or(DEFAULT_HANDOFF_CADENCE);

    let claim = WorkClaim {
        intent,
        agent: receiving_agent,
        approach: handoff.task_description.clone(),
        estimated_evidence_cadence: cadence,
    };
    let node = claim.to_intent_node(signer.voice_print());
    bridge.create_in(node, &hotash_work(), EditMode::AppendOnly, Some(signer))
}

// ── Step 9: work-region degradation ───────────────────────────────

/// Health snapshot for the work region. Caller-supplied.
#[derive(Debug, Clone)]
pub enum WorkHealth {
    Healthy,
    Degraded { reason: String },
}

pub const META_WORK_DEGRADED_FLAG: &str = "__work_degraded__";
pub const META_WORK_DEGRADED_REASON: &str = "__work_degraded_reason__";
pub const META_WORK_DEGRADED_TARGET_FP: &str = "__work_degraded_target_fp__";
pub const META_WORK_REPLAYED_AT_NS: &str = "__work_replayed_at_ns__";

/// Result of `submit_evidence_or_fallback`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceOutcome {
    /// Wrote evidence into the operational region and added a
    /// Fulfills edge to the target intent.
    Linked { evidence_id: LineageId },
    /// Wrote evidence into the operational region with the
    /// `__work_degraded__` marker but did NOT add the Fulfills edge
    /// (work region is unhealthy). Replay later via
    /// `replay_degraded_into_work`.
    Degraded { evidence_id: LineageId },
}

impl EvidenceOutcome {
    pub fn evidence_id(&self) -> &LineageId {
        match self {
            EvidenceOutcome::Linked { evidence_id }
            | EvidenceOutcome::Degraded { evidence_id } => evidence_id,
        }
    }
    pub fn was_degraded(&self) -> bool {
        matches!(self, EvidenceOutcome::Degraded { .. })
    }
}

/// Write `evidence_node` into `operational_namespace`. If the work
/// region is healthy, also wire a Fulfills edge from the evidence to
/// the target intent. If the work region is degraded, stamp the
/// evidence with the degraded marker (target intent's content
/// fingerprint travels in metadata for later replay) and skip the
/// edge.
pub fn submit_evidence_or_fallback(
    bridge: &BridgeFabric,
    operational_namespace: &NamespaceId,
    target_intent: &NodeIdentity,
    target_intent_id: &LineageId,
    evidence_node: IntentNode,
    health: &WorkHealth,
    signer: Option<&AgentKeypair>,
) -> Result<EvidenceOutcome, WriteError> {
    let work_region = hotash_work();
    let work_unreachable = bridge.is_region_terminated(&work_region);

    let degraded_reason = match health {
        WorkHealth::Healthy if !work_unreachable => None,
        WorkHealth::Healthy => Some(format!(
            "work namespace {} terminated by P53",
            work_region.name
        )),
        WorkHealth::Degraded { reason } => Some(reason.clone()),
    };

    if degraded_reason.is_none() {
        let evidence_id =
            bridge.create_in(evidence_node, operational_namespace, EditMode::AppendOnly, signer)?;
        bridge
            .add_edge(&evidence_id, target_intent_id, 1.0, RelationshipKind::Fulfills)
            .map_err(|e| match e {
                FabricError::NodeNotFound(_) => {
                    WriteError::FabricDegraded // unexpected — bubble as degraded
                }
                _ => WriteError::FabricDegraded,
            })?;
        return Ok(EvidenceOutcome::Linked { evidence_id });
    }

    let mut node = evidence_node;
    node.metadata.insert(
        META_WORK_DEGRADED_FLAG.into(),
        MetadataValue::Bool(true),
    );
    if let Some(reason) = degraded_reason {
        node.metadata.insert(
            META_WORK_DEGRADED_REASON.into(),
            MetadataValue::String(reason),
        );
    }
    node.metadata.insert(
        META_WORK_DEGRADED_TARGET_FP.into(),
        MetadataValue::String(target_intent.content_fingerprint.to_hex()),
    );
    node.recompute_signature();

    let id = bridge.create_in(node, operational_namespace, EditMode::AppendOnly, signer)?;
    Ok(EvidenceOutcome::Degraded { evidence_id: id })
}

/// True when the node carries the work-degraded fallback flag.
pub fn is_work_degraded_fallback(node: &IntentNode) -> bool {
    node.metadata
        .get(META_WORK_DEGRADED_FLAG)
        .map(|v| matches!(v, MetadataValue::Bool(true)))
        .unwrap_or(false)
}

/// On work-region recovery: scan the fabric for nodes carrying the
/// `__work_degraded__` marker, look up the target intent by the
/// fingerprint stored in metadata, and add the Fulfills edge that
/// was deferred during the degradation window. Returns the edges
/// successfully replayed (evidence_id → intent_id).
///
/// `already_replayed` lets the caller skip evidence handled in a
/// prior invocation. v1 doesn't mutate the original node to mark it
/// replayed — callers track this in memory.
pub fn replay_degraded_into_work(
    bridge: &BridgeFabric,
    already_replayed: &std::collections::HashSet<LineageId>,
) -> Vec<(LineageId, LineageId)> {
    let candidates: Vec<(LineageId, String)> = bridge.read_inner(|inner| {
        inner
            .nodes()
            .filter(|(id, n)| !already_replayed.contains(id) && is_work_degraded_fallback(n))
            .filter_map(|(id, n)| {
                n.metadata
                    .get(META_WORK_DEGRADED_TARGET_FP)
                    .map(|v| (id.clone(), v.as_str_repr()))
            })
            .collect()
    });

    let mut linked = Vec::new();
    for (evidence_id, target_fp) in candidates {
        let target_id = bridge.read_inner(|inner| {
            inner
                .nodes()
                .find(|(_, n)| n.content_fingerprint().to_hex() == target_fp)
                .map(|(id, _)| id.clone())
        });
        let Some(target_id) = target_id else {
            continue;
        };
        if bridge
            .add_edge(&evidence_id, &target_id, 1.0, RelationshipKind::Fulfills)
            .is_ok()
        {
            linked.push((evidence_id, target_id));
        }
    }
    linked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::message::Urgency;
    use crate::fabric::Fabric;
    use crate::identity::generate_agent_keypair;
    use crate::work::intent::{IntentDuration, WorkIntent};
    use crate::work::{query_visibility, VisibilityConfig};

    fn intent(description: &str) -> WorkIntent {
        WorkIntent {
            description: description.into(),
            intended_outcome: "ok".into(),
            constraints: vec![],
            success_checks: vec![],
            requested_by: generate_agent_keypair().voice_print(),
            urgency: Urgency::Normal,
            context: vec![],
            duration: IntentDuration::Discrete { deadline: None },
        }
    }

    #[test]
    fn handoff_produces_linked_work_claim() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent_id = fabric
            .create(
                intent("ship the visibility substrate")
                    .to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();

        let bridge = BridgeFabric::wrap(fabric);
        let receiving = generate_agent_keypair();
        let handoff = HandoffContext {
            task_description: "implement Steps 5-7".into(),
            source_context: vec![intent_identity.clone()],
            constraints: vec![],
            success_criteria: vec!["spec acceptance criteria 12 pass".into()],
            success_checks: vec![],
            deadline: None,
            escalation_path: receiving.voice_print(),
        };
        let claim_id =
            claim_from_handoff(&bridge, &handoff, intent_identity, receiving.voice_print(), &receiving)
                .unwrap();

        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        // Intent + claim → intent shows as Active (has claim).
        assert_eq!(snap.active.len(), 1, "snapshot: {:?}", snap);
        assert_eq!(snap.active[0].claim_count, 1);

        // Verify the claim node's metadata.
        bridge.read_inner(|inner| {
            let claim = inner.get_node(&claim_id).expect("claim must exist");
            assert!(WorkClaim::is_claim_node(claim));
        });
    }

    #[test]
    fn evidence_submit_links_when_healthy() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent_id = fabric
            .create(
                intent("standing journal").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();
        let bridge = BridgeFabric::wrap(fabric);

        let nisaba = NamespaceId::fresh("nisaba");
        let evidence = IntentNode::new("today's journal entry")
            .with_creator_voice(creator.voice_print());
        let outcome = submit_evidence_or_fallback(
            &bridge,
            &nisaba,
            &intent_identity,
            &intent_id,
            evidence,
            &WorkHealth::Healthy,
            None,
        )
        .unwrap();

        assert!(matches!(outcome, EvidenceOutcome::Linked { .. }));
        // Verify the Fulfills edge exists.
        bridge.read_inner(|inner| {
            let edges = inner.edges_from(outcome.evidence_id());
            assert!(edges
                .iter()
                .any(|e| e.target == intent_id && matches!(e.kind, RelationshipKind::Fulfills)));
        });
    }

    #[test]
    fn evidence_submit_falls_back_when_degraded() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let intent_id = fabric
            .create(
                intent("on a degraded day").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let intent_identity = fabric.node_identity(&intent_id).unwrap();
        let bridge = BridgeFabric::wrap(fabric);

        let nisaba = NamespaceId::fresh("nisaba");
        let evidence = IntentNode::new("survives the outage")
            .with_creator_voice(creator.voice_print());
        let outcome = submit_evidence_or_fallback(
            &bridge,
            &nisaba,
            &intent_identity,
            &intent_id,
            evidence,
            &WorkHealth::Degraded {
                reason: "test outage".into(),
            },
            None,
        )
        .unwrap();

        assert!(matches!(outcome, EvidenceOutcome::Degraded { .. }));
        // No Fulfills edge yet.
        bridge.read_inner(|inner| {
            let edges = inner.edges_from(outcome.evidence_id());
            assert!(edges.is_empty());
            // But the marker is on the node.
            let node = inner.get_node(outcome.evidence_id()).unwrap();
            assert!(is_work_degraded_fallback(node));
        });

        // Replay re-wires the deferred edge.
        let already = std::collections::HashSet::new();
        let linked = replay_degraded_into_work(&bridge, &already);
        assert_eq!(linked.len(), 1);
        bridge.read_inner(|inner| {
            let edges = inner.edges_from(outcome.evidence_id());
            assert!(edges
                .iter()
                .any(|e| e.target == intent_id && matches!(e.kind, RelationshipKind::Fulfills)));
        });
    }
}
