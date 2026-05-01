// VOICE PRINT — Ed25519 keypair as agent participation handle (Spec 5 §3.2)
//
// Per Spec 5: "A keypair is like having a voice. Signing is like speaking.
// Verification is like recognizing a voice." The voice print (public key)
// identifies WHO spoke, not whether to trust them — trust is behavioral
// (Spec 6 immune system), not credentialed.
//
// Per spec §3.2:
// - Every agent has an Ed25519 keypair generated at provisioning from
//   ≥128 bits of hardware entropy.
// - Public key → voice print → unique participation handle.
// - Stored on `creator_voice` field of every node the agent creates.
// - NOT verified per-read on normal regions (that's the v1.1 model).
// - Selective signing for high-sensitivity regions only (see node_signature).

use ed25519_dalek::{Signer, SigningKey, VerifyingKey, SECRET_KEY_LENGTH};
use rand_core::OsRng;

/// An agent's voice print — its Ed25519 public key.
///
/// Stored on `IntentNode::creator_voice`. The immune system uses this as
/// a stable handle for tracking an agent's behavioral patterns. It is
/// NOT a credential — trust accumulates through observation (Spec 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoicePrint(pub [u8; 32]);

impl VoicePrint {
    /// Construct from raw 32-byte public key.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Construct from an `ed25519_dalek::VerifyingKey`.
    pub fn from_verifying_key(vk: &VerifyingKey) -> Self {
        Self(vk.to_bytes())
    }

    /// Raw 32-byte public key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert into an `ed25519_dalek::VerifyingKey` for verification.
    /// Returns `None` if the bytes are not a valid point on the curve.
    pub fn to_verifying_key(&self) -> Option<VerifyingKey> {
        VerifyingKey::from_bytes(&self.0).ok()
    }

    /// Hex display of the full key.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

impl std::fmt::Display for VoicePrint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hex = self.to_hex();
        write!(f, "voice:{}", &hex[..16])
    }
}

/// An agent's keypair: signing key (private) + voice print (public).
///
/// Per spec §3.2.1: the private key is stored encrypted at rest in
/// `~/.ecphory/agents/<agent-name>/key.enc`, passphrase in OS keychain.
/// The private key NEVER leaves the host. (Encrypted-at-rest storage
/// is a deployment concern handled outside this struct.)
pub struct AgentKeypair {
    signing_key: SigningKey,
}

impl AgentKeypair {
    /// Wrap an existing signing key.
    pub fn from_signing_key(signing_key: SigningKey) -> Self {
        Self { signing_key }
    }

    /// Reconstruct from a 32-byte secret seed (for persistence, testing).
    pub fn from_secret_bytes(bytes: [u8; SECRET_KEY_LENGTH]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&bytes),
        }
    }

    /// The agent's voice print (public key).
    pub fn voice_print(&self) -> VoicePrint {
        VoicePrint::from_verifying_key(&self.signing_key.verifying_key())
    }

    /// Borrow the underlying signing key (for selective signing).
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// Sign arbitrary bytes — typically a content fingerprint.
    pub fn sign(&self, msg: &[u8]) -> ed25519_dalek::Signature {
        self.signing_key.sign(msg)
    }

    /// Export the 32-byte secret seed (for persistence).
    /// This is sensitive data — handle with care.
    pub fn secret_bytes(&self) -> [u8; SECRET_KEY_LENGTH] {
        self.signing_key.to_bytes()
    }
}

/// Generate a fresh agent keypair from OS hardware entropy.
///
/// Per spec §3.2.1: ≥128 bits from hardware entropy sources. `OsRng`
/// pulls from the OS CSPRNG (`/dev/urandom` on Linux), which combines
/// hardware sources (RDRAND/RDSEED when available), jitter, and
/// pool entropy.
pub fn generate_agent_keypair() -> AgentKeypair {
    let mut csprng = OsRng;
    AgentKeypair::from_signing_key(SigningKey::generate(&mut csprng))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn generate_produces_distinct_keypairs() {
        let a = generate_agent_keypair();
        let b = generate_agent_keypair();
        assert_ne!(a.voice_print(), b.voice_print(),
            "Two fresh keypairs must have distinct voice prints — \
             this is the Pauli-exclusion-style discernibility (Spec 5 §1.2).");
    }

    #[test]
    fn voice_print_is_stable_for_same_keypair() {
        let kp = generate_agent_keypair();
        let vp1 = kp.voice_print();
        let vp2 = kp.voice_print();
        assert_eq!(vp1, vp2);
    }

    #[test]
    fn keypair_can_sign_and_verify() {
        let kp = generate_agent_keypair();
        let msg = b"the quick brown fox";
        let sig = kp.sign(msg);

        let vk = kp.voice_print().to_verifying_key().unwrap();
        assert!(vk.verify(msg, &sig).is_ok());
    }

    #[test]
    fn voice_print_serialization_roundtrip() {
        let kp = generate_agent_keypair();
        let vp = kp.voice_print();
        let bytes = *vp.as_bytes();
        let vp2 = VoicePrint::from_bytes(bytes);
        assert_eq!(vp, vp2);
    }

    #[test]
    fn secret_seed_roundtrip_preserves_voice_print() {
        let kp = generate_agent_keypair();
        let original_vp = kp.voice_print();
        let seed = kp.secret_bytes();

        let kp2 = AgentKeypair::from_secret_bytes(seed);
        assert_eq!(kp2.voice_print(), original_vp,
            "Reconstructing from the secret seed must yield the same voice print.");
    }

    #[test]
    fn voice_print_to_verifying_key_succeeds_for_real_key() {
        let kp = generate_agent_keypair();
        let vp = kp.voice_print();
        assert!(vp.to_verifying_key().is_some());
    }
}
