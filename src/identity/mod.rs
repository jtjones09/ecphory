// IDENTITY THROUGH INTRINSIC PROPERTIES — Spec 5 v2.1
//
// This module implements the identity primitives for the Ecphory fabric.
// Per Spec 5, identity emerges from intrinsic properties and behavioral
// observation — no certificate authorities, no cert lifecycle, no CRL.
//
// Layers:
// - ContentFingerprint     (BLAKE3-256 of canonical content bytes — "mass")
// - CausalPosition         (Lamport + FabricInstant + NamespaceId — "worldline")
// - VoicePrint             (Ed25519 public key — "voice / charge")
// - GenesisEvent           (instance birth certificate — "Big Bang")
// - NodeSignature          (selective per-node signing for high-sensitivity regions)
// - TrustWeight            (behavioral trust accumulation, half-life decay)
// - CrossAttestation       (multi-host agent identity)
// - NodeQuarantineState    (autoimmune resolution mechanism)

pub mod content_fingerprint;
pub mod voice_print;
pub mod causal_position;
pub mod edit_mode;
pub mod genesis;
pub mod node_identity;
pub mod node_signature;
pub mod trust_weight;
pub mod cross_attestation;
pub mod quarantine;
pub mod region;

pub use content_fingerprint::ContentFingerprint;
pub use voice_print::{generate_agent_keypair, AgentKeypair, VoicePrint};
pub use causal_position::{CausalPosition, NamespaceId};
pub use edit_mode::EditMode;
pub use genesis::{GenesisCommitment, GenesisEvent, GenesisTuple, WitnessType};
pub use node_identity::{NodeIdentity, TopologicalPosition};
pub use node_signature::{NodeSignature, WriteError};
pub use trust_weight::TrustWeight;
pub use cross_attestation::{AgentRelation, CrossAttestation};
pub use quarantine::{
    ConflictingSignal, FabricQuestion, NodeQuarantineState, QuarantineReason,
    SignalType,
};
pub use region::RegionSensitivity;
