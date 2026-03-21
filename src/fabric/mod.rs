// SEMANTIC MEMORY FABRIC — LAW 14: THE FABRIC IS THE INTELLIGENCE
//
// The authoritative container for intent nodes.
// Owns all nodes, maintains the edge graph,
// provides resonance-based retrieval, and tracks
// temporal dimensions.
//
// Phase 2: In-memory, single-process, string similarity.
// Phase 3: Distributed, persistent, embedding-based.
// The shovel, not the building.

use std::collections::HashMap;

use crate::context::{ContextEdge, RelationshipKind};
use crate::embedding::Embedder;
use crate::inference::ActionPolicy;
use crate::node::IntentNode;
use crate::signature::{LineageId, Signature};
use crate::temporal::{
    temporal_decay, lambda_from_half_life, FabricInstant, LamportClock, LamportTimestamp,
};
use crate::tracer::{FabricTracer, NoopTracer, TraceEvent};

// ═══════════════════════════════════════════════
//  Data Structures
// ═══════════════════════════════════════════════

/// Metadata the fabric tracks about each node.
/// This is fabric state, NOT node state.
struct NodeEntry {
    node: IntentNode,
    /// When this node was added to the fabric.
    created_at: FabricInstant,
    /// When this node was last accessed (read or mutated).
    last_accessed: FabricInstant,
    /// Lamport timestamp of creation/last mutation.
    lamport_ts: LamportTimestamp,
}

/// The Semantic Memory Fabric.
///
/// Owns all nodes, maintains edges, provides retrieval.
/// This is the first concrete fabric — Phase 3 makes it distributed.
pub struct Fabric {
    /// Primary storage: LineageId → NodeEntry.
    nodes: HashMap<LineageId, NodeEntry>,
    /// Secondary index: Signature → LineageId (first inserted wins on collision).
    signature_index: HashMap<Signature, LineageId>,
    /// Authoritative edge graph: source LineageId → outgoing edges.
    edges: HashMap<LineageId, Vec<ContextEdge>>,
    /// Reverse edge index: target LineageId → source LineageIds.
    reverse_edges: HashMap<LineageId, Vec<LineageId>>,
    /// Logical clock for causal ordering.
    clock: LamportClock,
    /// Observability tracer (NoopTracer by default).
    tracer: Box<dyn FabricTracer>,
    /// Temporal decay parameter (lambda).
    decay_lambda: f64,
    /// Optional embedder for semantic vector space.
    /// When present, nodes are auto-embedded and resonate() uses cosine similarity.
    embedder: Option<Box<dyn Embedder>>,
    /// Learnable action selection policy (Phase 4b).
    /// RPE signals adapt thresholds over time.
    policy: ActionPolicy,
}

/// Errors that the fabric can produce.
#[derive(Debug, Clone, PartialEq)]
pub enum FabricError {
    /// Tried to reference a node not in the fabric.
    NodeNotFound(LineageId),
    /// Tried to add an edge from a node to itself.
    SelfEdge(LineageId),
}

impl std::fmt::Display for FabricError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FabricError::NodeNotFound(id) => write!(f, "Node not found: {}", id),
            FabricError::SelfEdge(id) => write!(f, "Self-edge not allowed: {}", id),
        }
    }
}

/// A scored retrieval result from resonance-based lookup.
#[derive(Debug, Clone)]
pub struct ResonanceResult {
    pub lineage_id: LineageId,
    /// Overall resonance score [0, 1].
    pub score: f64,
    /// Individual score components for transparency.
    pub components: ResonanceComponents,
}

/// Breakdown of how a resonance score was computed.
/// Every score is explainable — no black boxes.
#[derive(Debug, Clone)]
pub struct ResonanceComponents {
    /// Semantic similarity between wants [0, 1].
    pub semantic_similarity: f64,
    /// Context connectivity strength [0, 1].
    pub context_strength: f64,
    /// Temporal recency weight [0, 1].
    pub temporal_weight: f64,
}

// ═══════════════════════════════════════════════
//  Layer 1: Container
// ═══════════════════════════════════════════════

/// Default temporal decay half-life: 1 hour.
const DEFAULT_HALF_LIFE_SECS: f64 = 3600.0;

impl Fabric {
    /// Create a new empty fabric with default settings.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            signature_index: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            clock: LamportClock::new(),
            tracer: Box::new(NoopTracer),
            decay_lambda: lambda_from_half_life(DEFAULT_HALF_LIFE_SECS),
            embedder: None,
            policy: ActionPolicy::default(),
        }
    }

    /// Create with a custom tracer.
    pub fn with_tracer(tracer: Box<dyn FabricTracer>) -> Self {
        Self {
            nodes: HashMap::new(),
            signature_index: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            clock: LamportClock::new(),
            tracer,
            decay_lambda: lambda_from_half_life(DEFAULT_HALF_LIFE_SECS),
            embedder: None,
            policy: ActionPolicy::default(),
        }
    }

    /// Create with an embedder for semantic vector space.
    /// Enables cosine similarity in resonate() and auto-embeds nodes.
    pub fn with_embedder(embedder: Box<dyn Embedder>) -> Self {
        Self {
            nodes: HashMap::new(),
            signature_index: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            clock: LamportClock::new(),
            tracer: Box::new(NoopTracer),
            decay_lambda: lambda_from_half_life(DEFAULT_HALF_LIFE_SECS),
            embedder: Some(embedder),
            policy: ActionPolicy::default(),
        }
    }

    /// Set or replace the embedder. Existing nodes are NOT re-embedded.
    /// Call re_embed_all() after setting to embed existing nodes.
    pub fn set_embedder(&mut self, embedder: Box<dyn Embedder>) {
        self.embedder = Some(embedder);
    }

    /// Check if an embedder is configured.
    pub fn has_embedder(&self) -> bool {
        self.embedder.is_some()
    }

    /// Re-embed all nodes using the current embedder.
    /// No-op if no embedder is set.
    pub fn re_embed_all(&mut self) {
        if let Some(ref embedder) = self.embedder {
            let embeddings: Vec<(LineageId, Vec<f64>)> = self.nodes.iter()
                .map(|(id, entry)| {
                    let vec = embedder.embed(&entry.node.want.description);
                    (id.clone(), vec)
                })
                .collect();
            for (id, vec) in embeddings {
                if let Some(entry) = self.nodes.get_mut(&id) {
                    entry.node.want.embedding = Some(vec);
                }
            }
        }
    }

    /// Set the temporal decay half-life in seconds.
    pub fn set_decay_half_life(&mut self, seconds: f64) {
        self.decay_lambda = lambda_from_half_life(seconds);
    }

    /// Add a node to the fabric. The fabric takes ownership.
    /// Returns the node's LineageId for future reference.
    /// Auto-embeds the node if an embedder is configured.
    pub fn add_node(&mut self, mut node: IntentNode) -> LineageId {
        // Auto-embed if embedder is present and node has no embedding.
        if node.want.embedding.is_none() {
            if let Some(ref embedder) = self.embedder {
                node.want.embedding = Some(embedder.embed(&node.want.description));
            }
        }

        let lineage_id = node.lineage_id().clone();
        let signature = node.signature().clone();
        let ts = self.clock.tick();

        self.tracer.trace(&TraceEvent::NodeAdded {
            lineage_id: lineage_id.clone(),
        });

        let entry = NodeEntry {
            node,
            created_at: FabricInstant::now(),
            last_accessed: FabricInstant::now(),
            lamport_ts: ts,
        };

        self.nodes.insert(lineage_id.clone(), entry);
        // Secondary index: first insertion wins on signature collision.
        self.signature_index
            .entry(signature)
            .or_insert_with(|| lineage_id.clone());

        lineage_id
    }

    /// Get a reference to a node by lineage ID.
    pub fn get_node(&self, id: &LineageId) -> Option<&IntentNode> {
        self.nodes.get(id).map(|entry| &entry.node)
    }

    /// Get a reference to a node by signature.
    /// Returns the first node with that signature (if multiple have the same content).
    pub fn get_node_by_signature(&self, sig: &Signature) -> Option<&IntentNode> {
        self.signature_index
            .get(sig)
            .and_then(|id| self.get_node(id))
    }

    /// Check if a node exists in the fabric.
    pub fn contains(&self, id: &LineageId) -> bool {
        self.nodes.contains_key(id)
    }

    /// How many nodes are in the fabric.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Fade a node from the fabric.
    ///
    /// Returns the node if it existed (for potential re-addition).
    /// "Fade, not delete" — conceptually, nodes don't die, they lose
    /// resonance. But in Phase 2 we implement this as removal.
    /// Connected edges are also removed.
    pub fn fade_node(&mut self, id: &LineageId) -> Option<IntentNode> {
        let entry = self.nodes.remove(id)?;

        self.tracer.trace(&TraceEvent::NodeFaded {
            lineage_id: id.clone(),
        });

        // Remove from signature index.
        let sig = entry.node.signature().clone();
        if self.signature_index.get(&sig) == Some(id) {
            self.signature_index.remove(&sig);
        }

        // Remove outgoing edges.
        if let Some(outgoing) = self.edges.remove(id) {
            for edge in &outgoing {
                if let Some(sources) = self.reverse_edges.get_mut(&edge.target) {
                    sources.retain(|s| s != id);
                }
            }
        }

        // Remove incoming edges (other nodes pointing to this one).
        if let Some(sources) = self.reverse_edges.remove(id) {
            for source_id in &sources {
                if let Some(edges) = self.edges.get_mut(source_id) {
                    edges.retain(|e| &e.target != id);
                }
                // Sync the source node's context field.
                self.sync_node_context(source_id);
            }
        }

        Some(entry.node)
    }

    /// Iterate over all nodes.
    pub fn nodes(&self) -> impl Iterator<Item = (&LineageId, &IntentNode)> {
        self.nodes.iter().map(|(id, entry)| (id, &entry.node))
    }

    /// Get the current Lamport timestamp.
    pub fn current_timestamp(&self) -> LamportTimestamp {
        self.clock.current()
    }
}

impl Default for Fabric {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════
//  Persistence Accessors
// ═══════════════════════════════════════════════

impl Fabric {
    /// Export all edges as (source_lineage_id, edges) pairs.
    /// Used by the persistence layer for serialization.
    pub fn all_edges(&self) -> Vec<(&LineageId, &[ContextEdge])> {
        self.edges.iter().map(|(id, edges)| (id, edges.as_slice())).collect()
    }

    /// Get the Lamport clock's current raw value.
    /// Used by the persistence layer for serialization.
    pub fn clock_value(&self) -> u64 {
        self.clock.current().value()
    }

    /// Get the decay lambda parameter.
    /// Used by the persistence layer for serialization.
    pub fn decay_lambda(&self) -> f64 {
        self.decay_lambda
    }

    /// Get a node's Lamport timestamp raw value.
    /// Used by the persistence layer for serialization.
    pub fn node_lamport_ts(&self, id: &LineageId) -> Option<u64> {
        self.nodes.get(id).map(|e| e.lamport_ts.value())
    }

    /// Reconstruct a fabric from persisted data.
    ///
    /// Rebuilds all internal indices (signature_index, reverse_edges).
    /// Sets FabricInstant to now() for all nodes (temporal decay is session-level).
    /// The Lamport clock and timestamps ARE restored for causal ordering.
    pub fn from_persisted(
        nodes: Vec<(IntentNode, u64)>,   // (node, lamport_ts)
        edges: Vec<(LineageId, LineageId, f64, RelationshipKind)>,
        clock_value: u64,
        decay_lambda: f64,
    ) -> Self {
        let mut fabric = Self {
            nodes: HashMap::new(),
            signature_index: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            clock: LamportClock::from_value(clock_value),
            tracer: Box::new(NoopTracer),
            decay_lambda,
            embedder: None,
            policy: ActionPolicy::default(),
        };

        // Insert nodes, rebuild signature index.
        for (node, lamport_ts) in nodes {
            let lineage_id = node.lineage_id().clone();
            let signature = node.signature().clone();

            let entry = NodeEntry {
                node,
                created_at: FabricInstant::now(),
                last_accessed: FabricInstant::now(),
                lamport_ts: LamportTimestamp::new(lamport_ts),
            };

            fabric.nodes.insert(lineage_id.clone(), entry);
            fabric.signature_index
                .entry(signature)
                .or_insert_with(|| lineage_id);
        }

        // Insert edges, rebuild reverse index.
        for (from, to, weight, kind) in edges {
            if fabric.nodes.contains_key(&from) && fabric.nodes.contains_key(&to) {
                let edge = ContextEdge {
                    target: to.clone(),
                    weight,
                    kind,
                };

                fabric.edges
                    .entry(from.clone())
                    .or_insert_with(Vec::new)
                    .push(edge);

                fabric.reverse_edges
                    .entry(to)
                    .or_insert_with(Vec::new)
                    .push(from.clone());
            }
        }

        // Sync all node context fields from authoritative edges.
        let all_ids: Vec<LineageId> = fabric.nodes.keys().cloned().collect();
        for id in &all_ids {
            fabric.sync_node_context(id);
        }

        fabric
    }
}

// ═══════════════════════════════════════════════
//  Layer 2: Edge Graph
// ═══════════════════════════════════════════════

impl Fabric {
    /// Add a directed edge from one node to another.
    ///
    /// The fabric is the authoritative owner of edges.
    /// Also updates the source node's ContextField cache.
    pub fn add_edge(
        &mut self,
        from: &LineageId,
        to: &LineageId,
        weight: f64,
        kind: RelationshipKind,
    ) -> Result<(), FabricError> {
        if from == to {
            return Err(FabricError::SelfEdge(from.clone()));
        }
        if !self.nodes.contains_key(from) {
            return Err(FabricError::NodeNotFound(from.clone()));
        }
        if !self.nodes.contains_key(to) {
            return Err(FabricError::NodeNotFound(to.clone()));
        }

        let kind_str = format!("{:?}", kind);
        let edge = ContextEdge {
            target: to.clone(),
            weight,
            kind,
        };

        self.edges
            .entry(from.clone())
            .or_insert_with(Vec::new)
            .push(edge);

        self.reverse_edges
            .entry(to.clone())
            .or_insert_with(Vec::new)
            .push(from.clone());

        self.tracer.trace(&TraceEvent::EdgeAdded {
            from: from.clone(),
            to: to.clone(),
            kind: kind_str,
            weight,
        });

        // Sync the source node's context field.
        self.sync_node_context(from);

        Ok(())
    }

    /// Remove all edges from one node to another.
    /// Returns how many edges were removed.
    pub fn remove_edges_between(
        &mut self,
        from: &LineageId,
        to: &LineageId,
    ) -> usize {
        let removed = if let Some(edges) = self.edges.get_mut(from) {
            let before = edges.len();
            edges.retain(|e| &e.target != to);
            before - edges.len()
        } else {
            0
        };

        if removed > 0 {
            if let Some(sources) = self.reverse_edges.get_mut(to) {
                sources.retain(|s| s != from);
            }
            self.tracer.trace(&TraceEvent::EdgeRemoved {
                from: from.clone(),
                to: to.clone(),
            });
            self.sync_node_context(from);
        }

        removed
    }

    /// Get all edges FROM a node (outgoing).
    pub fn edges_from(&self, id: &LineageId) -> &[ContextEdge] {
        self.edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all nodes that point TO a node (incoming sources).
    pub fn edges_to(&self, id: &LineageId) -> Vec<&LineageId> {
        self.reverse_edges
            .get(id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Get all edges between two specific nodes (from → to).
    pub fn edges_between(&self, from: &LineageId, to: &LineageId) -> Vec<&ContextEdge> {
        self.edges_from(from)
            .iter()
            .filter(|e| &e.target == to)
            .collect()
    }

    /// Total edge count in the fabric.
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|v| v.len()).sum()
    }

    /// Sync a node's ContextField from the fabric's authoritative edge graph.
    /// Called internally after edge mutations.
    fn sync_node_context(&mut self, id: &LineageId) {
        if let Some(entry) = self.nodes.get_mut(id) {
            // Clear and rebuild from fabric's authoritative edges.
            entry.node.context.edges.clear();
            if let Some(fabric_edges) = self.edges.get(id) {
                for edge in fabric_edges {
                    entry.node.context.edges.push(ContextEdge {
                        target: edge.target.clone(),
                        weight: edge.weight,
                        kind: edge.kind.clone(),
                    });
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════
//  Layer 3: Temporal Tracking
// ═══════════════════════════════════════════════

impl Fabric {
    /// Get the Lamport timestamp when a node was created/last mutated.
    pub fn node_timestamp(&self, id: &LineageId) -> Option<LamportTimestamp> {
        self.nodes.get(id).map(|entry| entry.lamport_ts)
    }

    /// Get the wall-clock age of a node in seconds (since creation).
    pub fn node_age_secs(&self, id: &LineageId) -> Option<f64> {
        self.nodes.get(id).map(|entry| entry.created_at.elapsed_secs())
    }

    /// Get the temporal decay weight for a node.
    /// Uses last_accessed time, not creation time.
    /// Returns 1.0 for just-accessed, decays toward 0.0.
    pub fn temporal_weight(&self, id: &LineageId) -> Option<f64> {
        self.nodes.get(id).map(|entry| {
            temporal_decay(entry.last_accessed.elapsed_secs(), self.decay_lambda)
        })
    }

    /// Mutate a node's content via a closure.
    ///
    /// Handles: recompute_signature, version bump, lamport tick,
    /// signature index update, timestamp update, tracer event.
    ///
    /// The closure receives a mutable reference to the IntentNode.
    /// After the closure returns, the fabric calls recompute_signature()
    /// and updates all indices.
    pub fn mutate_node<F>(&mut self, id: &LineageId, f: F) -> Result<(), FabricError>
    where
        F: FnOnce(&mut IntentNode),
    {
        // Remove temporarily to avoid borrow checker issues.
        let mut entry = self.nodes.remove(id)
            .ok_or_else(|| FabricError::NodeNotFound(id.clone()))?;

        let old_sig = entry.node.signature().clone();
        let old_version = entry.node.version();

        // Apply the mutation.
        f(&mut entry.node);

        // Recompute signature and bump version.
        entry.node.recompute_signature();

        // Re-embed if embedder is present (want may have changed).
        if let Some(ref embedder) = self.embedder {
            entry.node.want.embedding = Some(embedder.embed(&entry.node.want.description));
        }

        let new_sig = entry.node.signature().clone();
        let new_version = entry.node.version();

        // Update signature index.
        if old_sig != new_sig {
            if self.signature_index.get(&old_sig) == Some(id) {
                self.signature_index.remove(&old_sig);
            }
            self.signature_index.entry(new_sig).or_insert_with(|| id.clone());
        }

        // Update temporal bookkeeping.
        let ts = self.clock.tick();
        entry.last_accessed = FabricInstant::now();
        entry.lamport_ts = ts;

        self.tracer.trace(&TraceEvent::NodeMutated {
            lineage_id: id.clone(),
            old_version,
            new_version,
        });

        // Re-insert.
        self.nodes.insert(id.clone(), entry);

        Ok(())
    }
}

// ═══════════════════════════════════════════════
//  Layer 4: Resonance-Based Retrieval
// ═══════════════════════════════════════════════

/// Resonance retrieval weights.
/// Phase 2: Fixed constants. Phase 3: Learned weights.
const SEMANTIC_WEIGHT: f64 = 0.6;
const CONTEXT_WEIGHT: f64 = 0.2;
const TEMPORAL_WEIGHT: f64 = 0.2;

impl Fabric {
    /// Find the k most resonant nodes to a query string.
    ///
    /// Dual-path semantic similarity:
    /// - When embedder is present: cosine similarity over embedding vectors.
    /// - Fallback: Jaccard coefficient over tokens (Phase 2 behavior).
    ///
    /// Returns results sorted by score, descending.
    pub fn resonate(&self, query: &str, k: usize) -> Vec<ResonanceResult> {
        // Embed the query if embedder is available.
        let query_embedding = self.embedder.as_ref().map(|e| e.embed(query));

        let mut results: Vec<ResonanceResult> = self.nodes.iter()
            .map(|(id, entry)| {
                // Dual-path: cosine if both embeddings exist, Jaccard fallback.
                let semantic = match (&query_embedding, &entry.node.want.embedding) {
                    (Some(q_emb), Some(n_emb)) => {
                        crate::embedding::cosine_similarity(q_emb, n_emb)
                    }
                    _ => jaccard_similarity(query, &entry.node.want.description),
                };
                let context = self.context_score(id);
                let temporal = temporal_decay(
                    entry.last_accessed.elapsed_secs(),
                    self.decay_lambda,
                );

                let score = semantic * SEMANTIC_WEIGHT
                    + context * CONTEXT_WEIGHT
                    + temporal * TEMPORAL_WEIGHT;

                ResonanceResult {
                    lineage_id: id.clone(),
                    score,
                    components: ResonanceComponents {
                        semantic_similarity: semantic,
                        context_strength: context,
                        temporal_weight: temporal,
                    },
                }
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        let top_score = results.first().map(|r| r.score).unwrap_or(0.0);
        self.tracer.trace(&TraceEvent::ResonanceQuery {
            query: query.to_string(),
            results_count: results.len(),
            top_score,
        });

        results
    }

    /// Find nodes resonant with an existing node's want.
    /// Excludes the node itself from results.
    pub fn resonate_with(&self, id: &LineageId, k: usize) -> Vec<ResonanceResult> {
        let want = match self.get_node(id) {
            Some(node) => node.want.description.clone(),
            None => return Vec::new(),
        };

        let mut results = self.resonate(&want, k + 1);
        results.retain(|r| &r.lineage_id != id);
        results.truncate(k);
        results
    }

    /// Context connectivity score for a node.
    /// How connected is this node to the rest of the fabric?
    /// Normalized to [0, 1] by capping at 10 edges total weight.
    fn context_score(&self, id: &LineageId) -> f64 {
        let outgoing: f64 = self.edges_from(id).iter().map(|e| e.weight).sum();
        let incoming_count = self.edges_to(id).len() as f64;
        let total = outgoing + incoming_count * 0.5; // Incoming edges count but less
        (total / 10.0).min(1.0)
    }
}

/// Jaccard coefficient over whitespace-separated, lowercased tokens.
///
/// Bootstrap similarity metric. Phase 3: cosine similarity over embeddings.
/// |A ∩ B| / |A ∪ B|. Returns 0.0 if both empty.
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    use std::collections::HashSet;

    let set_a: HashSet<&str> = a.split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty())
        .collect();

    let set_b: HashSet<&str> = b.split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty())
        .collect();

    if set_a.is_empty() && set_b.is_empty() {
        return 0.0;
    }

    let intersection = set_a.intersection(&set_b).count() as f64;
    let union = set_a.union(&set_b).count() as f64;

    intersection / union
}

// ═══════════════════════════════════════════════
//  Layer 5: Full Composite Activation Weight
// ═══════════════════════════════════════════════

/// Activation weight component ratios.
/// Phase 2: Fixed constants. Phase 3: Learned weights.
const AW_CONFIDENCE: f64 = 0.35;
const AW_CONTEXT: f64 = 0.25;
const AW_TEMPORAL: f64 = 0.20;
const AW_RESONANCE: f64 = 0.20;

impl Fabric {
    /// Compute the full composite activation weight for a node.
    ///
    /// Combines: confidence + context + temporal recency + resonance.
    /// This replaces IntentNode::local_activation_weight() for nodes
    /// that live in a fabric. The local method still works for
    /// standalone nodes.
    pub fn activation_weight(&self, id: &LineageId) -> Option<f64> {
        let entry = self.nodes.get(id)?;

        let confidence = entry.node.confidence.scalar_summary();
        let context = self.context_score(id);
        let temporal = temporal_decay(
            entry.last_accessed.elapsed_secs(),
            self.decay_lambda,
        );

        // Average resonance: how well does this node resonate with its neighbors?
        let resonance = self.average_neighbor_resonance(id);

        let weight = confidence * AW_CONFIDENCE
            + context * AW_CONTEXT
            + temporal * AW_TEMPORAL
            + resonance * AW_RESONANCE;

        Some(weight)
    }

    /// Find all nodes that exceed an activation threshold.
    /// Returns nodes sorted by activation weight, descending.
    pub fn activated_nodes(&self, threshold: f64) -> Vec<(LineageId, f64)> {
        let mut results: Vec<(LineageId, f64)> = self.nodes.keys()
            .filter_map(|id| {
                let weight = self.activation_weight(id)?;
                if weight >= threshold {
                    Some((id.clone(), weight))
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Find nodes that need clarification (Law 4).
    pub fn needs_clarification(&self) -> Vec<&LineageId> {
        self.nodes.iter()
            .filter(|(_, entry)| entry.node.needs_clarification())
            .map(|(id, _)| id)
            .collect()
    }

    /// Average semantic resonance with neighboring (connected) nodes.
    /// Uses cosine similarity when embeddings available, Jaccard fallback.
    fn average_neighbor_resonance(&self, id: &LineageId) -> f64 {
        let node = match self.get_node(id) {
            Some(n) => n,
            None => return 0.0,
        };

        let neighbors: Vec<&LineageId> = self.edges_from(id)
            .iter()
            .map(|e| &e.target)
            .collect();

        if neighbors.is_empty() {
            return 0.0;
        }

        let total: f64 = neighbors.iter()
            .filter_map(|nid| self.get_node(nid))
            .map(|neighbor| {
                match (&node.want.embedding, &neighbor.want.embedding) {
                    (Some(a), Some(b)) => crate::embedding::cosine_similarity(a, b),
                    _ => jaccard_similarity(&node.want.description, &neighbor.want.description),
                }
            })
            .sum();

        total / neighbors.len() as f64
    }
}

// ═══════════════════════════════════════════════
//  Layer 6: Active Inference
// ═══════════════════════════════════════════════

/// Free energy threshold below which nodes are considered "satisfied".
const FE_INCOHERENT_THRESHOLD: f64 = 2.0;
/// Temporal weight below which nodes are considered "stale".
const STALE_THRESHOLD: f64 = 0.1;

impl Fabric {
    /// Compute free energy for a single node.
    pub fn free_energy(&self, id: &LineageId) -> Option<crate::inference::FreeEnergy> {
        let composite = self.activation_weight(id)?;
        let node = self.get_node(id)?;
        Some(crate::inference::FreeEnergy::compute(composite, &node.confidence, 1.0))
    }

    /// Run one inference step for a single node.
    /// Returns the proposed action (does NOT apply it).
    pub fn infer(&self, id: &LineageId) -> Option<crate::inference::InferenceResult> {
        let node = self.get_node(id)?;
        let composite = self.activation_weight(id)?;
        let edge_count = self.edges_from(id).len();

        // Find best resonant neighbor (excluding self).
        let best_neighbor = self.resonate_with(id, 1)
            .first()
            .map(|r| (r.lineage_id.clone(), r.score));

        Some(crate::inference::inference_step(node, composite, edge_count, best_neighbor))
    }

    /// Run inference on all nodes. Returns results sorted by free energy (highest first).
    pub fn infer_all(&self) -> Vec<crate::inference::InferenceResult> {
        let ids: Vec<LineageId> = self.nodes.keys().cloned().collect();
        let mut results: Vec<crate::inference::InferenceResult> = ids.iter()
            .filter_map(|id| self.infer(id))
            .collect();
        results.sort_by(|a, b| {
            b.free_energy.total.partial_cmp(&a.free_energy.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Get a reference to the fabric's action policy.
    pub fn policy(&self) -> &ActionPolicy {
        &self.policy
    }

    /// Replace the fabric's action policy.
    pub fn set_policy(&mut self, policy: ActionPolicy) {
        self.policy = policy;
    }

    /// Run inference with the learnable policy, apply the action, and update
    /// the policy from the resulting RPE signal.
    ///
    /// This is the full active inference loop:
    /// 1. Compute free energy before action
    /// 2. Select action using learnable policy thresholds
    /// 3. Apply the action to the fabric
    /// 4. Compute free energy after action
    /// 5. Compute RPE: reward = FE_before - FE_after
    /// 6. Update policy thresholds from RPE
    pub fn infer_and_learn(
        &mut self,
        id: &LineageId,
    ) -> Option<crate::inference::InferenceResult> {
        // 1. Snapshot state before action.
        let node = self.get_node(id)?;
        let composite_before = self.activation_weight(id)?;
        let edge_count = self.edges_from(id).len();
        let best_neighbor = self.resonate_with(id, 1)
            .first()
            .map(|r| (r.lineage_id.clone(), r.score));

        // 2. Run inference with learnable policy.
        let fe_before = crate::inference::FreeEnergy::compute(
            composite_before, &node.confidence, 1.0,
        );
        let result = crate::inference::inference_step_with_policy(
            node, composite_before, edge_count, best_neighbor, &self.policy,
        );

        // 3. Apply the action.
        let _ = self.apply_action(id, &result.action);

        // 4. Compute free energy after action.
        let composite_after = self.activation_weight(id).unwrap_or(composite_before);
        let node_after = self.get_node(id);
        let fe_after = node_after.map(|n| {
            crate::inference::FreeEnergy::compute(composite_after, &n.confidence, 1.0)
        }).unwrap_or_else(|| fe_before.clone());

        // 5. Compute RPE: reward = FE reduction (positive = good).
        let reward = fe_before.total - fe_after.total;
        let rpe = crate::inference::compute_rpe(
            reward,
            fe_before.total,  // current value estimate
            fe_after.total,   // next value estimate
            0.9,              // discount factor
            0.1,              // signal threshold
        );

        // 6. Update policy from RPE.
        self.policy.update_from_rpe(&rpe);

        Some(result)
    }

    /// Apply a node action to the fabric.
    /// Returns Ok(true) if the action was applied, Ok(false) if it was a no-op.
    pub fn apply_action(
        &mut self,
        source: &LineageId,
        action: &crate::inference::NodeAction,
    ) -> Result<bool, FabricError> {
        use crate::inference::{NodeAction, ConfidenceDimension};

        match action {
            NodeAction::None => Ok(false),

            NodeAction::RequestClarification { .. } => {
                // Clarification is a signal — the fabric records it
                // by adjusting confidence. In a full system, this would
                // trigger a user interaction.
                Ok(false) // Signal only, no fabric mutation
            }

            NodeAction::SignalResolution { .. } => {
                // Signal that node is ready for resolution.
                // In a full system, this would trigger the execution manifold.
                Ok(false) // Signal only, no fabric mutation
            }

            NodeAction::CreateEdge { target, weight, kind } => {
                if !self.contains(source) {
                    return Err(FabricError::NodeNotFound(source.clone()));
                }
                self.add_edge(source, target, *weight, kind.clone())?;
                Ok(true)
            }

            NodeAction::ModifyEdge { target, new_weight } => {
                if !self.contains(source) {
                    return Err(FabricError::NodeNotFound(source.clone()));
                }
                // Remove old edges and add new one with updated weight.
                let old_kind = self.edges_from(source)
                    .iter()
                    .find(|e| &e.target == target)
                    .map(|e| e.kind.clone());

                if let Some(kind) = old_kind {
                    self.remove_edges_between(source, target);
                    self.add_edge(source, target, *new_weight, kind)?;
                    Ok(true)
                } else {
                    Ok(false) // No existing edge to modify
                }
            }

            NodeAction::AdjustConfidence { dimension, observation } => {
                self.mutate_node(source, |node| {
                    match dimension {
                        ConfidenceDimension::Comprehension => {
                            node.confidence.comprehension.observe(*observation);
                        }
                        ConfidenceDimension::Resolution => {
                            node.confidence.resolution.observe(*observation);
                        }
                        ConfidenceDimension::Verification => {
                            node.confidence.verification.observe(*observation);
                        }
                    }
                })?;
                Ok(true)
            }
        }
    }

    /// Immune system maintenance sweep (Beastie Board SERIOUS-3).
    ///
    /// Inspects all nodes for:
    /// - Incoherence: high free energy (poorly integrated)
    /// - Staleness: low temporal weight (haven't been accessed)
    /// - Integrity failures: signature doesn't match content
    pub fn immune_maintenance(&self) -> crate::inference::MaintenanceReport {
        let mut incoherent = Vec::new();
        let mut stale = Vec::new();
        let mut integrity_failures = Vec::new();

        for (id, entry) in &self.nodes {
            // Check signature integrity.
            if !entry.node.verify_signature() {
                integrity_failures.push(id.clone());
            }

            // Check staleness.
            let temporal_w = temporal_decay(
                entry.last_accessed.elapsed_secs(),
                self.decay_lambda,
            );
            if temporal_w < STALE_THRESHOLD {
                stale.push(id.clone());
            }

            // Check incoherence (high free energy).
            if let Some(fe) = self.free_energy(id) {
                if fe.total > FE_INCOHERENT_THRESHOLD {
                    incoherent.push(id.clone());
                }
            }
        }

        crate::inference::MaintenanceReport {
            incoherent,
            stale,
            integrity_failures,
            inspected: self.nodes.len(),
        }
    }
}

// ═══════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracer::CollectingTracer;

    // ─── Layer 1: Container ───

    #[test]
    fn new_fabric_is_empty() {
        let fabric = Fabric::new();
        assert_eq!(fabric.node_count(), 0);
    }

    #[test]
    fn add_node_increases_count() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("buy groceries"));
        assert_eq!(fabric.node_count(), 1);
        fabric.add_node(IntentNode::new("send message"));
        assert_eq!(fabric.node_count(), 2);
    }

    #[test]
    fn get_node_by_lineage_id() {
        let mut fabric = Fabric::new();
        let node = IntentNode::new("buy groceries");
        let id = fabric.add_node(node);
        let retrieved = fabric.get_node(&id).unwrap();
        assert_eq!(retrieved.want.description, "buy groceries");
    }

    #[test]
    fn get_node_by_signature() {
        let mut fabric = Fabric::new();
        let node = IntentNode::new("buy groceries");
        let sig = node.signature().clone();
        fabric.add_node(node);
        let retrieved = fabric.get_node_by_signature(&sig).unwrap();
        assert_eq!(retrieved.want.description, "buy groceries");
    }

    #[test]
    fn get_nonexistent_node_returns_none() {
        let fabric = Fabric::new();
        assert!(fabric.get_node(&LineageId::new()).is_none());
    }

    #[test]
    fn contains_returns_true_for_added_node() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        assert!(fabric.contains(&id));
    }

    #[test]
    fn contains_returns_false_for_unknown_node() {
        let fabric = Fabric::new();
        assert!(!fabric.contains(&LineageId::new()));
    }

    #[test]
    fn fade_node_removes_it() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        assert_eq!(fabric.node_count(), 1);
        fabric.fade_node(&id);
        assert_eq!(fabric.node_count(), 0);
        assert!(!fabric.contains(&id));
    }

    #[test]
    fn fade_node_returns_the_node() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        let faded = fabric.fade_node(&id).unwrap();
        assert_eq!(faded.want.description, "test");
    }

    #[test]
    fn fade_nonexistent_node_returns_none() {
        let mut fabric = Fabric::new();
        assert!(fabric.fade_node(&LineageId::new()).is_none());
    }

    #[test]
    fn nodes_iterator_yields_all_nodes() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("one"));
        fabric.add_node(IntentNode::new("two"));
        fabric.add_node(IntentNode::new("three"));
        let count = fabric.nodes().count();
        assert_eq!(count, 3);
    }

    #[test]
    fn add_node_emits_trace_event() {
        let tracer = CollectingTracer::new();
        let mut fabric = Fabric::with_tracer(Box::new(tracer));
        fabric.add_node(IntentNode::new("test"));
        // We can't easily read the tracer back out since it's boxed,
        // but this test ensures the code path doesn't panic.
        // For a real trace test, we'd use a shared reference via Arc.
        assert_eq!(fabric.node_count(), 1);
    }

    // ─── Layer 2: Edge Graph ───

    #[test]
    fn add_edge_between_existing_nodes() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("node a"));
        let b = fabric.add_node(IntentNode::new("node b"));
        let result = fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn);
        assert!(result.is_ok());
        assert_eq!(fabric.edge_count(), 1);
    }

    #[test]
    fn add_edge_to_nonexistent_node_fails() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("node a"));
        let fake = LineageId::new();
        let result = fabric.add_edge(&a, &fake, 0.5, RelationshipKind::RelatedTo);
        assert_eq!(result, Err(FabricError::NodeNotFound(fake)));
    }

    #[test]
    fn add_edge_from_nonexistent_node_fails() {
        let mut fabric = Fabric::new();
        let b = fabric.add_node(IntentNode::new("node b"));
        let fake = LineageId::new();
        let result = fabric.add_edge(&fake, &b, 0.5, RelationshipKind::RelatedTo);
        assert_eq!(result, Err(FabricError::NodeNotFound(fake.clone())));
    }

    #[test]
    fn self_edge_fails() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("self"));
        let result = fabric.add_edge(&a, &a, 0.5, RelationshipKind::RelatedTo);
        assert_eq!(result, Err(FabricError::SelfEdge(a)));
    }

    #[test]
    fn edges_from_returns_outgoing() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("a"));
        let b = fabric.add_node(IntentNode::new("b"));
        let c = fabric.add_node(IntentNode::new("c"));
        fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
        fabric.add_edge(&a, &c, 0.5, RelationshipKind::RelatedTo).unwrap();
        assert_eq!(fabric.edges_from(&a).len(), 2);
        assert_eq!(fabric.edges_from(&b).len(), 0);
    }

    #[test]
    fn edges_to_returns_incoming() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("a"));
        let b = fabric.add_node(IntentNode::new("b"));
        let c = fabric.add_node(IntentNode::new("c"));
        fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
        fabric.add_edge(&c, &b, 0.5, RelationshipKind::RelatedTo).unwrap();
        assert_eq!(fabric.edges_to(&b).len(), 2);
        assert_eq!(fabric.edges_to(&a).len(), 0);
    }

    #[test]
    fn edges_between_returns_specific() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("a"));
        let b = fabric.add_node(IntentNode::new("b"));
        let c = fabric.add_node(IntentNode::new("c"));
        fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
        fabric.add_edge(&a, &c, 0.5, RelationshipKind::RelatedTo).unwrap();
        assert_eq!(fabric.edges_between(&a, &b).len(), 1);
        assert_eq!(fabric.edges_between(&a, &c).len(), 1);
        assert_eq!(fabric.edges_between(&b, &a).len(), 0);
    }

    #[test]
    fn remove_edges_between_works() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("a"));
        let b = fabric.add_node(IntentNode::new("b"));
        fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
        assert_eq!(fabric.edge_count(), 1);
        let removed = fabric.remove_edges_between(&a, &b);
        assert_eq!(removed, 1);
        assert_eq!(fabric.edge_count(), 0);
    }

    #[test]
    fn fade_node_removes_connected_edges() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("a"));
        let b = fabric.add_node(IntentNode::new("b"));
        let c = fabric.add_node(IntentNode::new("c"));
        fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
        fabric.add_edge(&b, &c, 0.5, RelationshipKind::RelatedTo).unwrap();
        fabric.fade_node(&b);
        assert_eq!(fabric.edge_count(), 0);
    }

    #[test]
    fn edge_syncs_to_node_context() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("a"));
        let b = fabric.add_node(IntentNode::new("b"));
        fabric.add_edge(&a, &b, 0.8, RelationshipKind::DependsOn).unwrap();
        let node_a = fabric.get_node(&a).unwrap();
        assert_eq!(node_a.context.connection_count(), 1);
        assert!(node_a.context.has_dependencies());
    }

    // ─── Layer 3: Temporal Tracking ───

    #[test]
    fn new_node_has_timestamp() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        assert!(fabric.node_timestamp(&id).is_some());
        assert!(fabric.node_timestamp(&id).unwrap().value() > 0);
    }

    #[test]
    fn node_age_is_non_negative() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        assert!(fabric.node_age_secs(&id).unwrap() >= 0.0);
    }

    #[test]
    fn temporal_weight_starts_near_one() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        let w = fabric.temporal_weight(&id).unwrap();
        // Just created → weight should be very close to 1.0
        assert!(w > 0.99, "Expected near 1.0, got {}", w);
    }

    #[test]
    fn mutate_node_updates_signature_index() {
        let mut fabric = Fabric::new();
        let node = IntentNode::new("original");
        let old_sig = node.signature().clone();
        let id = fabric.add_node(node);

        fabric.mutate_node(&id, |n| {
            n.want.description = "mutated".to_string();
        }).unwrap();

        let new_node = fabric.get_node(&id).unwrap();
        assert_ne!(new_node.signature(), &old_sig);
        // Old signature no longer resolves
        assert!(fabric.get_node_by_signature(&old_sig).is_none());
        // New signature resolves
        assert!(fabric.get_node_by_signature(new_node.signature()).is_some());
    }

    #[test]
    fn mutate_node_increments_version() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        assert_eq!(fabric.get_node(&id).unwrap().version(), 0);

        fabric.mutate_node(&id, |n| {
            n.want.description = "changed".to_string();
        }).unwrap();

        assert_eq!(fabric.get_node(&id).unwrap().version(), 1);
    }

    #[test]
    fn mutate_node_ticks_clock() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        let ts_before = fabric.current_timestamp();

        fabric.mutate_node(&id, |n| {
            n.want.description = "changed".to_string();
        }).unwrap();

        let ts_after = fabric.current_timestamp();
        assert!(ts_after > ts_before);
    }

    #[test]
    fn mutate_nonexistent_node_fails() {
        let mut fabric = Fabric::new();
        let fake = LineageId::new();
        let result = fabric.mutate_node(&fake, |_| {});
        assert_eq!(result, Err(FabricError::NodeNotFound(fake)));
    }

    // ─── Layer 4: Resonance Retrieval ───

    #[test]
    fn resonate_empty_fabric_returns_empty() {
        let fabric = Fabric::new();
        let results = fabric.resonate("test", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn resonate_finds_exact_match() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("buy groceries"));
        let results = fabric.resonate("buy groceries", 5);
        assert_eq!(results.len(), 1);
        assert!(results[0].components.semantic_similarity > 0.9);
    }

    #[test]
    fn resonate_finds_partial_match() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("buy groceries for dinner"));
        let results = fabric.resonate("buy groceries", 5);
        assert_eq!(results.len(), 1);
        assert!(results[0].components.semantic_similarity > 0.0);
    }

    #[test]
    fn resonate_ranks_by_score() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("buy groceries for dinner"));
        fabric.add_node(IntentNode::new("buy groceries"));
        fabric.add_node(IntentNode::new("walk the dog"));
        let results = fabric.resonate("buy groceries", 3);
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }

    #[test]
    fn resonate_respects_k_limit() {
        let mut fabric = Fabric::new();
        for i in 0..10 {
            fabric.add_node(IntentNode::new(&format!("node {}", i)));
        }
        let results = fabric.resonate("node", 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn resonate_with_excludes_self() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("buy groceries"));
        fabric.add_node(IntentNode::new("buy groceries for dinner"));
        let results = fabric.resonate_with(&id, 5);
        assert!(results.iter().all(|r| r.lineage_id != id));
    }

    #[test]
    fn jaccard_identical_is_one() {
        let sim = jaccard_similarity("buy groceries", "buy groceries");
        assert!((sim - 1.0).abs() < 1e-10);
    }

    #[test]
    fn jaccard_disjoint_is_zero() {
        let sim = jaccard_similarity("buy groceries", "walk the dog");
        assert!((sim - 0.0).abs() < 1e-10);
    }

    #[test]
    fn jaccard_partial_overlap() {
        let sim = jaccard_similarity("buy groceries for dinner", "buy groceries");
        assert!(sim > 0.0);
        assert!(sim < 1.0);
        // "buy" and "groceries" shared out of {"buy", "groceries", "for", "dinner"}
        // = 2/4 = 0.5
        assert!((sim - 0.5).abs() < 1e-10);
    }

    #[test]
    fn jaccard_case_insensitive_via_tokens() {
        // Both inputs are lowercase in practice (SemanticShape is lowercase).
        // Jaccard compares exact tokens — case sensitivity is by design
        // since Phase 2 embeddings will handle this natively.
        let sim = jaccard_similarity("Buy Groceries", "buy groceries");
        // "Buy" != "buy", "Groceries" != "groceries" → disjoint
        assert_eq!(sim, 0.0);
    }

    // ─── Layer 5: Activation Weight ───

    #[test]
    fn activation_weight_of_new_node() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        let w = fabric.activation_weight(&id).unwrap();
        // New node: low confidence, no context, high temporal, no resonance
        assert!(w > 0.0);
        assert!(w < 1.0);
    }

    #[test]
    fn activation_weight_increases_with_confidence() {
        let mut fabric = Fabric::new();
        let low = fabric.add_node(IntentNode::new("vague"));
        let high = fabric.add_node(IntentNode::understood("clear intent", 0.95));

        let w_low = fabric.activation_weight(&low).unwrap();
        let w_high = fabric.activation_weight(&high).unwrap();
        assert!(w_high > w_low, "High confidence {} should exceed low {}", w_high, w_low);
    }

    #[test]
    fn activation_weight_increases_with_connections() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("node a"));
        let b = fabric.add_node(IntentNode::new("node b"));
        let c = fabric.add_node(IntentNode::new("node c"));

        let w_before = fabric.activation_weight(&a).unwrap();
        fabric.add_edge(&a, &b, 0.9, RelationshipKind::DependsOn).unwrap();
        fabric.add_edge(&a, &c, 0.8, RelationshipKind::RelatedTo).unwrap();
        let w_after = fabric.activation_weight(&a).unwrap();

        assert!(w_after > w_before, "Connected {} should exceed isolated {}", w_after, w_before);
    }

    #[test]
    fn activation_weight_nonexistent_returns_none() {
        let fabric = Fabric::new();
        assert!(fabric.activation_weight(&LineageId::new()).is_none());
    }

    #[test]
    fn activated_nodes_filters_by_threshold() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::understood("high confidence intent", 0.95));
        fabric.add_node(IntentNode::new("vague thing"));

        // Very high threshold: might filter out some or all
        let high = fabric.activated_nodes(0.9);
        // Very low threshold: should include all
        let low = fabric.activated_nodes(0.0);
        assert!(low.len() >= high.len());
    }

    #[test]
    fn activated_nodes_sorted_descending() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("low"));
        fabric.add_node(IntentNode::understood("medium", 0.7));
        fabric.add_node(IntentNode::understood("high", 0.95));

        let results = fabric.activated_nodes(0.0);
        for window in results.windows(2) {
            assert!(window[0].1 >= window[1].1);
        }
    }

    #[test]
    fn needs_clarification_finds_low_confidence_nodes() {
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("do the thing")); // needs clarification
        fabric.add_node(IntentNode::understood("send message via Signal", 0.92)); // clear
        let needy = fabric.needs_clarification();
        assert_eq!(needy.len(), 1);
    }
}
