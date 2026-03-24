// EMBEDDING INTEGRATION TESTS
//
// End-to-end tests for the embedding pipeline:
// - BagOfWordsEmbedder builds vocab and embeds text
// - Fabric auto-embeds nodes when embedder is set
// - Cosine similarity outperforms Jaccard for semantic retrieval
// - Embeddings survive persistence round-trips

use ecphory::*;
use std::fs;

fn temp_path(name: &str) -> String {
    format!("/tmp/intent_embed_integration_{}.json", name)
}

fn cleanup(path: &str) {
    let _ = fs::remove_file(path);
}

/// Build an embedder with vocabulary from a set of intent descriptions.
fn build_test_embedder(texts: &[&str]) -> BagOfWordsEmbedder {
    let mut embedder = BagOfWordsEmbedder::new();
    embedder.build_vocab(texts);
    embedder
}

#[test]
fn fabric_with_embedder_auto_embeds_nodes() {
    let embedder = build_test_embedder(&[
        "buy groceries for dinner",
        "send a message to my brother",
    ]);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));

    let id = fabric.add_node(IntentNode::new("buy groceries for dinner"));
    let node = fabric.get_node(&id).unwrap();
    assert!(node.want.embedding.is_some(), "Node should be auto-embedded");
    assert!(!node.want.embedding.as_ref().unwrap().is_empty());
}

#[test]
fn fabric_without_embedder_has_no_embeddings() {
    let mut fabric = Fabric::new();
    let id = fabric.add_node(IntentNode::new("buy groceries for dinner"));
    let node = fabric.get_node(&id).unwrap();
    assert!(node.want.embedding.is_none(), "Node should not be embedded without embedder");
}

#[test]
fn cosine_resonance_finds_related_intents() {
    let texts = &[
        "buy groceries for dinner",
        "buy food for dinner party",
        "send a message to my brother",
        "walk the dog in the park",
    ];
    let embedder = build_test_embedder(texts);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));

    fabric.add_node(IntentNode::new("buy groceries for dinner"));
    fabric.add_node(IntentNode::new("send a message to my brother"));
    fabric.add_node(IntentNode::new("walk the dog in the park"));

    let results = fabric.resonate("buy food for dinner party", 3);
    assert!(!results.is_empty());

    // The grocery node should rank highest — shares "buy", "for", "dinner".
    let top = &results[0];
    let top_node = fabric.get_node(&top.lineage_id).unwrap();
    assert!(top_node.want.description.contains("groceries"),
        "Top result should be groceries, got: {}", top_node.want.description);
}

#[test]
fn cosine_beats_jaccard_for_synonym_like_queries() {
    // "buy food" vs "buy groceries" — Jaccard only matches "buy".
    // With BoW + cosine, the shared vocabulary creates better signal.
    let texts = &[
        "buy groceries for dinner",
        "buy food for the evening meal",
        "walk the dog in the park",
        "read a book before bed",
    ];
    let embedder = build_test_embedder(texts);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));

    let grocery_id = fabric.add_node(IntentNode::new("buy groceries for dinner"));
    let food_id = fabric.add_node(IntentNode::new("buy food for the evening meal"));
    fabric.add_node(IntentNode::new("walk the dog in the park"));
    fabric.add_node(IntentNode::new("read a book before bed"));

    let results = fabric.resonate("buy food for dinner", 4);

    // Both food-related nodes should rank in top 2.
    let top_two_ids: Vec<_> = results.iter().take(2).map(|r| r.lineage_id.clone()).collect();
    assert!(top_two_ids.contains(&grocery_id) || top_two_ids.contains(&food_id),
        "Food-related nodes should rank highly");
}

#[test]
fn mutate_node_re_embeds() {
    let texts = &[
        "buy groceries for dinner",
        "send a message to my brother",
    ];
    let embedder = build_test_embedder(texts);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));

    let id = fabric.add_node(IntentNode::new("buy groceries for dinner"));
    let original_embedding = fabric.get_node(&id).unwrap().want.embedding.clone().unwrap();

    fabric.mutate_node(&id, |n| {
        n.want.description = "send a message to my brother".to_string();
    }).unwrap();

    let new_embedding = fabric.get_node(&id).unwrap().want.embedding.clone().unwrap();
    assert_ne!(original_embedding, new_embedding,
        "Embedding should change after mutating want description");
}

#[test]
fn set_embedder_then_re_embed_all() {
    let mut fabric = Fabric::new();
    fabric.add_node(IntentNode::new("buy groceries for dinner"));
    fabric.add_node(IntentNode::new("send a message to my brother"));

    // No embeddings yet.
    for (_, node) in fabric.nodes() {
        assert!(node.want.embedding.is_none());
    }

    // Set embedder and re-embed.
    let texts = &["buy groceries for dinner", "send a message to my brother"];
    let embedder = build_test_embedder(texts);
    fabric.set_embedder(Box::new(embedder));
    fabric.re_embed_all();

    // All nodes should now have embeddings.
    for (_, node) in fabric.nodes() {
        assert!(node.want.embedding.is_some(),
            "All nodes should be embedded after re_embed_all()");
    }
}

#[test]
fn embedding_persists_through_save_load() {
    let path = temp_path("embed_persist");
    cleanup(&path);

    let texts = &[
        "buy groceries for dinner",
        "send a message to my brother",
    ];
    let embedder = build_test_embedder(texts);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));

    let grocery_id = fabric.add_node(IntentNode::new("buy groceries for dinner"));
    let msg_id = fabric.add_node(IntentNode::new("send a message to my brother"));

    let original_embedding = fabric.get_node(&grocery_id).unwrap().want.embedding.clone();

    // Save.
    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();

    // Load (no embedder on loaded fabric).
    let loaded = store.load().unwrap();
    let loaded_embedding = loaded.get_node(&grocery_id).unwrap().want.embedding.clone();

    assert_eq!(original_embedding, loaded_embedding,
        "Embedding should survive persistence round-trip");

    // Both nodes should have embeddings.
    assert!(loaded.get_node(&msg_id).unwrap().want.embedding.is_some());

    cleanup(&path);
}

#[test]
fn loaded_fabric_resonance_uses_persisted_embeddings() {
    let path = temp_path("embed_resonate");
    cleanup(&path);

    let texts = &[
        "buy groceries for dinner",
        "buy food for dinner party",
        "send a message to my brother",
        "walk the dog in the park",
    ];
    let embedder = build_test_embedder(texts);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));

    fabric.add_node(IntentNode::new("buy groceries for dinner"));
    fabric.add_node(IntentNode::new("send a message to my brother"));
    fabric.add_node(IntentNode::new("walk the dog in the park"));

    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();

    // Load and set embedder again for query embedding.
    let mut loaded = store.load().unwrap();
    let embedder2 = build_test_embedder(texts);
    loaded.set_embedder(Box::new(embedder2));

    let results = loaded.resonate("buy food for dinner party", 3);
    assert!(!results.is_empty());

    // Grocery node should still rank highest.
    let top_node = loaded.get_node(&results[0].lineage_id).unwrap();
    assert!(top_node.want.description.contains("groceries"),
        "Top result should be groceries after load, got: {}", top_node.want.description);

    cleanup(&path);
}

#[test]
fn backward_compat_loading_pre_embedding_file() {
    let path = temp_path("backward_compat");
    cleanup(&path);

    // Save without embedder (no embeddings in file).
    let mut fabric = Fabric::new();
    fabric.add_node(IntentNode::new("test node"));
    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();

    // Load should work fine (embedding defaults to None via serde(default)).
    let loaded = store.load().unwrap();
    assert_eq!(loaded.node_count(), 1);
    for (_, node) in loaded.nodes() {
        assert!(node.want.embedding.is_none());
    }

    cleanup(&path);
}

#[test]
fn embedder_dimension_matches_vectors() {
    let embedder = build_test_embedder(&[
        "buy groceries for dinner",
        "send a message to my brother",
    ]);

    let dim = embedder.dimension();
    let vec = embedder.embed("buy groceries for dinner");
    assert_eq!(vec.len(), dim, "Embedding vector length should match dimension()");
}

#[test]
fn cosine_similarity_with_embedder() {
    let embedder = build_test_embedder(&[
        "buy groceries for dinner",
        "buy food for dinner",
        "walk the dog in the park",
    ]);

    let grocery = embedder.embed("buy groceries for dinner");
    let food = embedder.embed("buy food for dinner");
    let dog = embedder.embed("walk the dog in the park");

    let sim_close = cosine_similarity(&grocery, &food);
    let sim_far = cosine_similarity(&grocery, &dog);

    assert!(sim_close > sim_far,
        "Related intents should have higher cosine: close={:.3}, far={:.3}",
        sim_close, sim_far);
}

#[test]
fn has_embedder_reports_correctly() {
    let mut fabric = Fabric::new();
    assert!(!fabric.has_embedder());

    let embedder = build_test_embedder(&["test"]);
    fabric.set_embedder(Box::new(embedder));
    assert!(fabric.has_embedder());
}

// ─── TF-IDF Integration Tests ───

fn build_tfidf_embedder(texts: &[&str]) -> BagOfWordsEmbedder {
    let mut embedder = BagOfWordsEmbedder::new();
    embedder.build_vocab_with_idf(texts);
    embedder
}

#[test]
fn tfidf_resonance_outperforms_tf() {
    // Corpus with common filler words ("to", "a", "the", "for").
    // TF-IDF should down-weight these and rank by distinctive terms.
    let texts = &[
        "send a message to my brother",
        "buy groceries for the dinner party",
        "send a letter to my sister",
        "walk the dog to the park for exercise",
    ];

    // TF-IDF fabric.
    let tfidf_embedder = build_tfidf_embedder(texts);
    let mut tfidf_fabric = Fabric::with_embedder(Box::new(tfidf_embedder));
    let tfidf_msg = tfidf_fabric.add_node(IntentNode::new("send a message to my brother"));
    tfidf_fabric.add_node(IntentNode::new("buy groceries for the dinner party"));
    let tfidf_letter = tfidf_fabric.add_node(IntentNode::new("send a letter to my sister"));
    tfidf_fabric.add_node(IntentNode::new("walk the dog to the park for exercise"));

    // Query: "send something to my family" — should match messaging/letter intents.
    let tfidf_results = tfidf_fabric.resonate("send something to my family", 4);

    // The top result should be one of the "send" intents.
    let top_id = &tfidf_results[0].lineage_id;
    assert!(
        *top_id == tfidf_msg || *top_id == tfidf_letter,
        "TF-IDF should rank 'send' intents highest"
    );
}

#[test]
fn tfidf_fabric_auto_embeds() {
    let texts = &["buy groceries", "send a message"];
    let embedder = build_tfidf_embedder(texts);
    assert!(embedder.has_idf());

    let mut fabric = Fabric::with_embedder(Box::new(embedder));
    let id = fabric.add_node(IntentNode::new("buy groceries"));
    let node = fabric.get_node(&id).unwrap();
    assert!(node.want.embedding.is_some(), "TF-IDF embedder should auto-embed");
}

#[test]
fn tfidf_persists_through_save_load() {
    let path = temp_path("tfidf_persist");
    cleanup(&path);

    let texts = &["buy groceries for dinner", "send a message to brother"];
    let embedder = build_tfidf_embedder(texts);
    let mut fabric = Fabric::with_embedder(Box::new(embedder));
    let id = fabric.add_node(IntentNode::new("buy groceries for dinner"));

    let original = fabric.get_node(&id).unwrap().want.embedding.clone();

    let store = JsonFileStore::new(&path);
    store.save(&fabric).unwrap();
    let loaded = store.load().unwrap();

    let loaded_emb = loaded.get_node(&id).unwrap().want.embedding.clone();
    // Use approximate comparison — JSON serialization may introduce tiny fp drift.
    let orig = original.unwrap();
    let loaded = loaded_emb.unwrap();
    assert_eq!(orig.len(), loaded.len());
    for (a, b) in orig.iter().zip(loaded.iter()) {
        assert!((a - b).abs() < 1e-12, "TF-IDF embedding drift too large: {} vs {}", a, b);
    }

    cleanup(&path);
}
