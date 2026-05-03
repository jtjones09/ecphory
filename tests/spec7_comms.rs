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
        references: vec![],
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
            references: vec![],
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
        references: vec![],
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
        references: vec![],
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
        references: vec![],
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
        references: vec![],
    };
    let directed_at_third = CommsMessage {
        content: MessageContent::Text("FYI third party".into()),
        thread: None,
        mentions: vec![third.voice_print()],
        intent: MessageIntent::Inform,
        urgency: Urgency::Background,
        sensitivity: Sensitivity::Normal,
        references: vec![],
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
        references: vec![],
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
        references: vec![],
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
fn coordination_without_operator_observation_when_jeremy_never_subscribes() {
    // Spec 7 §6.2 / Step 6 (Cohen I.1 FATAL fold): two agents
    // coordinate a fabric change via comms thread; Jeremy never
    // subscribes to that thread. The provenance tracer detects this
    // and produces a CoordinationWithoutOperator AnomalyObservation
    // (NOT DamageObservation per Matzinger M.1 — governance gap, not
    // structural harm).
    use ecphory::comms::{
        check_coordination_without_operator, decision_message, decision_proposals_in_thread,
        submit_decision_proposal, DecisionProposal, KIND_COORDINATION_WITHOUT_OPERATOR,
        META_DECISION_COUNT, META_OBSERVATION_KIND, META_OPERATOR_REF, META_THREAD_REF,
        OBSERVATION_KIND_ANOMALY,
    };
    use std::collections::HashSet;

    let bridge = comms_bridge();
    let agent_a = generate_agent_keypair();
    let agent_b = generate_agent_keypair();
    let jeremy = generate_agent_keypair();
    // Fabric-internal voice that the immune system signs anomaly
    // observations with — in production this is the bridge's
    // immune-aggregation voice.
    let fabric_voice = bridge.immune_aggregation_voice();

    // Target node coordination affects.
    let target_id = bridge
        .create(
            ecphory::IntentNode::new("target node A and B both touch"),
            EditMode::Semantic,
            Some(&agent_a),
        )
        .unwrap();

    // Thread A and B coordinate inside.
    let thread = CommsThread {
        topic: "should we change the target?".into(),
        participants: vec![agent_a.voice_print(), agent_b.voice_print()],
        started_by: agent_a.voice_print(),
        sensitivity: Sensitivity::Normal,
        state: ThreadState::Open,
    };
    let thread_id = bridge
        .create(thread.to_intent_node(), EditMode::AppendOnly, Some(&agent_a))
        .unwrap();
    let thread_identity = bridge.node_identity(&thread_id).unwrap();

    // A and B each submit a DecisionProposal — coordination happens.
    let prop_a = decision_message(
        DecisionProposal {
            proposed_change: "rephrase".into(),
            rationale: "clarity".into(),
            affected_nodes: vec![target_id.clone()],
            affected_regions: vec![],
            affected_agents: vec![],
        },
        Some(thread_identity.clone()),
        MessageIntent::Decide,
        Urgency::Normal,
    );
    submit_decision_proposal(&bridge, &prop_a, &agent_a).unwrap();

    let prop_b = decision_message(
        DecisionProposal {
            proposed_change: "rephrase differently".into(),
            rationale: "even clearer".into(),
            affected_nodes: vec![target_id.clone()],
            affected_regions: vec![],
            affected_agents: vec![],
        },
        Some(thread_identity.clone()),
        MessageIntent::Decide,
        Urgency::Normal,
    );
    submit_decision_proposal(&bridge, &prop_b, &agent_b).unwrap();

    // Sanity: the provenance tracer finds both DecisionProposal nodes
    // in the thread.
    let decision_ids = decision_proposals_in_thread(&bridge, &thread_id, 32);
    assert_eq!(
        decision_ids.len(),
        2,
        "both DecisionProposals must be reachable via thread traversal; got {:?}",
        decision_ids
    );

    // Jeremy never subscribed to this thread — operator_observed_threads
    // is empty for him.
    let jeremy_observed: HashSet<ecphory::signature::LineageId> = HashSet::new();
    let observation = check_coordination_without_operator(
        &bridge,
        &thread_id,
        &jeremy.voice_print(),
        &jeremy_observed,
        fabric_voice.clone(),
    )
    .expect("operator never observed → CoordinationWithoutOperator must fire");

    // The observation node carries the AnomalyObservation kind tag (NOT
    // DamageObservation — Matzinger M.1: governance gap, not structural).
    assert_eq!(
        observation
            .metadata
            .get(META_OBSERVATION_KIND)
            .map(|v| v.as_str_repr()),
        Some(OBSERVATION_KIND_ANOMALY.into())
    );
    assert_eq!(
        observation
            .metadata
            .get("__bridge_node_kind__")
            .map(|v| v.as_str_repr()),
        Some(KIND_COORDINATION_WITHOUT_OPERATOR.into())
    );
    assert_eq!(
        observation
            .metadata
            .get(META_THREAD_REF)
            .map(|v| v.as_str_repr()),
        Some(thread_id.as_uuid().to_string())
    );
    assert_eq!(
        observation
            .metadata
            .get(META_OPERATOR_REF)
            .map(|v| v.as_str_repr()),
        Some(jeremy.voice_print().to_hex())
    );
    assert_eq!(
        observation
            .metadata
            .get(META_DECISION_COUNT)
            .and_then(|v| v.as_f64()),
        Some(2.0)
    );

    // The caller commits the observation as a fabric node — for the
    // test we just confirm the bridge accepts it.
    let observation_id = bridge
        .create(observation, EditMode::AppendOnly, None)
        .expect("commit CoordinationWithoutOperator");
    assert!(bridge.get_node(&observation_id).is_some());

    // Now repeat with Jeremy's subscription having observed the
    // thread — no anomaly should fire.
    let mut jeremy_observed = HashSet::new();
    jeremy_observed.insert(thread_id.clone());
    let no_observation = check_coordination_without_operator(
        &bridge,
        &thread_id,
        &jeremy.voice_print(),
        &jeremy_observed,
        fabric_voice,
    );
    assert!(
        no_observation.is_none(),
        "operator subscribed → tracer must not fire"
    );
}

#[test]
fn opacity_observer_flags_message_with_five_unobserved_refs() {
    // Spec 7 §6.1 / Step 7 (Cantrill C.2 + Gershman G.2 fold v1):
    // OpacityObserver flags messages containing >3 node references
    // that the operator has not observed in the last 7 days.
    use ecphory::immune::{CellAgent, ObservationContext, ObservationOutcome, ObservedEvent,
        OpacityObserver, OperatorObservedSet};
    use ecphory::signature::LineageId;
    use std::collections::HashSet;
    use std::sync::{Arc, RwLock};

    let bridge = comms_bridge();
    let agent = generate_agent_keypair();
    let jeremy = generate_agent_keypair();

    // Five fabric nodes Jeremy hasn't observed.
    let mut ref_ids: Vec<LineageId> = Vec::new();
    for i in 0..5 {
        let node = ecphory::IntentNode::new(format!("referenced node {}", i));
        let id = bridge
            .create(node, EditMode::AppendOnly, Some(&agent))
            .unwrap();
        ref_ids.push(id);
    }

    // Operator's observation set is empty — Jeremy has seen nothing.
    let observed: OperatorObservedSet = Arc::new(RwLock::new(HashSet::new()));
    let mut opacity = OpacityObserver::new(
        NamespaceId::hotash_comms(),
        jeremy.voice_print(),
        Arc::clone(&observed),
    );

    // Agent writes a comms message referencing all five.
    let msg = CommsMessage {
        content: MessageContent::Text("relevant context".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
        references: ref_ids.clone(),
    };
    let msg_id = bridge
        .create(
            msg.to_intent_node(agent.voice_print()),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();
    let msg_node = bridge.get_node(&msg_id).unwrap();

    // Direct dispatch into the OpacityObserver. (The bridge fires
    // observe() on its own registered cell-agents during create();
    // here we exercise the observer directly so the assertion is
    // tight — Spec 6's bridge wiring is exercised separately.)
    let outcome = opacity.observe(
        ObservedEvent::Node(&msg_node),
        &ObservationContext::default(),
    );
    match outcome {
        ObservationOutcome::Anomaly(a) => {
            assert_eq!(a.specialization, "OpacityObserver");
            assert_eq!(a.observed_value, 5.0);
            assert_eq!(a.region, NamespaceId::hotash_comms());
        }
        other => panic!("expected anomaly for 5 unobserved refs, got {:?}", other),
    }

    // Now Jeremy "observes" 3 of them — his observed set covers most
    // of the references; only 2 remain unobserved → under threshold.
    {
        let mut w = observed.write().unwrap();
        for id in &ref_ids[..3] {
            w.insert(id.clone());
        }
    }
    let outcome2 = opacity.observe(
        ObservedEvent::Node(&msg_node),
        &ObservationContext::default(),
    );
    assert!(
        matches!(outcome2, ObservationOutcome::Quiet),
        "after operator observes 3 of 5 refs, opacity must drop below threshold"
    );
}

#[test]
fn operator_intent_companion_only_for_jeremy_authored_messages() {
    // Spec 7 §7 / Step 8: a message authored by the operator's voice
    // gets a companion `OperatorIntent` node so the immune system can
    // tell "Jeremy told Nabu to do this" from "Nabu decided on its
    // own." Non-operator messages get no companion.
    use ecphory::comms::{submit_with_operator_intent, KIND_OPERATOR_INTENT};

    let bridge = comms_bridge();
    let jeremy = generate_agent_keypair();
    let nabu = generate_agent_keypair();

    // Jeremy-authored message — companion must be written.
    let jeremy_msg = CommsMessage {
        content: MessageContent::Text("please rebuild the MCP tool list".into()),
        thread: None,
        mentions: vec![nabu.voice_print()],
        intent: MessageIntent::Request,
        urgency: Urgency::Prompt,
        sensitivity: Sensitivity::Normal,
        references: vec![],
    };
    let (msg_id, intent_id) =
        submit_with_operator_intent(&bridge, &jeremy_msg, &jeremy, &jeremy.voice_print())
            .expect("submit jeremy message");
    let intent_id = intent_id.expect("operator-authored → companion intent must be written");

    let intent_node = bridge.get_node(&intent_id).unwrap();
    assert_eq!(
        intent_node
            .metadata
            .get("__bridge_node_kind__")
            .map(|v| v.as_str_repr()),
        Some(KIND_OPERATOR_INTENT.into())
    );
    assert_eq!(
        intent_node
            .metadata
            .get("__operator_voice__")
            .map(|v| v.as_str_repr()),
        Some(jeremy.voice_print().to_hex())
    );
    assert_eq!(
        intent_node
            .metadata
            .get("__operator_instruction_node__")
            .map(|v| v.as_str_repr()),
        Some(msg_id.as_uuid().to_string())
    );

    // Edge from intent → message via DerivedFrom.
    let walked =
        bridge.traverse(&intent_id, &[ecphory::context::RelationshipKind::DerivedFrom], 5);
    assert!(
        walked.contains(&msg_id),
        "intent must link to message via DerivedFrom edge"
    );

    // Nabu-authored message with operator = jeremy → no companion.
    let nabu_msg = CommsMessage {
        content: MessageContent::Text("done. 12 tools registered.".into()),
        thread: None,
        mentions: vec![jeremy.voice_print()],
        intent: MessageIntent::Inform,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
        references: vec![],
    };
    let (_, no_intent) =
        submit_with_operator_intent(&bridge, &nabu_msg, &nabu, &jeremy.voice_print())
            .expect("submit nabu message");
    assert!(
        no_intent.is_none(),
        "non-operator message must not produce a companion OperatorIntent"
    );
}

#[test]
fn comms_degradation_falls_back_to_operational_region_then_replays_on_recovery() {
    // Spec 7 §8 / Step 9 (Jeremy J.4 + J.5 fold): comms unhealthy →
    // agent writes a fallback node into its operational region with
    // a `comms_degraded` flag. On recovery, replay reconstructs the
    // comms message into the comms region.
    use ecphory::comms::{
        is_degraded_fallback, replay_degraded_into_comms, submit_or_fallback, CommsHealth,
        KIND_COMMS_DEGRADED, KIND_COMMS_MESSAGE, META_DEGRADED_FLAG, META_INTENDED_NAMESPACE,
        META_REPLAYED_AT_NS, SubmitOutcome,
    };
    use std::collections::HashSet;

    let comms_bridge = comms_bridge();
    let comms_ns = NamespaceId::hotash_comms();
    let op_ns = NamespaceId::fresh("hotash:nisaba");
    let op_bridge = Arc::new(BridgeFabric::new().with_default_namespace(op_ns.clone()));

    let agent = generate_agent_keypair();
    let msg = CommsMessage {
        content: MessageContent::Text("the project ledger needs review".into()),
        thread: None,
        mentions: vec![],
        intent: MessageIntent::Request,
        urgency: Urgency::Normal,
        sensitivity: Sensitivity::Normal,
        references: vec![],
    };

    // Comms is unhealthy (e.g., immune system flagged, ImmuneResponseMode=Disabled).
    let degraded_outcome = submit_or_fallback(
        &comms_bridge,
        &comms_ns,
        &op_bridge,
        &CommsHealth::Degraded {
            reason: "ImmuneResponseMode=Disabled".into(),
        },
        &msg,
        &agent,
    )
    .expect("fallback write must succeed");

    let op_id = match degraded_outcome {
        SubmitOutcome::Degraded(id) => id,
        SubmitOutcome::Comms(_) => panic!("expected fallback to operational region"),
    };

    let op_node = op_bridge.get_node(&op_id).unwrap();
    assert!(is_degraded_fallback(&op_node));
    assert_eq!(
        op_node
            .metadata
            .get("__bridge_node_kind__")
            .map(|v| v.as_str_repr()),
        Some(KIND_COMMS_DEGRADED.into())
    );
    assert_eq!(
        op_node
            .metadata
            .get(META_INTENDED_NAMESPACE)
            .map(|v| v.as_str_repr()),
        Some("hotash:comms".into())
    );

    // The fallback is in op_ns, NOT the comms region.
    let op_identity = op_bridge.node_identity(&op_id).unwrap();
    assert_eq!(op_identity.causal_position.namespace, op_ns);

    // Recovery: comms is healthy again. Replay drains degraded nodes
    // into the comms region as proper messages.
    let already: HashSet<ecphory::signature::LineageId> = HashSet::new();
    let replayed = replay_degraded_into_comms(&comms_bridge, &op_bridge, &agent, &already)
        .expect("replay must succeed");
    assert_eq!(replayed.len(), 1, "exactly one degraded node should replay");
    let (replayed_op_id, replayed_comms_id) = &replayed[0];
    assert_eq!(replayed_op_id, &op_id);

    // The replayed node lives in the comms region with KIND_COMMS_MESSAGE
    // and a trail back to the operational original.
    let comms_node = comms_bridge.get_node(replayed_comms_id).unwrap();
    assert_eq!(
        comms_node
            .metadata
            .get("__bridge_node_kind__")
            .map(|v| v.as_str_repr()),
        Some(KIND_COMMS_MESSAGE.into())
    );
    assert!(
        comms_node.metadata.get(META_DEGRADED_FLAG).is_none(),
        "replayed comms message must not still claim to be a fallback"
    );
    assert_eq!(
        comms_node
            .metadata
            .get(META_REPLAYED_AT_NS)
            .map(|v| v.as_str_repr()),
        Some(op_id.as_uuid().to_string())
    );
    let comms_identity = comms_bridge.node_identity(replayed_comms_id).unwrap();
    assert_eq!(comms_identity.causal_position.namespace, comms_ns);

    // Idempotent re-run with `already_replayed` populated → no new
    // duplicate writes.
    let mut already = HashSet::new();
    already.insert(op_id.clone());
    let replayed_again = replay_degraded_into_comms(&comms_bridge, &op_bridge, &agent, &already)
        .expect("idempotent replay");
    assert!(
        replayed_again.is_empty(),
        "already-replayed set must prevent duplicate writes"
    );
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
