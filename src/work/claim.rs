// WorkClaim — an agent's commitment to work an intent (Spec 9 §4.1)
//
// A claim is observable. Multiple agents can claim the same intent
// (Kingsbury K.2 fold — biological over-response is acceptable). The
// claim carries a self-estimated `expected_evidence_cadence` that
// seeds the immune-baseline calibration in Step 6 — the calibrated
// cadence replaces the estimate after N=10 evidence nodes.

use std::time::Duration;

use crate::identity::{NodeIdentity, VoicePrint};
use crate::node::{IntentNode, MetadataValue};

use super::*;

#[derive(Debug, Clone)]
pub struct WorkClaim {
    /// The intent being claimed. The four-tuple identity carries the
    /// content fingerprint + namespace so the fingerprint metadata
    /// below can be computed and cross-region edges can be wired in
    /// Step 2.
    pub intent: NodeIdentity,
    pub agent: VoicePrint,
    pub approach: String,
    pub estimated_evidence_cadence: Duration,
}

impl WorkClaim {
    pub fn to_intent_node(&self, creator: VoicePrint) -> IntentNode {
        let body = format!(
            "[WorkClaim] {} → intent {}\nApproach: {}\nEstimated cadence: {}s",
            self.agent,
            self.intent.content_fingerprint.to_hex(),
            self.approach,
            self.estimated_evidence_cadence.as_secs()
        );
        let mut node = IntentNode::new(body).with_creator_voice(creator);

        node.metadata.insert(
            META_KIND.into(),
            MetadataValue::String(KIND_WORK_CLAIM.into()),
        );
        node.metadata.insert(
            META_CLAIM_INTENT_FINGERPRINT.into(),
            MetadataValue::String(self.intent.content_fingerprint.to_hex()),
        );
        node.metadata.insert(
            META_CLAIM_INTENT_NAMESPACE.into(),
            MetadataValue::String(self.intent.causal_position.namespace.name.clone()),
        );
        node.metadata.insert(
            META_CLAIM_AGENT.into(),
            MetadataValue::String(self.agent.to_hex()),
        );
        node.metadata.insert(
            META_CLAIM_APPROACH.into(),
            MetadataValue::String(self.approach.clone()),
        );
        node.metadata.insert(
            META_CLAIM_CADENCE_SECS.into(),
            MetadataValue::Int(self.estimated_evidence_cadence.as_secs() as i64),
        );

        node.recompute_signature();
        node
    }

    /// True if a materialized node was a WorkClaim.
    pub fn is_claim_node(node: &IntentNode) -> bool {
        node.metadata
            .get(META_KIND)
            .map(|v| v.as_str_repr() == KIND_WORK_CLAIM)
            .unwrap_or(false)
    }

    /// Extract the claiming agent's voice print hex from a materialized
    /// claim node. Returns `None` if the metadata is missing or this
    /// is not a claim node.
    pub fn agent_hex_from_node(node: &IntentNode) -> Option<String> {
        if !Self::is_claim_node(node) {
            return None;
        }
        node.metadata
            .get(META_CLAIM_AGENT)
            .map(|v| v.as_str_repr())
    }

    /// Extract the claimed intent's content-fingerprint hex.
    pub fn intent_fingerprint_from_node(node: &IntentNode) -> Option<String> {
        if !Self::is_claim_node(node) {
            return None;
        }
        node.metadata
            .get(META_CLAIM_INTENT_FINGERPRINT)
            .map(|v| v.as_str_repr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;
    use crate::work::intent::{IntentDuration, WorkIntent};
    use crate::comms::message::Urgency;

    fn sample_intent_identity() -> NodeIdentity {
        // For Step 1 unit-test purposes we hand-build a NodeIdentity.
        // Step 3's visibility query will use the real identity
        // returned by `Fabric::create()` / `node_identity()`.
        let kp = generate_agent_keypair();
        let intent = WorkIntent {
            description: "test intent".into(),
            intended_outcome: "tested".into(),
            constraints: vec![],
            success_checks: vec![],
            requested_by: kp.voice_print(),
            urgency: Urgency::Normal,
            context: vec![],
            duration: IntentDuration::Discrete { deadline: None },
        };
        let node = intent.to_intent_node(kp.voice_print());
        crate::identity::NodeIdentity::new(
            node.content_fingerprint().clone(),
            crate::identity::CausalPosition::new(
                crate::temporal::LamportTimestamp::new(1),
                crate::temporal::FabricInstant::now(),
                hotash_work(),
            ),
            Some(kp.voice_print()),
            crate::identity::node_identity::TopologicalPosition::new(0, 0, [0u8; 32]),
        )
    }

    #[test]
    fn claim_materializes_with_agent_metadata() {
        let agent_kp = generate_agent_keypair();
        let claim = WorkClaim {
            intent: sample_intent_identity(),
            agent: agent_kp.voice_print(),
            approach: "implement step by step".into(),
            estimated_evidence_cadence: Duration::from_secs(900),
        };
        let node = claim.to_intent_node(agent_kp.voice_print());
        assert!(WorkClaim::is_claim_node(&node));
        assert_eq!(
            WorkClaim::agent_hex_from_node(&node).unwrap(),
            agent_kp.voice_print().to_hex()
        );
    }

    #[test]
    fn cadence_round_trips_as_int_metadata() {
        let agent_kp = generate_agent_keypair();
        let claim = WorkClaim {
            intent: sample_intent_identity(),
            agent: agent_kp.voice_print(),
            approach: "x".into(),
            estimated_evidence_cadence: Duration::from_secs(7200),
        };
        let node = claim.to_intent_node(agent_kp.voice_print());
        let cadence = match node.metadata.get(META_CLAIM_CADENCE_SECS).unwrap() {
            MetadataValue::Int(n) => *n,
            _ => panic!("cadence should be Int"),
        };
        assert_eq!(cadence, 7200);
    }
}
