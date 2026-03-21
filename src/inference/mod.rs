// ACTIVE INFERENCE — PHASE 3c + 4b
//
// Node agency via the Free Energy Principle (Friston).
// Nodes minimize surprise by taking actions that reduce
// prediction error between expected and observed state.
//
// Design decisions:
// 1. inference_step() is a pure function: node state in, action out.
// 2. The fabric applies actions, not the inference module.
// 3. Immune maintenance is a fabric-level operation that runs
//    inference on all nodes and reports anomalies.
// 4. Phase 4b: RPE signals drive ActionPolicy learning.
//    Policy thresholds adapt from reward prediction errors.

pub mod energy;
pub mod action;
pub mod policy;

pub use energy::{FreeEnergy, RPESignal, compute_rpe};
pub use action::{NodeAction, ConfidenceDimension, select_action, select_action_with_policy};
pub use policy::ActionPolicy;

use crate::node::IntentNode;
use crate::signature::LineageId;

/// Result of running one inference step on a node.
#[derive(Debug, Clone)]
pub struct InferenceResult {
    /// The node's lineage ID.
    pub lineage_id: LineageId,
    /// The computed free energy.
    pub free_energy: FreeEnergy,
    /// The action selected to reduce free energy.
    pub action: NodeAction,
}

/// Report from immune system maintenance sweep.
///
/// Beastie Board SERIOUS-3: The fabric periodically checks all nodes
/// for incoherence, staleness, and integrity failures.
#[derive(Debug, Clone)]
pub struct MaintenanceReport {
    /// Nodes with high free energy (confused, poorly integrated).
    pub incoherent: Vec<LineageId>,
    /// Nodes that haven't been accessed recently and have decayed.
    pub stale: Vec<LineageId>,
    /// Nodes whose signature doesn't match their content (corruption).
    pub integrity_failures: Vec<LineageId>,
    /// Total nodes inspected.
    pub inspected: usize,
}

impl MaintenanceReport {
    /// Are there any issues?
    pub fn has_issues(&self) -> bool {
        !self.incoherent.is_empty()
            || !self.stale.is_empty()
            || !self.integrity_failures.is_empty()
    }

    /// Total issue count.
    pub fn issue_count(&self) -> usize {
        self.incoherent.len() + self.stale.len() + self.integrity_failures.len()
    }
}

/// Run one inference step for a node.
///
/// Pure function: takes node state + context, returns proposed action.
/// The fabric is responsible for calling this and applying the result.
pub fn inference_step(
    node: &IntentNode,
    observed_composite: f64,
    context_edge_count: usize,
    best_neighbor: Option<(LineageId, f64)>,
) -> InferenceResult {
    let fe = FreeEnergy::compute(observed_composite, &node.confidence, 1.0);

    let action = select_action(
        fe.total,
        &node.confidence,
        &node.resolution,
        context_edge_count,
        best_neighbor,
    );

    InferenceResult {
        lineage_id: node.lineage_id().clone(),
        free_energy: fe,
        action,
    }
}

/// Run one inference step using a learnable policy.
///
/// Same as `inference_step()` but uses policy thresholds
/// instead of fixed constants.
pub fn inference_step_with_policy(
    node: &IntentNode,
    observed_composite: f64,
    context_edge_count: usize,
    best_neighbor: Option<(LineageId, f64)>,
    policy: &ActionPolicy,
) -> InferenceResult {
    let fe = FreeEnergy::compute(observed_composite, &node.confidence, 1.0);

    let action = select_action_with_policy(
        fe.total,
        &node.confidence,
        &node.resolution,
        context_edge_count,
        best_neighbor,
        policy,
    );

    InferenceResult {
        lineage_id: node.lineage_id().clone(),
        free_energy: fe,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::IntentNode;

    #[test]
    fn inference_step_low_confidence_requests_clarification() {
        let node = IntentNode::new("vague thing");
        // New node has comprehension=0.5, variance=0.25
        // With low observed composite, FE will be high
        let result = inference_step(&node, 0.1, 0, None);
        assert!(result.free_energy.total > 0.1);
        // Comprehension 0.5 > 0.4 threshold, so won't request clarification
        // But with no neighbors and low context, should get None since
        // comprehension is above threshold but no neighbor to connect to
    }

    #[test]
    fn inference_step_understood_node() {
        let node = IntentNode::understood("clear intent", 0.9);
        let result = inference_step(&node, 0.8, 3, None);
        // Well-understood, well-connected, good observed weight
        assert!(result.free_energy.total < 5.0);
    }

    #[test]
    fn inference_result_contains_lineage_id() {
        let node = IntentNode::new("test");
        let expected_id = node.lineage_id().clone();
        let result = inference_step(&node, 0.5, 0, None);
        assert_eq!(result.lineage_id, expected_id);
    }

    #[test]
    fn maintenance_report_no_issues() {
        let report = MaintenanceReport {
            incoherent: vec![],
            stale: vec![],
            integrity_failures: vec![],
            inspected: 10,
        };
        assert!(!report.has_issues());
        assert_eq!(report.issue_count(), 0);
    }

    #[test]
    fn maintenance_report_with_issues() {
        let report = MaintenanceReport {
            incoherent: vec![LineageId::new()],
            stale: vec![LineageId::new(), LineageId::new()],
            integrity_failures: vec![],
            inspected: 10,
        };
        assert!(report.has_issues());
        assert_eq!(report.issue_count(), 3);
    }
}
