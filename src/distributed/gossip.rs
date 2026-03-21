// GOSSIP PROTOCOL — ANTI-ENTROPY
//
// Replicas exchange digests to detect missing/outdated nodes,
// then exchange full node data to converge.
//
// Design decisions:
// 1. Pull-based gossip: replica requests what it's missing.
// 2. Digest = set of (LineageId, version) pairs.
// 3. Phase 3d: synchronous in-process. Phase 4: async over network.
// 4. No Merkle trees yet — full digest exchange. Scale optimization deferred.

use std::collections::HashMap;
use crate::node::IntentNode;
use crate::signature::LineageId;
use super::vclock::VectorClock;

/// A gossip digest entry: what a replica knows about a node.
#[derive(Debug, Clone)]
pub struct DigestEntry {
    pub lineage_id: LineageId,
    pub version: u64,
}

/// A gossip digest: summary of all nodes a replica has.
#[derive(Debug, Clone)]
pub struct Digest {
    pub entries: Vec<DigestEntry>,
}

/// A gossip message exchanged between replicas.
#[derive(Debug, Clone)]
pub enum GossipMessage {
    /// "Here's what I have" — sent to initiate sync.
    Digest(Digest),
    /// "Here's the nodes you're missing or behind on."
    NodeData(Vec<NodeTransfer>),
    /// "I need these nodes" — request after comparing digests.
    NeedNodes(Vec<LineageId>),
}

/// A node being transferred between replicas.
#[derive(Debug, Clone)]
pub struct NodeTransfer {
    pub node: IntentNode,
    pub vector_clock: VectorClock,
}

/// Gossip state for a replica.
///
/// Tracks what this replica knows and manages sync.
#[derive(Debug)]
pub struct GossipState {
    /// Known nodes: LineageId → (version, vector_clock).
    pub(crate) known: HashMap<LineageId, (u64, VectorClock)>,
}

impl GossipState {
    pub fn new() -> Self {
        Self {
            known: HashMap::new(),
        }
    }

    /// Record that we have a node at a given version.
    pub fn record(&mut self, lineage_id: LineageId, version: u64, clock: VectorClock) {
        self.known.insert(lineage_id, (version, clock));
    }

    /// Generate a digest of everything we know.
    pub fn generate_digest(&self) -> Digest {
        let entries = self.known.iter()
            .map(|(id, (version, _))| DigestEntry {
                lineage_id: id.clone(),
                version: *version,
            })
            .collect();
        Digest { entries }
    }

    /// Compare a remote digest against our local state.
    /// Returns LineageIds we need (remote has newer or unknown nodes).
    pub fn compare_digest(&self, remote_digest: &Digest) -> Vec<LineageId> {
        let mut needed = Vec::new();
        for entry in &remote_digest.entries {
            match self.known.get(&entry.lineage_id) {
                None => needed.push(entry.lineage_id.clone()), // We don't have it.
                Some((local_version, _)) => {
                    if entry.version > *local_version {
                        needed.push(entry.lineage_id.clone()); // Remote is newer.
                    }
                }
            }
        }
        needed
    }

    /// How many nodes are tracked.
    pub fn node_count(&self) -> usize {
        self.known.len()
    }

    /// Get the version we know about for a node.
    pub fn known_version(&self, id: &LineageId) -> Option<u64> {
        self.known.get(id).map(|(v, _)| *v)
    }
}

impl Default for GossipState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_gossip_state() {
        let gs = GossipState::new();
        assert_eq!(gs.node_count(), 0);
        let digest = gs.generate_digest();
        assert!(digest.entries.is_empty());
    }

    #[test]
    fn record_and_digest() {
        let mut gs = GossipState::new();
        let id = LineageId::new();
        gs.record(id.clone(), 1, VectorClock::new());
        assert_eq!(gs.node_count(), 1);
        let digest = gs.generate_digest();
        assert_eq!(digest.entries.len(), 1);
        assert_eq!(digest.entries[0].version, 1);
    }

    #[test]
    fn compare_digest_finds_missing() {
        let gs = GossipState::new(); // knows nothing

        let id = LineageId::new();
        let remote_digest = Digest {
            entries: vec![DigestEntry { lineage_id: id.clone(), version: 1 }],
        };

        let needed = gs.compare_digest(&remote_digest);
        assert_eq!(needed.len(), 1);
        assert_eq!(needed[0], id);
    }

    #[test]
    fn compare_digest_finds_outdated() {
        let mut gs = GossipState::new();
        let id = LineageId::new();
        gs.record(id.clone(), 1, VectorClock::new());

        let remote_digest = Digest {
            entries: vec![DigestEntry { lineage_id: id.clone(), version: 3 }],
        };

        let needed = gs.compare_digest(&remote_digest);
        assert_eq!(needed.len(), 1);
    }

    #[test]
    fn compare_digest_skips_up_to_date() {
        let mut gs = GossipState::new();
        let id = LineageId::new();
        gs.record(id.clone(), 5, VectorClock::new());

        let remote_digest = Digest {
            entries: vec![DigestEntry { lineage_id: id.clone(), version: 3 }],
        };

        let needed = gs.compare_digest(&remote_digest);
        assert!(needed.is_empty(), "Should not need node we already have at higher version");
    }

    #[test]
    fn known_version_returns_recorded() {
        let mut gs = GossipState::new();
        let id = LineageId::new();
        gs.record(id.clone(), 7, VectorClock::new());
        assert_eq!(gs.known_version(&id), Some(7));
    }

    #[test]
    fn known_version_unknown_returns_none() {
        let gs = GossipState::new();
        assert_eq!(gs.known_version(&LineageId::new()), None);
    }
}
