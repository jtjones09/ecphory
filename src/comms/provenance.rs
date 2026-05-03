// DECISION-PROVENANCE TRACING — Spec 7 §6.2 / Step 6
//
// Per Spec 7 §6.2 (Cohen I.1 FATAL fold): when a fabric change traces
// back to a `DecisionProposal` in comms, the immune system walks the
// provenance chain backward to the comms thread. It checks whether the
// operator observed any message in that thread; if not, it writes a
// `CoordinationWithoutOperator` anomaly observation.
//
// `CoordinationWithoutOperator` is an `AnomalyObservation`, NOT a
// `DamageObservation` (per Matzinger M.1: agents may have been acting
// correctly; the lack of operator awareness is a governance gap, not
// structural harm).
//
// v1 scope: a pure-read tracer plus a materialization helper. The
// caller decides whether to commit the observation. Full automatic
// hooking into the consensus-snapshot path can land alongside Spec 7
// Step 6's Nisaba-side enforcement; for v1 the primitive is what the
// immune system / nisaba-the-agent calls when it traces a snapshot
// back to a DecisionProposal-bearing thread.

use std::collections::HashSet;

use crate::bridge::BridgeFabric;
use crate::comms::message::{
    CONTENT_VARIANT_DECISION, KIND_COMMS_MESSAGE, META_CONTENT_VARIANT, META_KIND,
};
use crate::context::RelationshipKind;
use crate::identity::VoicePrint;
use crate::node::{IntentNode, MetadataValue};
use crate::signature::LineageId;

pub const KIND_COORDINATION_WITHOUT_OPERATOR: &str = "CoordinationWithoutOperator";
pub const META_THREAD_REF: &str = "__provenance_thread_ref__";
pub const META_OPERATOR_REF: &str = "__provenance_operator_ref__";
pub const META_DECISION_COUNT: &str = "__provenance_decision_count__";
/// `AnomalyObservation` per the Spec 6 immune system contract — the
/// aggregator and downstream consumers key off this metadata field.
pub const META_OBSERVATION_KIND: &str = "__bridge_observation_kind__";
pub const OBSERVATION_KIND_ANOMALY: &str = "AnomalyObservation";

/// Pure read: walk the thread topology under `RelationshipKind::Thread`
/// edges and return the LineageIds of comms messages whose content
/// variant is `Decision`.
pub fn decision_proposals_in_thread(
    bridge: &BridgeFabric,
    thread_id: &LineageId,
    max_depth: usize,
) -> Vec<LineageId> {
    let walked = bridge.traverse(thread_id, &[RelationshipKind::Thread], max_depth);
    walked
        .into_iter()
        .filter(|id| {
            bridge
                .read_inner(|inner| inner.get_node(id).cloned())
                .map(|node| {
                    let is_comms = node
                        .metadata
                        .get(META_KIND)
                        .map(|v| v.as_str_repr() == KIND_COMMS_MESSAGE)
                        .unwrap_or(false);
                    let is_decision = node
                        .metadata
                        .get(META_CONTENT_VARIANT)
                        .map(|v| v.as_str_repr() == CONTENT_VARIANT_DECISION)
                        .unwrap_or(false);
                    is_comms && is_decision
                })
                .unwrap_or(false)
        })
        .collect()
}

/// Spec 7 §6.2 (Cohen I.1): if a thread carries at least one
/// `DecisionProposal` AND the operator has not observed the thread,
/// return a `CoordinationWithoutOperator` `AnomalyObservation` node
/// ready for the caller to commit via `Fabric::create()`.
///
/// `operator_observed_threads` is the set of thread LineageIds the
/// operator's comms subscription has fired on. v1 holds this in the
/// caller (immune system / Nisaba); the fabric does not yet maintain
/// per-thread observation logs natively.
///
/// Returns `None` when:
/// - the thread has no DecisionProposal messages, OR
/// - `operator_observed_threads` contains `thread_id`.
pub fn check_coordination_without_operator(
    bridge: &BridgeFabric,
    thread_id: &LineageId,
    operator: &VoicePrint,
    operator_observed_threads: &HashSet<LineageId>,
    fabric_voice: VoicePrint,
) -> Option<IntentNode> {
    let proposals = decision_proposals_in_thread(bridge, thread_id, 32);
    if proposals.is_empty() {
        return None;
    }
    if operator_observed_threads.contains(thread_id) {
        return None;
    }
    Some(coordination_without_operator_to_node(
        thread_id,
        operator,
        proposals.len() as u64,
        fabric_voice,
    ))
}

fn coordination_without_operator_to_node(
    thread_id: &LineageId,
    operator: &VoicePrint,
    decision_count: u64,
    fabric_voice: VoicePrint,
) -> IntentNode {
    let mut node = IntentNode::new(format!(
        "[CoordinationWithoutOperator] thread {} carried {} DecisionProposal(s) but operator {} has no observation in its subscription log",
        thread_id, decision_count, operator
    ))
    .with_creator_voice(fabric_voice);
    node.metadata.insert(
        crate::comms::message::META_KIND.into(),
        MetadataValue::String(KIND_COORDINATION_WITHOUT_OPERATOR.into()),
    );
    node.metadata.insert(
        META_OBSERVATION_KIND.into(),
        MetadataValue::String(OBSERVATION_KIND_ANOMALY.into()),
    );
    node.metadata.insert(
        META_THREAD_REF.into(),
        MetadataValue::String(thread_id.as_uuid().to_string()),
    );
    node.metadata.insert(
        META_OPERATOR_REF.into(),
        MetadataValue::String(operator.to_hex()),
    );
    node.metadata.insert(
        META_DECISION_COUNT.into(),
        MetadataValue::Int(decision_count as i64),
    );
    node.recompute_signature();
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    #[test]
    fn observation_node_carries_anomaly_metadata() {
        let agent = generate_agent_keypair();
        let operator = generate_agent_keypair();
        let thread_id = LineageId::new();
        let node = coordination_without_operator_to_node(
            &thread_id,
            &operator.voice_print(),
            2,
            agent.voice_print(),
        );
        assert_eq!(
            node.metadata
                .get(META_OBSERVATION_KIND)
                .map(|v| v.as_str_repr()),
            Some(OBSERVATION_KIND_ANOMALY.into()),
        );
        assert_eq!(
            node.metadata.get(META_DECISION_COUNT).and_then(|v| v.as_f64()),
            Some(2.0)
        );
    }
}
