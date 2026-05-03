// DECISION PROPOSAL FLOW — Spec 7 §4.3 / Step 5
//
// Per Spec 7 §4.3 (Kingsbury K.2 fold): "When a `checkout` fires from a
// `DecisionProposal`, the comms layer checks for existing open
// checkouts on the same target node. If a concurrent checkout exists,
// the layer writes a `ConflictDetected` message to the thread …
// Informational, not blocking — doesn't prevent concurrent proposals,
// just makes the conflict visible in the conversation before the
// consensus snapshot resolves it."
//
// Step-5 deliverables:
// - `ConflictDetected` struct + materialization helper
// - `submit_decision_proposal(bridge, message, signer)` — writes the
//   proposal, opens checkouts on each affected node, writes a
//   `ConflictDetected` per target where a prior Open checkout exists.

use std::time::Duration;

use crate::bridge::{BridgeFabric, CheckoutHandle, FabricTrait};
use crate::comms::message::{
    CommsMessage, DecisionProposal, MessageContent, MessageIntent, Sensitivity, Urgency,
};
use crate::context::RelationshipKind;
use crate::identity::{AgentKeypair, EditMode, NodeIdentity, VoicePrint, WriteError};
use crate::node::{IntentNode, MetadataValue};
use crate::signature::LineageId;

pub const KIND_CONFLICT_DETECTED: &str = "ConflictDetected";
pub const META_KIND: &str = "__bridge_node_kind__";
pub const META_CONFLICT_TARGET: &str = "__comms_conflict_target__";
pub const META_CONFLICT_PROPOSERS: &str = "__comms_conflict_proposers__";
pub const META_CONFLICT_PROPOSAL_REF: &str = "__comms_conflict_proposal_ref__";

/// Default TTL for checkouts opened by `submit_decision_proposal`. Spec
/// 7 doesn't fix this; v1 uses 60s as a reasonable default for
/// human-mediated decisions.
pub const DEFAULT_DECISION_CHECKOUT_TTL: Duration = Duration::from_secs(60);

/// Surface emitted when an agent submits a `DecisionProposal` through
/// the comms layer. The caller drives the resulting checkouts through
/// `propose` / `finalize_proposal` per Spec 8 §3.4.
pub struct DecisionSubmission {
    /// LineageId of the comms-region message that carries the proposal.
    pub message_id: LineageId,
    /// LineageIds of any `ConflictDetected` markers written. One per
    /// affected node where a prior Open checkout already existed.
    pub conflicts: Vec<LineageId>,
    /// Checkouts the comms layer opened on `decision.affected_nodes`.
    /// Empty when no targets are present or all checkouts failed.
    pub checkouts: Vec<CheckoutHandle>,
}

/// Informational marker written into the thread when a second decision
/// proposal lands on a target that's already under an Open checkout.
#[derive(Debug, Clone)]
pub struct ConflictDetected {
    /// The target node whose state is being contested.
    pub target_node: LineageId,
    /// Voice prints of agents whose proposals collided. v1 records the
    /// new proposer; the prior proposer is recoverable from the
    /// thread's prior `DecisionProposal` messages.
    pub proposers: Vec<VoicePrint>,
    /// The `DecisionProposal` message that triggered this marker.
    pub triggering_message: NodeIdentity,
    /// Human-readable explanation. Surfaced through the projection
    /// bridge (Step 10) so participants see WHAT collided.
    pub explanation: String,
}

impl ConflictDetected {
    pub fn to_intent_node(&self, voice: VoicePrint) -> IntentNode {
        let mut node = IntentNode::new(format!(
            "[ConflictDetected] {} — {}",
            self.target_node, self.explanation
        ))
        .with_creator_voice(voice);
        node.metadata.insert(
            META_KIND.into(),
            MetadataValue::String(KIND_CONFLICT_DETECTED.into()),
        );
        node.metadata.insert(
            META_CONFLICT_TARGET.into(),
            MetadataValue::String(self.target_node.as_uuid().to_string()),
        );
        if !self.proposers.is_empty() {
            let proposers = self
                .proposers
                .iter()
                .map(|v| v.to_hex())
                .collect::<Vec<_>>()
                .join(",");
            node.metadata
                .insert(META_CONFLICT_PROPOSERS.into(), MetadataValue::String(proposers));
        }
        node.metadata.insert(
            META_CONFLICT_PROPOSAL_REF.into(),
            MetadataValue::String(self.triggering_message.content_fingerprint.to_hex()),
        );
        node.recompute_signature();
        node
    }
}

/// Submit a `DecisionProposal` through the comms layer.
///
/// Flow per Spec 7 §4.3:
/// 1. Write the proposal as a comms-region `CommsMessage`.
/// 2. For each node in `decision.affected_nodes`:
///    - If a prior Open checkout exists, write a `ConflictDetected`
///      marker (and link it into the proposal's thread, if any).
///    - Open this proposer's checkout regardless. The Spec 8 §3.4.3
///      consensus mechanism resolves contention; the marker is purely
///      informational so participants see the collision before the
///      snapshot lock fires.
///
/// `proposal.content` MUST be `MessageContent::Decision(_)`. Anything
/// else returns `WriteError::FabricInternal`. Checkouts that fail with
/// a writable error (NodeLocked, EditModeMismatch, etc.) are skipped;
/// the returned `DecisionSubmission.checkouts` reflects what actually
/// opened.
pub fn submit_decision_proposal(
    bridge: &BridgeFabric,
    proposal: &CommsMessage,
    signer: &AgentKeypair,
) -> Result<DecisionSubmission, WriteError> {
    let decision = match &proposal.content {
        MessageContent::Decision(d) => d.clone(),
        _ => {
            return Err(WriteError::FabricInternal(
                "submit_decision_proposal called with non-Decision content".into(),
            ));
        }
    };

    // 1. Write the proposal message.
    let message_id = bridge.create(
        proposal.to_intent_node(signer.voice_print()),
        EditMode::AppendOnly,
        Some(signer),
    )?;

    let triggering_identity = bridge
        .node_identity(&message_id)
        .expect("just-created message must have identity");

    // Resolve thread LineageId once for conflict-marker linking.
    let thread_lineage = if let Some(thread_id) = &proposal.thread {
        crate::comms::find_lineage_by_fingerprint(bridge, &thread_id.content_fingerprint)
    } else {
        None
    };

    let mut conflicts = Vec::new();
    let mut checkouts = Vec::new();

    for target in &decision.affected_nodes {
        let prior_open = bridge.open_checkout_count(target);

        if prior_open > 0 {
            let marker = ConflictDetected {
                target_node: target.clone(),
                proposers: vec![signer.voice_print()],
                triggering_message: triggering_identity.clone(),
                explanation: format!(
                    "Concurrent DecisionProposal on node {}; consensus mechanism will resolve",
                    target
                ),
            };
            let conflict_id = bridge.create(
                marker.to_intent_node(signer.voice_print()),
                EditMode::AppendOnly,
                Some(signer),
            )?;
            // Wire the marker into the thread topology when the
            // proposal had one; ignore relate-failures (a stale
            // thread reference shouldn't sink the conflict marker).
            if let Some(thread_lid) = &thread_lineage {
                let _ = bridge.relate(&conflict_id, thread_lid, RelationshipKind::Thread, 1.0);
            }
            conflicts.push(conflict_id);
        }

        // Open my own checkout regardless. Skip on writable error so
        // the caller still sees any conflicts that were recorded.
        if let Ok(handle) = bridge.checkout(
            target,
            describe_checkout(&decision),
            DEFAULT_DECISION_CHECKOUT_TTL,
            signer,
        ) {
            checkouts.push(handle);
        }
    }

    Ok(DecisionSubmission {
        message_id,
        conflicts,
        checkouts,
    })
}

fn describe_checkout(decision: &DecisionProposal) -> String {
    format!(
        "DecisionProposal: {} — {}",
        decision.proposed_change, decision.rationale
    )
}

/// Convenience for callers building a `MessageContent::Decision`-shaped
/// `CommsMessage` to pass to `submit_decision_proposal`.
pub fn decision_message(
    proposal: DecisionProposal,
    thread: Option<NodeIdentity>,
    intent: MessageIntent,
    urgency: Urgency,
) -> CommsMessage {
    CommsMessage {
        content: MessageContent::Decision(proposal),
        thread,
        mentions: vec![],
        intent,
        urgency,
        sensitivity: Sensitivity::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    #[test]
    fn conflict_marker_carries_metadata_and_voice() {
        let agent = generate_agent_keypair();
        let target = LineageId::new();
        let triggering = NodeIdentity {
            content_fingerprint: crate::identity::ContentFingerprint([0u8; 32]),
            causal_position: crate::identity::CausalPosition::new(
                crate::temporal::LamportClock::new().tick(),
                crate::temporal::FabricInstant::now(),
                crate::identity::NamespaceId::default_namespace(),
            ),
            creator_voice: Some(agent.voice_print()),
            topological_position: crate::identity::TopologicalPosition::new(0, 0, [0u8; 32]),
        };
        let marker = ConflictDetected {
            target_node: target.clone(),
            proposers: vec![agent.voice_print()],
            triggering_message: triggering,
            explanation: "test".into(),
        };
        let node = marker.to_intent_node(agent.voice_print());
        assert_eq!(node.creator_voice, Some(agent.voice_print()));
        assert_eq!(
            node.metadata.get(META_KIND).map(|v| v.as_str_repr()),
            Some(KIND_CONFLICT_DETECTED.into())
        );
        assert_eq!(
            node.metadata
                .get(META_CONFLICT_TARGET)
                .map(|v| v.as_str_repr()),
            Some(target.as_uuid().to_string())
        );
    }
}
