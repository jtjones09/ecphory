// CROSS-ATTESTATION — Multi-host agent identity (Spec 5 §3.2.2)
//
// Per spec §3.2.2:
// > An agent that operates on multiple hosts gets a fresh keypair on each
// > host. The keypairs are connected as belonging to the same logical agent
// > through cross-attestation: the agent's keypair on host A signs a
// > CrossAttestation node recognizing keypair B as the same logical agent,
// > and vice versa.
//
// The agent's identity is its pattern of behavior across all hosts. The
// keypairs are participation handles, not the identity itself.

use crate::identity::voice_print::{AgentKeypair, VoicePrint};
use ed25519_dalek::{Signature as Ed25519Signature, Verifier};

/// What kind of relationship a cross-attestation asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRelation {
    /// Same logical agent on a different host (e.g., Nabu on Enki and Nexus).
    SameAgent,
    /// Different agent, mutually trusted.
    Trusted,
    /// Known to this agent through observation, no mutual trust.
    Observed,
}

/// A signed assertion linking two voice prints.
#[derive(Debug, Clone)]
pub struct CrossAttestation {
    /// "I, agent on host A..."
    pub attester_pk: VoicePrint,
    /// "...recognize this key on host B..."
    pub attested_pk: VoicePrint,
    /// "...as having this relationship."
    pub relationship: AgentRelation,
    /// Ed25519 signature by the attester over (attested_pk || relationship_tag).
    pub signature: Ed25519Signature,
}

impl CrossAttestation {
    /// Bytes signed by the attester. Including the relationship tag in the
    /// signed bytes prevents an attacker from re-labeling a `SameAgent`
    /// attestation as `Trusted`.
    fn signed_bytes(attested_pk: &VoicePrint, relationship: AgentRelation) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(33);
        bytes.extend_from_slice(attested_pk.as_bytes());
        bytes.push(match relationship {
            AgentRelation::SameAgent => 0x01,
            AgentRelation::Trusted => 0x02,
            AgentRelation::Observed => 0x03,
        });
        bytes
    }

    /// Build an attestation: `attester` recognizes `attested_pk` as `relationship`.
    pub fn new(
        attester: &AgentKeypair,
        attested_pk: VoicePrint,
        relationship: AgentRelation,
    ) -> Self {
        let bytes = Self::signed_bytes(&attested_pk, relationship);
        let signature = attester.sign(&bytes);
        Self {
            attester_pk: attester.voice_print(),
            attested_pk,
            relationship,
            signature,
        }
    }

    /// Verify that this attestation was actually produced by `attester_pk`
    /// over (attested_pk, relationship).
    pub fn verify(&self) -> bool {
        let vk = match self.attester_pk.to_verifying_key() {
            Some(vk) => vk,
            None => return false,
        };
        let bytes = Self::signed_bytes(&self.attested_pk, self.relationship);
        vk.verify(&bytes, &self.signature).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::voice_print::generate_agent_keypair;

    #[test]
    fn mutual_same_agent_attestation_links_keypairs() {
        // Agent on host A and host B — different keypairs, same logical agent.
        let host_a = generate_agent_keypair();
        let host_b = generate_agent_keypair();

        let a_recognizes_b = CrossAttestation::new(
            &host_a,
            host_b.voice_print(),
            AgentRelation::SameAgent,
        );
        let b_recognizes_a = CrossAttestation::new(
            &host_b,
            host_a.voice_print(),
            AgentRelation::SameAgent,
        );

        assert!(a_recognizes_b.verify());
        assert!(b_recognizes_a.verify());

        // The attestations link the two voice prints in both directions.
        assert_eq!(a_recognizes_b.attester_pk, host_a.voice_print());
        assert_eq!(a_recognizes_b.attested_pk, host_b.voice_print());
        assert_eq!(b_recognizes_a.attester_pk, host_b.voice_print());
        assert_eq!(b_recognizes_a.attested_pk, host_a.voice_print());
    }

    #[test]
    fn relationship_is_part_of_signed_bytes() {
        let host_a = generate_agent_keypair();
        let host_b = generate_agent_keypair();

        let mut att = CrossAttestation::new(
            &host_a,
            host_b.voice_print(),
            AgentRelation::SameAgent,
        );
        // Attacker re-labels the relationship without re-signing.
        att.relationship = AgentRelation::Trusted;
        assert!(!att.verify(),
            "Re-labeling the relationship must invalidate the signature \
             (Spec 5 §3.2.2: tag bound into signed bytes).");
    }

    #[test]
    fn forged_attester_pk_fails_verification() {
        let host_a = generate_agent_keypair();
        let host_b = generate_agent_keypair();
        let mallory = generate_agent_keypair();

        let mut att = CrossAttestation::new(
            &host_a,
            host_b.voice_print(),
            AgentRelation::SameAgent,
        );
        // Mallory tries to impersonate as the attester.
        att.attester_pk = mallory.voice_print();
        assert!(!att.verify());
    }

    #[test]
    fn observed_and_trusted_relationships_round_trip() {
        let host_a = generate_agent_keypair();
        let host_b = generate_agent_keypair();

        for rel in [AgentRelation::Trusted, AgentRelation::Observed] {
            let att = CrossAttestation::new(&host_a, host_b.voice_print(), rel);
            assert!(att.verify());
            assert_eq!(att.relationship, rel);
        }
    }
}
