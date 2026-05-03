// VISIBILITY SUBSTRATE — Work as fabric state (Spec 9 v1.1)
//
// Per Spec 9 §1: "Work produces its own visibility as a side effect of
// being done, not as a separate reporting activity." A `WorkIntent` is
// an attractor in the fabric's state space — a node that draws agent
// activity through capability matching and urgency. Status is computed
// from the intent's relationship to its evidence (Step 3 visibility
// query); there is no `set_status()` method anywhere.
//
// Step 1 scope (this module):
// - WorkIntent / WorkClaim / IntentDuration types
// - Materialization helpers (`to_intent_node`) following the Spec 7
//   comms pattern
// - Gravity function: `urgency × relevance × evidence_gap` with a
//   tunable switching-cost inertia (per Kingsbury K.S1 fold)
// - `hotash:work` region constant
//
// Steps 2-10 (future):
// - Step 2: `RelationshipKind::Fulfills` cross-region edge
// - Step 3: visibility query — eight categories from topology
// - Step 4: `fabric_work_status` MCP tool on Nabu
// - Step 5: split / merge via `split_from` + `converged_with` edges
// - Step 6: standing-intent cadence auto-calibration
// - Step 7: `WorkObserver` cell-agent specialization
// - Step 8: comms integration (HandoffContext → WorkClaim)
// - Step 9: work-region degradation fallback
// - Step 10: full §9 acceptance criteria

pub mod claim;
pub mod gravity;
pub mod intent;
pub mod visibility;

pub use claim::WorkClaim;
pub use gravity::{gravity, AgentProfile, SWITCHING_COST};
pub use intent::{IntentDuration, WorkIntent};
pub use visibility::{
    query_visibility, IntentSummary, VisibilityConfig, VisibilitySnapshot, WorkStatus,
};

use crate::identity::NamespaceId;
use uuid::Uuid;

/// Stable UUID for the `hotash:work` region. Mirrors the
/// `hotash:comms` convention from Spec 7 — agents subscribe by UUID,
/// not by name lookup, so the value must not drift across instances.
pub fn hotash_work() -> NamespaceId {
    NamespaceId::new(
        "hotash:work",
        Uuid::from_bytes([
            0xec, 0x44, 0x07, 0x09, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x09,
        ]),
    )
}

// ─── Metadata keys for materialized WorkIntent / WorkClaim nodes ───
// Downstream observers (visibility query in Step 3, WorkObserver in
// Step 7) parse these without re-rendering the node body. Key names
// match the Spec 7 comms convention: snake_case, `intent_*` /
// `claim_*` prefixed by content type.

pub const META_KIND: &str = "__work_kind__";
pub const KIND_WORK_INTENT: &str = "work_intent";
pub const KIND_WORK_CLAIM: &str = "work_claim";

pub const META_INTENT_OUTCOME: &str = "intent_outcome";
pub const META_INTENT_URGENCY: &str = "intent_urgency";
pub const META_INTENT_DURATION_KIND: &str = "intent_duration_kind";
pub const META_INTENT_DEADLINE_NS: &str = "intent_deadline_ns";
pub const META_INTENT_CADENCE_SECS: &str = "intent_cadence_secs";
pub const META_INTENT_CONSTRAINTS: &str = "intent_constraints";
pub const META_INTENT_REQUESTED_BY: &str = "intent_requested_by";
pub const META_INTENT_CHECK_COUNT: &str = "intent_check_count";

pub const META_CLAIM_INTENT_FINGERPRINT: &str = "claim_intent_fingerprint";
pub const META_CLAIM_INTENT_NAMESPACE: &str = "claim_intent_namespace";
pub const META_CLAIM_AGENT: &str = "claim_agent";
pub const META_CLAIM_APPROACH: &str = "claim_approach";
pub const META_CLAIM_CADENCE_SECS: &str = "claim_cadence_secs";

pub const DURATION_DISCRETE: &str = "discrete";
pub const DURATION_STANDING: &str = "standing";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::message::Urgency;
    use crate::context::RelationshipKind;
    use crate::fabric::Fabric;
    use crate::identity::generate_agent_keypair;
    use crate::work::intent::{IntentDuration, WorkIntent};

    #[test]
    fn hotash_work_is_stable() {
        let a = hotash_work();
        let b = hotash_work();
        assert_eq!(a.uuid, b.uuid);
        assert_eq!(a.name, "hotash:work");
    }

    #[test]
    fn hotash_work_is_distinct_from_comms() {
        let work = hotash_work();
        let comms = NamespaceId::hotash_comms();
        assert_ne!(work.uuid, comms.uuid);
    }

    /// Step 2 acceptance: an evidence node in `hotash:nisaba` linked
    /// to a work intent in `hotash:work` via `Fulfills` is reachable
    /// by traversal. The fabric does not constrain edges to a single
    /// region — the cross-region traversal is the whole point.
    #[test]
    fn fulfills_edge_traverses_across_regions() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();

        let intent = WorkIntent {
            description: "publish session 21 journal".into(),
            intended_outcome: "JOURNAL.md updated".into(),
            constraints: vec![],
            success_checks: vec![],
            requested_by: creator.voice_print(),
            urgency: Urgency::Normal,
            context: vec![],
            duration: IntentDuration::Standing {
                expected_cadence: std::time::Duration::from_secs(86_400),
            },
        };
        let intent_node = intent.to_intent_node(creator.voice_print());
        let intent_id = fabric.create(intent_node, &hotash_work(), None).unwrap();

        // Evidence lives in nisaba — that's the whole point of the
        // cross-region edge: evidence is wherever the work happens.
        let nisaba = NamespaceId::fresh("hotash:nisaba");
        let evidence_node = crate::node::IntentNode::new("Session 21 journal entry committed")
            .with_creator_voice(creator.voice_print());
        let evidence_id = fabric.create(evidence_node, &nisaba, None).unwrap();

        fabric
            .add_edge(&evidence_id, &intent_id, 1.0, RelationshipKind::Fulfills)
            .unwrap();

        // Traverse from evidence to intent — finds the cross-region
        // target and reports the Fulfills relationship.
        let edges_out = fabric.edges_from(&evidence_id);
        assert_eq!(edges_out.len(), 1);
        assert_eq!(edges_out[0].target, intent_id);
        assert!(matches!(edges_out[0].kind, RelationshipKind::Fulfills));

        // Reverse traversal — find evidence pointing at the intent.
        let incoming: Vec<_> = fabric.edges_to(&intent_id).into_iter().collect();
        assert_eq!(incoming.len(), 1);
        assert_eq!(*incoming[0], evidence_id);

        // The intent and evidence sit in different regions —
        // confirm via the fabric's identity surface.
        let intent_identity = fabric.node_identity(&intent_id).unwrap();
        let evidence_identity = fabric.node_identity(&evidence_id).unwrap();
        assert_eq!(intent_identity.causal_position.namespace.name, "hotash:work");
        assert_eq!(
            evidence_identity.causal_position.namespace.name,
            "hotash:nisaba"
        );
    }
}
