// Spec 7 §3 — comms region acceptance tests.
//
// Step 1 scope: a CommsMessage materialized as an IntentNode and
// committed via `Fabric::create()` into the `hotash:comms` region
// carries the BLAKE3 content fingerprint, the creator's voice print,
// and a causal position whose namespace IS the comms region. The
// thread node materializes the same way.

use std::sync::Arc;

use ecphory::bridge::{BridgeFabric, FabricTrait};
use ecphory::comms::{
    CommsMessage, CommsThread, MessageContent, MessageIntent, Sensitivity, ThreadState,
    Urgency, KIND_COMMS_MESSAGE, KIND_COMMS_THREAD, META_INTENT, META_KIND, META_MENTIONS,
    META_THREAD_STATE, META_THREAD_TOPIC, META_URGENCY,
};
use ecphory::context::RelationshipKind;
use ecphory::identity::NamespaceId;
use ecphory::{generate_agent_keypair, AgentKeypair, EditMode};

fn comms_bridge() -> Arc<BridgeFabric> {
    Arc::new(BridgeFabric::new().with_default_namespace(NamespaceId::hotash_comms()))
}

#[test]
fn comms_message_writes_into_hotash_comms_region_with_full_identity() {
    let bridge = comms_bridge();
    let nisaba = generate_agent_keypair();
    let nabu = generate_agent_keypair();

    let msg = CommsMessage {
        content: MessageContent::Text(
            "Capability 'print3d' registered. Please rebuild your MCP tool list.".into(),
        ),
        thread: None,
        mentions: vec![nabu.voice_print()],
        intent: MessageIntent::Request,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
    };
    let node = msg.to_intent_node(nisaba.voice_print());

    let expected_fingerprint = node.content_fingerprint().clone();

    let id = bridge
        .create(node, EditMode::AppendOnly, Some(&nisaba))
        .expect("create comms message");

    let stored = bridge
        .get_node(&id)
        .expect("just-written comms message must be readable");

    // Spec 5 §3.1: content fingerprint is intrinsic — survives the round-trip.
    assert_eq!(stored.content_fingerprint(), &expected_fingerprint);
    // Spec 5 §3.2: voice print stamped from the writer.
    assert_eq!(stored.creator_voice, Some(nisaba.voice_print()));

    let identity = bridge.node_identity(&id).expect("identity available");
    // Spec 7 §2.1: the message lives in `hotash:comms`.
    assert_eq!(identity.causal_position.namespace, NamespaceId::hotash_comms());
    // Spec 5 §2.1.1: lamport timestamp assigned at insertion.
    assert!(identity.causal_position.lamport.value() > 0);

    // Metadata observers will parse for downstream processing.
    assert_eq!(
        stored.metadata.get(META_KIND).map(|v| v.as_str_repr()),
        Some(KIND_COMMS_MESSAGE.into())
    );
    assert_eq!(
        stored.metadata.get(META_INTENT).map(|v| v.as_str_repr()),
        Some("Request".into())
    );
    assert_eq!(
        stored.metadata.get(META_URGENCY).map(|v| v.as_str_repr()),
        Some("Normal".into())
    );
    assert_eq!(
        stored.metadata.get(META_MENTIONS).map(|v| v.as_str_repr()),
        Some(nabu.voice_print().to_hex())
    );
}

#[test]
fn comms_thread_writes_into_hotash_comms_region() {
    let bridge = comms_bridge();
    let nisaba = generate_agent_keypair();
    let nabu = generate_agent_keypair();

    let thread = CommsThread {
        topic: "rebuild MCP tool list".into(),
        participants: vec![nisaba.voice_print(), nabu.voice_print()],
        started_by: nisaba.voice_print(),
        sensitivity: Sensitivity::Normal,
        state: ThreadState::Open,
    };
    let node = thread.to_intent_node();

    let id = bridge
        .create(node, EditMode::AppendOnly, Some(&nisaba))
        .expect("create thread node");

    let stored = bridge
        .get_node(&id)
        .expect("just-written thread node must be readable");

    assert_eq!(stored.creator_voice, Some(nisaba.voice_print()));
    assert_eq!(
        stored.metadata.get(META_KIND).map(|v| v.as_str_repr()),
        Some(KIND_COMMS_THREAD.into())
    );
    assert_eq!(
        stored.metadata.get(META_THREAD_TOPIC).map(|v| v.as_str_repr()),
        Some("rebuild MCP tool list".into())
    );
    assert_eq!(
        stored.metadata.get(META_THREAD_STATE).map(|v| v.as_str_repr()),
        Some("Open".into())
    );

    let identity = bridge.node_identity(&id).expect("identity available");
    assert_eq!(identity.causal_position.namespace, NamespaceId::hotash_comms());
}

#[test]
fn thread_with_five_messages_is_recoverable_via_traverse() {
    // Spec 7 §3.2 / Step 2: messages link to their thread via
    // `relate(message, thread, RelationshipKind::Thread, …)`. Walking
    // the thread node along Thread edges (undirected, kind-filtered)
    // returns every message in the conversation.
    let bridge = comms_bridge();
    let starter = generate_agent_keypair();
    let other = generate_agent_keypair();

    let thread = CommsThread {
        topic: "rebuild MCP tool list".into(),
        participants: vec![starter.voice_print(), other.voice_print()],
        started_by: starter.voice_print(),
        sensitivity: Sensitivity::Normal,
        state: ThreadState::Open,
    };
    let thread_id = bridge
        .create(thread.to_intent_node(), EditMode::AppendOnly, Some(&starter))
        .expect("create thread");

    let mut message_ids = Vec::new();
    for i in 0..5 {
        let speaker: &AgentKeypair = if i % 2 == 0 { &starter } else { &other };
        let msg = CommsMessage {
            content: MessageContent::Text(format!("message {}", i)),
            thread: None,
            mentions: vec![],
            intent: MessageIntent::Inform,
            urgency: Urgency::Normal,
            sensitivity: Sensitivity::Normal,
        };
        let id = bridge
            .create(
                msg.to_intent_node(speaker.voice_print()),
                EditMode::AppendOnly,
                Some(speaker),
            )
            .expect("create message");
        bridge
            .relate(&id, &thread_id, RelationshipKind::Thread, 1.0)
            .expect("relate message → thread");
        message_ids.push(id);
    }

    let walked = bridge.traverse(&thread_id, &[RelationshipKind::Thread], 10);

    assert_eq!(
        walked.len(),
        5,
        "thread traversal must return all 5 messages, got: {:?}",
        walked
    );
    for id in &message_ids {
        assert!(
            walked.contains(id),
            "message {} missing from thread traversal",
            id
        );
    }
}

#[test]
fn traverse_filters_out_other_edge_kinds() {
    // The kind filter is real — `Thread` traversal must not bleed into
    // `RelatedTo` edges that happen to touch the same nodes.
    let bridge = comms_bridge();
    let agent = generate_agent_keypair();

    let thread = CommsThread {
        topic: "isolation".into(),
        participants: vec![agent.voice_print()],
        started_by: agent.voice_print(),
        sensitivity: Sensitivity::Normal,
        state: ThreadState::Open,
    };
    let thread_id = bridge
        .create(thread.to_intent_node(), EditMode::AppendOnly, Some(&agent))
        .expect("create thread");

    let in_thread = CommsMessage {
        content: MessageContent::Text("in thread".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
    };
    let in_thread_id = bridge
        .create(
            in_thread.to_intent_node(agent.voice_print()),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .expect("create in-thread message");
    bridge
        .relate(&in_thread_id, &thread_id, RelationshipKind::Thread, 1.0)
        .expect("relate as thread");

    let unrelated = CommsMessage {
        content: MessageContent::Text("unrelated".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
    };
    let unrelated_id = bridge
        .create(
            unrelated.to_intent_node(agent.voice_print()),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .expect("create unrelated message");
    bridge
        .relate(&unrelated_id, &thread_id, RelationshipKind::RelatedTo, 0.5)
        .expect("relate as RelatedTo");

    let thread_walked = bridge.traverse(&thread_id, &[RelationshipKind::Thread], 10);
    assert_eq!(thread_walked, vec![in_thread_id.clone()]);

    let related_walked = bridge.traverse(&thread_id, &[RelationshipKind::RelatedTo], 10);
    assert_eq!(related_walked, vec![unrelated_id]);
}

#[test]
fn relate_rejects_unknown_node() {
    use ecphory::signature::LineageId;
    let bridge = comms_bridge();
    let agent = generate_agent_keypair();

    let msg = CommsMessage {
        content: MessageContent::Text("orphan".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
    };
    let id = bridge
        .create(
            msg.to_intent_node(agent.voice_print()),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();

    let bogus = LineageId::new();
    let err = bridge
        .relate(&id, &bogus, RelationshipKind::Thread, 1.0)
        .unwrap_err();
    matches!(err, ecphory::identity::WriteError::NodeNotFound(_));
}

#[test]
fn comms_namespace_is_stable_across_constructions() {
    // Per Spec 7 §2.1 the comms region UUID must be stable so any agent
    // building a `BridgeFabric` reaches the same region.
    let a = NamespaceId::hotash_comms();
    let b = NamespaceId::hotash_comms();
    assert_eq!(a, b);
    assert_eq!(a.name, "hotash:comms");
}
