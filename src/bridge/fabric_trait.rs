// FABRIC TRAIT — Spec 8 §7 surface
//
// The trait surface Nabu and downstream consumers (Spec 6 immune system,
// Spec 7 agent comms, team-node) call into. The concrete implementation
// is `BridgeFabric` (this module). Subscriptions, P53, and decay tick
// methods are present in the trait but their full bodies land in
// Steps 4 (subscriptions) and 6 (P53) — for now they return
// `WriteError::FabricInternal("not yet implemented in v1")`.

use crate::identity::{
    AgentKeypair, EditMode, NodeIdentity, WriteError,
};
use crate::node::IntentNode;
use crate::signature::LineageId;
use std::time::Duration;

use super::mechanical::EditReceipt;
use super::semantic::{CheckoutHandle, ConsensusSnapshot, ProposalHandle};

/// The Phase F bridge surface (Spec 8 §7). All write methods take
/// `&self` — interior mutability is the implementation's concern.
///
/// v1 status:
/// - `create` (AppendOnly path): implemented.
/// - `edit_mechanical`: implemented (try-lock fail-fast).
/// - `checkout` / `propose` / `finalize_proposal`: implemented with the
///   Spec 8 §3.4.3 atomic SnapshotLock transition.
/// - `subscribe` / `unsubscribe`: Step 4 (next session).
/// - `p53_trigger` / `decay_tick`: Step 6 (next session).
pub trait Fabric: Send + Sync {
    // ── Spec 8 §3.2.1 — append-commutative writes ──

    /// Create a new node tagged with the given `EditMode`. The mode
    /// dictates how subsequent edits may target this node:
    /// - `AppendOnly`: no direct content edits; supersession via edges.
    /// - `Mechanical`: future edits via `edit_mechanical()`.
    /// - `Semantic`: future edits via `checkout` → `propose` → `finalize`.
    ///
    /// `signer` is required for high-sensitivity regions (Spec 5 §3.3);
    /// `None` is fine for normal regions.
    fn create(
        &self,
        content: IntentNode,
        edit_mode: EditMode,
        signer: Option<&AgentKeypair>,
    ) -> Result<LineageId, WriteError>;

    // ── Spec 8 §3.2.2 — mechanical edits ──

    /// Mechanically edit a node's content. Per-node lock with fail-fast
    /// semantics (v1) — caller retries on `WriteError::NodeLocked`.
    /// The target's recorded `EditMode` must be `Mechanical`.
    fn edit_mechanical<F>(
        &self,
        target: &LineageId,
        signer: &AgentKeypair,
        mutation: F,
    ) -> Result<EditReceipt, WriteError>
    where
        F: FnOnce(&mut IntentNode);

    // ── Spec 8 §3.4 — semantic edits ──

    /// Open a checkout on a `Semantic`-mode target node. `ttl` bounds
    /// how long the caller has to call `propose` and
    /// `finalize_proposal` before the checkout dissolves.
    fn checkout(
        &self,
        target: &LineageId,
        rationale: String,
        ttl: Duration,
        signer: &AgentKeypair,
    ) -> Result<CheckoutHandle, WriteError>;

    /// Propose new content for an Open checkout. Replaces any earlier
    /// Draft proposal on the same checkout (predecessor marked
    /// `Superseded`).
    fn propose(
        &self,
        checkout: &LineageId,
        content: IntentNode,
        signer: &AgentKeypair,
    ) -> Result<ProposalHandle, WriteError>;

    /// Finalize a proposal. When this is the last outstanding checkout
    /// on the target, the fabric atomically takes the SnapshotLock and
    /// writes a `ConsensusSnapshot` (Spec 8 §3.4.3). Returns the
    /// snapshot if one was written by this finalize, else `None`.
    fn finalize_proposal(
        &self,
        proposal: &LineageId,
        signer: &AgentKeypair,
    ) -> Result<Option<ConsensusSnapshot>, WriteError>;

    // ── Reads (Spec 8 §5) — minimal surface for v1 ──

    /// Snapshot read of a node by LineageId.
    fn get_node(&self, id: &LineageId) -> Option<IntentNode>;

    /// The four-tuple identity per Spec 8 §4.2.
    fn node_identity(&self, id: &LineageId) -> Option<NodeIdentity>;

    /// What `EditMode` was the node tagged with at create time?
    fn edit_mode_of(&self, id: &LineageId) -> Option<EditMode>;
}
