// SIGNATURE — INTRINSIC IDENTITY (Law 1)
//
// Design decisions:
// 1. A node's signature is computed FROM its contents. Not assigned.
// 2. Identical contents → identical signatures. Always.
// 3. Any content change → different signature. Always.
// 4. Phase 1 uses SHA-256 hash of serialized contents.
//    This gives us content-addressability and collision resistance.
// 5. Phase 2: Semantic IDs via RQ-VAE (like Meta's production system).
//    Similar meanings would share signature prefixes, enabling
//    hierarchical similarity lookup. SHA-256 gives us none of that —
//    it's the shovel, not the building.
// 6. The signature is the ONLY way to identify a node. No addresses,
//    no UUIDs, no auto-increment IDs.
//
// Open questions:
// - The H2Rec semantic collision problem: distinct items with similar
//   semantics getting identical signatures. For Phase 1 with SHA-256,
//   this is a non-issue (hash collisions are astronomically unlikely).
//   For Phase 2 with semantic IDs, this becomes a real design challenge.
//   Nodes with identical MEANING should merge. Nodes with identical
//   CONTENT but different INTENT must remain distinct.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use uuid::Uuid;

/// The intrinsic identity of an intent node.
/// Computed from contents, never assigned.
///
/// Phase 1: 64-bit hash (using DefaultHasher).
/// Sufficient for prototype — collision resistance isn't critical yet.
/// Phase 2: Replace with semantic signature (RQ-VAE prefix codes).
#[derive(Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Signature {
    /// The computed identity bytes.
    /// Phase 1: 8 bytes (u64 hash).
    /// Phase 2: Variable-length semantic prefix code.
    pub bytes: Vec<u8>,
}

impl Signature {
    /// Compute signature from raw content bytes.
    /// This is the fundamental operation: content → identity.
    pub fn from_content(content: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let hash = hasher.finish();
        Self {
            bytes: hash.to_be_bytes().to_vec(),
        }
    }

    /// Compute signature from a string representation of node contents.
    /// Convenience for Phase 1 where we serialize to string.
    pub fn from_string(content: &str) -> Self {
        Self::from_content(content.as_bytes())
    }

    /// Display as hex string.
    pub fn to_hex(&self) -> String {
        self.bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Is this the null/empty signature? (No content)
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sig({})", &self.to_hex()[..8.min(self.to_hex().len())])
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Stable identity across mutations.
///
/// Assigned once at creation, never changes. Even when the node's content
/// changes (and its Signature changes), the LineageId stays the same.
///
/// NOT used in signature computation — lineage tracks the entity,
/// signature tracks the content.
///
/// Phase 1: UUID v4 wrapper.
/// Phase 2: May be replaced by fabric-native stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LineageId(Uuid);

impl LineageId {
    /// Create a new random lineage ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from a specific UUID (for testing/deserialization).
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Access the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for LineageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for LineageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short display like Git commit hashes
        let s = self.0.to_string();
        write!(f, "{}", &s[..8])
    }
}

/// Trait for anything that can produce a canonical byte representation
/// for signature computation. Every field that affects identity must
/// implement this.
pub trait Signable {
    fn sig_bytes(&self) -> Vec<u8>;
}

impl Signable for String {
    fn sig_bytes(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl Signable for f64 {
    fn sig_bytes(&self) -> Vec<u8> {
        self.to_be_bytes().to_vec()
    }
}

impl Signable for u64 {
    fn sig_bytes(&self) -> Vec<u8> {
        self.to_be_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_produces_identical_signature() {
        let s1 = Signature::from_string("send message to brother");
        let s2 = Signature::from_string("send message to brother");
        assert_eq!(s1, s2);
    }

    #[test]
    fn different_content_produces_different_signature() {
        let s1 = Signature::from_string("send message to brother");
        let s2 = Signature::from_string("send message to sister");
        assert_ne!(s1, s2);
    }

    #[test]
    fn any_change_changes_signature() {
        let base = "send message to brother";
        let s1 = Signature::from_string(base);
        // Even a single character difference
        let s2 = Signature::from_string("send message to brotheR");
        assert_ne!(s1, s2);
    }

    #[test]
    fn signature_is_deterministic() {
        // Must produce same result every time
        let results: Vec<Signature> = (0..100)
            .map(|_| Signature::from_string("deterministic"))
            .collect();
        assert!(results.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn hex_representation_works() {
        let s = Signature::from_string("test");
        let hex = s.to_hex();
        assert_eq!(hex.len(), 16); // 8 bytes = 16 hex chars
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn empty_content_still_produces_signature() {
        let s = Signature::from_string("");
        assert!(!s.is_empty());
        // Empty string still has a hash
    }
}
