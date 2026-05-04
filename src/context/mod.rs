// CONTEXT — RESONANCE, NOT REFERENCE (Law 6)
//
// Design decisions:
// 1. Context is a set of semantic edges to other nodes.
// 2. Each edge has weight (strength) and quality (relationship type).
// 3. Context grows WITHOUT modifying the node — because context is
//    a property of the fabric's state, not the node's internal state.
//    For Phase 1, we store it on the node. Phase 2 moves it to the fabric.
// 4. No foreign keys. No pointers. Edges reference other nodes by Signature.
//    If the target node's meaning changes (signature changes), the edge
//    doesn't break — it becomes a historical reference. The fabric
//    handles the rebinding.
// 5. Relationship quality is semantic, not categorical. Phase 1 uses
//    an enum as placeholder. Phase 2 replaces with embedding vectors.
//
// Open questions:
// - Should context edges be bidirectional?
//   Decision: No. Edges are directional. "A depends on B" doesn't mean
//   "B depends on A." Bidirectional relationships are two separate edges.
// - How does context contribute to composite activation weight?
//   More connected nodes have more weight. The count, total weight,
//   and diversity of connections all factor in.

use crate::signature::LineageId;
use std::fmt;

/// How two nodes are related.
/// Phase 1: Categorical placeholder.
/// Phase 2: Embedding vector capturing nuanced relationship semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum RelationshipKind {
    /// This node was derived from the target
    DerivedFrom,
    /// This node depends on the target for resolution
    DependsOn,
    /// This node contradicts or conflicts with the target
    ConflictsWith,
    /// This node was inspired by or related to the target
    RelatedTo,
    /// Temporal: this must happen before the target
    Precedes,
    /// Temporal: this must happen after the target
    Follows,
    /// This node refines or clarifies the target
    Refines,
    /// Security: this node constrains the target
    Constrains,
    /// Spec 7 §3.2: this message belongs to the target conversation thread
    Thread,
    /// Spec 9 §3.1: this node is evidence that fulfills a WorkIntent.
    /// Cross-region by design — evidence lives where the work happens
    /// (nisaba, propmgmt, etc.), the WorkIntent lives in `hotash:work`.
    Fulfills,
    /// Spec 9 §2.3 K.S2: this child WorkIntent was produced by
    /// splitting a parent intent. Edge points from child → parent.
    /// Parent's visibility status becomes `Split` once any child
    /// links to it.
    SplitFrom,
    /// Spec 9 §2.3 K.S2: two WorkIntents whose evidence converged on
    /// the same problem. Edge is mutual provenance — both originals
    /// are preserved; neither is primary. Wired in pairs (a → b and
    /// b → a) by the merge helper.
    ConvergedWith,
    /// Custom semantic relationship (Phase 1 escape hatch)
    Custom(String),
}

/// A single edge in the context field.
/// Represents a semantic relationship to another node.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextEdge {
    /// The target node, identified by lineage ID (stable across mutations).
    pub target: LineageId,

    /// How strongly related (0.0 = barely, 1.0 = inseparable).
    pub weight: f64,

    /// What kind of relationship.
    pub kind: RelationshipKind,
}

impl ContextEdge {
    pub fn new(target: LineageId, weight: f64, kind: RelationshipKind) -> Self {
        assert!((0.0..=1.0).contains(&weight), "Edge weight must be in [0, 1]");
        Self { target, weight, kind }
    }
}

/// The complete context field for an intent node.
/// A web of semantic edges to other nodes in the fabric.
///
/// This is the node's view of its relationships.
/// In Phase 2, the fabric maintains the authoritative relationship graph
/// and the node's context field is a cached projection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextField {
    pub edges: Vec<ContextEdge>,
}

impl ContextField {
    pub fn new() -> Self {
        Self { edges: Vec::new() }
    }

    pub fn add_edge(&mut self, target: LineageId, weight: f64, kind: RelationshipKind) {
        self.edges.push(ContextEdge::new(target, weight, kind));
    }

    /// Total connectivity weight — how connected this node is.
    /// Contributes to composite activation weight.
    pub fn total_weight(&self) -> f64 {
        self.edges.iter().map(|e| e.weight).sum()
    }

    /// Number of connections.
    pub fn connection_count(&self) -> usize {
        self.edges.len()
    }

    /// Find all edges to a specific target.
    pub fn edges_to(&self, target: &LineageId) -> Vec<&ContextEdge> {
        self.edges.iter().filter(|e| &e.target == target).collect()
    }

    /// Find all edges of a specific relationship kind.
    pub fn edges_of_kind(&self, kind: &RelationshipKind) -> Vec<&ContextEdge> {
        self.edges.iter().filter(|e| &e.kind == kind).collect()
    }

    /// Does this node have any dependencies that must resolve first?
    pub fn has_dependencies(&self) -> bool {
        self.edges.iter().any(|e| matches!(e.kind, RelationshipKind::DependsOn))
    }

    /// Get all nodes this depends on.
    pub fn dependencies(&self) -> Vec<&LineageId> {
        self.edges
            .iter()
            .filter(|e| matches!(e.kind, RelationshipKind::DependsOn))
            .map(|e| &e.target)
            .collect()
    }
}

impl fmt::Display for ContextField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Context [{} edges, weight={:.2}]", self.edges.len(), self.total_weight())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_context_has_no_weight() {
        let ctx = ContextField::new();
        assert_eq!(ctx.total_weight(), 0.0);
        assert_eq!(ctx.connection_count(), 0);
    }

    #[test]
    fn edges_accumulate_weight() {
        let mut ctx = ContextField::new();
        let target = LineageId::new();
        ctx.add_edge(target, 0.5, RelationshipKind::RelatedTo);
        ctx.add_edge(LineageId::new(), 0.3, RelationshipKind::DependsOn);
        assert!((ctx.total_weight() - 0.8).abs() < f64::EPSILON);
        assert_eq!(ctx.connection_count(), 2);
    }

    #[test]
    fn dependencies_are_findable() {
        let mut ctx = ContextField::new();
        let dep = LineageId::new();
        ctx.add_edge(dep.clone(), 0.9, RelationshipKind::DependsOn);
        ctx.add_edge(LineageId::new(), 0.5, RelationshipKind::RelatedTo);
        assert!(ctx.has_dependencies());
        assert_eq!(ctx.dependencies().len(), 1);
        assert_eq!(ctx.dependencies()[0], &dep);
    }

    #[test]
    fn edges_to_specific_target() {
        let mut ctx = ContextField::new();
        let target = LineageId::new();
        ctx.add_edge(target.clone(), 0.5, RelationshipKind::RelatedTo);
        ctx.add_edge(target.clone(), 0.3, RelationshipKind::DerivedFrom);
        ctx.add_edge(LineageId::new(), 0.7, RelationshipKind::RelatedTo);
        assert_eq!(ctx.edges_to(&target).len(), 2);
    }
}
