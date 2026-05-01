// NODE IDENTITY — The 4-tuple (Spec 5 §2.1.1, Spec 8 §4.2)
//
// A node's identity is the four-tuple
//   (content_fingerprint, causal_position, creator_voice, topological_position)
// The first three travel with the node and are stored on `IntentNode`. The
// fourth is contextual — computed by the fabric from the live edge graph
// at observation time. Spec 8 §4.2 estimates ~5µs to compute on demand.
//
// `NodeIdentity` is a *snapshot* bundling these together for callers that
// need a single value to compare, log, or pass to the immune system. It
// is constructed by `Fabric::node_identity(id)`.

use crate::identity::content_fingerprint::ContentFingerprint;
use crate::identity::causal_position::CausalPosition;
use crate::identity::voice_print::VoicePrint;

/// Where a node sits in the fabric's edge graph (Spec 5 §2.1.1, §2.1).
///
/// Computed from the live graph at observation time. Two nodes with
/// identical content + causal position can still be distinguished by
/// topology — same as how two electrons in different shells differ in
/// quantum-number context.
///
/// Implementation: in-degree, out-degree, and a BLAKE3 fingerprint over
/// the sorted set of neighbor LineageIds. Cheap to compute, stable while
/// the graph doesn't change, and gives the immune system a single field
/// to baseline against per-node clustering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TopologicalPosition {
    pub in_degree: u32,
    pub out_degree: u32,
    /// BLAKE3-256 over the sorted, hex-formatted neighbor LineageIds.
    /// Identical neighbor sets → identical fingerprints. Cheap diff signal.
    pub neighbor_fingerprint: [u8; 32],
}

impl TopologicalPosition {
    pub fn new(in_degree: u32, out_degree: u32, neighbor_fingerprint: [u8; 32]) -> Self {
        Self { in_degree, out_degree, neighbor_fingerprint }
    }
}

/// The four-component node identity (Spec 8 §4.2). Stored components +
/// computed `topological_position` rolled together for callers that
/// want a single value to pass around.
#[derive(Debug, Clone)]
pub struct NodeIdentity {
    pub content_fingerprint: ContentFingerprint,
    pub causal_position: CausalPosition,
    pub creator_voice: Option<VoicePrint>,
    pub topological_position: TopologicalPosition,
}

impl NodeIdentity {
    pub fn new(
        content_fingerprint: ContentFingerprint,
        causal_position: CausalPosition,
        creator_voice: Option<VoicePrint>,
        topological_position: TopologicalPosition,
    ) -> Self {
        Self {
            content_fingerprint,
            causal_position,
            creator_voice,
            topological_position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topological_position_is_copy() {
        let tp = TopologicalPosition::new(1, 2, [0u8; 32]);
        let tp2 = tp; // copy
        assert_eq!(tp, tp2);
    }
}
