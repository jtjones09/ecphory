// DISTRIBUTED CONSISTENCY — PHASE 3d
//
// Vector clocks, conflict resolution, gossip primitives.
// Single-process simulation — real network deferred to Phase 4.
//
// Design decisions:
// 1. DistributedFabric wraps Fabric + gossip state + conflict log.
// 2. receive_node() detects conflicts via vector clock comparison.
// 3. Conflict strategies route based on node properties.
// 4. LocalTransport for in-process testing.
// 5. No new external dependencies.

pub mod vclock;
pub mod conflict;
pub mod gossip;
pub mod transport;

pub use vclock::{VectorClock, ReplicaId};
pub use conflict::{Conflict, ConflictStrategy, Resolution, resolve_conflict, select_strategy};
pub use gossip::{GossipState, GossipMessage, Digest, DigestEntry, NodeTransfer};
pub use transport::{Transport, LocalTransport, Envelope};

use crate::fabric::Fabric;
use crate::node::IntentNode;
use crate::signature::LineageId;

/// A distributed fabric replica.
///
/// Wraps a local Fabric with distributed consistency primitives:
/// vector clocks, gossip state, and conflict log.
pub struct DistributedFabric {
    /// This replica's unique ID.
    pub replica_id: ReplicaId,
    /// The local fabric instance.
    pub fabric: Fabric,
    /// Vector clock for this replica.
    pub clock: VectorClock,
    /// Gossip state tracking known node versions.
    pub gossip: GossipState,
    /// Log of resolved conflicts.
    pub conflict_log: Vec<(Conflict, Resolution)>,
}

impl DistributedFabric {
    /// Create a new distributed fabric replica.
    pub fn new() -> Self {
        Self {
            replica_id: ReplicaId::new(),
            fabric: Fabric::new(),
            clock: VectorClock::new(),
            gossip: GossipState::new(),
            conflict_log: Vec::new(),
        }
    }

    /// Create with a specific replica ID (for testing/persistence).
    pub fn with_replica_id(replica_id: ReplicaId) -> Self {
        Self {
            replica_id,
            fabric: Fabric::new(),
            clock: VectorClock::new(),
            gossip: GossipState::new(),
            conflict_log: Vec::new(),
        }
    }

    /// Add a node to the local fabric.
    /// Ticks the vector clock and records in gossip state.
    pub fn add_node(&mut self, node: IntentNode) -> LineageId {
        self.clock.tick(&self.replica_id);
        let id = self.fabric.add_node(node);
        let version = self.fabric.get_node(&id).map(|n| n.version()).unwrap_or(0);
        self.gossip.record(id.clone(), version, self.clock.clone());
        id
    }

    /// Receive a node from a remote replica.
    ///
    /// Detects conflicts via vector clock comparison:
    /// - Remote happened-after local → accept remote (update).
    /// - Remote happened-before local → ignore (already have newer).
    /// - Concurrent → conflict resolution.
    /// - Unknown node → accept (new).
    pub fn receive_node(&mut self, transfer: NodeTransfer) -> ReceiveResult {
        let lineage_id = transfer.node.lineage_id().clone();

        // Merge the remote clock into ours.
        self.clock.merge(&transfer.vector_clock);
        self.clock.tick(&self.replica_id);

        match self.fabric.get_node(&lineage_id) {
            None => {
                // New node — accept it.
                // Record the transfer's clock (not our merged clock) so that
                // future updates from the same sender are detected as newer,
                // not concurrent (our local ticks are irrelevant to the node's causal history).
                self.fabric.add_node(transfer.node.clone());
                self.gossip.record(
                    lineage_id.clone(),
                    transfer.node.version(),
                    transfer.vector_clock,
                );
                ReceiveResult::Accepted
            }
            Some(local_node) => {
                let local_clock = self.gossip.known.get(&lineage_id)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_else(VectorClock::new);

                // Compare vector clocks.
                match local_clock.partial_cmp(&transfer.vector_clock) {
                    Some(std::cmp::Ordering::Less) => {
                        // Remote is newer — replace local.
                        // Record the transfer's clock to preserve the node's causal history.
                        self.fabric.fade_node(&lineage_id);
                        self.fabric.add_node(transfer.node.clone());
                        self.gossip.record(
                            lineage_id.clone(),
                            transfer.node.version(),
                            transfer.vector_clock,
                        );
                        ReceiveResult::Updated
                    }
                    Some(std::cmp::Ordering::Greater) | Some(std::cmp::Ordering::Equal) => {
                        // We already have this or newer — ignore.
                        ReceiveResult::AlreadyHave
                    }
                    None => {
                        // Concurrent — conflict!
                        let conflict = Conflict {
                            lineage_id: lineage_id.clone(),
                            local: local_node.clone(),
                            local_clock,
                            remote: transfer.node.clone(),
                            remote_clock: transfer.vector_clock,
                        };

                        let resolution = resolve_conflict(&conflict);

                        // Apply resolution.
                        match &resolution {
                            Resolution::Resolved { winner, .. } => {
                                self.fabric.fade_node(&lineage_id);
                                self.fabric.add_node(winner.clone());
                                self.gossip.record(
                                    lineage_id.clone(),
                                    winner.version(),
                                    self.clock.clone(),
                                );
                            }
                            Resolution::Branched { branch_a: _, branch_b } => {
                                // Keep local (branch_a), add branch_b as new node.
                                // The branched node gets a new lineage ID.
                                self.fabric.add_node(branch_b.clone());
                            }
                        }

                        self.conflict_log.push((conflict, resolution));
                        ReceiveResult::Conflicted
                    }
                }
            }
        }
    }

    /// Generate a gossip digest for this replica.
    pub fn generate_digest(&self) -> Digest {
        self.gossip.generate_digest()
    }

    /// Compare a remote digest and find what we need.
    pub fn needs_from_digest(&self, remote_digest: &Digest) -> Vec<LineageId> {
        self.gossip.compare_digest(remote_digest)
    }

    /// Package nodes for transfer to another replica.
    pub fn package_nodes(&self, ids: &[LineageId]) -> Vec<NodeTransfer> {
        ids.iter()
            .filter_map(|id| {
                self.fabric.get_node(id).map(|node| NodeTransfer {
                    node: node.clone(),
                    vector_clock: self.clock.clone(),
                })
            })
            .collect()
    }

    /// How many conflicts have been resolved.
    pub fn conflict_count(&self) -> usize {
        self.conflict_log.len()
    }
}

impl Default for DistributedFabric {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of receiving a node from a remote replica.
#[derive(Debug, Clone, PartialEq)]
pub enum ReceiveResult {
    /// New node accepted.
    Accepted,
    /// Existing node updated (remote was newer).
    Updated,
    /// Already have this version or newer.
    AlreadyHave,
    /// Concurrent modification — conflict resolved.
    Conflicted,
}

// Make gossip state fields accessible for receive_node.
// This is a module-level design choice — gossip.known is pub(super).
mod gossip_access {
    // The `known` field on GossipState needs to be accessible from DistributedFabric.
    // We handle this by making it pub(crate) in gossip.rs.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_distributed_fabric() {
        let df = DistributedFabric::new();
        assert_eq!(df.fabric.node_count(), 0);
        assert_eq!(df.conflict_count(), 0);
    }

    #[test]
    fn add_node_ticks_clock() {
        let mut df = DistributedFabric::new();
        let before = df.clock.get(&df.replica_id);
        df.add_node(IntentNode::new("test"));
        let after = df.clock.get(&df.replica_id);
        assert!(after > before);
    }

    #[test]
    fn add_node_records_in_gossip() {
        let mut df = DistributedFabric::new();
        let id = df.add_node(IntentNode::new("test"));
        assert_eq!(df.gossip.known_version(&id), Some(0));
    }

    #[test]
    fn receive_new_node_accepted() {
        let mut df = DistributedFabric::new();
        let node = IntentNode::new("remote node");
        let transfer = NodeTransfer {
            node,
            vector_clock: VectorClock::new(),
        };
        let result = df.receive_node(transfer);
        assert_eq!(result, ReceiveResult::Accepted);
        assert_eq!(df.fabric.node_count(), 1);
    }

    #[test]
    fn generate_and_compare_digest() {
        let mut df1 = DistributedFabric::new();
        let mut df2 = DistributedFabric::new();

        let id = df1.add_node(IntentNode::new("only on df1"));
        df2.add_node(IntentNode::new("only on df2"));

        let digest1 = df1.generate_digest();
        let needed = df2.needs_from_digest(&digest1);
        assert_eq!(needed.len(), 1);
        assert_eq!(needed[0], id);
    }

    #[test]
    fn package_nodes_for_transfer() {
        let mut df = DistributedFabric::new();
        let id = df.add_node(IntentNode::new("test"));
        let packages = df.package_nodes(&[id]);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].node.want.description, "test");
    }
}
