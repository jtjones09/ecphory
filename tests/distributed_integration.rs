// DISTRIBUTED INTEGRATION TESTS — PHASE 3d
//
// Multi-replica simulation using DistributedFabric + LocalTransport.
// Tests gossip protocol, conflict resolution, and vector clock convergence.

use ecphory::*;

// ─── Basic DistributedFabric Operations ───

#[test]
fn distributed_fabric_add_and_retrieve() {
    let mut df = DistributedFabric::new();
    let id = df.add_node(IntentNode::new("test node"));
    let node = df.fabric.get_node(&id).unwrap();
    assert_eq!(node.want.description, "test node");
}

#[test]
fn distributed_fabric_clock_advances_on_add() {
    let mut df = DistributedFabric::new();
    let c0 = df.clock.get(&df.replica_id);
    df.add_node(IntentNode::new("first"));
    let c1 = df.clock.get(&df.replica_id);
    df.add_node(IntentNode::new("second"));
    let c2 = df.clock.get(&df.replica_id);
    assert_eq!(c0, 0);
    assert_eq!(c1, 1);
    assert_eq!(c2, 2);
}

#[test]
fn distributed_fabric_gossip_tracks_nodes() {
    let mut df = DistributedFabric::new();
    let id = df.add_node(IntentNode::new("tracked"));
    assert_eq!(df.gossip.node_count(), 1);
    assert!(df.gossip.known_version(&id).is_some());
}

// ─── Two-Replica Sync ───

#[test]
fn two_replica_full_sync() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    // Each adds a unique node.
    let id_a = replica_a.add_node(IntentNode::new("node from A"));
    let id_b = replica_b.add_node(IntentNode::new("node from B"));

    // A sends digest to B; B determines what it needs.
    let digest_a = replica_a.generate_digest();
    let b_needs = replica_b.needs_from_digest(&digest_a);
    assert_eq!(b_needs.len(), 1);
    assert_eq!(b_needs[0], id_a);

    // A packages and sends the nodes B needs.
    let transfers = replica_a.package_nodes(&b_needs);
    for t in transfers {
        let result = replica_b.receive_node(t);
        assert_eq!(result, ReceiveResult::Accepted);
    }

    // B now has both nodes.
    assert_eq!(replica_b.fabric.node_count(), 2);

    // Now sync in reverse: B sends to A.
    let digest_b = replica_b.generate_digest();
    let a_needs = replica_a.needs_from_digest(&digest_b);
    // A needs the node originally from B.
    assert!(a_needs.contains(&id_b));

    let transfers = replica_b.package_nodes(&a_needs);
    for t in transfers {
        let result = replica_a.receive_node(t);
        assert_eq!(result, ReceiveResult::Accepted);
    }

    // Both replicas now have the same two nodes.
    assert_eq!(replica_a.fabric.node_count(), 2);
    assert_eq!(replica_b.fabric.node_count(), 2);
}

#[test]
fn receive_already_known_node() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    // A adds a node.
    let id = replica_a.add_node(IntentNode::new("shared"));

    // Sync to B.
    let transfers = replica_a.package_nodes(&[id.clone()]);
    replica_b.receive_node(transfers[0].clone());

    // Sync again — B should report AlreadyHave.
    let transfers2 = replica_a.package_nodes(&[id]);
    let result = replica_b.receive_node(transfers2[0].clone());
    assert_eq!(result, ReceiveResult::AlreadyHave);
}

#[test]
fn receive_newer_version_updates() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    // A creates a node.
    let id = replica_a.add_node(IntentNode::new("evolving"));

    // Sync to B (version 0).
    let transfers = replica_a.package_nodes(&[id.clone()]);
    replica_b.receive_node(transfers[0].clone());

    // A mutates the node (version bumps).
    replica_a.fabric.mutate_node(&id, |n| {
        n.want.description = "evolved".to_string();
    }).unwrap();
    let new_version = replica_a.fabric.get_node(&id).unwrap().version();
    assert!(new_version > 0);

    // A ticks its clock for the mutation.
    replica_a.clock.tick(&replica_a.replica_id);
    replica_a.gossip.record(id.clone(), new_version, replica_a.clock.clone());

    // Send updated node to B.
    let transfers2 = replica_a.package_nodes(&[id.clone()]);
    let result = replica_b.receive_node(transfers2[0].clone());
    assert_eq!(result, ReceiveResult::Updated);

    // B now has the updated description.
    let b_node = replica_b.fabric.get_node(&id).unwrap();
    assert_eq!(b_node.want.description, "evolved");
}

// ─── Conflict Detection and Resolution ───

#[test]
fn concurrent_modifications_trigger_conflict() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    // Both create a node with the same lineage ID.
    // Simulate by creating on A, syncing to B, then both modify independently.
    let id = replica_a.add_node(IntentNode::new("original"));

    // Sync to B.
    let transfers = replica_a.package_nodes(&[id.clone()]);
    replica_b.receive_node(transfers[0].clone());

    // A mutates independently.
    replica_a.fabric.mutate_node(&id, |n| {
        n.want.description = "version A".to_string();
    }).unwrap();
    replica_a.clock.tick(&replica_a.replica_id);
    let va = replica_a.fabric.get_node(&id).unwrap().version();
    replica_a.gossip.record(id.clone(), va, replica_a.clock.clone());

    // B also mutates independently.
    replica_b.fabric.mutate_node(&id, |n| {
        n.want.description = "version B".to_string();
    }).unwrap();
    replica_b.clock.tick(&replica_b.replica_id);
    let vb = replica_b.fabric.get_node(&id).unwrap().version();
    replica_b.gossip.record(id.clone(), vb, replica_b.clock.clone());

    // A sends to B → concurrent modification → conflict.
    let transfers2 = replica_a.package_nodes(&[id.clone()]);
    let result = replica_b.receive_node(transfers2[0].clone());
    assert_eq!(result, ReceiveResult::Conflicted);
    assert_eq!(replica_b.conflict_count(), 1);
}

#[test]
fn lww_conflict_picks_higher_version() {
    // Direct conflict resolution test.
    let mut local = IntentNode::new("test");
    local.recompute_signature(); // version 1

    let remote = IntentNode::new("test"); // version 0

    let conflict = Conflict {
        lineage_id: local.lineage_id().clone(),
        local: local.clone(),
        local_clock: VectorClock::new(),
        remote: remote.clone(),
        remote_clock: VectorClock::new(),
    };

    let resolution = resolve_conflict(&conflict);
    match resolution {
        Resolution::Resolved { winner, strategy } => {
            assert_eq!(strategy, ConflictStrategy::LastWriterWins);
            assert_eq!(winner.version(), 1);
        }
        _ => panic!("Expected Resolved with LWW"),
    }
}

#[test]
fn branch_and_surface_for_hard_violations() {
    let mut local = IntentNode::new("sensitive");
    local.constraints.add_hard("must be safe");
    local.constraints.constraints[0].violate();
    local.recompute_signature(); // version 1

    let remote = IntentNode::new("sensitive"); // version 0

    let conflict = Conflict {
        lineage_id: local.lineage_id().clone(),
        local,
        local_clock: VectorClock::new(),
        remote,
        remote_clock: VectorClock::new(),
    };

    let resolution = resolve_conflict(&conflict);
    match resolution {
        Resolution::Branched { branch_a, branch_b } => {
            assert_ne!(branch_a.signature(), branch_b.signature());
        }
        _ => panic!("Expected Branched resolution for hard violation"),
    }
}

#[test]
fn crdt_merge_unions_context_edges() {
    let mut local = IntentNode::new("test");
    let mut remote = IntentNode::new("test");

    let target_a = LineageId::new();
    let target_b = LineageId::new();

    local.context.add_edge(target_a.clone(), 0.5, RelationshipKind::RelatedTo);
    remote.context.add_edge(target_b.clone(), 0.7, RelationshipKind::DependsOn);

    let conflict = Conflict {
        lineage_id: local.lineage_id().clone(),
        local,
        local_clock: VectorClock::new(),
        remote,
        remote_clock: VectorClock::new(),
    };

    let resolution = resolve_conflict(&conflict);
    match resolution {
        Resolution::Resolved { winner, strategy } => {
            assert_eq!(strategy, ConflictStrategy::CrdtMerge);
            assert_eq!(winner.context.edges.len(), 2);
        }
        _ => panic!("Expected CRDT merge"),
    }
}

#[test]
fn crdt_merge_averages_confidence_by_observations() {
    let mut local = IntentNode::new("test");
    let mut remote = IntentNode::new("test");

    // Local has 10 observations at 0.8
    local.confidence.comprehension.mean = 0.8;
    local.confidence.comprehension.observations = 10;
    local.confidence.resolution.observations = 0;
    local.confidence.verification.observations = 0;

    // Remote has 10 observations at 0.4
    remote.confidence.comprehension.mean = 0.4;
    remote.confidence.comprehension.observations = 10;
    remote.confidence.resolution.observations = 0;
    remote.confidence.verification.observations = 0;

    let conflict = Conflict {
        lineage_id: local.lineage_id().clone(),
        local,
        local_clock: VectorClock::new(),
        remote,
        remote_clock: VectorClock::new(),
    };

    let resolution = resolve_conflict(&conflict);
    match resolution {
        Resolution::Resolved { winner, .. } => {
            // Equal observations → should average to ~0.6
            let merged = winner.confidence.comprehension.mean;
            assert!((merged - 0.6).abs() < 0.01, "Expected ~0.6, got {}", merged);
        }
        _ => panic!("Expected CRDT merge"),
    }
}

// ─── Vector Clock Properties ───

#[test]
fn vector_clocks_track_causality_across_replicas() {
    let r1 = ReplicaId::new();
    let r2 = ReplicaId::new();

    // r1 does some work.
    let mut vc1 = VectorClock::new();
    vc1.tick(&r1);
    vc1.tick(&r1);

    // r2 starts fresh.
    let mut vc2 = VectorClock::new();
    vc2.tick(&r2);

    // They are concurrent (neither happened before the other).
    assert!(vc1.is_concurrent_with(&vc2));

    // r2 receives r1's clock (merges).
    vc2.merge(&vc1);
    vc2.tick(&r2);

    // Now vc1 happened-before vc2.
    assert!(vc1.happened_before(&vc2));
    assert!(!vc2.happened_before(&vc1));
}

#[test]
fn clocks_merge_after_sync() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    // A does work.
    replica_a.add_node(IntentNode::new("a1"));
    replica_a.add_node(IntentNode::new("a2"));

    // B does work.
    replica_b.add_node(IntentNode::new("b1"));

    // Sync A → B.
    let digest_a = replica_a.generate_digest();
    let b_needs = replica_b.needs_from_digest(&digest_a);
    let transfers = replica_a.package_nodes(&b_needs);
    for t in transfers {
        replica_b.receive_node(t);
    }

    // B's clock should now know about A's replica.
    assert!(replica_b.clock.get(&replica_a.replica_id) > 0,
        "B's clock should know about A after sync");
}

// ─── Gossip Protocol ───

#[test]
fn gossip_digest_round_trip() {
    let mut gs = GossipState::new();
    let id1 = LineageId::new();
    let id2 = LineageId::new();

    gs.record(id1.clone(), 3, VectorClock::new());
    gs.record(id2.clone(), 7, VectorClock::new());

    let digest = gs.generate_digest();
    assert_eq!(digest.entries.len(), 2);

    // A fresh gossip state needs both nodes.
    let fresh = GossipState::new();
    let needed = fresh.compare_digest(&digest);
    assert_eq!(needed.len(), 2);
}

#[test]
fn gossip_compare_skips_up_to_date() {
    let mut gs = GossipState::new();
    let id = LineageId::new();
    gs.record(id.clone(), 5, VectorClock::new());

    // Remote has version 3 — we're ahead.
    let remote_digest = Digest {
        entries: vec![DigestEntry {
            lineage_id: id.clone(),
            version: 3,
        }],
    };

    let needed = gs.compare_digest(&remote_digest);
    assert!(needed.is_empty(), "Should not need node we already have at higher version");
}

// ─── Transport ───

#[test]
fn local_transport_full_gossip_exchange() {
    let mut transport = LocalTransport::new();
    let r1 = ReplicaId::new();
    let r2 = ReplicaId::new();

    // r1 sends a digest to r2.
    let digest = Digest { entries: vec![] };
    transport.send(Envelope {
        from: r1.clone(),
        to: r2.clone(),
        message: GossipMessage::Digest(digest),
    });

    assert!(transport.has_pending(&r2));
    assert!(!transport.has_pending(&r1));

    // r2 receives.
    let messages = transport.receive(&r2);
    assert_eq!(messages.len(), 1);
    assert!(!transport.has_pending(&r2));

    // r2 responds with NeedNodes.
    let need = LineageId::new();
    transport.send(Envelope {
        from: r2.clone(),
        to: r1.clone(),
        message: GossipMessage::NeedNodes(vec![need]),
    });

    let replies = transport.receive(&r1);
    assert_eq!(replies.len(), 1);
}

// ─── Three-Replica Convergence ───

#[test]
fn three_replica_eventual_convergence() {
    let mut r_a = DistributedFabric::new();
    let mut r_b = DistributedFabric::new();
    let mut r_c = DistributedFabric::new();

    // Each adds a unique node.
    let id_a = r_a.add_node(IntentNode::new("node A"));
    let id_b = r_b.add_node(IntentNode::new("node B"));
    let id_c = r_c.add_node(IntentNode::new("node C"));

    // Round 1: A → B
    let digest_a = r_a.generate_digest();
    let b_needs = r_b.needs_from_digest(&digest_a);
    for t in r_a.package_nodes(&b_needs) {
        r_b.receive_node(t);
    }

    // Round 2: B → C (B now has A's node too)
    let digest_b = r_b.generate_digest();
    let c_needs = r_c.needs_from_digest(&digest_b);
    for t in r_b.package_nodes(&c_needs) {
        r_c.receive_node(t);
    }

    // Round 3: C → A (C now has everything)
    let digest_c = r_c.generate_digest();
    let a_needs = r_a.needs_from_digest(&digest_c);
    for t in r_c.package_nodes(&a_needs) {
        r_a.receive_node(t);
    }

    // Round 4: C → B (B still missing C's node)
    let digest_c2 = r_c.generate_digest();
    let b_needs2 = r_b.needs_from_digest(&digest_c2);
    for t in r_c.package_nodes(&b_needs2) {
        r_b.receive_node(t);
    }

    // All three replicas should have all three nodes.
    assert_eq!(r_a.fabric.node_count(), 3);
    assert_eq!(r_b.fabric.node_count(), 3);
    assert_eq!(r_c.fabric.node_count(), 3);

    // Verify specific nodes exist on each replica.
    assert!(r_a.fabric.get_node(&id_b).is_some());
    assert!(r_a.fabric.get_node(&id_c).is_some());
    assert!(r_b.fabric.get_node(&id_a).is_some());
    assert!(r_b.fabric.get_node(&id_c).is_some());
    assert!(r_c.fabric.get_node(&id_a).is_some());
    assert!(r_c.fabric.get_node(&id_b).is_some());
}

// ─── Conflict Log Inspection ───

#[test]
fn conflict_log_is_auditable() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    let id = replica_a.add_node(IntentNode::new("contested"));
    let transfers = replica_a.package_nodes(&[id.clone()]);
    replica_b.receive_node(transfers[0].clone());

    // Both mutate independently.
    replica_a.fabric.mutate_node(&id, |n| {
        n.want.description = "A's version".to_string();
    }).unwrap();
    replica_a.clock.tick(&replica_a.replica_id);
    let va = replica_a.fabric.get_node(&id).unwrap().version();
    replica_a.gossip.record(id.clone(), va, replica_a.clock.clone());

    replica_b.fabric.mutate_node(&id, |n| {
        n.want.description = "B's version".to_string();
    }).unwrap();
    replica_b.clock.tick(&replica_b.replica_id);
    let vb = replica_b.fabric.get_node(&id).unwrap().version();
    replica_b.gossip.record(id.clone(), vb, replica_b.clock.clone());

    // Sync A → B triggers conflict.
    let transfers2 = replica_a.package_nodes(&[id.clone()]);
    replica_b.receive_node(transfers2[0].clone());

    // Inspect the conflict log.
    assert_eq!(replica_b.conflict_log.len(), 1);
    let (conflict, resolution) = &replica_b.conflict_log[0];
    assert_eq!(conflict.lineage_id, id);

    // Resolution should be CRDT merge (same version = 1 on both).
    match resolution {
        Resolution::Resolved { strategy, .. } => {
            assert_eq!(*strategy, ConflictStrategy::CrdtMerge);
        }
        Resolution::Branched { .. } => {
            // Also acceptable depending on constraints.
        }
    }
}

// ─── Package Nodes ───

#[test]
fn package_unknown_nodes_returns_empty() {
    let df = DistributedFabric::new();
    let unknown = LineageId::new();
    let packages = df.package_nodes(&[unknown]);
    assert!(packages.is_empty());
}

#[test]
fn package_includes_vector_clock() {
    let mut df = DistributedFabric::new();
    let id = df.add_node(IntentNode::new("clocked"));
    let packages = df.package_nodes(&[id]);
    assert_eq!(packages.len(), 1);
    // The transfer should include the current clock state.
    assert!(packages[0].vector_clock.get(&df.replica_id) > 0);
}

// ─── Digest Convergence ───

#[test]
fn synced_replicas_need_nothing_from_each_other() {
    let mut replica_a = DistributedFabric::new();
    let mut replica_b = DistributedFabric::new();

    // A adds a node, syncs to B.
    let id = replica_a.add_node(IntentNode::new("shared"));
    let transfers = replica_a.package_nodes(&[id]);
    replica_b.receive_node(transfers[0].clone());

    // Both digests should show the same node.
    let digest_a = replica_a.generate_digest();

    // The key test: B should not need anything from A.
    let b_needs = replica_b.needs_from_digest(&digest_a);
    // B received from A, so B's version >= A's version.
    assert!(b_needs.is_empty(), "B should not need anything it already received from A");
}
