// GENESIS EVENT — The fabric's "Big Bang" (Spec 5 §4)
//
// Per spec §4: "When an Ecphory instance is created, the genesis event
// produces the initial conditions from which all subsequent identity
// emerges. Like the Big Bang producing fundamental forces and initial
// particle distributions, the genesis produces the minimum viable fabric
// state."
//
// What genesis creates:
// 1. The instance's genesis tuple: (pk, h_genesis, CID_state, lineage_parent)
//    — the fabric's birth certificate, self-certifying from intrinsic
//    properties.
// 2. The root namespace.
// 3. The first agent's behavioral seed.
// 4. The initial region topology.
// 5. The immune system's baseline training period (1 hour default).

use std::time::Duration;

use crate::identity::causal_position::NamespaceId;
use crate::identity::content_fingerprint::ContentFingerprint;
use crate::identity::voice_print::VoicePrint;
use crate::temporal::FabricInstant;
use ed25519_dalek::Signature as Ed25519Signature;

/// A `GenesisCommitment` anchors the genesis to an external, publicly
/// witnessable event so the instance's birth time is verifiable.
#[derive(Debug, Clone)]
pub struct GenesisCommitment {
    pub witness_type: WitnessType,
    /// The external event data (e.g., serialized Bitcoin block header).
    pub witness_data: Vec<u8>,
    /// `BLAKE3(witness_data)` — the `h_genesis` field of the genesis tuple.
    pub witness_hash: ContentFingerprint,
}

impl GenesisCommitment {
    /// Build a commitment by hashing the supplied witness data.
    pub fn new(witness_type: WitnessType, witness_data: Vec<u8>) -> Self {
        let witness_hash = ContentFingerprint::compute(&witness_data);
        Self { witness_type, witness_data, witness_hash }
    }

    /// Verify that `witness_hash` matches `BLAKE3(witness_data)`.
    pub fn verify(&self) -> bool {
        self.witness_hash.verify(&self.witness_data)
    }
}

/// What kind of external event the genesis is anchored to.
///
/// v1 ships with `ManualTimestamp` (operator signs a timestamp at install
/// time). Bitcoin block headers and drand beacons are options for stronger
/// public witnessability.
#[derive(Debug, Clone)]
pub enum WitnessType {
    BitcoinBlockHeader { height: u64 },
    DrandBeacon { round: u64 },
    ManualTimestamp {
        operator_pk: VoicePrint,
        signed_timestamp: Ed25519Signature,
    },
}

/// The four-component identity tuple of an Ecphory instance.
///
/// Per spec §2.1.3:
/// - `instance_pk`: Ed25519 public key — "charge", invariant
/// - `h_genesis`: hash commitment to an external witnessable event — "birth moment"
/// - `state_root`: BLAKE3 Merkle root of initial code + state — "mass"
/// - `lineage_parent`: `None` for first installations; recursive for forks
#[derive(Debug, Clone)]
pub struct GenesisTuple {
    pub instance_pk: VoicePrint,
    pub h_genesis: ContentFingerprint,
    pub state_root: ContentFingerprint,
    pub lineage_parent: Option<Box<GenesisTuple>>,
}

/// The genesis event itself — written as the first node in the fabric.
#[derive(Debug, Clone)]
pub struct GenesisEvent {
    pub instance_pk: VoicePrint,
    pub genesis_commitment: GenesisCommitment,
    pub state_root: ContentFingerprint,
    pub lineage_parent: Option<GenesisTuple>,
    pub training_started: FabricInstant,
    /// Default 1 hour — see spec §4.2 maternal immunity.
    pub training_duration: Duration,
    pub initial_regions: Vec<NamespaceId>,
    /// Voice prints of agents provisioned at genesis.
    pub initial_agents: Vec<VoicePrint>,
}

impl GenesisEvent {
    /// Default training period: 1 hour (spec §4.3, §5.5.5 derivation).
    pub const DEFAULT_TRAINING_DURATION: Duration = Duration::from_secs(3600);

    /// Build a genesis event.
    ///
    /// Per spec §4: the operator supplies the instance keypair, the witness
    /// commitment, the state root over the source tree at install commit,
    /// and the initial set of regions and agents.
    pub fn new(
        instance_pk: VoicePrint,
        genesis_commitment: GenesisCommitment,
        state_root: ContentFingerprint,
        initial_regions: Vec<NamespaceId>,
        initial_agents: Vec<VoicePrint>,
    ) -> Self {
        Self {
            instance_pk,
            genesis_commitment,
            state_root,
            lineage_parent: None,
            training_started: FabricInstant::now(),
            training_duration: Self::DEFAULT_TRAINING_DURATION,
            initial_regions,
            initial_agents,
        }
    }

    /// Build a fork-genesis event, recording the parent tuple.
    pub fn fork_from(
        parent: GenesisTuple,
        instance_pk: VoicePrint,
        genesis_commitment: GenesisCommitment,
        state_root: ContentFingerprint,
        initial_regions: Vec<NamespaceId>,
        initial_agents: Vec<VoicePrint>,
    ) -> Self {
        Self {
            instance_pk,
            genesis_commitment,
            state_root,
            lineage_parent: Some(parent),
            training_started: FabricInstant::now(),
            training_duration: Self::DEFAULT_TRAINING_DURATION,
            initial_regions,
            initial_agents,
        }
    }

    /// Project this event into its four-component genesis tuple.
    pub fn tuple(&self) -> GenesisTuple {
        GenesisTuple {
            instance_pk: self.instance_pk,
            h_genesis: self.genesis_commitment.witness_hash,
            state_root: self.state_root,
            lineage_parent: self.lineage_parent.clone().map(Box::new),
        }
    }

    /// Has the maternal-immunity training period elapsed?
    /// Per spec §4.2, after this, the immune system's own learned baselines
    /// take over.
    pub fn training_complete(&self) -> bool {
        self.training_started.elapsed_secs() >= self.training_duration.as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::voice_print::generate_agent_keypair;

    fn fresh_commitment() -> GenesisCommitment {
        let kp = generate_agent_keypair();
        let timestamp = b"2026-04-30T00:00:00Z";
        let signature = kp.sign(timestamp);
        GenesisCommitment::new(
            WitnessType::ManualTimestamp {
                operator_pk: kp.voice_print(),
                signed_timestamp: signature,
            },
            timestamp.to_vec(),
        )
    }

    #[test]
    fn commitment_self_verifies() {
        let c = fresh_commitment();
        assert!(c.verify());
    }

    #[test]
    fn commitment_fails_if_data_tampered() {
        let mut c = fresh_commitment();
        c.witness_data[0] ^= 0xff;
        assert!(!c.verify(),
            "A tampered witness_data must not verify against the stored hash.");
    }

    #[test]
    fn genesis_event_produces_complete_tuple() {
        let instance_kp = generate_agent_keypair();
        let agent_kp = generate_agent_keypair();
        let region = NamespaceId::fresh("propmgmt");
        let state_root = ContentFingerprint::compute(b"initial state bytes");

        let event = GenesisEvent::new(
            instance_kp.voice_print(),
            fresh_commitment(),
            state_root,
            vec![region.clone()],
            vec![agent_kp.voice_print()],
        );

        let tuple = event.tuple();
        assert_eq!(tuple.instance_pk, instance_kp.voice_print(),
            "Genesis tuple must include instance public key (Spec 5 §2.1.3).");
        assert_eq!(tuple.state_root, state_root);
        assert!(tuple.lineage_parent.is_none(),
            "First installations have no parent (Spec 5 §2.1.3).");
        assert_eq!(event.initial_regions.len(), 1);
        assert_eq!(event.initial_regions[0], region);
        assert_eq!(event.initial_agents.len(), 1);
    }

    #[test]
    fn fork_records_parent_lineage() {
        let parent_kp = generate_agent_keypair();
        let parent_event = GenesisEvent::new(
            parent_kp.voice_print(),
            fresh_commitment(),
            ContentFingerprint::compute(b"parent state"),
            vec![],
            vec![],
        );
        let parent_tuple = parent_event.tuple();

        let child_kp = generate_agent_keypair();
        let child = GenesisEvent::fork_from(
            parent_tuple.clone(),
            child_kp.voice_print(),
            fresh_commitment(),
            ContentFingerprint::compute(b"parent state"), // same code, different lineage
            vec![],
            vec![],
        );

        let child_tuple = child.tuple();
        let lineage_parent = child_tuple.lineage_parent.expect("fork must record parent");
        assert_eq!(lineage_parent.instance_pk, parent_tuple.instance_pk);
        assert_ne!(child_tuple.instance_pk, parent_tuple.instance_pk,
            "A fork has its own instance public key — the divergence is identity.");
    }

    #[test]
    fn default_training_duration_is_one_hour() {
        let kp = generate_agent_keypair();
        let event = GenesisEvent::new(
            kp.voice_print(),
            fresh_commitment(),
            ContentFingerprint::compute(b""),
            vec![],
            vec![],
        );
        assert_eq!(event.training_duration, Duration::from_secs(3600));
    }
}
