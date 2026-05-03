// The Ecphory Fabric — Phase 3: Persistence + Embeddings + Inference + Distributed
//
// This crate defines the atomic primitive (IntentNode) and
// the semantic memory fabric that connects them.
//
// Module structure:
// - signature: Content-addressable identity (Law 1)
// - confidence: Three-dimensional living surface
// - constraint: Boundaries, not instructions (Law 2)
// - context: Semantic resonance edges (Law 6)
// - node: The IntentNode itself — brings it all together
// - temporal: Causal ordering and temporal decay (Law 11)
// - tracer: Fabric observability (DTrace philosophy)
// - fabric: The semantic memory fabric (Law 14: The Fabric IS the Intelligence)
// - persist: Durable fabric state (Phase 3a)
// - embedding: Semantic vector space (Phase 3b)
// - inference: Active inference / node agency (Phase 3c)
// - distributed: Vector clocks, conflict resolution, gossip (Phase 3d)

pub mod signature;
pub mod confidence;
pub mod constraint;
pub mod context;
pub mod identity;
pub mod node;
pub mod temporal;
pub mod tracer;
pub mod fabric;
pub mod bridge;
pub mod comms;
pub mod immune;
pub mod persist;
pub mod embedding;
pub mod inference;
pub mod distributed;
pub mod work;

// Re-export the main types at crate root for convenience
pub use node::{IntentNode, MetadataValue};
pub use signature::{Signature, LineageId};
pub use confidence::{ConfidenceSurface, Distribution};
pub use constraint::{Constraint, ConstraintField, ConstraintKind};
pub use context::{ContextField, ContextEdge, RelationshipKind};
pub use temporal::{LamportClock, LamportTimestamp, FabricInstant};
pub use tracer::{FabricTracer, NoopTracer, TraceEvent};
pub use fabric::{Fabric, FabricError, ResonanceResult};
pub use persist::{FabricStore, JsonFileStore, PersistError};
pub use embedding::{Embedder, cosine_similarity, normalized_cosine};
pub use embedding::bow::BagOfWordsEmbedder;
pub use inference::{FreeEnergy, RPESignal, NodeAction, ConfidenceDimension, InferenceResult, MaintenanceReport, ActionPolicy};
pub use distributed::{
    DistributedFabric, ReceiveResult,
    VectorClock, ReplicaId,
    Conflict, ConflictStrategy, Resolution, resolve_conflict, select_strategy,
    GossipState, GossipMessage, Digest, DigestEntry, NodeTransfer,
    Transport, LocalTransport, Envelope,
};
pub use identity::{
    AgentKeypair, AgentRelation, CausalPosition, ConflictingSignal, ContentFingerprint,
    CrossAttestation, EditMode, FabricQuestion, GenesisCommitment, GenesisEvent, GenesisTuple,
    NamespaceId, NodeIdentity, NodeQuarantineState, NodeSignature, QuarantineReason,
    RegionSensitivity, SignalType, TopologicalPosition, TrustWeight, VoicePrint,
    WitnessType, WriteError, generate_agent_keypair,
};
