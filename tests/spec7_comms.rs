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
fn mentioned_agent_subscription_fires_and_subscription_log_records_observation() {
    // Spec 7 §3 Step 3: Agent B subscribes to comms. Agent A writes a
    // message mentioning B. B's callback runs (subscription log
    // increments observation_count). Per Rovelli R.2 fold, B's
    // subscription matches every CommsMessage — the mentions filter
    // lives in the callback as prioritization, not in the predicate.
    use ecphory::bridge::{Callback, CallbackResult, Predicate};
    use ecphory::comms;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let bridge = comms_bridge();
    let agent_a = generate_agent_keypair();
    let agent_b = generate_agent_keypair();

    // B's "directly mentioned" counter increments only when the
    // callback parses the message and finds B in the mentions list.
    let mentioned_for_b = Arc::new(AtomicUsize::new(0));
    let mentioned_clone = Arc::clone(&mentioned_for_b);
    let b_voice = agent_b.voice_print();

    let pat: Predicate = Arc::new(|node| comms::is_comms_message(node));
    let cb: Callback = Arc::new(move |node, _ctx| {
        if comms::is_mentioned(node, &b_voice) {
            mentioned_clone.fetch_add(1, Ordering::SeqCst);
        }
        CallbackResult::Success
    });
    let sub_id = bridge.subscribe(pat, cb).expect("subscribe");

    // Agent A writes one mentioning B and one mentioning a third party.
    let third = generate_agent_keypair();
    let directed_at_b = CommsMessage {
        content: MessageContent::Text("hey B, please look at this".into()),
        thread: None,
        mentions: vec![agent_b.voice_print()],
        intent: MessageIntent::Request,
        urgency: Urgency::Prompt,
        sensitivity: Sensitivity::Normal,
    };
    let directed_at_third = CommsMessage {
        content: MessageContent::Text("FYI third party".into()),
        thread: None,
        mentions: vec![third.voice_print()],
        intent: MessageIntent::Inform,
        urgency: Urgency::Background,
        sensitivity: Sensitivity::Normal,
    };
    bridge
        .create(
            directed_at_b.to_intent_node(agent_a.voice_print()),
            EditMode::AppendOnly,
            Some(&agent_a),
        )
        .unwrap();
    bridge
        .create(
            directed_at_third.to_intent_node(agent_a.voice_print()),
            EditMode::AppendOnly,
            Some(&agent_a),
        )
        .unwrap();

    // Subscription dispatch is async — wait up to 2s for both
    // observations to settle.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let state = bridge.subscription_state(sub_id).expect("sub state");
        if state.observation_count >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // The subscription log records that B's subscription observed
    // BOTH messages — every comms subscriber sees every message per
    // Rovelli R.2.
    let state = bridge.subscription_state(sub_id).expect("sub state");
    assert_eq!(
        state.observation_count, 2,
        "B's subscription should observe both writes (mentions is a hint, not routing); state={:?}",
        state
    );

    // The mentions-filter inside the callback fired exactly once: the
    // message that named B, not the one that named the third party.
    assert_eq!(
        mentioned_for_b.load(Ordering::SeqCst),
        1,
        "callback's mentions filter must distinguish B's message from the third party's"
    );
}

#[test]
fn handoff_success_checks_evaluate_against_fabric_state() {
    // Spec 7 §4.2 / Step 4: a HandoffContext carries machine-verifiable
    // SuccessCheck predicates. The delegate runs them against the
    // fabric and reports pass/fail. This test stages a 4-check handoff
    // (one per variant) and verifies each evaluates correctly.
    use ecphory::comms::{CheckOutcome, HandoffContext, SuccessCheck};
    use ecphory::temporal::FabricInstant;
    use uuid::Uuid;

    let bridge = comms_bridge();
    let agent = generate_agent_keypair();
    let escalation = generate_agent_keypair();

    // Create two messages in the comms region; A → thread; B → thread.
    let thread = CommsThread {
        topic: "research handoff".into(),
        participants: vec![agent.voice_print()],
        started_by: agent.voice_print(),
        sensitivity: Sensitivity::Normal,
        state: ThreadState::Open,
    };
    let thread_id = bridge
        .create(thread.to_intent_node(), EditMode::AppendOnly, Some(&agent))
        .unwrap();

    let msg_a = CommsMessage {
        content: MessageContent::Text("audit: findings filed".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
    };
    let msg_a_id = bridge
        .create(
            msg_a.to_intent_node(agent.voice_print()),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();
    bridge
        .relate(&msg_a_id, &thread_id, RelationshipKind::Thread, 1.0)
        .unwrap();

    let msg_b = CommsMessage {
        content: MessageContent::Text("status: complete".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
    };
    let msg_b_id = bridge
        .create(
            msg_b.to_intent_node(agent.voice_print()),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();

    // Capture a NodeIdentity for the content-matches and edge-exists
    // checks via the trait surface.
    let msg_a_identity = bridge.node_identity(&msg_a_id).unwrap();
    let msg_b_identity = bridge.node_identity(&msg_b_id).unwrap();
    let thread_identity = bridge.node_identity(&thread_id).unwrap();

    // Stage a handoff with one passing and one failing check per type.
    let bogus_uuid = Uuid::new_v4().to_string();
    let handoff = HandoffContext {
        task_description: "wrap up research".into(),
        source_context: vec![msg_a_identity.clone()],
        constraints: vec!["read-only".into()],
        success_criteria: vec!["audit log filed".into()],
        success_checks: vec![
            SuccessCheck::NodeExists {
                reference: msg_a_id.as_uuid().to_string(),
            },
            SuccessCheck::NodeCountInRegion {
                region: NamespaceId::hotash_comms(),
                min: 3, // thread + 2 messages
            },
            SuccessCheck::ContentMatches {
                node: msg_a_identity.clone(),
                pattern: "audit".into(),
            },
            SuccessCheck::EdgeExists {
                from: msg_a_identity.clone(),
                to: thread_identity.clone(),
                edge_type: "Thread".into(),
            },
        ],
        deadline: Some(FabricInstant::now()),
        escalation_path: escalation.voice_print(),
    };

    let outcomes = handoff.evaluate_all(&bridge);
    assert_eq!(outcomes.len(), 4);
    for (i, o) in outcomes.iter().enumerate() {
        assert!(o.is_pass(), "check #{} should pass: {:?}", i, o);
    }
    assert!(handoff.all_checks_pass(&bridge));

    // Now stage failing checks and confirm they fail with the right
    // message variant.
    let failing = HandoffContext {
        task_description: "should fail".into(),
        source_context: vec![],
        constraints: vec![],
        success_criteria: vec![],
        success_checks: vec![
            SuccessCheck::NodeExists {
                reference: bogus_uuid.clone(),
            },
            SuccessCheck::NodeCountInRegion {
                region: NamespaceId::hotash_comms(),
                min: 999,
            },
            SuccessCheck::ContentMatches {
                node: msg_b_identity.clone(),
                pattern: "audit".into(), // msg_b is "status: complete"
            },
            SuccessCheck::EdgeExists {
                from: msg_b_identity.clone(),
                to: thread_identity.clone(),
                edge_type: "Thread".into(),
            },
        ],
        deadline: None,
        escalation_path: escalation.voice_print(),
    };
    let failing_outcomes = failing.evaluate_all(&bridge);
    assert_eq!(failing_outcomes.len(), 4);
    for o in &failing_outcomes {
        assert!(matches!(o, CheckOutcome::Fail(_)), "expected Fail: {:?}", o);
    }
    assert!(!failing.all_checks_pass(&bridge));
}

#[test]
fn concurrent_decision_proposals_emit_conflict_detected_into_thread() {
    // Spec 7 §4.3 / Step 5: when a second DecisionProposal lands on a
    // target that already has an Open checkout, the comms layer writes
    // a `ConflictDetected` marker (informational, not blocking) and
    // opens its own checkout. Both checkouts can then proceed through
    // consensus per Spec 8 §3.4.
    use ecphory::comms::{
        decision_message, submit_decision_proposal, ConflictDetected, DecisionProposal,
        KIND_CONFLICT_DETECTED, META_CONFLICT_TARGET,
    };

    let bridge = comms_bridge();
    let agent_a = generate_agent_keypair();
    let agent_b = generate_agent_keypair();

    // Target node: created Semantic so it accepts checkouts.
    let target_id = bridge
        .create(
            ecphory::IntentNode::new("the contested node"),
            EditMode::Semantic,
            Some(&agent_a),
        )
        .unwrap();

    // Thread to host both proposals.
    let thread = CommsThread {
        topic: "should we reword this?".into(),
        participants: vec![agent_a.voice_print(), agent_b.voice_print()],
        started_by: agent_a.voice_print(),
        sensitivity: Sensitivity::Normal,
        state: ThreadState::Open,
    };
    let thread_id = bridge
        .create(thread.to_intent_node(), EditMode::AppendOnly, Some(&agent_a))
        .unwrap();
    let thread_identity = bridge.node_identity(&thread_id).unwrap();

    // Agent A submits the first DecisionProposal.
    let proposal_a = decision_message(
        DecisionProposal {
            proposed_change: "rephrase as 'X'".into(),
            rationale: "clearer".into(),
            affected_nodes: vec![target_id.clone()],
            affected_regions: vec![],
            affected_agents: vec![],
        },
        Some(thread_identity.clone()),
        MessageIntent::Decide,
        Urgency::Normal,
    );
    let submission_a =
        submit_decision_proposal(&bridge, &proposal_a, &agent_a).expect("submit A");
    assert!(
        submission_a.conflicts.is_empty(),
        "first proposal must not produce conflicts; got {:?}",
        submission_a.conflicts
    );
    assert_eq!(
        submission_a.checkouts.len(),
        1,
        "first proposal must open exactly one checkout"
    );

    // Agent B submits a competing DecisionProposal on the same target.
    let proposal_b = decision_message(
        DecisionProposal {
            proposed_change: "rephrase as 'Y'".into(),
            rationale: "more accurate".into(),
            affected_nodes: vec![target_id.clone()],
            affected_regions: vec![],
            affected_agents: vec![],
        },
        Some(thread_identity.clone()),
        MessageIntent::Decide,
        Urgency::Normal,
    );
    let submission_b =
        submit_decision_proposal(&bridge, &proposal_b, &agent_b).expect("submit B");
    assert_eq!(
        submission_b.conflicts.len(),
        1,
        "second proposal must produce one ConflictDetected marker (target had A's open checkout)"
    );
    assert_eq!(
        submission_b.checkouts.len(),
        1,
        "second proposal still opens its own checkout — informational, not blocking"
    );

    // Verify the conflict marker is in the comms region and reachable
    // from the thread via Thread traversal.
    let conflict_id = submission_b.conflicts[0].clone();
    let conflict_node = bridge.get_node(&conflict_id).unwrap();
    assert_eq!(
        conflict_node
            .metadata
            .get("__bridge_node_kind__")
            .map(|v| v.as_str_repr()),
        Some(KIND_CONFLICT_DETECTED.into())
    );
    assert_eq!(
        conflict_node
            .metadata
            .get(META_CONFLICT_TARGET)
            .map(|v| v.as_str_repr()),
        Some(target_id.as_uuid().to_string())
    );

    let walked = bridge.traverse(&thread_id, &[RelationshipKind::Thread], 10);
    assert!(
        walked.contains(&conflict_id),
        "ConflictDetected marker must be reachable from the thread via Thread traversal"
    );

    // Both checkouts can drive through consensus. Agent A finalizes
    // first; the round stays Open because B hasn't finalized yet.
    let proposal_a_handle = bridge
        .propose(
            &submission_a.checkouts[0].id,
            ecphory::IntentNode::new("rephrase as 'X'"),
            &agent_a,
        )
        .unwrap();
    let snap_after_a = bridge
        .finalize_proposal(&proposal_a_handle.id, &agent_a)
        .unwrap();
    assert!(
        snap_after_a.is_none(),
        "snapshot must not write yet — B's checkout is still Open"
    );

    let proposal_b_handle = bridge
        .propose(
            &submission_b.checkouts[0].id,
            ecphory::IntentNode::new("rephrase as 'Y'"),
            &agent_b,
        )
        .unwrap();
    let snap_after_b = bridge
        .finalize_proposal(&proposal_b_handle.id, &agent_b)
        .unwrap();
    let snapshot = snap_after_b.expect("last finalize must write the consensus snapshot");
    assert_eq!(snapshot.target, target_id);
    // Both finalized proposals appear in the snapshot.
    assert_eq!(snapshot.finalized_proposals.len(), 2);
    assert!(snapshot.finalized_proposals.contains(&proposal_a_handle.id));
    assert!(snapshot.finalized_proposals.contains(&proposal_b_handle.id));

    let _ = ConflictDetected {
        target_node: target_id,
        proposers: vec![],
        triggering_message: thread_identity,
        explanation: "compile-only sanity".into(),
    };
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
