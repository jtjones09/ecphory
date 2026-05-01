// CAUSAL POSITION — (LamportTimestamp, FabricInstant, NamespaceId) (Spec 5 §2.1.1)
//
// The "worldline" component of a node's identity: when it emerged
// relative to everything else. Three dimensions:
// - LamportTimestamp: total ordering within the fabric (already in temporal/)
// - FabricInstant: wall-clock for decay calculations (already in temporal/)
// - NamespaceId: which region/namespace the node was written into
//
// Together these give every node a discoverable position in the fabric's
// causal structure — verifiable by any observer through local computation.

use crate::temporal::{FabricInstant, LamportTimestamp};
use uuid::Uuid;

/// Identifies a region/namespace in the fabric (e.g., `propmgmt`, `nisaba`).
///
/// Per spec §2.2: regions emerge from topology, but `NamespaceId` is the
/// stable handle the fabric uses to attach sensitivity policy and route
/// writes. Per spec §4.1: deterministic from installation parameters via
/// `NamespaceId::from_entropy(genesis_entropy)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NamespaceId {
    /// Human-readable name (e.g., "propmgmt", "nisaba").
    pub name: String,
    /// Stable UUID — invariant across renames.
    pub uuid: Uuid,
}

impl NamespaceId {
    /// Construct a namespace with an explicit UUID (e.g., from genesis).
    pub fn new(name: impl Into<String>, uuid: Uuid) -> Self {
        Self { name: name.into(), uuid }
    }

    /// Construct a namespace with a freshly-generated UUID.
    pub fn fresh(name: impl Into<String>) -> Self {
        Self::new(name, Uuid::new_v4())
    }

    /// Deterministic namespace ID from genesis entropy.
    /// Per spec §4.1: "Deterministic from the installation parameters".
    pub fn from_entropy(name: impl Into<String>, entropy: &[u8]) -> Self {
        // Hash entropy → 16-byte UUID.
        let hash = blake3::hash(entropy);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self::new(name, Uuid::from_bytes(bytes))
    }

    /// The default namespace, used when no explicit region is provided.
    /// Stable across instances so legacy `add_node` callers behave consistently.
    pub fn default_namespace() -> Self {
        Self::new(
            "default",
            Uuid::from_bytes([
                0xec, 0x44, 0xb0, 0x05, 0x00, 0x00, 0x40, 0x00,
                0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            ]),
        )
    }
}

impl std::fmt::Display for NamespaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ns:{}", self.name)
    }
}

/// Where in the fabric's causal structure this node sits.
///
/// Set by the fabric at insertion time. Travels with the node forever —
/// "the moment it emerged relative to everything else" (Spec 5 §2.1.1).
#[derive(Debug, Clone)]
pub struct CausalPosition {
    pub lamport: LamportTimestamp,
    pub instant: FabricInstant,
    pub namespace: NamespaceId,
}

impl CausalPosition {
    pub fn new(lamport: LamportTimestamp, instant: FabricInstant, namespace: NamespaceId) -> Self {
        Self { lamport, instant, namespace }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_entropy_is_deterministic() {
        let entropy = b"genesis-seed-2026-04-30";
        let a = NamespaceId::from_entropy("propmgmt", entropy);
        let b = NamespaceId::from_entropy("propmgmt", entropy);
        assert_eq!(a, b,
            "Same entropy → same NamespaceId. Per spec §4.1 deterministic from installation.");
    }

    #[test]
    fn from_entropy_differs_on_different_seeds() {
        let a = NamespaceId::from_entropy("propmgmt", b"seed-a");
        let b = NamespaceId::from_entropy("propmgmt", b"seed-b");
        assert_ne!(a.uuid, b.uuid);
    }

    #[test]
    fn fresh_namespaces_are_distinct() {
        let a = NamespaceId::fresh("test");
        let b = NamespaceId::fresh("test");
        assert_ne!(a.uuid, b.uuid);
    }

    #[test]
    fn causal_position_carries_three_components() {
        use crate::temporal::LamportClock;
        let mut clock = LamportClock::new();
        let lamport = clock.tick();
        let instant = FabricInstant::now();
        let ns = NamespaceId::fresh("nisaba");

        let pos = CausalPosition::new(lamport, instant, ns.clone());
        assert_eq!(pos.lamport, lamport);
        assert_eq!(pos.namespace, ns);
        assert!(pos.instant.elapsed_secs() >= 0.0);
    }
}
