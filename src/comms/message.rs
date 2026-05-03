// COMMS MESSAGE — Spec 7 §3.1
//
// A message is a node. Materialized via `to_intent_node`, the resulting
// `IntentNode` carries content fingerprinting (intrinsic per Spec 5
// §3.1), the creator's voice print, and structured metadata that
// observers parse to recover intent / urgency / mentions / thread.

use crate::identity::{NodeIdentity, VoicePrint};
use crate::node::{IntentNode, MetadataValue};
use crate::signature::LineageId;

pub const META_KIND: &str = "__bridge_node_kind__";
pub const KIND_COMMS_MESSAGE: &str = "CommsMessage";

pub const META_INTENT: &str = "__comms_intent__";
pub const META_URGENCY: &str = "__comms_urgency__";
pub const META_MENTIONS: &str = "__comms_mentions__";
pub const META_SENSITIVITY: &str = "__comms_sensitivity__";
pub const META_THREAD_FINGERPRINT: &str = "__comms_thread_fingerprint__";
pub const META_THREAD_NAMESPACE: &str = "__comms_thread_namespace__";
pub const META_CONTENT_VARIANT: &str = "__comms_content_variant__";
pub const META_ACK_FINGERPRINT: &str = "__comms_ack_fingerprint__";

pub const CONTENT_VARIANT_TEXT: &str = "Text";
pub const CONTENT_VARIANT_STRUCTURED: &str = "Structured";
pub const CONTENT_VARIANT_HANDOFF: &str = "Handoff";
pub const CONTENT_VARIANT_DECISION: &str = "Decision";
pub const CONTENT_VARIANT_ACK: &str = "Acknowledgment";

/// A single comms-region message. Agents construct one and call
/// `to_intent_node(creator)` to materialize it for `Fabric::create()`.
#[derive(Debug, Clone)]
pub struct CommsMessage {
    pub content: MessageContent,
    /// The thread this message belongs to. `None` opens a new thread
    /// (Step 2 wires the `thread` edge after the thread node is created).
    pub thread: Option<NodeIdentity>,
    /// Voice prints of agents this message tags. Per Spec 7 §3.1
    /// (Rovelli R.2 fold): subscription HINT, not routing — every
    /// comms subscriber observes every message regardless of mentions.
    pub mentions: Vec<VoicePrint>,
    pub intent: MessageIntent,
    pub urgency: Urgency,
    /// Inherited from thread when set; otherwise `Normal`.
    pub sensitivity: Sensitivity,
}

#[derive(Debug, Clone)]
pub enum MessageContent {
    /// Natural-language text.
    Text(String),
    /// Machine-readable structured payload (JSON-shaped).
    Structured(serde_json::Value),
    /// Task delegation with full context.
    Handoff(HandoffContext),
    /// Proposed decision affecting the fabric.
    Decision(DecisionProposal),
    /// Explicit acknowledgment of a prior message (Spec 7 §3.3).
    Acknowledgment(NodeIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageIntent {
    Inform,
    Request,
    Delegate,
    Decide,
    Escalate,
}

impl MessageIntent {
    pub fn label(self) -> &'static str {
        match self {
            MessageIntent::Inform => "Inform",
            MessageIntent::Request => "Request",
            MessageIntent::Delegate => "Delegate",
            MessageIntent::Decide => "Decide",
            MessageIntent::Escalate => "Escalate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Background,
    Normal,
    Prompt,
    Immediate,
}

impl Urgency {
    pub fn label(self) -> &'static str {
        match self {
            Urgency::Background => "Background",
            Urgency::Normal => "Normal",
            Urgency::Prompt => "Prompt",
            Urgency::Immediate => "Immediate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    Normal,
    High,
}

impl Sensitivity {
    pub fn label(self) -> &'static str {
        match self {
            Sensitivity::Normal => "Normal",
            Sensitivity::High => "High",
        }
    }
}

/// Step-4 type — placeholder fields per Spec 7 §4.2. Behavior
/// (predicate evaluation against the fabric) lands in Step 4.
#[derive(Debug, Clone)]
pub struct HandoffContext {
    pub task_description: String,
    pub source_context: Vec<NodeIdentity>,
    pub constraints: Vec<String>,
    pub success_criteria: Vec<String>,
    pub success_checks: Vec<SuccessCheck>,
    pub deadline: Option<crate::temporal::FabricInstant>,
    pub escalation_path: VoicePrint,
}

/// Step-4 type — machine-verifiable completion predicates per Spec 7
/// §4.2 / Gershman G.1 fold. Evaluation lands in Step 4.
#[derive(Debug, Clone)]
pub enum SuccessCheck {
    NodeExists {
        reference: String,
    },
    NodeCountInRegion {
        region: crate::identity::NamespaceId,
        min: u64,
    },
    ContentMatches {
        node: NodeIdentity,
        pattern: String,
    },
    EdgeExists {
        from: NodeIdentity,
        to: NodeIdentity,
        edge_type: String,
    },
}

/// Step-5 type — placeholder fields per Spec 7 §4.3. Conflict
/// detection + checkout triggering lands in Step 5.
#[derive(Debug, Clone)]
pub struct DecisionProposal {
    pub proposed_change: String,
    pub rationale: String,
    pub affected_nodes: Vec<LineageId>,
    pub affected_regions: Vec<crate::identity::NamespaceId>,
    pub affected_agents: Vec<VoicePrint>,
}

impl CommsMessage {
    /// Materialize this message as an `IntentNode` ready for
    /// `Fabric::create()`. Content fingerprint is computed from the
    /// rendered description; voice print is stamped from `creator`.
    /// Thread reference + mentions + intent + urgency travel as
    /// metadata so observers can parse without re-rendering.
    pub fn to_intent_node(&self, creator: VoicePrint) -> IntentNode {
        let description = self.render_description();
        let mut node = IntentNode::new(description).with_creator_voice(creator);

        node.metadata
            .insert(META_KIND.into(), MetadataValue::String(KIND_COMMS_MESSAGE.into()));
        node.metadata.insert(
            META_INTENT.into(),
            MetadataValue::String(self.intent.label().into()),
        );
        node.metadata.insert(
            META_URGENCY.into(),
            MetadataValue::String(self.urgency.label().into()),
        );
        node.metadata.insert(
            META_SENSITIVITY.into(),
            MetadataValue::String(self.sensitivity.label().into()),
        );
        node.metadata.insert(
            META_CONTENT_VARIANT.into(),
            MetadataValue::String(self.content.variant_label().into()),
        );

        if !self.mentions.is_empty() {
            let mentions_hex = self
                .mentions
                .iter()
                .map(|v| v.to_hex())
                .collect::<Vec<_>>()
                .join(",");
            node.metadata
                .insert(META_MENTIONS.into(), MetadataValue::String(mentions_hex));
        }

        if let Some(thread) = &self.thread {
            node.metadata.insert(
                META_THREAD_FINGERPRINT.into(),
                MetadataValue::String(thread.content_fingerprint.to_hex()),
            );
            node.metadata.insert(
                META_THREAD_NAMESPACE.into(),
                MetadataValue::String(thread.causal_position.namespace.name.clone()),
            );
        }

        if let MessageContent::Acknowledgment(ack_target) = &self.content {
            node.metadata.insert(
                META_ACK_FINGERPRINT.into(),
                MetadataValue::String(ack_target.content_fingerprint.to_hex()),
            );
        }

        node.recompute_signature();
        node
    }

    fn render_description(&self) -> String {
        match &self.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Structured(v) => v.to_string(),
            MessageContent::Handoff(h) => format!(
                "[Handoff] {} (criteria: {})",
                h.task_description,
                h.success_criteria.join("; ")
            ),
            MessageContent::Decision(d) => {
                format!("[Decision] {} — {}", d.proposed_change, d.rationale)
            }
            MessageContent::Acknowledgment(_) => "[Ack]".to_string(),
        }
    }
}

impl MessageContent {
    pub fn variant_label(&self) -> &'static str {
        match self {
            MessageContent::Text(_) => CONTENT_VARIANT_TEXT,
            MessageContent::Structured(_) => CONTENT_VARIANT_STRUCTURED,
            MessageContent::Handoff(_) => CONTENT_VARIANT_HANDOFF,
            MessageContent::Decision(_) => CONTENT_VARIANT_DECISION,
            MessageContent::Acknowledgment(_) => CONTENT_VARIANT_ACK,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    fn message_text(text: &str) -> CommsMessage {
        CommsMessage {
            content: MessageContent::Text(text.into()),
            thread: None,
            mentions: vec![],
            intent: MessageIntent::Inform,
            urgency: Urgency::Normal,
            sensitivity: Sensitivity::Normal,
        }
    }

    #[test]
    fn text_message_renders_into_intent_node() {
        let agent = generate_agent_keypair();
        let msg = message_text("rebuild the MCP tool list");
        let node = msg.to_intent_node(agent.voice_print());
        assert_eq!(node.want.description, "rebuild the MCP tool list");
        assert_eq!(node.creator_voice, Some(agent.voice_print()));
        assert_eq!(
            node.metadata.get(META_KIND).map(|v| v.as_str_repr()),
            Some(KIND_COMMS_MESSAGE.into())
        );
        assert_eq!(
            node.metadata.get(META_INTENT).map(|v| v.as_str_repr()),
            Some("Inform".into())
        );
        assert_eq!(
            node.metadata.get(META_URGENCY).map(|v| v.as_str_repr()),
            Some("Normal".into())
        );
    }

    #[test]
    fn mentions_are_serialized_as_comma_separated_hex() {
        let agent = generate_agent_keypair();
        let target_a = generate_agent_keypair();
        let target_b = generate_agent_keypair();
        let mut msg = message_text("paged");
        msg.mentions = vec![target_a.voice_print(), target_b.voice_print()];
        let node = msg.to_intent_node(agent.voice_print());
        let stored = node
            .metadata
            .get(META_MENTIONS)
            .map(|v| v.as_str_repr())
            .expect("mentions metadata");
        let parts: Vec<&str> = stored.split(',').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], target_a.voice_print().to_hex());
        assert_eq!(parts[1], target_b.voice_print().to_hex());
    }

    #[test]
    fn variant_label_round_trips_through_metadata() {
        let agent = generate_agent_keypair();
        for (msg, expected) in [
            (message_text("t"), CONTENT_VARIANT_TEXT),
            (
                CommsMessage {
                    content: MessageContent::Structured(serde_json::json!({"k": "v"})),
                    thread: None,
                    mentions: vec![],
                    intent: MessageIntent::Inform,
                    urgency: Urgency::Background,
                    sensitivity: Sensitivity::Normal,
                },
                CONTENT_VARIANT_STRUCTURED,
            ),
        ] {
            let node = msg.to_intent_node(agent.voice_print());
            assert_eq!(
                node.metadata
                    .get(META_CONTENT_VARIANT)
                    .map(|v| v.as_str_repr())
                    .as_deref(),
                Some(expected),
            );
        }
    }
}
