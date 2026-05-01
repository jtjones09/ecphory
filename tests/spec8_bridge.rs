// Integration tests for Spec 8 (Phase F bridge) — Steps 1-3.
//
// Per handoff: tests must use real keypairs and real BLAKE3 hashes —
// no mocks, no shortcuts. The fabric runs in-process through the
// `BridgeFabric` API.

use std::time::Duration;

use ecphory::bridge::{BridgeFabric, FabricTrait};
use ecphory::{
    generate_agent_keypair, EditMode, IntentNode, MetadataValue, WriteError,
};

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
