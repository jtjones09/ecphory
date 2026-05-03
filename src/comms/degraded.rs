// COMMS DEGRADATION FALLBACK — Spec 7 §8 / Step 9
//
// Per Spec 7 §8 (Jeremy J.4 + J.5 v1.1 fold): "When the comms region
// is unhealthy (immune system flags it, P53 triggers, or the region is
// simply unreachable):
//   - Agents fall back to writing operational nodes with a
//     `comms_degraded` metadata flag. These nodes are NOT in the comms
//     region — they're in the agent's operational region.
//   - When the comms region comes back healthy, agents check for
//     `comms_degraded` operational nodes and replay them into the
//     comms region as proper CommsMessage nodes.
//
// v1 implementation:
// - `CommsHealth { Healthy | Degraded { reason } }` — passed in by
//   the caller. Spec 6 CANTRILL.4 ImmuneResponseMode + Spec 8 P53
//   surface produce these signals; for v1 we keep the helper agnostic
//   so callers (immune system, nisaba-the-agent) can plug in their
//   own health source.
// - `submit_or_fallback` — writes to comms when healthy; otherwise
//   writes a fallback node into the agent's operational region carrying
//   the message envelope as metadata.
// - `replay_degraded_into_comms` — scans the operational region for
//   `comms_degraded` markers and re-writes them into the comms region
//   as proper CommsMessages; tags the original with a replayed marker
//   on the returned set so callers can dedupe.

use crate::bridge::{BridgeFabric, FabricTrait};
use crate::comms::message::CommsMessage;
use crate::identity::{AgentKeypair, EditMode, NamespaceId, WriteError};
use crate::node::{IntentNode, MetadataValue};
use crate::signature::LineageId;

pub const META_KIND: &str = "__bridge_node_kind__";
pub const KIND_COMMS_DEGRADED: &str = "CommsDegradedFallback";
pub const META_DEGRADED_FLAG: &str = "__comms_degraded__";
pub const META_DEGRADED_REASON: &str = "__comms_degraded_reason__";
pub const META_INTENDED_NAMESPACE: &str = "__comms_intended_namespace__";
pub const META_REPLAYED_AT_NS: &str = "__comms_replayed_at_ns__";

/// Snapshot of comms-region health used by the fallback helper. The
/// caller produces it from whatever signal is authoritative — for v1
/// the immune system's `ImmuneResponseMode` for the comms region, plus
/// `BridgeFabric::is_region_terminated`, are the two natural sources.
#[derive(Debug, Clone)]
pub enum CommsHealth {
    Healthy,
    Degraded { reason: String },
}

/// Result of a fallback-aware submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Comms(LineageId),
    Degraded(LineageId),
}

impl SubmitOutcome {
    pub fn lineage_id(&self) -> &LineageId {
        match self {
            SubmitOutcome::Comms(id) | SubmitOutcome::Degraded(id) => id,
        }
    }
    pub fn was_degraded(&self) -> bool {
        matches!(self, SubmitOutcome::Degraded(_))
    }
}

/// Submit `message` to comms when healthy; otherwise write a fallback
/// node into `operational_bridge`'s default namespace carrying the
/// message envelope as metadata.
pub fn submit_or_fallback(
    comms_bridge: &BridgeFabric,
    comms_namespace: &NamespaceId,
    operational_bridge: &BridgeFabric,
    health: &CommsHealth,
    message: &CommsMessage,
    signer: &AgentKeypair,
) -> Result<SubmitOutcome, WriteError> {
    let comms_unreachable = comms_bridge.is_region_terminated(comms_namespace);
    let degraded_reason = match health {
        CommsHealth::Healthy if !comms_unreachable => None,
        CommsHealth::Healthy => Some(format!(
            "comms namespace {} terminated by P53",
            comms_namespace.name
        )),
        CommsHealth::Degraded { reason } => Some(reason.clone()),
    };

    if degraded_reason.is_none() {
        let id = comms_bridge.create(
            message.to_intent_node(signer.voice_print()),
            EditMode::AppendOnly,
            Some(signer),
        )?;
        return Ok(SubmitOutcome::Comms(id));
    }

    // Fallback path: write into operational namespace carrying the
    // comms-message metadata so a future `replay_degraded_into_comms`
    // can reconstruct it.
    let mut node = message.to_intent_node(signer.voice_print());
    node.metadata
        .insert(META_KIND.into(), MetadataValue::String(KIND_COMMS_DEGRADED.into()));
    node.metadata.insert(
        META_DEGRADED_FLAG.into(),
        MetadataValue::Bool(true),
    );
    if let Some(reason) = degraded_reason {
        node.metadata.insert(
            META_DEGRADED_REASON.into(),
            MetadataValue::String(reason),
        );
    }
    node.metadata.insert(
        META_INTENDED_NAMESPACE.into(),
        MetadataValue::String(comms_namespace.name.clone()),
    );
    node.recompute_signature();

    let id = operational_bridge.create(node, EditMode::AppendOnly, Some(signer))?;
    Ok(SubmitOutcome::Degraded(id))
}

/// Returns true when a node carries the comms-degraded fallback flag.
pub fn is_degraded_fallback(node: &IntentNode) -> bool {
    node.metadata
        .get(META_DEGRADED_FLAG)
        .map(|v| matches!(v, MetadataValue::Bool(true)))
        .unwrap_or(false)
}

/// On comms recovery: scan `operational_bridge` for nodes carrying the
/// `__comms_degraded__` flag, re-write each one into the comms region
/// as a fresh `CommsMessage`-shaped node. Returns the LineageIds of the
/// newly-written comms-region nodes, paired with the operational
/// LineageId they were replayed from.
///
/// `already_replayed` lets the caller skip nodes that have been
/// replayed in a prior invocation. The fabric never deletes, and v1
/// doesn't mutate existing nodes to mark them — callers track this in
/// memory or by reading the comms-side `__comms_replayed_at_ns__`
/// metadata on replayed nodes.
pub fn replay_degraded_into_comms(
    comms_bridge: &BridgeFabric,
    operational_bridge: &BridgeFabric,
    signer: &AgentKeypair,
    already_replayed: &std::collections::HashSet<LineageId>,
) -> Result<Vec<(LineageId, LineageId)>, WriteError> {
    let mut replayed = Vec::new();

    let candidates: Vec<(LineageId, IntentNode)> = operational_bridge.read_inner(|inner| {
        inner
            .nodes()
            .filter(|(id, n)| {
                !already_replayed.contains(id) && is_degraded_fallback(n)
            })
            .map(|(id, n)| (id.clone(), n.clone()))
            .collect()
    });

    for (op_id, op_node) in candidates {
        let mut replay_node = op_node.clone();
        // Restore the comms-message kind and drop the fallback flag
        // so the replayed node looks like a regular comms message.
        replay_node.metadata.insert(
            META_KIND.into(),
            MetadataValue::String(crate::comms::message::KIND_COMMS_MESSAGE.into()),
        );
        replay_node.metadata.remove(META_DEGRADED_FLAG);
        replay_node.metadata.remove(META_DEGRADED_REASON);
        replay_node.metadata.remove(META_INTENDED_NAMESPACE);
        // Trail the original op-side LineageId so observers can verify
        // provenance.
        replay_node.metadata.insert(
            META_REPLAYED_AT_NS.into(),
            MetadataValue::String(op_id.as_uuid().to_string()),
        );
        replay_node.recompute_signature();

        let comms_id = comms_bridge.create(replay_node, EditMode::AppendOnly, Some(signer))?;
        replayed.push((op_id, comms_id));
    }

    Ok(replayed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::{CommsMessage, MessageContent, MessageIntent, Sensitivity, Urgency};

    fn dummy_message() -> CommsMessage {
        CommsMessage {
            content: MessageContent::Text("ping".into()),
            thread: None,
            mentions: vec![],
            intent: MessageIntent::Inform,
            urgency: Urgency::Normal,
            sensitivity: Sensitivity::Normal,
            references: vec![],
        }
    }

    #[test]
    fn fallback_rendering_carries_degraded_flag_and_intended_namespace() {
        // Render the fallback node directly via the helper logic.
        let comms_ns = NamespaceId::hotash_comms();
        let agent = crate::identity::generate_agent_keypair();
        let mut node = dummy_message().to_intent_node(agent.voice_print());
        node.metadata.insert(
            META_KIND.into(),
            MetadataValue::String(KIND_COMMS_DEGRADED.into()),
        );
        node.metadata
            .insert(META_DEGRADED_FLAG.into(), MetadataValue::Bool(true));
        node.metadata.insert(
            META_INTENDED_NAMESPACE.into(),
            MetadataValue::String(comms_ns.name.clone()),
        );
        assert!(is_degraded_fallback(&node));
        assert_eq!(
            node.metadata
                .get(META_INTENDED_NAMESPACE)
                .map(|v| v.as_str_repr()),
            Some("hotash:comms".into())
        );
    }
}
