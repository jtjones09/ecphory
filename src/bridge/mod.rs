// PHASE F BRIDGE — Spec 8 in-process trait surface
//
// Per Spec 8 §2: "Nabu embeds the ecphory crate as a library dependency.
// The fabric runs in-process within Nabu. No subprocess, no socket, no
// IPC. The Fabric trait (defined in §7) is implemented by the ecphory
// crate and consumed by Nabu's MCP handlers, its WebSocket chat, and
// its intent classifier."
//
// Per Jeremy's v1 calls (recorded in `project_spec8_decisions.md`
// memory):
// - **Single `RwLock<FabricState>`** for interior mutability. Don't
//   shard yet; revisit if 1,000-concurrent-creates contention shows.
// - **EditMode is per-call** on `create()`; no `NodeKind` on IntentNode.
//   Concrete callers (property-mgmt, nisaba-on-fabric) drive the
//   node-type system when they arrive.
//
// Layering: this module is *additive* — it wraps the existing
// `crate::fabric::Fabric` rather than mutating it. CLI, persistence,
// distributed and existing tests continue using the inner type. The
// bridge is what Nabu, the immune system (Spec 6), and team-node
// agents will speak to.

pub mod fabric_trait;
pub mod bridge_fabric;
pub mod mechanical;
pub mod semantic;

pub use fabric_trait::Fabric as FabricTrait;
pub use bridge_fabric::BridgeFabric;
pub use mechanical::EditReceipt;
pub use semantic::{
    CheckoutHandle, CheckoutStatus, ConsensusSnapshot, ProposalHandle, ProposalStatus,
    SemanticEditConfig,
};
