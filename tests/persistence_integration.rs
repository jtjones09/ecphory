// PERSISTENCE INTEGRATION TESTS
//
// End-to-end tests that save and load fabrics through the full pipeline.
// These test the persistence system as a whole, not individual conversions.

use ecphory::*;
use std::fs;

fn temp_path(name: &str) -> String {
    format!("/tmp/intent_persist_integration_{}.json", name)
}

fn cleanup(path: &str) {
    let _ = fs::remove_file(path);
}

#[test]
fn save_and_load_full_fabric() {
    let path = temp_path("full");
    cleanup(&path);

    // Build a fabric with nodes, edges, mutations.
    let mut fabric = Fabric::new();
    let msg = fabric.add_node(IntentNode::understood("send a message to my brother", 0.85));
    let privacy = fabric.add_node(IntentNode::understood("ensure message privacy end-to-end", 0.95));
    let groceries = fabric.add_node(IntentNode::new("buy groceries for dinner"));

    fabric.add_edge(&msg, &privacy, 0.9, RelationshipKind::DependsOn).unwrap();

    fabric.mutate_node(&msg, |n| {
        n.want.description = "send a private message to my brother via Signal".to_string();
    }).unwrap();

    // Save.
    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();

    // Load.
    let loaded = store.load().unwrap();

    // Verify everything survived.
    assert_eq!(loaded.node_count(), 3);
    assert_eq!(loaded.edge_count(), 1);

    // Lineage IDs preserved.
    assert!(loaded.contains(&msg));
    assert!(loaded.contains(&privacy));
    assert!(loaded.contains(&groceries));

    // Mutated content preserved.
    let loaded_msg = loaded.get_node(&msg).unwrap();
    assert_eq!(loaded_msg.want.description, "send a private message to my brother via Signal");
    assert_eq!(loaded_msg.version(), 1);

    // Edges preserved with correct targets.
    let edges = loaded.edges_from(&msg);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target, privacy);
    assert!((edges[0].weight - 0.9).abs() < 1e-10);

    // Node context synced from edges.
    assert_eq!(loaded_msg.context.connection_count(), 1);

    cleanup(&path);
}

#[test]
fn loaded_fabric_can_add_nodes() {
    let path = temp_path("add_after_load");
    cleanup(&path);

    let mut fabric = Fabric::new();
    fabric.add_node(IntentNode::new("existing node"));
    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();

    let mut loaded = store.load().unwrap();
    let new_id = loaded.add_node(IntentNode::new("new node after load"));
    assert_eq!(loaded.node_count(), 2);
    assert!(loaded.contains(&new_id));

    cleanup(&path);
}

#[test]
fn loaded_fabric_resonance_works() {
    let path = temp_path("resonance");
    cleanup(&path);

    let mut fabric = Fabric::new();
    fabric.add_node(IntentNode::new("buy groceries for dinner"));
    fabric.add_node(IntentNode::new("send a message to my brother"));
    fabric.add_node(IntentNode::new("walk the dog in the park"));

    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();
    let loaded = store.load().unwrap();

    let results = loaded.resonate("buy food for dinner", 3);
    assert!(!results.is_empty());
    // The grocery node should have highest semantic similarity.
    let top = &results[0];
    let top_node = loaded.get_node(&top.lineage_id).unwrap();
    assert!(top_node.want.description.contains("groceries"));

    cleanup(&path);
}

#[test]
fn loaded_fabric_clock_continues() {
    let path = temp_path("clock");
    cleanup(&path);

    let mut fabric = Fabric::new();
    fabric.add_node(IntentNode::new("node 1"));
    fabric.add_node(IntentNode::new("node 2"));
    let saved_clock = fabric.clock_value();

    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();
    let loaded = store.load().unwrap();

    // Clock value should be at least as high as when saved.
    assert!(loaded.clock_value() >= saved_clock);

    cleanup(&path);
}

#[test]
fn loaded_fabric_signatures_match() {
    let path = temp_path("signatures");
    cleanup(&path);

    let mut fabric = Fabric::new();
    let node = IntentNode::understood("test signature persistence", 0.9);
    let original_sig = node.signature().clone();
    let id = fabric.add_node(node);

    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();
    let loaded = store.load().unwrap();

    let loaded_node = loaded.get_node(&id).unwrap();
    assert_eq!(loaded_node.signature(), &original_sig);
    assert!(loaded.get_node_by_signature(&original_sig).is_some());

    cleanup(&path);
}
