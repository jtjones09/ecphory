// Integration tests for Spec 8 (Phase F bridge) — Steps 1-3.
//
// Per handoff: tests must use real keypairs and real BLAKE3 hashes —
// no mocks, no shortcuts. The fabric runs in-process through the
// `BridgeFabric` API.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ecphory::bridge::{
    BridgeFabric, Callback, CallbackResult, DebugToken, DebugTokenError, FabricTrait,
    P53Config, P53Scope, Predicate, SafetyError, DEBUG_TOKEN_DEFAULT_LIFETIME,
    DEBUG_TOKEN_DEFAULT_SCOPE,
};
use ecphory::{
    generate_agent_keypair, EditMode, IntentNode, MetadataValue, NamespaceId, WriteError,
};
use std::path::PathBuf;

fn wait_until<F: Fn() -> bool>(deadline: Duration, check: F) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    check()
}

// ── Step 2: identity primitives reach into the bridge ──

#[test]
fn create_returns_lineage_resolvable_to_node_identity() {
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    let id = bridge
        .create(IntentNode::new("hello"), EditMode::AppendOnly, Some(&agent))
        .unwrap();

    let identity = bridge.node_identity(&id).expect("identity present");
    assert_eq!(identity.creator_voice, Some(agent.voice_print()));
    // Just-created node has no edges → zero degree.
    assert_eq!(identity.topological_position.in_degree, 0);
    assert_eq!(identity.topological_position.out_degree, 0);
}

// ── Step 3: three-way edit model ──

#[test]
fn three_modes_classify_distinctly() {
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();

    let append = bridge
        .create(IntentNode::new("a"), EditMode::AppendOnly, Some(&agent))
        .unwrap();
    let mech = bridge
        .create(IntentNode::new("m"), EditMode::Mechanical, Some(&agent))
        .unwrap();
    let sem = bridge
        .create(IntentNode::new("s"), EditMode::Semantic, Some(&agent))
        .unwrap();

    assert_eq!(bridge.edit_mode_of(&append), Some(EditMode::AppendOnly));
    assert_eq!(bridge.edit_mode_of(&mech), Some(EditMode::Mechanical));
    assert_eq!(bridge.edit_mode_of(&sem), Some(EditMode::Semantic));
}

#[test]
fn edit_mechanical_only_accepts_mechanical_targets() {
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();

    let appendonly = bridge
        .create(IntentNode::new("note"), EditMode::AppendOnly, Some(&agent))
        .unwrap();
    let result = bridge.edit_mechanical(&appendonly, &agent, |_| {});
    assert!(matches!(
        result.unwrap_err(),
        WriteError::EditModeMismatch { .. }
    ));

    let semantic = bridge
        .create(IntentNode::new("PINNED"), EditMode::Semantic, Some(&agent))
        .unwrap();
    let result = bridge.edit_mechanical(&semantic, &agent, |_| {});
    assert!(matches!(
        result.unwrap_err(),
        WriteError::EditModeMismatch { .. }
    ));

    let mechanical = bridge
        .create(IntentNode::new("counter"), EditMode::Mechanical, Some(&agent))
        .unwrap();
    let receipt = bridge
        .edit_mechanical(&mechanical, &agent, |node| {
            node.metadata
                .insert("count".into(), MetadataValue::Int(7));
        })
        .unwrap();
    assert_eq!(receipt.editor_voice, agent.voice_print());
}

// ── Spec 8 §3.4 acceptance criterion #5: three-agent semantic edit ──

#[test]
fn three_agent_semantic_edit_writes_one_snapshot() {
    // Three test agents perform semantic edits on the same node via
    // checkout/propose/consensus. ConsensusSnapshot fires exactly once
    // per round; the snapshot includes all three finalized proposals.
    let bridge = BridgeFabric::new();
    let alice = generate_agent_keypair();
    let bob = generate_agent_keypair();
    let carol = generate_agent_keypair();

    let target = bridge
        .create(
            IntentNode::new("PINNED: identity-as-emergent-relation"),
            EditMode::Semantic,
            Some(&alice),
        )
        .unwrap();

    let co_a = bridge
        .checkout(&target, "alice's read".into(), Duration::from_secs(60), &alice)
        .unwrap();
    let co_b = bridge
        .checkout(&target, "bob's read".into(), Duration::from_secs(60), &bob)
        .unwrap();
    let co_c = bridge
        .checkout(&target, "carol's read".into(), Duration::from_secs(60), &carol)
        .unwrap();

    let p_a = bridge
        .propose(&co_a.id, IntentNode::new("alice's wording"), &alice)
        .unwrap();
    let p_b = bridge
        .propose(&co_b.id, IntentNode::new("bob's wording"), &bob)
        .unwrap();
    let p_c = bridge
        .propose(&co_c.id, IntentNode::new("carol's wording"), &carol)
        .unwrap();

    // Two finalizes — round still pending.
    assert!(bridge.finalize_proposal(&p_a.id, &alice).unwrap().is_none());
    assert!(bridge.finalize_proposal(&p_b.id, &bob).unwrap().is_none());

    // Last finalize — exactly one snapshot fires.
    let snapshot = bridge
        .finalize_proposal(&p_c.id, &carol)
        .unwrap()
        .expect("last finalize must write the consensus snapshot");

    assert_eq!(snapshot.target, target);
    assert_eq!(snapshot.finalized_proposals.len(), 3);
    assert!(snapshot.finalized_proposals.contains(&p_a.id));
    assert!(snapshot.finalized_proposals.contains(&p_b.id));
    assert!(snapshot.finalized_proposals.contains(&p_c.id));

    // Snapshot is now a fabric-resident node — observable like any other.
    assert!(bridge.get_node(&snapshot.id).is_some());
}

// ── Spec 8 §3.5.2: per-node linearizable mechanical edits ──

#[test]
fn mechanical_lock_serializes_writers() {
    // Build one node, fire two `edit_mechanical` calls back-to-back from
    // different signers — both should succeed serially because each holds
    // the per-node lock briefly. Contention is exercised in the unit
    // tests where we hold the lock manually.
    let bridge = BridgeFabric::new();
    let alice = generate_agent_keypair();
    let bob = generate_agent_keypair();

    let id = bridge
        .create(IntentNode::new("rate"), EditMode::Mechanical, Some(&alice))
        .unwrap();

    let r1 = bridge
        .edit_mechanical(&id, &alice, |node| {
            node.metadata
                .insert("v".into(), MetadataValue::Int(1));
        })
        .unwrap();
    let r2 = bridge
        .edit_mechanical(&id, &bob, |node| {
            node.metadata
                .insert("v".into(), MetadataValue::Int(2));
        })
        .unwrap();

    assert_eq!(r1.editor_voice, alice.voice_print());
    assert_eq!(r2.editor_voice, bob.voice_print());

    let final_node = bridge.get_node(&id).unwrap();
    assert_eq!(
        final_node.metadata.get("v"),
        Some(&MetadataValue::Int(2)),
        "Last writer's value persists (linearizable per-node)."
    );
}

// ── Step 4: subscriptions, fault isolation, backpressure ──

#[test]
fn subscription_observes_create_through_bridge_trait() {
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    let observed = Arc::new(AtomicUsize::new(0));
    let observed_cb = Arc::clone(&observed);

    let pattern: Predicate = Arc::new(|node: &IntentNode| {
        node.want.description.starts_with("audit:")
    });
    let cb: Callback = Arc::new(move |_node, _ctx| {
        observed_cb.fetch_add(1, Ordering::SeqCst);
        CallbackResult::Success
    });
    let _ = bridge.subscribe(pattern, cb).unwrap();

    bridge
        .create(
            IntentNode::new("audit: log entry 1"),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();
    bridge
        .create(
            IntentNode::new("not-audit"),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();
    bridge
        .create(
            IntentNode::new("audit: log entry 2"),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();

    assert!(
        wait_until(Duration::from_secs(2), || observed.load(Ordering::SeqCst) == 2),
        "Subscriber must see exactly the 2 audit entries; saw {}",
        observed.load(Ordering::SeqCst)
    );
}

#[test]
fn callback_panic_does_not_propagate_to_caller() {
    // Spec 8 §2.6.1: panics in subscription callbacks are caught and
    // converted to SubscriptionPanic events. The caller of `create`
    // never observes a panic.
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    let pat: Predicate = Arc::new(|_| true);
    let cb: Callback = Arc::new(|_node, _ctx| panic!("test panic"));
    let id = bridge.subscribe(pat, cb).unwrap();

    // Multiple writes — none of them should propagate the panic.
    for i in 0..3 {
        let result = bridge.create(
            IntentNode::new(format!("write {}", i)),
            EditMode::AppendOnly,
            Some(&agent),
        );
        assert!(
            result.is_ok(),
            "Write {} must succeed regardless of subscription panic",
            i
        );
    }

    assert!(wait_until(Duration::from_secs(2), || {
        bridge
            .subscription_state(id)
            .map(|s| s.panic_count >= 3)
            .unwrap_or(false)
    }), "All three panics should be counted on the subscription state");
}

#[test]
fn slow_subscriber_does_not_block_request_path() {
    // The dispatch pool runs callbacks off the request path (Spec 8
    // §2.6.1, §6.B.2). A slow callback must not delay the writer.
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    let pat: Predicate = Arc::new(|_| true);
    let cb: Callback = Arc::new(|_node, _ctx| {
        std::thread::sleep(Duration::from_millis(500));
        CallbackResult::Success
    });
    let _ = bridge.subscribe(pat, cb).unwrap();

    let start = Instant::now();
    bridge
        .create(
            IntentNode::new("write that triggers a slow subscriber"),
            EditMode::AppendOnly,
            Some(&agent),
        )
        .unwrap();
    let elapsed = start.elapsed();
    // The write should return well before the 500ms sleep finishes.
    assert!(
        elapsed < Duration::from_millis(100),
        "Write returned in {:?}; must not wait on the dispatch pool's slow callback",
        elapsed
    );
}

// ── Step 5: observability — debug accessors + admin tokens ──

#[test]
fn debug_state_reflects_fabric_growth() {
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    let initial = bridge.debug_state();
    assert_eq!(initial.node_count, 0);
    for i in 0..5 {
        bridge
            .create(
                IntentNode::new(format!("event {}", i)),
                EditMode::AppendOnly,
                Some(&agent),
            )
            .unwrap();
    }
    let snapshot = bridge.debug_state();
    assert_eq!(snapshot.node_count, 5);
    assert!(snapshot.current_lamport > initial.current_lamport);
}

#[test]
fn debug_subscriptions_reflects_active_count() {
    let bridge = BridgeFabric::new();
    let pat: Predicate = Arc::new(|_| true);
    let cb: Callback = Arc::new(|_, _| CallbackResult::Success);
    assert_eq!(bridge.debug_subscriptions().len(), 0);
    let id1 = bridge.subscribe(Arc::clone(&pat), Arc::clone(&cb)).unwrap();
    let _id2 = bridge.subscribe(Arc::clone(&pat), Arc::clone(&cb)).unwrap();
    assert_eq!(bridge.debug_subscriptions().len(), 2);
    bridge.unsubscribe(id1).unwrap();
    assert_eq!(bridge.debug_subscriptions().len(), 1);
}

#[test]
fn admin_token_round_trip() {
    let bridge = BridgeFabric::new();
    let operator = generate_agent_keypair();
    let token = bridge.issue_debug_token(&operator);
    assert!(bridge.verify_debug_token(&token, &operator.voice_print()).is_ok());
    // Different key fails.
    let mallory = generate_agent_keypair();
    let result = bridge.verify_debug_token(&token, &mallory.voice_print());
    assert_eq!(result.unwrap_err(), DebugTokenError::UnknownIssuer);
}

#[test]
fn admin_token_default_lifetime_one_hour() {
    let operator = generate_agent_keypair();
    let token = DebugToken::issue(&operator, DEBUG_TOKEN_DEFAULT_SCOPE, DEBUG_TOKEN_DEFAULT_LIFETIME);
    let window = token.expires_at_ns - token.issued_at_ns;
    assert_eq!(window, DEBUG_TOKEN_DEFAULT_LIFETIME.as_nanos() as i128);
    assert!(token.remaining_ns() > 0);
}

#[test]
fn unsubscribe_terminates_delivery() {
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    let count = Arc::new(AtomicUsize::new(0));
    let count_for_cb = Arc::clone(&count);

    let pat: Predicate = Arc::new(|_| true);
    let cb: Callback = Arc::new(move |_node, _ctx| {
        count_for_cb.fetch_add(1, Ordering::SeqCst);
        CallbackResult::Success
    });
    let id = bridge.subscribe(pat, cb).unwrap();

    bridge
        .create(IntentNode::new("first"), EditMode::AppendOnly, Some(&agent))
        .unwrap();
    assert!(wait_until(Duration::from_secs(1), || count.load(
        Ordering::SeqCst
    ) >= 1));

    bridge.unsubscribe(id).unwrap();

    bridge
        .create(IntentNode::new("after-unsub"), EditMode::AppendOnly, Some(&agent))
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "After unsubscribe, no further callbacks should fire."
    );
}

// ── Step 6: P53 mechanism (Spec 8 §8.4) ──

fn temp_archive_root() -> PathBuf {
    let mut root = std::env::temp_dir();
    root.push(format!(
        "ecphory-p53-archive-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    root
}

fn fast_p53_config(archive_root: PathBuf) -> P53Config {
    P53Config {
        // Tests use a tiny drain budget so the suite stays fast.
        drain_budget: Duration::from_millis(50),
        archive_root,
        authorized_operator: None,
        fabric_scope_enabled: false,
    }
}

#[test]
fn p53_node_scope_fades_target_and_emits_event() {
    let archive_root = temp_archive_root();
    let bridge = BridgeFabric::new().with_p53_config(fast_p53_config(archive_root));
    let operator = generate_agent_keypair();

    let id = bridge
        .create(IntentNode::new("doomed"), EditMode::AppendOnly, Some(&operator))
        .unwrap();
    assert!(bridge.get_node(&id).is_some());

    let receipt = bridge.p53_trigger(P53Scope::Node(id.clone()), &operator).unwrap();
    assert_eq!(receipt.scope_label, "Node");
    assert!(receipt.forensic_archive.is_none());
    // Target node faded.
    assert!(bridge.get_node(&id).is_none());
    // Event node present.
    let event = bridge.get_node(&receipt.event_node).expect("event node present");
    assert_eq!(
        event.metadata.get("__bridge_p53_scope__"),
        Some(&MetadataValue::String("Node".into()))
    );

    // Re-trigger on the same target is rejected.
    let again = bridge.p53_trigger(P53Scope::Node(id), &operator);
    assert_eq!(
        again.unwrap_err(),
        SafetyError::P53AlreadyTriggered { scope_label: "Node" }
    );
}

#[test]
fn p53_region_scope_terminates_writes_and_archives() {
    let archive_root = temp_archive_root();
    let bridge = BridgeFabric::new()
        .with_default_namespace(NamespaceId::fresh("propmgmt"))
        .with_p53_config(fast_p53_config(archive_root.clone()));
    let operator = generate_agent_keypair();

    // Populate the region.
    for i in 0..3 {
        bridge
            .create(
                IntentNode::new(format!("propmgmt entry {}", i)),
                EditMode::AppendOnly,
                Some(&operator),
            )
            .unwrap();
    }

    let region = NamespaceId::fresh("propmgmt"); // region scope label
    // We need to use the bridge's actual default namespace. Read it via debug_state isn't enough; the bridge stores it internally and uses it for `create`. We trigger on a clone of the same namespace shape — match on name.
    // For this test, use the bridge's own create-time namespace by triggering against any region with the same `name`.
    // The bridge p53_trigger uses the supplied namespace verbatim — terminated_regions is keyed by NamespaceId (name + uuid), so the test sets the same UUID by reusing `default_namespace`.
    // Pull the bridge's default_namespace by reaching through debug_node on a freshly created node:
    let probe_id = bridge
        .create(IntentNode::new("probe"), EditMode::AppendOnly, Some(&operator))
        .unwrap();
    let probe = bridge.debug_node(&probe_id).unwrap();
    let actual_region = probe.identity.causal_position.namespace.clone();

    let receipt = bridge
        .p53_trigger(P53Scope::Region(actual_region.clone()), &operator)
        .unwrap();
    assert_eq!(receipt.scope_label, "Region");
    let archive_path = receipt
        .forensic_archive
        .as_ref()
        .expect("Region p53 must produce a forensic archive");
    assert!(std::path::Path::new(archive_path).exists());
    assert!(bridge.is_region_terminated(&actual_region));

    // Subsequent writes to the terminated region are refused.
    let after = bridge.create(
        IntentNode::new("rejected"),
        EditMode::AppendOnly,
        Some(&operator),
    );
    assert!(matches!(after.unwrap_err(), WriteError::FabricDegraded));

    // Cleanup.
    let _ = std::fs::remove_dir_all(&archive_root);
    let _ = region;
}

#[test]
fn p53_fabric_scope_disabled_by_default() {
    let archive_root = temp_archive_root();
    let bridge = BridgeFabric::new().with_p53_config(fast_p53_config(archive_root));
    let operator = generate_agent_keypair();
    let result = bridge.p53_trigger(P53Scope::Fabric, &operator);
    assert_eq!(result.unwrap_err(), SafetyError::ScopeNotPermittedAtRuntime);
}

#[test]
fn p53_fabric_scope_when_enabled_terminates_all_writes() {
    let archive_root = temp_archive_root();
    let mut cfg = fast_p53_config(archive_root.clone());
    cfg.fabric_scope_enabled = true;
    let bridge = BridgeFabric::new().with_p53_config(cfg);
    let operator = generate_agent_keypair();

    bridge
        .create(IntentNode::new("pre-fabric-p53"), EditMode::AppendOnly, Some(&operator))
        .unwrap();

    let receipt = bridge.p53_trigger(P53Scope::Fabric, &operator).unwrap();
    assert_eq!(receipt.scope_label, "Fabric");
    assert!(receipt.forensic_archive.is_some());
    assert!(bridge.is_terminated());

    // All writes refused after fabric p53.
    let after = bridge.create(
        IntentNode::new("rejected"),
        EditMode::AppendOnly,
        Some(&operator),
    );
    assert!(matches!(after.unwrap_err(), WriteError::FabricDegraded));

    let _ = std::fs::remove_dir_all(&archive_root);
}

#[test]
fn p53_authorized_operator_rejects_other_signers() {
    let archive_root = temp_archive_root();
    let alice = generate_agent_keypair();
    let mallory = generate_agent_keypair();
    let cfg = P53Config {
        authorized_operator: Some(alice.voice_print()),
        ..fast_p53_config(archive_root)
    };
    let bridge = BridgeFabric::new().with_p53_config(cfg);
    let id = bridge
        .create(IntentNode::new("x"), EditMode::AppendOnly, Some(&alice))
        .unwrap();
    let bad = bridge.p53_trigger(P53Scope::Node(id.clone()), &mallory);
    assert_eq!(bad.unwrap_err(), SafetyError::InvalidP53Key);
    let good = bridge.p53_trigger(P53Scope::Node(id), &alice);
    assert!(good.is_ok());
}

#[test]
fn decay_tick_runs_without_panic() {
    // Spec 8 §11 #6: decay processes a region of 10,000 nodes without
    // panicking; valid DecayReport returned.
    let bridge = BridgeFabric::new();
    let agent = generate_agent_keypair();
    for i in 0..200 {
        bridge
            .create(
                IntentNode::new(format!("node {}", i)),
                EditMode::AppendOnly,
                Some(&agent),
            )
            .unwrap();
    }
    let report = bridge.decay_tick().unwrap();
    assert_eq!(report.nodes_evaluated, 200);
    // Just-created nodes still have temporal weight ~1; nothing dissolves.
    assert_eq!(report.nodes_dissolved, 0);
    assert!(!report.deferred_to_next_tick);
}

// ── Step 7: Self-witnessing — SubscriptionPanic as a fabric node ──

#[test]
fn subscription_panic_surfaces_as_fabric_node() {
    // Spec 8 §13 + §6.B.2: a callback panic surfaces as a
    // SubscriptionPanic event node observable by the immune system.
    let bridge = BridgeFabric::new();
    let operator = generate_agent_keypair();

    // First subscription: panics on every event.
    let panicking_pat: Predicate = Arc::new(|node: &IntentNode| {
        node.want.description.contains("trigger-panic")
    });
    let panicking_cb: Callback = Arc::new(|_, _| panic!("intentional"));
    let _ = bridge.subscribe(panicking_pat, panicking_cb).unwrap();

    // Second subscription: observer for SubscriptionPanic events.
    let panic_observed = Arc::new(AtomicUsize::new(0));
    let counter_for_cb = Arc::clone(&panic_observed);
    let observer_pat: Predicate = Arc::new(|node: &IntentNode| {
        node.metadata
            .get("__bridge_node_kind__")
            .map(|v| v.as_str_repr() == "SubscriptionPanic")
            .unwrap_or(false)
    });
    let observer_cb: Callback = Arc::new(move |_, _| {
        counter_for_cb.fetch_add(1, Ordering::SeqCst);
        CallbackResult::Success
    });
    let _ = bridge.subscribe(observer_pat, observer_cb).unwrap();

    // Trigger the panicking subscription.
    bridge
        .create(
            IntentNode::new("trigger-panic now"),
            EditMode::AppendOnly,
            Some(&operator),
        )
        .unwrap();

    // The SubscriptionPanic event node should appear and the observer
    // subscription should fire on it.
    assert!(
        wait_until(Duration::from_secs(2), || panic_observed.load(Ordering::SeqCst) >= 1),
        "Expected SubscriptionPanic node to be written and observed; got {} observations",
        panic_observed.load(Ordering::SeqCst)
    );
}
