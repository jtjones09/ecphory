// HANDOFF EVALUATION — Spec 7 §4.2 / Step 4
//
// Per Spec 7 §4.2 (Gershman G.1 fold): a `HandoffContext` carries both
// natural-language `success_criteria` and machine-verifiable
// `success_checks` predicates. The delegate runs the predicates
// against the fabric and reports pass/fail alongside completion.
//
// This module supplies the evaluation logic. The `SuccessCheck`
// variants and `HandoffContext` struct themselves were defined in
// Step 1 as placeholders; this step makes them executable.

use crate::bridge::BridgeFabric;
use crate::comms::message::{HandoffContext, SuccessCheck};
use crate::context::RelationshipKind;
use crate::identity::{ContentFingerprint, NodeIdentity};
use crate::signature::LineageId;
use uuid::Uuid;

/// Result of evaluating a single `SuccessCheck`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The predicate held against the fabric's current state.
    Pass,
    /// The predicate did not hold. The string is a short reason
    /// the delegate can surface in its completion report.
    Fail(String),
}

impl CheckOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, CheckOutcome::Pass)
    }
}

impl SuccessCheck {
    /// Evaluate this predicate against `bridge`. Pure read — no
    /// fabric writes, no edges, no subscriptions touched.
    pub fn evaluate(&self, bridge: &BridgeFabric) -> CheckOutcome {
        match self {
            SuccessCheck::NodeExists { reference } => {
                let id = match parse_lineage_reference(reference) {
                    Some(id) => id,
                    None => {
                        return CheckOutcome::Fail(format!(
                            "reference '{}' is not a valid LineageId UUID",
                            reference
                        ));
                    }
                };
                if bridge.read_inner(|inner| inner.get_node(&id).is_some()) {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail(format!("node {} not in fabric", reference))
                }
            }
            SuccessCheck::NodeCountInRegion { region, min } => {
                let count = bridge.read_inner(|inner| {
                    inner
                        .nodes()
                        .filter(|(_, n)| {
                            n.causal_position
                                .as_ref()
                                .map(|p| &p.namespace == region)
                                .unwrap_or(false)
                        })
                        .count() as u64
                });
                if count >= *min {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail(format!(
                        "region {} has {} nodes; needed {}",
                        region.name, count, min
                    ))
                }
            }
            SuccessCheck::ContentMatches { node, pattern } => {
                let target = match find_lineage_by_fingerprint(bridge, &node.content_fingerprint)
                {
                    Some(id) => id,
                    None => {
                        return CheckOutcome::Fail(format!(
                            "no node with fingerprint {} found",
                            node.content_fingerprint.to_hex()
                        ));
                    }
                };
                let description = bridge.read_inner(|inner| {
                    inner
                        .get_node(&target)
                        .map(|n| n.want.description.clone())
                        .unwrap_or_default()
                });
                if description.contains(pattern) {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail(format!(
                        "content '{}' does not contain pattern '{}'",
                        description, pattern
                    ))
                }
            }
            SuccessCheck::EdgeExists {
                from,
                to,
                edge_type,
            } => {
                let from_id =
                    match find_lineage_by_fingerprint(bridge, &from.content_fingerprint) {
                        Some(id) => id,
                        None => {
                            return CheckOutcome::Fail(format!(
                                "edge source fingerprint {} not in fabric",
                                from.content_fingerprint.to_hex()
                            ));
                        }
                    };
                let to_id = match find_lineage_by_fingerprint(bridge, &to.content_fingerprint) {
                    Some(id) => id,
                    None => {
                        return CheckOutcome::Fail(format!(
                            "edge target fingerprint {} not in fabric",
                            to.content_fingerprint.to_hex()
                        ));
                    }
                };
                let kind = match edge_type_to_kind(edge_type) {
                    Some(k) => k,
                    None => {
                        return CheckOutcome::Fail(format!(
                            "unrecognized edge type '{}'",
                            edge_type
                        ));
                    }
                };
                let present = bridge.read_inner(|inner| {
                    inner
                        .edges_from(&from_id)
                        .iter()
                        .any(|e| e.target == to_id && e.kind == kind)
                });
                if present {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail(format!(
                        "no {:?} edge from {} to {}",
                        kind, from_id, to_id
                    ))
                }
            }
        }
    }
}

impl HandoffContext {
    /// Evaluate every `SuccessCheck` in the handoff against `bridge`,
    /// returning per-check outcomes in declaration order.
    pub fn evaluate_all(&self, bridge: &BridgeFabric) -> Vec<CheckOutcome> {
        self.success_checks
            .iter()
            .map(|c| c.evaluate(bridge))
            .collect()
    }

    /// True if every `SuccessCheck` evaluates to `Pass`. Vacuously true
    /// if no checks are attached.
    pub fn all_checks_pass(&self, bridge: &BridgeFabric) -> bool {
        self.evaluate_all(bridge).iter().all(|o| o.is_pass())
    }
}

fn parse_lineage_reference(s: &str) -> Option<LineageId> {
    Uuid::parse_str(s).ok().map(LineageId::from_uuid)
}

/// Scan the fabric for a node whose `content_fingerprint` matches `fp`.
/// Linear in node count; v1 acceptable for handoff eval. Future
/// versions may index by fingerprint if this becomes hot.
pub fn find_lineage_by_fingerprint(
    bridge: &BridgeFabric,
    fp: &ContentFingerprint,
) -> Option<LineageId> {
    bridge.read_inner(|inner| {
        inner
            .nodes()
            .find(|(_, n)| n.content_fingerprint() == fp)
            .map(|(id, _)| id.clone())
    })
}

fn edge_type_to_kind(edge_type: &str) -> Option<RelationshipKind> {
    match edge_type {
        "thread" | "Thread" => Some(RelationshipKind::Thread),
        "depends_on" | "DependsOn" => Some(RelationshipKind::DependsOn),
        "derived_from" | "DerivedFrom" => Some(RelationshipKind::DerivedFrom),
        "related_to" | "RelatedTo" => Some(RelationshipKind::RelatedTo),
        "refines" | "Refines" => Some(RelationshipKind::Refines),
        "constrains" | "Constrains" => Some(RelationshipKind::Constrains),
        "precedes" | "Precedes" => Some(RelationshipKind::Precedes),
        "follows" | "Follows" => Some(RelationshipKind::Follows),
        "conflicts_with" | "ConflictsWith" => Some(RelationshipKind::ConflictsWith),
        s if s.starts_with("custom:") => Some(RelationshipKind::Custom(s[7..].to_string())),
        _ => None,
    }
}

/// Helper for constructing a `NodeIdentity` from a freshly-created
/// node — useful for tests and for delegates building checks.
pub fn node_identity(bridge: &BridgeFabric, id: &LineageId) -> Option<NodeIdentity> {
    use crate::bridge::FabricTrait;
    bridge.node_identity(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lineage_reference_accepts_uuid_strings() {
        let id = LineageId::new();
        let s = id.as_uuid().to_string();
        assert_eq!(parse_lineage_reference(&s), Some(id));
    }

    #[test]
    fn parse_lineage_reference_rejects_garbage() {
        assert!(parse_lineage_reference("not-a-uuid").is_none());
    }

    #[test]
    fn edge_type_to_kind_handles_canonical_names() {
        assert_eq!(edge_type_to_kind("thread"), Some(RelationshipKind::Thread));
        assert_eq!(edge_type_to_kind("Thread"), Some(RelationshipKind::Thread));
        assert_eq!(
            edge_type_to_kind("custom:abc"),
            Some(RelationshipKind::Custom("abc".to_string()))
        );
        assert!(edge_type_to_kind("unknown").is_none());
    }
}
