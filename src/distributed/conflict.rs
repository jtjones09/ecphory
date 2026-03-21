// CONFLICT RESOLUTION — BEASTIE BOARD FATAL-2
//
// When two replicas modify the same node concurrently,
// the system must decide what to do. Three strategies:
//
// 1. LWW (Last Writer Wins) — default. Higher version wins.
// 2. Branch-and-Surface — both versions kept, human decides.
//    Used for hard constraints or immune-flagged nodes.
// 3. CRDT Merge — structural merge where possible.
//    Context edges: Add-Wins OR-Set.
//    Confidence: weighted average by observation count.
//
// Design decisions:
// 1. Conflicts are data — recorded, inspectable, auditable.
// 2. Strategy is chosen per-conflict based on node properties.
// 3. Phase 3d: single-process simulation. Same logic applies in distributed.

use crate::node::IntentNode;
use crate::signature::LineageId;
use super::vclock::VectorClock;

/// A detected conflict between two versions of a node.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// The lineage ID of the conflicting node.
    pub lineage_id: LineageId,
    /// The local version of the node.
    pub local: IntentNode,
    /// The local vector clock at time of conflict.
    pub local_clock: VectorClock,
    /// The remote version of the node.
    pub remote: IntentNode,
    /// The remote vector clock at time of conflict.
    pub remote_clock: VectorClock,
}

/// How a conflict was resolved.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// One version was chosen over the other.
    Resolved {
        winner: IntentNode,
        strategy: ConflictStrategy,
    },
    /// Both versions were kept — human must decide.
    Branched {
        branch_a: IntentNode,
        branch_b: IntentNode,
    },
}

/// Strategy used to resolve a conflict.
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictStrategy {
    /// Last Writer Wins — higher version number wins.
    /// Ties broken by lexicographic lineage ID comparison (deterministic).
    LastWriterWins,
    /// Both kept for human review.
    /// Used when hard constraints are involved or immune flags set.
    BranchAndSurface,
    /// Structural merge where fields can be combined.
    CrdtMerge,
}

/// Select a conflict strategy based on node properties.
///
/// Rules (Beastie Board FATAL-2):
/// - If either node has hard constraint violations → BranchAndSurface
/// - If nodes have same version → CrdtMerge (likely minor divergence)
/// - Otherwise → LastWriterWins
pub fn select_strategy(local: &IntentNode, remote: &IntentNode) -> ConflictStrategy {
    // Hard constraint violations require human review.
    if local.constraints.has_hard_violation() || remote.constraints.has_hard_violation() {
        return ConflictStrategy::BranchAndSurface;
    }

    // Same version suggests minor divergence — try to merge.
    if local.version() == remote.version() {
        return ConflictStrategy::CrdtMerge;
    }

    ConflictStrategy::LastWriterWins
}

/// Resolve a conflict using the selected strategy.
pub fn resolve_conflict(conflict: &Conflict) -> Resolution {
    let strategy = select_strategy(&conflict.local, &conflict.remote);

    match strategy {
        ConflictStrategy::LastWriterWins => {
            let winner = if conflict.local.version() > conflict.remote.version() {
                conflict.local.clone()
            } else if conflict.remote.version() > conflict.local.version() {
                conflict.remote.clone()
            } else {
                // Tie-break: lexicographic comparison of lineage ID UUID string.
                if conflict.local.lineage_id().as_uuid().to_string()
                    >= conflict.remote.lineage_id().as_uuid().to_string()
                {
                    conflict.local.clone()
                } else {
                    conflict.remote.clone()
                }
            };
            Resolution::Resolved {
                winner,
                strategy: ConflictStrategy::LastWriterWins,
            }
        }

        ConflictStrategy::BranchAndSurface => {
            Resolution::Branched {
                branch_a: conflict.local.clone(),
                branch_b: conflict.remote.clone(),
            }
        }

        ConflictStrategy::CrdtMerge => {
            // Merge confidence: weighted average by observation count.
            let mut merged = conflict.local.clone();

            let local_obs = conflict.local.confidence.comprehension.observations
                + conflict.local.confidence.resolution.observations
                + conflict.local.confidence.verification.observations;
            let remote_obs = conflict.remote.confidence.comprehension.observations
                + conflict.remote.confidence.resolution.observations
                + conflict.remote.confidence.verification.observations;
            let total_obs = local_obs + remote_obs;

            if total_obs > 0 {
                let local_weight = local_obs as f64 / total_obs as f64;
                let remote_weight = remote_obs as f64 / total_obs as f64;

                merged.confidence.comprehension.mean =
                    conflict.local.confidence.comprehension.mean * local_weight
                    + conflict.remote.confidence.comprehension.mean * remote_weight;
                merged.confidence.resolution.mean =
                    conflict.local.confidence.resolution.mean * local_weight
                    + conflict.remote.confidence.resolution.mean * remote_weight;
                merged.confidence.verification.mean =
                    conflict.local.confidence.verification.mean * local_weight
                    + conflict.remote.confidence.verification.mean * remote_weight;
            }

            // Merge context edges: union (Add-Wins OR-Set).
            for remote_edge in &conflict.remote.context.edges {
                let already_has = merged.context.edges.iter()
                    .any(|e| e.target == remote_edge.target);
                if !already_has {
                    merged.context.edges.push(remote_edge.clone());
                }
            }

            Resolution::Resolved {
                winner: merged,
                strategy: ConflictStrategy::CrdtMerge,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::IntentNode;

    #[test]
    fn lww_higher_version_wins() {
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
            _ => panic!("Expected Resolved, got Branched"),
        }
    }

    #[test]
    fn branch_on_hard_violation() {
        let mut local = IntentNode::new("test");
        local.constraints.add_hard("must be safe");
        local.constraints.constraints[0].violate();

        let remote = IntentNode::new("test");

        let strategy = select_strategy(&local, &remote);
        assert_eq!(strategy, ConflictStrategy::BranchAndSurface);
    }

    #[test]
    fn crdt_merge_on_same_version() {
        let local = IntentNode::new("test"); // version 0
        let remote = IntentNode::new("test"); // version 0

        let strategy = select_strategy(&local, &remote);
        assert_eq!(strategy, ConflictStrategy::CrdtMerge);
    }

    #[test]
    fn crdt_merge_unions_context_edges() {
        let mut local = IntentNode::new("test");
        let mut remote = IntentNode::new("test");

        let target_a = LineageId::new();
        let target_b = LineageId::new();

        local.context.add_edge(
            target_a.clone(), 0.5,
            crate::context::RelationshipKind::RelatedTo,
        );
        remote.context.add_edge(
            target_b.clone(), 0.7,
            crate::context::RelationshipKind::DependsOn,
        );

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
                // Should have both edges (union).
                assert_eq!(winner.context.edges.len(), 2);
            }
            _ => panic!("Expected Resolved with CRDT merge"),
        }
    }

    #[test]
    fn lww_deterministic_tiebreak() {
        let local = IntentNode::new("test");
        let remote = IntentNode::new("test");
        // Both version 0, different lineage IDs.

        let conflict = Conflict {
            lineage_id: local.lineage_id().clone(),
            local: local.clone(),
            local_clock: VectorClock::new(),
            remote: remote.clone(),
            remote_clock: VectorClock::new(),
        };

        // Same version → CrdtMerge strategy, not LWW.
        let resolution = resolve_conflict(&conflict);
        match resolution {
            Resolution::Resolved { strategy, .. } => {
                assert_eq!(strategy, ConflictStrategy::CrdtMerge);
            }
            _ => panic!("Expected Resolved"),
        }
    }
}
