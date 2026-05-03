// OPERATOR INTENT — Spec 7 §7 / Step 8
//
// Per Spec 7 §7 + Spec 5 §5.6.6: when a message enters the fabric with
// `creator_voice: jeremy_pk`, the comms layer writes a companion
// `OperatorIntent` node so the immune system can distinguish
// "Jeremy told Nabu to do this" from "Nabu decided to do this on its
// own." Operator-originated comms messages also carry higher initial
// trust weight per Spec 5 §5.5.4.
//
// v1 scope:
// - `OperatorIntent` node-kind with materialization helper
// - `submit_with_operator_intent(bridge, message, signer, operator_pk)`
//   convenience that writes the message and, if `signer == operator`,
//   writes the OperatorIntent companion node and links it to the
//   message via a `DerivedFrom` edge.
//
// Future (v1.5+): the bridge could enforce companion writes
// automatically by inspecting `creator_voice` at commit time. For v1
// the comms helper is the canonical entry point — agents wanting an
// OperatorIntent companion call this; agents that don't, don't.

use crate::bridge::{BridgeFabric, FabricTrait};
use crate::comms::message::CommsMessage;
use crate::context::RelationshipKind;
use crate::identity::{
    AgentKeypair, ContentFingerprint, EditMode, VoicePrint, WriteError,
};
use crate::node::{IntentNode, MetadataValue};
use crate::signature::LineageId;

pub const KIND_OPERATOR_INTENT: &str = "OperatorIntent";
pub const META_KIND: &str = "__bridge_node_kind__";
pub const META_OPERATOR_VOICE: &str = "__operator_voice__";
pub const META_INSTRUCTION_NODE: &str = "__operator_instruction_node__";
pub const META_INSTRUCTION_FINGERPRINT: &str = "__operator_instruction_fingerprint__";

/// Companion node written alongside an operator-authored comms message.
/// Spec 5 §5.6.6: the immune system uses operator intent as a
/// first-class observable signal.
#[derive(Debug, Clone)]
pub struct OperatorIntent {
    pub operator: VoicePrint,
    /// LineageId of the message this intent companion references.
    pub instruction_node: LineageId,
    /// Content fingerprint of the same message — gives observers a
    /// fingerprint-based reference that survives `LineageId` changes.
    pub instruction_fingerprint: ContentFingerprint,
}

impl OperatorIntent {
    pub fn to_intent_node(&self, signer: VoicePrint) -> IntentNode {
        let mut node = IntentNode::new(format!(
            "[OperatorIntent] {} authored instruction {}",
            self.operator, self.instruction_node
        ))
        .with_creator_voice(signer);
        node.metadata.insert(
            META_KIND.into(),
            MetadataValue::String(KIND_OPERATOR_INTENT.into()),
        );
        node.metadata.insert(
            META_OPERATOR_VOICE.into(),
            MetadataValue::String(self.operator.to_hex()),
        );
        node.metadata.insert(
            META_INSTRUCTION_NODE.into(),
            MetadataValue::String(self.instruction_node.as_uuid().to_string()),
        );
        node.metadata.insert(
            META_INSTRUCTION_FINGERPRINT.into(),
            MetadataValue::String(self.instruction_fingerprint.to_hex()),
        );
        node.recompute_signature();
        node
    }
}

/// Submit a comms message and, if the signer matches `operator`, write
/// a companion `OperatorIntent` node linked to the message via a
/// `DerivedFrom` edge. Returns `(message_id, Some(intent_id))` for
/// operator-authored messages; `(message_id, None)` otherwise.
pub fn submit_with_operator_intent(
    bridge: &BridgeFabric,
    message: &CommsMessage,
    signer: &AgentKeypair,
    operator: &VoicePrint,
) -> Result<(LineageId, Option<LineageId>), WriteError> {
    let signer_voice = signer.voice_print();
    let message_id = bridge.create(
        message.to_intent_node(signer_voice),
        EditMode::AppendOnly,
        Some(signer),
    )?;

    if signer_voice != *operator {
        return Ok((message_id, None));
    }

    let identity = bridge
        .node_identity(&message_id)
        .expect("just-created message must have identity");
    let intent = OperatorIntent {
        operator: *operator,
        instruction_node: message_id.clone(),
        instruction_fingerprint: identity.content_fingerprint,
    };
    let intent_id = bridge.create(
        intent.to_intent_node(signer_voice),
        EditMode::AppendOnly,
        Some(signer),
    )?;
    // Link intent → message so observers walking the comms region
    // can locate the companion node from either side.
    let _ = bridge.relate(&intent_id, &message_id, RelationshipKind::DerivedFrom, 1.0);
    Ok((message_id, Some(intent_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;
    use crate::identity::ContentFingerprint;

    #[test]
    fn operator_intent_node_carries_correct_metadata() {
        let operator = generate_agent_keypair();
        let signer = generate_agent_keypair();
        let target = LineageId::new();
        let fp = ContentFingerprint([7u8; 32]);
        let intent = OperatorIntent {
            operator: operator.voice_print(),
            instruction_node: target.clone(),
            instruction_fingerprint: fp,
        };
        let node = intent.to_intent_node(signer.voice_print());
        assert_eq!(
            node.metadata.get(META_KIND).map(|v| v.as_str_repr()),
            Some(KIND_OPERATOR_INTENT.into())
        );
        assert_eq!(
            node.metadata.get(META_OPERATOR_VOICE).map(|v| v.as_str_repr()),
            Some(operator.voice_print().to_hex())
        );
        assert_eq!(
            node.metadata
                .get(META_INSTRUCTION_NODE)
                .map(|v| v.as_str_repr()),
            Some(target.as_uuid().to_string())
        );
    }
}
