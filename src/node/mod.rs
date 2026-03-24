// INTENT NODE — THE ATOMIC PRIMITIVE
//
// This is the foundational data structure of a new computing paradigm.
// Every design decision here carries weight.
//
// What this IS:
// - A unit of purpose with agency
// - Content-addressable (identity from meaning)
// - Self-describing (carries its own confidence, constraints, context)
// - Part of a fabric (exists in semantic-temporal space)
//
// What this is NOT:
// - Not an object (no methods that mutate hidden state)
// - Not a record (no fixed schema imposed from outside)
// - Not a message (not consumed on read)
// - Not a file (no path, no name, no directory)
//
// Design decisions:
// 1. No timestamp field — time is a dimension (Law 11), handled by fabric
// 2. No owner field — ownership is a context relationship
// 3. No type field — category is emergent from signature, context, behavior
// 4. No address — the node exists in semantic space, found by resonance
// 5. Signature is computed, never assigned (Law 1)
// 6. Activation threshold is NOT a field — it's emergent from the node's
//    position in the fabric, its composite weight, and the system state
//
// What about composite activation weight?
// It's derived from: confidence + context connectivity + temporal recency
// + resonance strength + other weights we haven't discovered yet.
// Phase 1: We can compute a partial composite from what's stored on the node.
// Phase 2: Full composite requires fabric context (recency, resonance, etc.)

use crate::signature::{Signature, Signable, LineageId};
use crate::confidence::ConfidenceSurface;
use crate::constraint::ConstraintField;
use crate::context::ContextField;
use std::collections::HashMap;
use std::fmt;

/// Typed metadata value for arbitrary key-value pairs on nodes.
///
/// Metadata is mutable, NOT part of signature computation.
/// It carries operational data (cost, model, timestamps, etc.)
/// that enriches the node without changing its semantic identity.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    String(String),
    Float(f64),
    Int(i64),
    Bool(bool),
}

impl MetadataValue {
    /// Parse a string into the most specific MetadataValue type.
    /// "true"/"false" → Bool, integers → Int, decimals → Float, else String.
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("true") {
            return MetadataValue::Bool(true);
        }
        if s.eq_ignore_ascii_case("false") {
            return MetadataValue::Bool(false);
        }
        if let Ok(i) = s.parse::<i64>() {
            return MetadataValue::Int(i);
        }
        if let Ok(f) = s.parse::<f64>() {
            return MetadataValue::Float(f);
        }
        MetadataValue::String(s.to_string())
    }

    /// Get the value as f64, if numeric.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            MetadataValue::Float(f) => Some(*f),
            MetadataValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Get the value as a string representation.
    pub fn as_str_repr(&self) -> String {
        match self {
            MetadataValue::String(s) => s.clone(),
            MetadataValue::Float(f) => f.to_string(),
            MetadataValue::Int(i) => i.to_string(),
            MetadataValue::Bool(b) => b.to_string(),
        }
    }
}

impl fmt::Display for MetadataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetadataValue::String(s) => write!(f, "{}", s),
            MetadataValue::Float(v) => write!(f, "{}", v),
            MetadataValue::Int(v) => write!(f, "{}", v),
            MetadataValue::Bool(v) => write!(f, "{}", v),
        }
    }
}

/// The semantic shape of what "satisfied" looks like.
///
/// Phase 1: String placeholder. This is absolutely the shovel.
/// Phase 2: Embedding vector — a region in meaning-space.
/// Phase 3: Native semantic representation that IS the meaning.
///
/// The want does not prescribe HOW. It describes what DONE looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticShape {
    /// Human-readable description of the desired state.
    /// Phase 1 only. Will be replaced by embedding.
    pub description: String,
    /// Embedding vector — a position in meaning-space.
    /// Phase 3b: Bag-of-words TF vector. Future: pre-trained model embeddings.
    /// NOT part of signature computation (it's a derived representation).
    /// Set by the fabric's embedder on add_node/mutate_node.
    pub embedding: Option<Vec<f64>>,
    // Future: pub region_bounds: SemanticRegion,
}

impl SemanticShape {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            embedding: None,
        }
    }
}

impl Signable for SemanticShape {
    fn sig_bytes(&self) -> Vec<u8> {
        self.description.as_bytes().to_vec()
    }
}

impl fmt::Display for SemanticShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Want: {}", self.description)
    }
}

/// The resolution target — a provisional plan filled by the execution manifold.
///
/// Starts empty. Gets populated when the system determines how to
/// achieve the want. Can be revised mid-execution.
///
/// Phase 1: Simple enum of resolution states.
/// Phase 2: Hardware-agnostic execution plan.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolutionTarget {
    /// No resolution plan yet. The default state.
    Unresolved,
    /// System is actively working on resolution.
    InProgress { plan_description: String },
    /// Resolution completed.
    Resolved { outcome_description: String },
    /// Resolution failed.
    Failed { reason: String },
    /// Resolution was abandoned (node deactivated, superseded, etc.)
    Abandoned { reason: String },
}

impl Default for ResolutionTarget {
    fn default() -> Self {
        Self::Unresolved
    }
}

impl Signable for ResolutionTarget {
    fn sig_bytes(&self) -> Vec<u8> {
        match self {
            Self::Unresolved => b"unresolved".to_vec(),
            Self::InProgress { plan_description } => {
                let mut v = b"in_progress:".to_vec();
                v.extend(plan_description.as_bytes());
                v
            }
            Self::Resolved { outcome_description } => {
                let mut v = b"resolved:".to_vec();
                v.extend(outcome_description.as_bytes());
                v
            }
            Self::Failed { reason } => {
                let mut v = b"failed:".to_vec();
                v.extend(reason.as_bytes());
                v
            }
            Self::Abandoned { reason } => {
                let mut v = b"abandoned:".to_vec();
                v.extend(reason.as_bytes());
                v
            }
        }
    }
}

// ═══════════════════════════════════════════════
// THE INTENT NODE
// ═══════════════════════════════════════════════

/// The atomic primitive of intent computing.
///
/// A unit of purpose that exists in semantic-temporal space,
/// possesses agency, and participates in a living fabric.
///
/// ```text
/// IntentNode {
///     signature:         Computed from contents (Law 1)
///     want:              What satisfied looks like
///     constraints:       Boundaries, never instructions (Law 2)
///     confidence:        3D living surface
///     context:           Resonance edges (Law 6)
///     resolution_target: Provisional plan
/// }
/// ```
#[derive(Debug, Clone)]
pub struct IntentNode {
    /// Intrinsic identity, computed from all content fields.
    /// This is recomputed whenever content changes.
    signature: Signature,

    /// Stable identity across mutations — assigned once, never changes.
    /// Even when content changes (and signature changes), lineage_id persists.
    /// NOT included in signature computation.
    lineage_id: LineageId,

    /// Monotonic version counter — increments on every mutation.
    version: u64,

    /// What satisfied looks like — a region in meaning-space.
    pub want: SemanticShape,

    /// Boundaries on resolution — never instructions.
    pub constraints: ConstraintField,

    /// Three-dimensional living surface.
    /// One contributor to composite activation weight.
    pub confidence: ConfidenceSurface,

    /// Semantic edges to other nodes.
    /// Phase 1: Stored on node. Phase 2: Maintained by fabric.
    pub context: ContextField,

    /// Provisional execution plan.
    pub resolution: ResolutionTarget,

    /// Arbitrary typed key-value metadata.
    /// NOT part of signature computation (metadata is mutable operational data).
    pub metadata: HashMap<String, MetadataValue>,
}

impl IntentNode {
    /// Create a new intent node from a want description.
    /// Signature is computed automatically from contents.
    pub fn new(want: impl Into<String>) -> Self {
        let want = SemanticShape::new(want);
        let constraints = ConstraintField::new();
        let confidence = ConfidenceSurface::new();
        let context = ContextField::new();
        let resolution = ResolutionTarget::Unresolved;

        let signature = Self::compute_signature(&want, &constraints, &resolution);

        Self {
            signature,
            lineage_id: LineageId::new(),
            version: 0,
            want,
            constraints,
            confidence,
            context,
            resolution,
            metadata: HashMap::new(),
        }
    }

    /// Create with explicit comprehension confidence.
    /// Use when the intent is clearly expressed.
    pub fn understood(want: impl Into<String>, comprehension: f64) -> Self {
        let mut node = Self::new(want);
        node.confidence = ConfidenceSurface::understood(comprehension);
        node
    }

    /// Get the node's signature (computed identity).
    pub fn signature(&self) -> &Signature {
        &self.signature
    }

    /// Get the node's lineage ID (stable across mutations).
    pub fn lineage_id(&self) -> &LineageId {
        &self.lineage_id
    }

    /// Get the node's version (increments on every mutation).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Set the lineage ID. Used by persistence layer during deserialization.
    /// NOT for general use — lineage IDs should be assigned once at creation.
    pub fn set_lineage_id(&mut self, id: LineageId) {
        self.lineage_id = id;
    }

    /// Set the version counter. Used by persistence layer during deserialization.
    /// NOT for general use — version is normally bumped by recompute_signature().
    pub fn set_version(&mut self, version: u64) {
        self.version = version;
    }

    /// Recompute signature after any content change.
    /// MUST be called after modifying want, constraints, or resolution.
    /// Also increments the version counter.
    ///
    /// Context and confidence do NOT affect signature — they are
    /// observations about the node, not the node's meaning.
    ///
    /// CRITICAL: lineage_id is intentionally NOT modified.
    /// Signature tracks content; lineage_id tracks the entity.
    ///
    /// Design decision: Why exclude confidence and context?
    /// - Confidence changes as the system learns more. The node's MEANING
    ///   hasn't changed just because we understand it better.
    /// - Context changes as the fabric evolves. The node's MEANING
    ///   hasn't changed just because new relationships appeared.
    /// - Want, constraints, and resolution ARE the meaning.
    pub fn recompute_signature(&mut self) {
        self.signature = Self::compute_signature(&self.want, &self.constraints, &self.resolution);
        self.version += 1;
    }

    /// Internal signature computation.
    fn compute_signature(
        want: &SemanticShape,
        constraints: &ConstraintField,
        resolution: &ResolutionTarget,
    ) -> Signature {
        let mut content = Vec::new();

        // Want contributes to identity
        content.extend(b"want:");
        content.extend(want.sig_bytes());

        // Constraints contribute to identity
        content.extend(b"constraints:");
        for c in &constraints.constraints {
            content.extend(c.semantic.as_bytes());
            match &c.kind {
                crate::constraint::ConstraintKind::Hard => {
                    content.extend(b":hard");
                }
                crate::constraint::ConstraintKind::Soft { weight } => {
                    content.extend(b":soft:");
                    content.extend(weight.sig_bytes());
                }
            }
            content.push(b',');
        }

        // Resolution state contributes to identity
        content.extend(b"|resolution:");
        content.extend(resolution.sig_bytes());

        Signature::from_content(&content)
    }

    /// Partial composite activation weight from locally available data.
    /// Phase 1: Only confidence and context contribute.
    /// Phase 2: Fabric adds temporal recency, resonance strength, etc.
    pub fn local_activation_weight(&self) -> f64 {
        let confidence_weight = self.confidence.scalar_summary();
        let context_weight = (self.context.total_weight() / 10.0).min(1.0); // Normalize
        // Simple average for Phase 1. Phase 2 will use learned weighting.
        (confidence_weight + context_weight) / 2.0
    }

    /// Should this node's vagueness be surfaced? (Law 4)
    pub fn needs_clarification(&self) -> bool {
        !self.confidence.should_resolve(0.6)
    }

    /// Is this node resolved?
    pub fn is_resolved(&self) -> bool {
        matches!(self.resolution, ResolutionTarget::Resolved { .. })
    }

    /// Has resolution been attempted?
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.resolution,
            ResolutionTarget::Resolved { .. }
                | ResolutionTarget::Failed { .. }
                | ResolutionTarget::Abandoned { .. }
        )
    }

    /// Verify that the stored signature matches the computed signature.
    /// Used by immune maintenance to detect corruption.
    pub fn verify_signature(&self) -> bool {
        let expected = Self::compute_signature(&self.want, &self.constraints, &self.resolution);
        self.signature == expected
    }
}

impl fmt::Display for IntentNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "═══ Intent Node ═══")?;
        writeln!(f, "  Signature: {:?}", self.signature)?;
        writeln!(f, "  Lineage:   {}", self.lineage_id)?;
        writeln!(f, "  Version:   {}", self.version)?;
        writeln!(f, "  {}", self.want)?;
        writeln!(f, "  {}", self.constraints)?;
        writeln!(f, "  {}", self.confidence)?;
        writeln!(f, "  {}", self.context)?;
        writeln!(f, "  Resolution: {:?}", self.resolution)?;
        if !self.metadata.is_empty() {
            writeln!(f, "  Metadata:")?;
            for (k, v) in &self.metadata {
                writeln!(f, "    {}: {}", k, v)?;
            }
        }
        write!(f, "  Local weight: {:.3}", self.local_activation_weight())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RelationshipKind;

    #[test]
    fn identical_want_produces_identical_signature() {
        let n1 = IntentNode::new("send message to brother");
        let n2 = IntentNode::new("send message to brother");
        assert_eq!(n1.signature(), n2.signature());
    }

    #[test]
    fn different_want_produces_different_signature() {
        let n1 = IntentNode::new("send message to brother");
        let n2 = IntentNode::new("send message to sister");
        assert_ne!(n1.signature(), n2.signature());
    }

    #[test]
    fn adding_constraint_changes_signature() {
        let n1 = IntentNode::new("send message to brother");
        let mut n2 = IntentNode::new("send message to brother");
        n2.constraints.add_hard("must be private");
        n2.recompute_signature();
        assert_ne!(n1.signature(), n2.signature());
    }

    #[test]
    fn confidence_change_does_not_change_signature() {
        let n1 = IntentNode::new("send message");
        let mut n2 = IntentNode::new("send message");
        n2.confidence.comprehension.observe(0.9);
        n2.recompute_signature();
        assert_eq!(n1.signature(), n2.signature(),
            "Confidence is an observation about the node, not its meaning");
    }

    #[test]
    fn context_change_does_not_change_signature() {
        let n1 = IntentNode::new("send message");
        let mut n2 = IntentNode::new("send message");
        n2.context.add_edge(
            LineageId::new(),
            0.5,
            RelationshipKind::RelatedTo,
        );
        n2.recompute_signature();
        assert_eq!(n1.signature(), n2.signature(),
            "Context is a fabric property, not the node's meaning");
    }

    #[test]
    fn resolution_change_changes_signature() {
        let n1 = IntentNode::new("send message");
        let mut n2 = IntentNode::new("send message");
        n2.resolution = ResolutionTarget::Resolved {
            outcome_description: "message sent via Signal".into(),
        };
        n2.recompute_signature();
        assert_ne!(n1.signature(), n2.signature(),
            "Resolution is part of the node's state");
    }

    #[test]
    fn new_node_needs_clarification() {
        let node = IntentNode::new("do something vague");
        assert!(node.needs_clarification(), "New node with unknown confidence should need clarification");
    }

    #[test]
    fn understood_node_can_proceed() {
        let node = IntentNode::understood("send message to brother", 0.9);
        assert!(!node.needs_clarification(), "Well-understood node should not need clarification");
    }

    #[test]
    fn unresolved_node_is_not_terminal() {
        let node = IntentNode::new("test");
        assert!(!node.is_resolved());
        assert!(!node.is_terminal());
    }

    #[test]
    fn display_format_works() {
        let node = IntentNode::understood("send message to brother", 0.85);
        let display = format!("{}", node);
        assert!(display.contains("Intent Node"));
        assert!(display.contains("send message to brother"));
        assert!(display.contains("Lineage:"));
        assert!(display.contains("Version:"));
    }

    #[test]
    fn local_activation_weight_increases_with_confidence_and_context() {
        let mut node = IntentNode::understood("test", 0.9);
        let base_weight = node.local_activation_weight();

        node.context.add_edge(
            LineageId::new(),
            0.8,
            RelationshipKind::RelatedTo,
        );
        let connected_weight = node.local_activation_weight();

        assert!(connected_weight > base_weight,
            "Adding context should increase activation weight");
    }

    // ── Lineage ID tests ──

    #[test]
    fn lineage_id_stable_across_mutations() {
        let mut node = IntentNode::new("send message to brother");
        let original_lineage = node.lineage_id().clone();

        // Mutate content — signature changes, lineage does not
        node.constraints.add_hard("must be private");
        node.recompute_signature();

        assert_eq!(node.lineage_id(), &original_lineage,
            "Lineage ID must be stable across mutations");
    }

    #[test]
    fn lineage_id_differs_for_same_content() {
        let n1 = IntentNode::new("send message to brother");
        let n2 = IntentNode::new("send message to brother");

        assert_eq!(n1.signature(), n2.signature(),
            "Same content → same signature");
        assert_ne!(n1.lineage_id(), n2.lineage_id(),
            "Different nodes → different lineage IDs even with same content");
    }

    #[test]
    fn version_increments_on_recompute() {
        let mut node = IntentNode::new("test");
        assert_eq!(node.version(), 0);

        node.recompute_signature();
        assert_eq!(node.version(), 1);

        node.constraints.add_hard("constraint");
        node.recompute_signature();
        assert_eq!(node.version(), 2);
    }

    #[test]
    fn signature_does_not_include_lineage_id() {
        // Two nodes with same content but different lineage_ids must have same signature
        let n1 = IntentNode::new("identical content");
        let n2 = IntentNode::new("identical content");

        assert_ne!(n1.lineage_id(), n2.lineage_id(),
            "Precondition: different lineage IDs");
        assert_eq!(n1.signature(), n2.signature(),
            "Lineage ID must NOT be included in signature computation");
    }

    // ── Metadata tests ──

    #[test]
    fn metadata_does_not_change_signature() {
        let n1 = IntentNode::new("send message");
        let mut n2 = IntentNode::new("send message");
        n2.metadata.insert("cost".into(), MetadataValue::Float(0.42));
        n2.recompute_signature();
        assert_eq!(n1.signature(), n2.signature(),
            "Metadata is operational data, not the node's meaning");
    }

    #[test]
    fn metadata_value_parse_int() {
        assert_eq!(MetadataValue::parse("42"), MetadataValue::Int(42));
        assert_eq!(MetadataValue::parse("-7"), MetadataValue::Int(-7));
    }

    #[test]
    fn metadata_value_parse_float() {
        assert_eq!(MetadataValue::parse("3.14"), MetadataValue::Float(3.14));
        assert_eq!(MetadataValue::parse("0.001"), MetadataValue::Float(0.001));
    }

    #[test]
    fn metadata_value_parse_bool() {
        assert_eq!(MetadataValue::parse("true"), MetadataValue::Bool(true));
        assert_eq!(MetadataValue::parse("false"), MetadataValue::Bool(false));
        assert_eq!(MetadataValue::parse("TRUE"), MetadataValue::Bool(true));
    }

    #[test]
    fn metadata_value_parse_string() {
        assert_eq!(MetadataValue::parse("hello"), MetadataValue::String("hello".into()));
        assert_eq!(MetadataValue::parse("marketing"), MetadataValue::String("marketing".into()));
    }

    #[test]
    fn new_node_has_empty_metadata() {
        let node = IntentNode::new("test");
        assert!(node.metadata.is_empty());
    }

    #[test]
    fn display_includes_metadata_when_present() {
        let mut node = IntentNode::new("test");
        node.metadata.insert("cost".into(), MetadataValue::Float(0.42));
        let display = format!("{}", node);
        assert!(display.contains("Metadata:"));
        assert!(display.contains("cost"));
        assert!(display.contains("0.42"));
    }

    #[test]
    fn display_omits_metadata_when_empty() {
        let node = IntentNode::new("test");
        let display = format!("{}", node);
        assert!(!display.contains("Metadata:"));
    }

    #[test]
    fn metadata_as_f64() {
        assert_eq!(MetadataValue::Float(3.14).as_f64(), Some(3.14));
        assert_eq!(MetadataValue::Int(42).as_f64(), Some(42.0));
        assert_eq!(MetadataValue::String("hello".into()).as_f64(), None);
        assert_eq!(MetadataValue::Bool(true).as_f64(), None);
    }
}
