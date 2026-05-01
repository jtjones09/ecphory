// CONTENT FINGERPRINT — BLAKE3-256 of canonical content bytes (Spec 5 §3.1)
//
// Like a particle's mass: an intrinsic, observer-independent property
// computed from the content itself. Every node has one. Computed at
// creation. Stored permanently. Violation of `BLAKE3(content) == fingerprint`
// is a `DamageObservation` — evidence of corruption or tampering.
//
// Cost: ~1µs per node. This is "always on, intrinsic" (Spec 5 §3.1).

/// BLAKE3-256 fingerprint of a node's canonical content bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentFingerprint(pub [u8; 32]);

impl ContentFingerprint {
    /// Compute a content fingerprint over arbitrary bytes.
    pub fn compute(content: &[u8]) -> Self {
        Self(blake3::hash(content).into())
    }

    /// Verify that the fingerprint matches the given content.
    /// Returns false if the content has been modified.
    pub fn verify(&self, content: &[u8]) -> bool {
        self.0 == *blake3::hash(content).as_bytes()
    }

    /// Raw 32-byte fingerprint.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex display of the full fingerprint.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

impl std::fmt::Display for ContentFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hex = self.to_hex();
        write!(f, "blake3:{}…{}", &hex[..6], &hex[58..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_produces_identical_fingerprint() {
        let a = ContentFingerprint::compute(b"hello world");
        let b = ContentFingerprint::compute(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn different_content_produces_different_fingerprint() {
        let a = ContentFingerprint::compute(b"hello world");
        let b = ContentFingerprint::compute(b"hello worlD");
        assert_ne!(a, b);
    }

    #[test]
    fn verify_succeeds_on_unmodified_content() {
        let content = b"the quick brown fox";
        let fp = ContentFingerprint::compute(content);
        assert!(fp.verify(content));
    }

    #[test]
    fn verify_fails_on_modified_content() {
        let original = b"the quick brown fox";
        let modified = b"the quick brown FOX";
        let fp = ContentFingerprint::compute(original);
        assert!(!fp.verify(modified),
            "Modified content must not verify against the original fingerprint — \
             this is the DamageObservation trigger.");
    }

    #[test]
    fn fingerprint_is_32_bytes() {
        let fp = ContentFingerprint::compute(b"any content");
        assert_eq!(fp.as_bytes().len(), 32);
    }

    #[test]
    fn empty_content_still_fingerprints() {
        let fp = ContentFingerprint::compute(b"");
        // BLAKE3 of empty input is a well-known value; just confirm it's non-zero.
        assert_ne!(fp.0, [0u8; 32]);
    }

    #[test]
    fn hex_representation_is_64_chars() {
        let fp = ContentFingerprint::compute(b"x");
        assert_eq!(fp.to_hex().len(), 64);
    }
}
