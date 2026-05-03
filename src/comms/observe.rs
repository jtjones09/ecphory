// COMMS OBSERVATION HELPERS — Spec 7 §3 / Step 3
//
// Per Spec 7 §3 (Rovelli R.2 fold): "every agent subscribed to the
// comms region observes every message; `mentions` helps agents
// prioritize." This module provides:
//
// - `is_comms_message(node)` — predicate convenience for subscriptions
//   that should match every CommsMessage node committed to the fabric
// - `message_mentions(node)` — parse the mentions metadata back into
//   hex voice-print strings so the callback can filter for "this
//   agent was mentioned"
// - `message_urgency(node)` — parse the urgency metadata
// - `message_intent(node)` — parse the intent metadata
//
// Subscription dispatch itself is Spec 8 §6 — the bridge's `subscribe`
// already exists. These helpers just adapt the IntentNode the
// dispatch hands to a callback into the comms-shaped fields the
// callback wants to act on.

use crate::comms::message::{
    KIND_COMMS_MESSAGE, META_INTENT, META_KIND, META_MENTIONS, META_URGENCY, MessageIntent,
    Urgency,
};
use crate::identity::VoicePrint;
use crate::node::IntentNode;

/// Returns `true` when the node is a comms message — matches the
/// `__bridge_node_kind__ == "CommsMessage"` metadata tag set by
/// `CommsMessage::to_intent_node`.
pub fn is_comms_message(node: &IntentNode) -> bool {
    node.metadata
        .get(META_KIND)
        .map(|v| v.as_str_repr() == KIND_COMMS_MESSAGE)
        .unwrap_or(false)
}

/// Parse the `__comms_mentions__` metadata back into hex voice-print
/// strings. Order is preserved to match the writer's `Vec` order.
/// Empty `Vec` if no mentions were attached.
pub fn message_mentions_hex(node: &IntentNode) -> Vec<String> {
    node.metadata
        .get(META_MENTIONS)
        .map(|v| {
            v.as_str_repr()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Returns `true` if `voice` appears in the message's mentions list.
pub fn is_mentioned(node: &IntentNode, voice: &VoicePrint) -> bool {
    let needle = voice.to_hex();
    message_mentions_hex(node).iter().any(|m| m == &needle)
}

/// Parse the `__comms_urgency__` metadata, returning `None` if not
/// present or unrecognized.
pub fn message_urgency(node: &IntentNode) -> Option<Urgency> {
    node.metadata
        .get(META_URGENCY)
        .and_then(|v| match v.as_str_repr().as_str() {
            "Background" => Some(Urgency::Background),
            "Normal" => Some(Urgency::Normal),
            "Prompt" => Some(Urgency::Prompt),
            "Immediate" => Some(Urgency::Immediate),
            _ => None,
        })
}

/// Parse the `__comms_intent__` metadata.
pub fn message_intent(node: &IntentNode) -> Option<MessageIntent> {
    node.metadata
        .get(META_INTENT)
        .and_then(|v| match v.as_str_repr().as_str() {
            "Inform" => Some(MessageIntent::Inform),
            "Request" => Some(MessageIntent::Request),
            "Delegate" => Some(MessageIntent::Delegate),
            "Decide" => Some(MessageIntent::Decide),
            "Escalate" => Some(MessageIntent::Escalate),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::{CommsMessage, MessageContent, Sensitivity};
    use crate::identity::generate_agent_keypair;

    fn dummy_message(intent: MessageIntent, urgency: Urgency, mentions: Vec<VoicePrint>) -> CommsMessage {
        CommsMessage {
            content: MessageContent::Text("hi".into()),
            thread: None,
            mentions,
            intent,
            urgency,
            sensitivity: Sensitivity::Normal,
            references: vec![],
        }
    }

    #[test]
    fn predicate_matches_only_comms_messages() {
        let agent = generate_agent_keypair();
        let comms_node =
            dummy_message(MessageIntent::Inform, Urgency::Normal, vec![]).to_intent_node(agent.voice_print());
        assert!(is_comms_message(&comms_node));

        let plain = crate::node::IntentNode::new("just a node");
        assert!(!is_comms_message(&plain));
    }

    #[test]
    fn round_trips_intent_and_urgency() {
        let agent = generate_agent_keypair();
        let node = dummy_message(MessageIntent::Request, Urgency::Prompt, vec![])
            .to_intent_node(agent.voice_print());
        assert_eq!(message_intent(&node), Some(MessageIntent::Request));
        assert_eq!(message_urgency(&node), Some(Urgency::Prompt));
    }

    #[test]
    fn is_mentioned_finds_target_voice_print() {
        let speaker = generate_agent_keypair();
        let target = generate_agent_keypair();
        let bystander = generate_agent_keypair();

        let node = dummy_message(
            MessageIntent::Inform,
            Urgency::Normal,
            vec![target.voice_print()],
        )
        .to_intent_node(speaker.voice_print());

        assert!(is_mentioned(&node, &target.voice_print()));
        assert!(!is_mentioned(&node, &bystander.voice_print()));
    }

    #[test]
    fn empty_mentions_round_trip_to_empty_vec() {
        let agent = generate_agent_keypair();
        let node = dummy_message(MessageIntent::Inform, Urgency::Normal, vec![])
            .to_intent_node(agent.voice_print());
        assert!(message_mentions_hex(&node).is_empty());
    }
}
