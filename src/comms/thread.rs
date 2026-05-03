// COMMS THREAD — Spec 7 §3.2
//
// A thread is a node that messages link to via `thread` edges. Step 1
// defines the type and the materialization helper. Step 2 wires the
// edge type and the `traverse(thread_id, &[..Thread..])` query.

use crate::comms::message::Sensitivity;
use crate::identity::VoicePrint;
use crate::node::{IntentNode, MetadataValue};

pub const META_KIND: &str = "__bridge_node_kind__";
pub const KIND_COMMS_THREAD: &str = "CommsThread";

pub const META_THREAD_TOPIC: &str = "__comms_thread_topic__";
pub const META_THREAD_PARTICIPANTS: &str = "__comms_thread_participants__";
pub const META_THREAD_STARTED_BY: &str = "__comms_thread_started_by__";
pub const META_THREAD_STATE: &str = "__comms_thread_state__";
pub const META_THREAD_SENSITIVITY: &str = "__comms_thread_sensitivity__";

#[derive(Debug, Clone)]
pub struct CommsThread {
    pub topic: String,
    pub participants: Vec<VoicePrint>,
    pub started_by: VoicePrint,
    pub sensitivity: Sensitivity,
    pub state: ThreadState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadState {
    Open,
    Resolved,
    Escalated { to: VoicePrint },
    Dormant,
}

impl ThreadState {
    pub fn label(&self) -> &'static str {
        match self {
            ThreadState::Open => "Open",
            ThreadState::Resolved => "Resolved",
            ThreadState::Escalated { .. } => "Escalated",
            ThreadState::Dormant => "Dormant",
        }
    }
}

impl CommsThread {
    /// Materialize the thread as an `IntentNode`. Per Spec 7 §3.2 the
    /// thread is a fabric node like any other — content fingerprinted,
    /// voice-print stamped, ready for `Fabric::create()` in
    /// `NamespaceId::hotash_comms()`.
    pub fn to_intent_node(&self) -> IntentNode {
        let description = format!("[Thread] {}", self.topic);
        let mut node = IntentNode::new(description).with_creator_voice(self.started_by);

        node.metadata
            .insert(META_KIND.into(), MetadataValue::String(KIND_COMMS_THREAD.into()));
        node.metadata.insert(
            META_THREAD_TOPIC.into(),
            MetadataValue::String(self.topic.clone()),
        );
        node.metadata.insert(
            META_THREAD_STARTED_BY.into(),
            MetadataValue::String(self.started_by.to_hex()),
        );
        node.metadata.insert(
            META_THREAD_STATE.into(),
            MetadataValue::String(self.state.label().into()),
        );
        node.metadata.insert(
            META_THREAD_SENSITIVITY.into(),
            MetadataValue::String(self.sensitivity.label().into()),
        );

        if !self.participants.is_empty() {
            let participants_hex = self
                .participants
                .iter()
                .map(|v| v.to_hex())
                .collect::<Vec<_>>()
                .join(",");
            node.metadata.insert(
                META_THREAD_PARTICIPANTS.into(),
                MetadataValue::String(participants_hex),
            );
        }

        node.recompute_signature();
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    #[test]
    fn thread_renders_into_intent_node() {
        let starter = generate_agent_keypair();
        let participant = generate_agent_keypair();
        let thread = CommsThread {
            topic: "rebuild MCP tool list".into(),
            participants: vec![starter.voice_print(), participant.voice_print()],
            started_by: starter.voice_print(),
            sensitivity: Sensitivity::Normal,
            state: ThreadState::Open,
        };
        let node = thread.to_intent_node();
        assert!(node.want.description.contains("[Thread]"));
        assert!(node.want.description.contains("rebuild MCP tool list"));
        assert_eq!(node.creator_voice, Some(starter.voice_print()));
        assert_eq!(
            node.metadata.get(META_KIND).map(|v| v.as_str_repr()),
            Some(KIND_COMMS_THREAD.into())
        );
        assert_eq!(
            node.metadata.get(META_THREAD_STATE).map(|v| v.as_str_repr()),
            Some("Open".into())
        );
        let participants = node
            .metadata
            .get(META_THREAD_PARTICIPANTS)
            .map(|v| v.as_str_repr())
            .expect("participants metadata");
        assert!(participants.contains(&starter.voice_print().to_hex()));
        assert!(participants.contains(&participant.voice_print().to_hex()));
    }

    #[test]
    fn thread_state_label_for_each_variant() {
        let target = generate_agent_keypair();
        assert_eq!(ThreadState::Open.label(), "Open");
        assert_eq!(ThreadState::Resolved.label(), "Resolved");
        assert_eq!(
            ThreadState::Escalated { to: target.voice_print() }.label(),
            "Escalated"
        );
        assert_eq!(ThreadState::Dormant.label(), "Dormant");
    }
}
