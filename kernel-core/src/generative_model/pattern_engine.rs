//! Pattern completion engine — when the causal graph has no known
//! cause for a surprising effect, look for STRUCTURALLY SIMILAR effects
//! elsewhere in the graph and propose their causes as hypotheses.
//!
//! This is the "Thing 3" of the position paper: hypothesis generation
//! by analogy, NOT by language modelling. It is graph-structure
//! matching — cheap on a 200-node graph, conceptually powerful because
//! it lets the agent reason about failure modes it has never seen
//! before.
//!
//! Step 2 lays down the API and a placeholder implementation. Step 6
//! turns the simple metric into the full structural-similarity
//! algorithm described in `nisaba/positions/nucleation-architecture.md`.

use alloc::string::String;
use alloc::vec::Vec;

use super::causal_graph::{CausalCandidate, CausalGraph};

#[derive(Clone, Debug)]
pub struct Hypothesis {
    pub proposed_cause: String,
    pub confidence: f32,
    pub source_analogy: String,
}

#[derive(Default)]
pub struct PatternEngine {
    pub hypotheses_generated: u64,
}

impl PatternEngine {
    pub fn new() -> Self {
        Self {
            hypotheses_generated: 0,
        }
    }

    /// Generate hypotheses for the unexplained effect by looking for
    /// effects in the graph that share a region or share a label-prefix
    /// (the simplest structural-similarity heuristic). Step 6 swaps
    /// this for the full implementation. The current behavior: if the
    /// effect's region has any other effects with known causes, propose
    /// those causes (with confidence damped by a similarity penalty).
    pub fn hypothesize(&mut self, graph: &CausalGraph, effect_label: &str) -> Vec<Hypothesis> {
        let target = match graph.node_by_label(effect_label) {
            Some(n) => n,
            None => return Vec::new(),
        };

        // Find similar effects: same region, similar labels.
        let mut hypotheses: Vec<Hypothesis> = Vec::new();
        for n in &graph.nodes {
            if n.id == target.id {
                continue;
            }
            if n.region != target.region {
                continue;
            }
            let sim = label_similarity(&target.label, &n.label);
            if sim < 0.30 {
                continue;
            }
            // For each candidate similar effect, fetch its known causes
            // from the graph and propose them as hypotheses for the
            // unexplained effect.
            let causes: Vec<CausalCandidate> = graph.backward_trace(n.id, 3);
            for c in causes {
                hypotheses.push(Hypothesis {
                    proposed_cause: c.cause_label,
                    confidence: c.strength * sim,
                    source_analogy: alloc::format!("{} → {}", n.label, target.label),
                });
            }
        }

        // De-duplicate by proposed_cause, keeping max confidence.
        hypotheses.sort_by(|a, b| a.proposed_cause.cmp(&b.proposed_cause));
        hypotheses.dedup_by(|a, b| {
            if a.proposed_cause == b.proposed_cause {
                if a.confidence > b.confidence {
                    b.confidence = a.confidence;
                    b.source_analogy = a.source_analogy.clone();
                }
                true
            } else {
                false
            }
        });
        hypotheses.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        hypotheses.truncate(5);

        if !hypotheses.is_empty() {
            self.hypotheses_generated = self.hypotheses_generated.saturating_add(1);
        }
        hypotheses
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.hypotheses_generated.to_le_bytes());
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        if *off + 8 > bytes.len() {
            return None;
        }
        let v = u64::from_le_bytes(bytes[*off..*off + 8].try_into().ok()?);
        *off += 8;
        Some(Self {
            hypotheses_generated: v,
        })
    }
}

/// Simple label similarity: ratio of shared whitespace-split tokens
/// over total unique tokens. Cheap and good enough for the
/// "checksum_mismatch ≈ data_mismatch" comparison the position paper
/// describes.
fn label_similarity(a: &str, b: &str) -> f32 {
    let a_tokens: Vec<&str> = a.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
    let b_tokens: Vec<&str> = b.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let shared: usize = a_tokens
        .iter()
        .filter(|t| b_tokens.contains(t))
        .count();
    let total = a_tokens.len() + b_tokens.len() - shared;
    if total == 0 {
        return 0.0;
    }
    shared as f32 / total as f32
}
