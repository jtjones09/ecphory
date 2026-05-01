// EDIT MODE — Three-way write classification (Spec 8 §3.2)
//
// Per spec §3.2, every node belongs to one of three edit categories.
// In v1 (per Jeremy's call), EditMode is recorded per-node by the Phase F
// bridge as a per-call argument on `create()`, NOT as a NodeKind on
// IntentNode. Concrete callers (property-mgmt, nisaba-on-fabric) will
// drive a node-type system when they arrive.

/// How a node may be edited after creation (Spec 8 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditMode {
    /// **Append-commutative.** New node creation; the node's content
    /// never changes after creation. Supersession is via edges (e.g.,
    /// `RelationshipKind::Refines`), not via mutation. No locks held.
    /// Default for journal entries, observations, immutable extractions.
    AppendOnly,

    /// **Mechanical.** Counter, timestamp, weight, last-observed-at:
    /// the *value* matters less than the fact that *some* agent has
    /// updated it. Per-node lock with a 500ms target. First writer
    /// wins; the loser receives `WriteError::NodeLocked` and retries.
    Mechanical,

    /// **Semantic.** Changing the content of a PINNED entry, revising a
    /// decision, amending a spec paragraph. The specific value matters,
    /// and disagreement deserves to be resolved on its merits rather
    /// than by winning a race. Edits go through the `checkout` →
    /// `propose` → `finalize_proposal` → `ConsensusSnapshot` state
    /// machine (Spec 8 §3.4).
    Semantic,
}

impl EditMode {
    /// Human-readable name for error messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            EditMode::AppendOnly => "AppendOnly",
            EditMode::Mechanical => "Mechanical",
            EditMode::Semantic => "Semantic",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_mode_strings_are_distinct() {
        let modes = [EditMode::AppendOnly, EditMode::Mechanical, EditMode::Semantic];
        let strs: Vec<&str> = modes.iter().map(|m| m.as_str()).collect();
        let unique: std::collections::HashSet<&str> = strs.iter().copied().collect();
        assert_eq!(unique.len(), 3);
    }
}
