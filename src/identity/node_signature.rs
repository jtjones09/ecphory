// NODE SIGNATURE — Selective per-node signing for high-sensitivity regions (Spec 5 §3.3)
//
// Per spec §3.3:
// > Nodes in regions marked `sensitivity: high` carry a per-node signature.
// > The signature is verified when the node is first observed by an agent
// > outside the creating agent's session. Cost: ~50µs per signed node creation
// > + ~100µs per first-read verification. This cost is paid only in
// > high-sensitivity regions.
//
// Required when target region is High; rejected with `WriteError::SignatureRequired`
// if absent. Normal regions never require this — content_fingerprint alone
// suffices, and trust comes from the immune system (Spec 6).

use crate::identity::content_fingerprint::ContentFingerprint;
use crate::identity::edit_mode::EditMode;
use crate::identity::voice_print::{AgentKeypair, VoicePrint};
use crate::signature::LineageId;
use ed25519_dalek::{Signature as Ed25519Signature, Verifier};

/// A per-node Ed25519 signature attached to high-sensitivity nodes.
#[derive(Debug, Clone)]
pub struct NodeSignature {
    /// Voice print of the signing agent.
    pub signer_voice: VoicePrint,
    /// The fingerprint that was signed (must match the node's content_fingerprint).
    pub content_fingerprint: ContentFingerprint,
    /// Ed25519 signature over `content_fingerprint`.
    pub signature: Ed25519Signature,
}

impl NodeSignature {
    /// Sign a content fingerprint with the given agent keypair.
    pub fn sign(keypair: &AgentKeypair, content_fingerprint: ContentFingerprint) -> Self {
        let signature = keypair.sign(content_fingerprint.as_bytes());
        Self {
            signer_voice: keypair.voice_print(),
            content_fingerprint,
            signature,
        }
    }

    /// Verify the signature against the stored fingerprint.
    /// Returns true iff the signature is valid AND the fingerprint matches
    /// the supplied content.
    pub fn verify(&self, content: &[u8]) -> bool {
        if !self.content_fingerprint.verify(content) {
            return false;
        }
        let vk = match self.signer_voice.to_verifying_key() {
            Some(vk) => vk,
            None => return false,
        };
        vk.verify(self.content_fingerprint.as_bytes(), &self.signature).is_ok()
    }

    /// Verify only that the signature itself is valid for the given fingerprint
    /// (without re-checking content). Used during first-read verification when
    /// the fabric has already content-fingerprinted the bytes.
    pub fn verify_signature_only(&self) -> bool {
        let vk = match self.signer_voice.to_verifying_key() {
            Some(vk) => vk,
            None => return false,
        };
        vk.verify(self.content_fingerprint.as_bytes(), &self.signature).is_ok()
    }
}

/// Errors emitted by the fabric's write paths (Spec 5 §3.3, Spec 8 §7).
#[derive(Debug, Clone, PartialEq)]
pub enum WriteError {
    /// Target region is `sensitivity: high` but no signer was provided.
    SignatureRequired,
    /// A signer was provided but its signature is invalid (or doesn't match
    /// the node's content fingerprint).
    InvalidSignature,
    /// The named namespace has not been registered with the fabric.
    UnknownNamespace,
    /// Mechanical edit contention (Spec 8 §3.2.2). Another agent holds
    /// the per-node lock on the target. Caller retries.
    NodeLocked {
        by: VoicePrint,
        /// Wall-clock ns at which the lock-holder's 500ms deadline elapses.
        /// Hint to the caller — they may retry sooner.
        until_ns: i128,
    },
    /// Semantic edit checkout TTL expired before finalization (Spec 8 §3.4.1).
    CheckoutExpired { checkout: LineageId },
    /// The target's recorded `EditMode` doesn't match the operation.
    /// Example: `edit_mechanical` invoked on a node tagged `Semantic`.
    EditModeMismatch { expected: EditMode, got: &'static str },
    /// Snapshot transition is in progress (Spec 8 §3.4.3 atomic
    /// `SnapshotLock`). New checkouts must retry after a bounded delay.
    SnapshotInProgress,
    /// Backpressure on the snapshot/persistence queue (Spec 8 §2.6.3).
    FabricCongested,
    /// Fabric is in degraded mode after a caught panic (Spec 8 §2.6.4).
    /// Reads continue; writes are refused until restart.
    FabricDegraded,
    /// A panic was caught at the trait boundary (Spec 8 §2.6.4).
    FabricInternal(String),
    /// The referenced node is not in the fabric.
    NodeNotFound(LineageId),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::SignatureRequired => write!(
                f,
                "high-sensitivity region requires a NodeSignature; none provided"
            ),
            WriteError::InvalidSignature => {
                write!(f, "provided signature failed verification")
            }
            WriteError::UnknownNamespace => write!(f, "namespace not registered with fabric"),
            WriteError::NodeLocked { by, until_ns } => write!(
                f,
                "node locked by {} until {}ns (wall-clock); retry after backoff",
                by, until_ns
            ),
            WriteError::CheckoutExpired { checkout } => {
                write!(f, "checkout {} expired before finalization", checkout)
            }
            WriteError::EditModeMismatch { expected, got } => write!(
                f,
                "edit mode mismatch: target is {:?}, operation requires {}",
                expected, got
            ),
            WriteError::SnapshotInProgress => write!(
                f,
                "consensus snapshot in progress; retry after bounded delay (default 100ms)"
            ),
            WriteError::FabricCongested => {
                write!(f, "fabric snapshot queue is full; retry after backpressure clears")
            }
            WriteError::FabricDegraded => write!(
                f,
                "fabric is in degraded mode (post-panic); reads available, writes refused"
            ),
            WriteError::FabricInternal(reason) => {
                write!(f, "fabric internal error: {}", reason)
            }
            WriteError::NodeNotFound(id) => write!(f, "node not found: {}", id),
        }
    }
}

impl std::error::Error for WriteError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::voice_print::generate_agent_keypair;

    #[test]
    fn signature_round_trip_verifies() {
        let kp = generate_agent_keypair();
        let content = b"some node content";
        let fp = ContentFingerprint::compute(content);

        let sig = NodeSignature::sign(&kp, fp);
        assert!(sig.verify(content));
    }

    #[test]
    fn signature_fails_on_modified_content() {
        let kp = generate_agent_keypair();
        let content = b"original content";
        let fp = ContentFingerprint::compute(content);
        let sig = NodeSignature::sign(&kp, fp);

        assert!(!sig.verify(b"tampered content"));
    }

    #[test]
    fn signature_fails_against_wrong_voice() {
        let kp_alice = generate_agent_keypair();
        let kp_mallory = generate_agent_keypair();
        let content = b"important data";
        let fp = ContentFingerprint::compute(content);

        // Alice signs.
        let mut sig = NodeSignature::sign(&kp_alice, fp);
        // Mallory tries to claim it.
        sig.signer_voice = kp_mallory.voice_print();

        assert!(!sig.verify(content),
            "A signature must not verify under a different voice print.");
    }

    #[test]
    fn signer_voice_is_creator_voice() {
        let kp = generate_agent_keypair();
        let fp = ContentFingerprint::compute(b"x");
        let sig = NodeSignature::sign(&kp, fp);
        assert_eq!(sig.signer_voice, kp.voice_print());
    }
}
