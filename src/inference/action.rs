// ACTION SELECTION — NODE AGENCY
//
// Each node can take actions to reduce its free energy.
// Actions are proposals — the fabric decides whether to apply them.
//
// Design decisions:
// 1. Greedy selection (highest priority action wins).
//    Phase 4b: select_action_with_policy() uses learnable thresholds.
// 2. Actions are data, not closures — serializable, inspectable, reversible.
// 3. The fabric validates actions before applying (e.g., target exists).
// 4. Action selection is pure — depends only on node state + fabric context.

use crate::confidence::ConfidenceSurface;
use crate::context::RelationshipKind;
use crate::node::ResolutionTarget;
use crate::signature::LineageId;

/// An action a node proposes to reduce its free energy.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeAction {
    /// No action needed — free energy is already low.
    None,

    /// Request clarification from the user (Law 4: Vagueness is Surfaced).
    /// Triggered when comprehension is too low to proceed.
    RequestClarification {
        reason: String,
    },

    /// Signal that this node is ready for resolution.
    /// Triggered when comprehension is high but node is still unresolved.
    SignalResolution {
        reason: String,
    },

    /// Create an edge to a resonant neighbor.
    /// Triggered when the node has low context connectivity.
    CreateEdge {
        target: LineageId,
        weight: f64,
        kind: RelationshipKind,
    },

    /// Modify an existing edge weight.
    /// Triggered to strengthen/weaken connections based on resonance.
    ModifyEdge {
        target: LineageId,
        new_weight: f64,
    },

    /// Adjust a confidence dimension based on observed evidence.
    AdjustConfidence {
        dimension: ConfidenceDimension,
        observation: f64,
    },
}

/// Which confidence dimension to adjust.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfidenceDimension {
    Comprehension,
    Resolution,
    Verification,
}

/// Action selection thresholds (fixed defaults for backward compatibility).
const LOW_COMPREHENSION: f64 = 0.4;
const HIGH_COMPREHENSION: f64 = 0.7;
const HIGH_RESOLUTION_CONF: f64 = 0.7;
const LOW_FREE_ENERGY: f64 = 0.1;
const MIN_CONTEXT_EDGES: usize = 2;

/// Select the best action for a node given its state.
///
/// Uses fixed default thresholds. For learnable thresholds,
/// use `select_action_with_policy()`.
pub fn select_action(
    free_energy: f64,
    confidence: &ConfidenceSurface,
    resolution: &ResolutionTarget,
    context_edge_count: usize,
    best_neighbor: Option<(LineageId, f64)>,
) -> NodeAction {
    select_action_with_thresholds(
        free_energy, confidence, resolution, context_edge_count, best_neighbor,
        LOW_FREE_ENERGY, LOW_COMPREHENSION, HIGH_COMPREHENSION, HIGH_RESOLUTION_CONF, MIN_CONTEXT_EDGES,
    )
}

/// Select the best action using a learnable ActionPolicy.
///
/// Same greedy priority as `select_action()`, but thresholds
/// come from the policy instead of fixed constants.
pub fn select_action_with_policy(
    free_energy: f64,
    confidence: &ConfidenceSurface,
    resolution: &ResolutionTarget,
    context_edge_count: usize,
    best_neighbor: Option<(LineageId, f64)>,
    policy: &super::policy::ActionPolicy,
) -> NodeAction {
    select_action_with_thresholds(
        free_energy, confidence, resolution, context_edge_count, best_neighbor,
        policy.fe_threshold, policy.low_comprehension, policy.high_comprehension,
        policy.high_resolution, policy.min_context_edges,
    )
}

/// Internal action selection with explicit thresholds.
///
/// Greedy priority:
/// 1. FE < threshold → None (already good)
/// 2. Low comprehension → RequestClarification
/// 3. High comprehension + unresolved + resolution confidence → SignalResolution
/// 4. Low context → CreateEdge to best neighbor (if provided)
/// 5. Otherwise → None
fn select_action_with_thresholds(
    free_energy: f64,
    confidence: &ConfidenceSurface,
    resolution: &ResolutionTarget,
    context_edge_count: usize,
    best_neighbor: Option<(LineageId, f64)>,
    fe_threshold: f64,
    low_comprehension: f64,
    high_comprehension: f64,
    high_resolution: f64,
    min_context_edges: usize,
) -> NodeAction {
    // 1. Already satisfied.
    if free_energy < fe_threshold {
        return NodeAction::None;
    }

    // 2. Can't understand — ask for help.
    if confidence.comprehension.mean < low_comprehension {
        return NodeAction::RequestClarification {
            reason: format!(
                "Comprehension too low ({:.2}) to proceed",
                confidence.comprehension.mean
            ),
        };
    }

    // 3. Understands and ready — signal resolution.
    if confidence.comprehension.mean >= high_comprehension
        && confidence.resolution.mean >= high_resolution
        && matches!(resolution, ResolutionTarget::Unresolved)
    {
        return NodeAction::SignalResolution {
            reason: format!(
                "Ready to resolve (comprehension={:.2}, resolution_conf={:.2})",
                confidence.comprehension.mean,
                confidence.resolution.mean,
            ),
        };
    }

    // 4. Needs more context — connect to neighbors.
    if context_edge_count < min_context_edges {
        if let Some((target, weight)) = best_neighbor {
            return NodeAction::CreateEdge {
                target,
                weight: weight.min(1.0),
                kind: RelationshipKind::RelatedTo,
            };
        }
    }

    // 5. Nothing obvious to do.
    NodeAction::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::ConfidenceSurface;

    #[test]
    fn low_free_energy_selects_none() {
        let cs = ConfidenceSurface::understood(0.9);
        let action = select_action(0.05, &cs, &ResolutionTarget::Unresolved, 3, None);
        assert_eq!(action, NodeAction::None);
    }

    #[test]
    fn low_comprehension_requests_clarification() {
        let cs = ConfidenceSurface::new(); // comprehension = 0.5
        // But we need comprehension < 0.4, so create custom
        let mut cs_low = ConfidenceSurface::new();
        cs_low.comprehension.mean = 0.3;
        let action = select_action(1.0, &cs_low, &ResolutionTarget::Unresolved, 0, None);
        match action {
            NodeAction::RequestClarification { .. } => {}
            _ => panic!("Expected RequestClarification, got {:?}", action),
        }
    }

    #[test]
    fn high_confidence_unresolved_signals_resolution() {
        let mut cs = ConfidenceSurface::understood(0.9);
        cs.resolution = crate::confidence::Distribution::new(0.8, 0.05);
        let action = select_action(1.0, &cs, &ResolutionTarget::Unresolved, 3, None);
        match action {
            NodeAction::SignalResolution { .. } => {}
            _ => panic!("Expected SignalResolution, got {:?}", action),
        }
    }

    #[test]
    fn already_resolved_does_not_signal_again() {
        let mut cs = ConfidenceSurface::understood(0.9);
        cs.resolution = crate::confidence::Distribution::new(0.8, 0.05);
        let resolved = ResolutionTarget::Resolved {
            outcome_description: "done".to_string(),
        };
        let action = select_action(1.0, &cs, &resolved, 3, None);
        // Should NOT be SignalResolution since already resolved.
        assert!(
            !matches!(action, NodeAction::SignalResolution { .. }),
            "Already resolved node should not signal again: {:?}", action
        );
    }

    #[test]
    fn low_context_creates_edge() {
        let cs = ConfidenceSurface::understood(0.6);
        let neighbor = LineageId::new();
        let action = select_action(
            1.0,
            &cs,
            &ResolutionTarget::Unresolved,
            0,
            Some((neighbor.clone(), 0.7)),
        );
        match action {
            NodeAction::CreateEdge { target, .. } => {
                assert_eq!(target, neighbor);
            }
            _ => panic!("Expected CreateEdge, got {:?}", action),
        }
    }

    #[test]
    fn no_neighbor_available_returns_none() {
        let cs = ConfidenceSurface::understood(0.6);
        let action = select_action(1.0, &cs, &ResolutionTarget::Unresolved, 0, None);
        assert_eq!(action, NodeAction::None);
    }
}
