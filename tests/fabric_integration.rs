// FABRIC INTEGRATION TESTS
//
// End-to-end tests that span multiple modules.
// These test the fabric as a system, not individual components.

use ecphory::*;

#[test]
fn end_to_end_fabric_workflow() {
    // Add → Connect → Query → Mutate → Fade
    let mut fabric = Fabric::new();

    // Add nodes
    let msg = fabric.add_node(IntentNode::understood("send a message to my brother", 0.85));
    let privacy = fabric.add_node(IntentNode::understood("ensure message privacy end-to-end", 0.95));
    let groceries = fabric.add_node(IntentNode::new("buy groceries for dinner"));

    assert_eq!(fabric.node_count(), 3);

    // Connect
    fabric.add_edge(&msg, &privacy, 0.9, RelationshipKind::DependsOn).unwrap();

    // Query by resonance
    let results = fabric.resonate("send message privately", 3);
    assert!(!results.is_empty());
    // The messaging intent should rank highest
    assert!(results[0].score > 0.0);

    // Mutate
    fabric.mutate_node(&msg, |n| {
        n.want.description = "send a private message to my brother via Signal".to_string();
    }).unwrap();
    assert_eq!(fabric.get_node(&msg).unwrap().version(), 1);

    // Fade
    fabric.fade_node(&groceries);
    assert_eq!(fabric.node_count(), 2);
    assert!(!fabric.contains(&groceries));

    // Edges still work after fade
    assert_eq!(fabric.edges_from(&msg).len(), 1);
}

#[test]
fn resonance_finds_semantically_related_nodes() {
    let mut fabric = Fabric::new();

    fabric.add_node(IntentNode::new("buy groceries for dinner"));
    fabric.add_node(IntentNode::new("purchase food for the evening meal"));
    fabric.add_node(IntentNode::new("send a message to my brother"));
    fabric.add_node(IntentNode::new("walk the dog in the park"));

    // "buy food" should resonate with grocery-related nodes
    let results = fabric.resonate("buy food for dinner", 4);

    // The grocery node should be the top result (shares "buy", "dinner", "for")
    assert!(results[0].components.semantic_similarity > 0.0);

    // "walk the dog" should not resonate with "buy food"
    let dog_result = results.iter().find(|r| {
        fabric.get_node(&r.lineage_id).unwrap().want.description.contains("dog")
    });
    if let Some(r) = dog_result {
        assert_eq!(r.components.semantic_similarity, 0.0);
    }
}

#[test]
fn temporal_decay_affects_retrieval() {
    // Use a very short half-life to test decay without real sleeps
    let mut fabric = Fabric::new();
    fabric.set_decay_half_life(1.0); // 1 second half-life

    // Add a "fresh" node
    let fresh = IntentNode::new("important task");
    fabric.add_node(fresh);

    // Get weight of just-created node
    let results = fabric.resonate("important task", 1);
    assert!(!results.is_empty());
    // Temporal weight should be near 1.0 for fresh node
    assert!(results[0].components.temporal_weight > 0.9);
}

#[test]
fn fabric_edges_sync_to_node_context() {
    let mut fabric = Fabric::new();
    let a = fabric.add_node(IntentNode::new("task A"));
    let b = fabric.add_node(IntentNode::new("task B"));
    let c = fabric.add_node(IntentNode::new("task C"));

    // Add edges via fabric
    fabric.add_edge(&a, &b, 0.9, RelationshipKind::DependsOn).unwrap();
    fabric.add_edge(&a, &c, 0.5, RelationshipKind::RelatedTo).unwrap();

    // Node A's context should reflect fabric's edges
    let node_a = fabric.get_node(&a).unwrap();
    assert_eq!(node_a.context.connection_count(), 2);
    assert!(node_a.context.has_dependencies());

    // Remove one edge
    fabric.remove_edges_between(&a, &c);
    let node_a = fabric.get_node(&a).unwrap();
    assert_eq!(node_a.context.connection_count(), 1);
}

#[test]
fn fade_node_cleans_up_all_edges() {
    let mut fabric = Fabric::new();
    let a = fabric.add_node(IntentNode::new("A"));
    let b = fabric.add_node(IntentNode::new("B"));
    let c = fabric.add_node(IntentNode::new("C"));

    fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
    fabric.add_edge(&b, &c, 0.5, RelationshipKind::Follows).unwrap();
    assert_eq!(fabric.edge_count(), 2);

    // Fade B: should remove A→B and B→C
    fabric.fade_node(&b);
    assert_eq!(fabric.edge_count(), 0);

    // A's context should be empty now
    let node_a = fabric.get_node(&a).unwrap();
    assert_eq!(node_a.context.connection_count(), 0);
}

#[test]
fn mutate_node_updates_all_indices() {
    let mut fabric = Fabric::new();
    let node = IntentNode::new("original want");
    let old_sig = node.signature().clone();
    let id = fabric.add_node(node);

    // Verify old signature works
    assert!(fabric.get_node_by_signature(&old_sig).is_some());

    // Mutate
    fabric.mutate_node(&id, |n| {
        n.want.description = "completely different want".to_string();
    }).unwrap();

    // Old signature should no longer resolve
    assert!(fabric.get_node_by_signature(&old_sig).is_none());

    // New signature should resolve
    let new_node = fabric.get_node(&id).unwrap();
    assert!(fabric.get_node_by_signature(new_node.signature()).is_some());

    // LineageId unchanged
    assert_eq!(fabric.get_node(&id).unwrap().lineage_id(), &id);

    // Version bumped
    assert_eq!(fabric.get_node(&id).unwrap().version(), 1);
}
