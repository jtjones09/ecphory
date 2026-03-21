// SERIALIZABLE MIRROR TYPES FOR PERSISTENCE
//
// These types mirror the core types but carry serde derives.
// Core types remain serde-free — this is the boundary layer.
//
// Design decisions:
// 1. FabricInstant (wall clock) is NOT serialized — loaded nodes are "fresh".
//    Lamport timestamps ARE serialized for causal ordering across sessions.
// 2. signature_index and reverse_edges are NOT serialized — rebuilt from data.
// 3. format_version included from day one for schema migration.

use serde::{Serialize, Deserialize};

use crate::confidence::{ConfidenceSurface, Distribution};
use crate::constraint::{Constraint, ConstraintField, ConstraintKind};
use crate::context::RelationshipKind;
use crate::fabric::Fabric;
use crate::node::{IntentNode, ResolutionTarget};
use crate::signature::LineageId;

use super::PersistError;

/// Current serialization format version.
pub const FORMAT_VERSION: u32 = 1;

// ═══════════════════════════════════════════════
//  Mirror Types
// ═══════════════════════════════════════════════

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialFabric {
    pub format_version: u32,
    pub clock_value: u64,
    pub decay_lambda: f64,
    pub nodes: Vec<SerialNodeEntry>,
    pub edges: Vec<SerialEdgeRecord>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialNodeEntry {
    pub lineage_id: String,
    pub version: u64,
    pub want_description: String,
    /// Embedding vector. Absent in pre-3b files — serde(default) handles that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f64>>,
    pub constraints: Vec<SerialConstraint>,
    pub confidence: SerialConfidenceSurface,
    pub resolution: SerialResolutionTarget,
    pub lamport_ts: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialConstraint {
    pub semantic: String,
    pub kind: String,
    pub weight: Option<f64>,
    pub verified: bool,
    pub violated: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialConfidenceSurface {
    pub comprehension: SerialDistribution,
    pub resolution: SerialDistribution,
    pub verification: SerialDistribution,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialDistribution {
    pub mean: f64,
    pub variance: f64,
    pub observations: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialEdgeRecord {
    pub from: String,
    pub to: String,
    pub weight: f64,
    pub kind: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerialResolutionTarget {
    pub state: String,
    pub description: Option<String>,
}

// ═══════════════════════════════════════════════
//  Core → Serial Conversions
// ═══════════════════════════════════════════════

impl SerialFabric {
    /// Convert a live Fabric into its serializable form.
    pub fn from_fabric(fabric: &Fabric) -> Self {
        let nodes: Vec<SerialNodeEntry> = fabric.nodes()
            .map(|(id, node)| {
                let lamport_ts = fabric.node_lamport_ts(id).unwrap_or(0);
                SerialNodeEntry::from_node(node, lamport_ts)
            })
            .collect();

        let mut edges = Vec::new();
        for (from_id, edge_slice) in fabric.all_edges() {
            for edge in edge_slice {
                edges.push(SerialEdgeRecord {
                    from: from_id.as_uuid().to_string(),
                    to: edge.target.as_uuid().to_string(),
                    weight: edge.weight,
                    kind: relationship_kind_to_string(&edge.kind),
                });
            }
        }

        SerialFabric {
            format_version: FORMAT_VERSION,
            clock_value: fabric.clock_value(),
            decay_lambda: fabric.decay_lambda(),
            nodes,
            edges,
        }
    }

    /// Reconstruct a Fabric from serialized data.
    pub fn into_fabric(self) -> Result<Fabric, PersistError> {
        if self.format_version != FORMAT_VERSION {
            return Err(PersistError::VersionMismatch {
                expected: FORMAT_VERSION,
                found: self.format_version,
            });
        }

        let mut nodes = Vec::new();
        for serial_node in self.nodes {
            let (node, lamport_ts) = serial_node.into_node()?;
            nodes.push((node, lamport_ts));
        }

        let mut edges = Vec::new();
        for serial_edge in self.edges {
            let from = parse_lineage_id(&serial_edge.from)?;
            let to = parse_lineage_id(&serial_edge.to)?;
            let kind = string_to_relationship_kind(&serial_edge.kind)?;
            edges.push((from, to, serial_edge.weight, kind));
        }

        Ok(Fabric::from_persisted(nodes, edges, self.clock_value, self.decay_lambda))
    }
}

impl SerialNodeEntry {
    fn from_node(node: &IntentNode, lamport_ts: u64) -> Self {
        SerialNodeEntry {
            lineage_id: node.lineage_id().as_uuid().to_string(),
            version: node.version(),
            want_description: node.want.description.clone(),
            embedding: node.want.embedding.clone(),
            constraints: node.constraints.constraints.iter()
                .map(SerialConstraint::from_constraint)
                .collect(),
            confidence: SerialConfidenceSurface::from_surface(&node.confidence),
            resolution: SerialResolutionTarget::from_target(&node.resolution),
            lamport_ts,
        }
    }

    fn into_node(self) -> Result<(IntentNode, u64), PersistError> {
        // Reconstruct the node by building it from parts.
        let mut node = IntentNode::new(&self.want_description);

        // Restore the lineage ID.
        let uuid = uuid::Uuid::parse_str(&self.lineage_id)
            .map_err(|e| PersistError::DeserializationError(
                format!("Invalid lineage UUID '{}': {}", self.lineage_id, e)
            ))?;
        node.set_lineage_id(LineageId::from_uuid(uuid));

        // Restore embedding if present.
        node.want.embedding = self.embedding;

        // Restore constraints.
        node.constraints = ConstraintField::new();
        for sc in &self.constraints {
            let constraint = sc.to_constraint()?;
            node.constraints.constraints.push(constraint);
        }

        // Restore confidence.
        node.confidence = self.confidence.to_surface();

        // Restore resolution.
        node.resolution = self.resolution.to_target()?;

        // Recompute signature to match the restored content.
        // This calls recompute_signature which also bumps version,
        // so we need to restore the version AFTER.
        node.recompute_signature();

        // Restore the persisted version.
        // The node was created at v0 and recompute bumped it to v1,
        // but the persisted version is what we want.
        node.set_version(self.version);

        Ok((node, self.lamport_ts))
    }
}

impl SerialConstraint {
    fn from_constraint(c: &Constraint) -> Self {
        let (kind, weight) = match &c.kind {
            ConstraintKind::Hard => ("hard".to_string(), None),
            ConstraintKind::Soft { weight } => ("soft".to_string(), Some(*weight)),
        };
        SerialConstraint {
            semantic: c.semantic.clone(),
            kind,
            weight,
            verified: c.verified,
            violated: c.violated,
        }
    }

    fn to_constraint(&self) -> Result<Constraint, PersistError> {
        let kind = match self.kind.as_str() {
            "hard" => ConstraintKind::Hard,
            "soft" => {
                let w = self.weight.ok_or_else(|| PersistError::DeserializationError(
                    "Soft constraint missing weight".to_string()
                ))?;
                ConstraintKind::Soft { weight: w }
            }
            other => return Err(PersistError::DeserializationError(
                format!("Unknown constraint kind: {}", other)
            )),
        };
        Ok(Constraint {
            semantic: self.semantic.clone(),
            kind,
            verified: self.verified,
            violated: self.violated,
        })
    }
}

impl SerialConfidenceSurface {
    fn from_surface(cs: &ConfidenceSurface) -> Self {
        SerialConfidenceSurface {
            comprehension: SerialDistribution::from_dist(&cs.comprehension),
            resolution: SerialDistribution::from_dist(&cs.resolution),
            verification: SerialDistribution::from_dist(&cs.verification),
        }
    }

    fn to_surface(&self) -> ConfidenceSurface {
        ConfidenceSurface {
            comprehension: self.comprehension.to_dist(),
            resolution: self.resolution.to_dist(),
            verification: self.verification.to_dist(),
        }
    }
}

impl SerialDistribution {
    fn from_dist(d: &Distribution) -> Self {
        SerialDistribution {
            mean: d.mean,
            variance: d.variance,
            observations: d.observations,
        }
    }

    fn to_dist(&self) -> Distribution {
        Distribution {
            mean: self.mean,
            variance: self.variance,
            observations: self.observations,
        }
    }
}

impl SerialResolutionTarget {
    fn from_target(rt: &ResolutionTarget) -> Self {
        match rt {
            ResolutionTarget::Unresolved => SerialResolutionTarget {
                state: "unresolved".to_string(),
                description: None,
            },
            ResolutionTarget::InProgress { plan_description } => SerialResolutionTarget {
                state: "in_progress".to_string(),
                description: Some(plan_description.clone()),
            },
            ResolutionTarget::Resolved { outcome_description } => SerialResolutionTarget {
                state: "resolved".to_string(),
                description: Some(outcome_description.clone()),
            },
            ResolutionTarget::Failed { reason } => SerialResolutionTarget {
                state: "failed".to_string(),
                description: Some(reason.clone()),
            },
            ResolutionTarget::Abandoned { reason } => SerialResolutionTarget {
                state: "abandoned".to_string(),
                description: Some(reason.clone()),
            },
        }
    }

    fn to_target(&self) -> Result<ResolutionTarget, PersistError> {
        match self.state.as_str() {
            "unresolved" => Ok(ResolutionTarget::Unresolved),
            "in_progress" => Ok(ResolutionTarget::InProgress {
                plan_description: self.description.clone().unwrap_or_default(),
            }),
            "resolved" => Ok(ResolutionTarget::Resolved {
                outcome_description: self.description.clone().unwrap_or_default(),
            }),
            "failed" => Ok(ResolutionTarget::Failed {
                reason: self.description.clone().unwrap_or_default(),
            }),
            "abandoned" => Ok(ResolutionTarget::Abandoned {
                reason: self.description.clone().unwrap_or_default(),
            }),
            other => Err(PersistError::DeserializationError(
                format!("Unknown resolution state: {}", other)
            )),
        }
    }
}

// ═══════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════

fn parse_lineage_id(s: &str) -> Result<LineageId, PersistError> {
    let uuid = uuid::Uuid::parse_str(s)
        .map_err(|e| PersistError::DeserializationError(
            format!("Invalid lineage UUID '{}': {}", s, e)
        ))?;
    Ok(LineageId::from_uuid(uuid))
}

fn relationship_kind_to_string(kind: &RelationshipKind) -> String {
    match kind {
        RelationshipKind::DerivedFrom => "derived_from".to_string(),
        RelationshipKind::DependsOn => "depends_on".to_string(),
        RelationshipKind::ConflictsWith => "conflicts_with".to_string(),
        RelationshipKind::RelatedTo => "related_to".to_string(),
        RelationshipKind::Precedes => "precedes".to_string(),
        RelationshipKind::Follows => "follows".to_string(),
        RelationshipKind::Refines => "refines".to_string(),
        RelationshipKind::Constrains => "constrains".to_string(),
        RelationshipKind::Custom(s) => format!("custom:{}", s),
    }
}

fn string_to_relationship_kind(s: &str) -> Result<RelationshipKind, PersistError> {
    match s {
        "derived_from" => Ok(RelationshipKind::DerivedFrom),
        "depends_on" => Ok(RelationshipKind::DependsOn),
        "conflicts_with" => Ok(RelationshipKind::ConflictsWith),
        "related_to" => Ok(RelationshipKind::RelatedTo),
        "precedes" => Ok(RelationshipKind::Precedes),
        "follows" => Ok(RelationshipKind::Follows),
        "refines" => Ok(RelationshipKind::Refines),
        "constrains" => Ok(RelationshipKind::Constrains),
        s if s.starts_with("custom:") => Ok(RelationshipKind::Custom(s[7..].to_string())),
        other => Err(PersistError::DeserializationError(
            format!("Unknown relationship kind: {}", other)
        )),
    }
}

// ═══════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RelationshipKind;

    #[test]
    fn serial_distribution_roundtrip() {
        let d = Distribution { mean: 0.75, variance: 0.05, observations: 10 };
        let sd = SerialDistribution::from_dist(&d);
        let d2 = sd.to_dist();
        assert_eq!(d.mean, d2.mean);
        assert_eq!(d.variance, d2.variance);
        assert_eq!(d.observations, d2.observations);
    }

    #[test]
    fn serial_confidence_roundtrip() {
        let cs = ConfidenceSurface::understood(0.85);
        let scs = SerialConfidenceSurface::from_surface(&cs);
        let cs2 = scs.to_surface();
        assert!((cs.comprehension.mean - cs2.comprehension.mean).abs() < 1e-10);
        assert!((cs.resolution.mean - cs2.resolution.mean).abs() < 1e-10);
        assert!((cs.verification.mean - cs2.verification.mean).abs() < 1e-10);
    }

    #[test]
    fn serial_constraint_hard_roundtrip() {
        let c = Constraint::hard("must be private");
        let sc = SerialConstraint::from_constraint(&c);
        let c2 = sc.to_constraint().unwrap();
        assert_eq!(c.semantic, c2.semantic);
        assert!(c2.is_hard());
    }

    #[test]
    fn serial_constraint_soft_roundtrip() {
        let c = Constraint::soft("prefer fast", 0.7);
        let sc = SerialConstraint::from_constraint(&c);
        let c2 = sc.to_constraint().unwrap();
        assert_eq!(c.semantic, c2.semantic);
        assert!(!c2.is_hard());
        assert!((c.weight() - c2.weight()).abs() < 1e-10);
    }

    #[test]
    fn serial_constraint_with_violation_state() {
        let mut c = Constraint::hard("must be private");
        c.satisfy();
        let sc = SerialConstraint::from_constraint(&c);
        let c2 = sc.to_constraint().unwrap();
        assert!(c2.verified);
        assert_eq!(c2.violated, Some(false));
    }

    #[test]
    fn serial_resolution_unresolved_roundtrip() {
        let rt = ResolutionTarget::Unresolved;
        let srt = SerialResolutionTarget::from_target(&rt);
        let rt2 = srt.to_target().unwrap();
        assert_eq!(rt, rt2);
    }

    #[test]
    fn serial_resolution_in_progress_roundtrip() {
        let rt = ResolutionTarget::InProgress { plan_description: "working on it".into() };
        let srt = SerialResolutionTarget::from_target(&rt);
        let rt2 = srt.to_target().unwrap();
        assert_eq!(rt, rt2);
    }

    #[test]
    fn serial_resolution_resolved_roundtrip() {
        let rt = ResolutionTarget::Resolved { outcome_description: "done".into() };
        let srt = SerialResolutionTarget::from_target(&rt);
        let rt2 = srt.to_target().unwrap();
        assert_eq!(rt, rt2);
    }

    #[test]
    fn serial_resolution_failed_roundtrip() {
        let rt = ResolutionTarget::Failed { reason: "not possible".into() };
        let srt = SerialResolutionTarget::from_target(&rt);
        let rt2 = srt.to_target().unwrap();
        assert_eq!(rt, rt2);
    }

    #[test]
    fn serial_resolution_abandoned_roundtrip() {
        let rt = ResolutionTarget::Abandoned { reason: "user cancelled".into() };
        let srt = SerialResolutionTarget::from_target(&rt);
        let rt2 = srt.to_target().unwrap();
        assert_eq!(rt, rt2);
    }

    #[test]
    fn serial_node_roundtrip() {
        let mut node = IntentNode::understood("send a message", 0.85);
        node.constraints.add_hard("must be private");
        node.constraints.add_soft("prefer Signal", 0.7);
        node.recompute_signature();

        let sn = SerialNodeEntry::from_node(&node, 42);
        let (node2, ts) = sn.into_node().unwrap();

        assert_eq!(node.want.description, node2.want.description);
        assert_eq!(node.lineage_id(), node2.lineage_id());
        assert_eq!(node.signature(), node2.signature());
        assert_eq!(node.constraints.count(), node2.constraints.count());
        assert_eq!(ts, 42);
    }

    #[test]
    fn serial_edge_roundtrip() {
        let kind = RelationshipKind::DependsOn;
        let s = relationship_kind_to_string(&kind);
        let kind2 = string_to_relationship_kind(&s).unwrap();
        assert_eq!(kind, kind2);
    }

    #[test]
    fn serial_custom_relationship_roundtrip() {
        let kind = RelationshipKind::Custom("my_relation".to_string());
        let s = relationship_kind_to_string(&kind);
        let kind2 = string_to_relationship_kind(&s).unwrap();
        assert_eq!(kind, kind2);
    }

    #[test]
    fn all_relationship_kinds_roundtrip() {
        let kinds = vec![
            RelationshipKind::DerivedFrom,
            RelationshipKind::DependsOn,
            RelationshipKind::ConflictsWith,
            RelationshipKind::RelatedTo,
            RelationshipKind::Precedes,
            RelationshipKind::Follows,
            RelationshipKind::Refines,
            RelationshipKind::Constrains,
            RelationshipKind::Custom("test".to_string()),
        ];
        for kind in kinds {
            let s = relationship_kind_to_string(&kind);
            let kind2 = string_to_relationship_kind(&s).unwrap();
            assert_eq!(kind, kind2);
        }
    }

    #[test]
    fn serial_fabric_empty_roundtrip() {
        let fabric = Fabric::new();
        let sf = SerialFabric::from_fabric(&fabric);
        let fabric2 = sf.into_fabric().unwrap();
        assert_eq!(fabric2.node_count(), 0);
        assert_eq!(fabric2.edge_count(), 0);
    }

    #[test]
    fn serial_fabric_with_nodes_roundtrip() {
        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::understood("send a message", 0.85));
        let b = fabric.add_node(IntentNode::understood("ensure privacy", 0.95));
        fabric.add_node(IntentNode::new("buy groceries"));
        fabric.add_edge(&a, &b, 0.9, RelationshipKind::DependsOn).unwrap();

        let sf = SerialFabric::from_fabric(&fabric);
        let json = serde_json::to_string_pretty(&sf).unwrap();
        let sf2: SerialFabric = serde_json::from_str(&json).unwrap();
        let fabric2 = sf2.into_fabric().unwrap();

        assert_eq!(fabric2.node_count(), 3);
        assert_eq!(fabric2.edge_count(), 1);
    }

    #[test]
    fn serial_lineage_id_preserved() {
        let mut fabric = Fabric::new();
        let node = IntentNode::new("test node");
        let original_id = node.lineage_id().clone();
        fabric.add_node(node);

        let sf = SerialFabric::from_fabric(&fabric);
        let fabric2 = sf.into_fabric().unwrap();

        assert!(fabric2.contains(&original_id));
        assert_eq!(fabric2.get_node(&original_id).unwrap().want.description, "test node");
    }

    #[test]
    fn serial_version_preserved() {
        let mut fabric = Fabric::new();
        let id = fabric.add_node(IntentNode::new("test"));
        fabric.mutate_node(&id, |n| {
            n.want.description = "changed once".to_string();
        }).unwrap();
        fabric.mutate_node(&id, |n| {
            n.want.description = "changed twice".to_string();
        }).unwrap();
        assert_eq!(fabric.get_node(&id).unwrap().version(), 2);

        let sf = SerialFabric::from_fabric(&fabric);
        let fabric2 = sf.into_fabric().unwrap();
        assert_eq!(fabric2.get_node(&id).unwrap().version(), 2);
    }

    #[test]
    fn version_mismatch_rejected() {
        let sf = SerialFabric {
            format_version: 999,
            clock_value: 0,
            decay_lambda: 0.001,
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        let result = sf.into_fabric();
        assert!(result.is_err());
    }
}
