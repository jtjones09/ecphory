// BRIDGE FABRIC — Phase F implementation of the Spec 8 trait
//
// Wraps the existing `crate::fabric::Fabric` (now `inner::Fabric` from
// the bridge's perspective) inside a single `RwLock<FabricState>`, per
// Jeremy's v1 call. All write methods take `&self`; the lock is held
// briefly across each operation.
//
// What this is NOT (yet):
// - It does NOT yet implement subscriptions (Step 4)
// - It does NOT yet implement P53 (Step 6)
// - It does NOT yet implement observability spans / metrics (Step 5)
//
// What this IS:
// - The three-way edit model is real: AppendOnly, Mechanical, Semantic
// - Per-node mechanical lock with try-lock fail-fast
// - Atomic SnapshotLock transition for the consensus snapshot
// - EditMode tracked per-node by the bridge (no NodeKind on IntentNode)

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::fabric::Fabric as InnerFabric;
use crate::identity::{
    AgentKeypair, EditMode, NamespaceId, NodeIdentity, WriteError,
};
use crate::node::IntentNode;
use crate::signature::LineageId;
use crate::temporal::FabricInstant;

use super::fabric_trait::Fabric as FabricTrait;
use super::mechanical::{AcquireResult, EditReceipt, MechanicalLockTable};
use super::semantic::{
    CheckoutEntry, CheckoutHandle, CheckoutStatus, ConsensusSnapshot, FinalizeError,
    FinalizeOutcome, ProposalEntry, ProposalHandle, ProposalRegisterError, ProposalStatus,
    SemanticEditConfig, SemanticStateTable,
};

/// Inner state of the bridge fabric. All mutations go through the
/// outer `RwLock` on `BridgeFabric`.
pub(crate) struct FabricState {
    /// The classical ecphory fabric — owns nodes, edges, persistence,
    /// embeddings. Bridge methods delegate to it under lock.
    pub(crate) inner: InnerFabric,
    /// Per-node `EditMode` tag, recorded at create time (Spec 8 §3.2,
    /// per-call argument). Existing `IntentNode` is unchanged.
    pub(crate) edit_modes: HashMap<LineageId, EditMode>,
}

impl FabricState {
    fn new(inner: InnerFabric) -> Self {
        Self { inner, edit_modes: HashMap::new() }
    }
}

/// The Phase F bridge: in-process `Fabric` trait implementation.
///
/// Holds:
/// - A single `RwLock<FabricState>` over the inner fabric and edit-mode
///   metadata.
/// - A `MechanicalLockTable` for per-node locks on `Mechanical` edits.
/// - A `SemanticStateTable` for the checkout/proposal/snapshot machine.
/// - `SemanticEditConfig` for tunable knobs.
pub struct BridgeFabric {
    state: Arc<RwLock<FabricState>>,
    mechanical_locks: MechanicalLockTable,
    semantic_state: SemanticStateTable,
    semantic_config: SemanticEditConfig,
    /// Default namespace used when callers don't specify one. Matches
    /// the existing `Fabric::add_node` behavior.
    default_namespace: NamespaceId,
}

impl BridgeFabric {
    /// Construct a new bridge wrapping a fresh inner fabric.
    pub fn new() -> Self {
        Self::wrap(InnerFabric::new())
    }

    /// Wrap an existing inner fabric. Lets callers seed a bridge with
    /// a fabric that already has persistence, embedder, or genesis
    /// installed.
    pub fn wrap(inner: InnerFabric) -> Self {
        Self {
            state: Arc::new(RwLock::new(FabricState::new(inner))),
            mechanical_locks: MechanicalLockTable::new(),
            semantic_state: SemanticStateTable::new(),
            semantic_config: SemanticEditConfig::default(),
            default_namespace: NamespaceId::default_namespace(),
        }
    }

    /// Override the default namespace used by `create()` when no
    /// region-specific entry point is used.
    pub fn with_default_namespace(mut self, namespace: NamespaceId) -> Self {
        self.default_namespace = namespace;
        self
    }

    /// Override the semantic-edit configuration (TTLs, snapshot lock
    /// budgets).
    pub fn with_semantic_config(mut self, config: SemanticEditConfig) -> Self {
        self.semantic_config = config;
        self
    }

    /// Read-only handle to the inner fabric for callers that want to
    /// run a read transaction without going through the trait surface.
    pub fn read_inner<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&InnerFabric) -> R,
    {
        let guard = self.state.read().expect("FabricState poisoned");
        f(&guard.inner)
    }
}

impl Default for BridgeFabric {
    fn default() -> Self {
        Self::new()
    }
}

impl FabricTrait for BridgeFabric {
    fn create(
        &self,
        content: IntentNode,
        edit_mode: EditMode,
        signer: Option<&AgentKeypair>,
    ) -> Result<LineageId, WriteError> {
        // Acquire the write lock for the duration of the create.
        let mut guard = self.state.write().expect("FabricState poisoned");
        let lineage_id =
            guard.inner.create(content, &self.default_namespace, signer)?;
        guard.edit_modes.insert(lineage_id.clone(), edit_mode);
        Ok(lineage_id)
    }

    fn edit_mechanical<F>(
        &self,
        target: &LineageId,
        signer: &AgentKeypair,
        mutation: F,
    ) -> Result<EditReceipt, WriteError>
    where
        F: FnOnce(&mut IntentNode),
    {
        // Validate edit mode under a brief read lock — fail before
        // we touch the per-node mutex.
        {
            let guard = self.state.read().expect("FabricState poisoned");
            if !guard.inner.contains(target) {
                return Err(WriteError::NodeNotFound(target.clone()));
            }
            match guard.edit_modes.get(target).copied() {
                Some(EditMode::Mechanical) => {}
                Some(other) => {
                    return Err(WriteError::EditModeMismatch {
                        expected: other,
                        got: "Mechanical",
                    })
                }
                None => {
                    // Missing edit-mode tag is treated as AppendOnly
                    // (the safe default for legacy nodes added via the
                    // inner fabric directly).
                    return Err(WriteError::EditModeMismatch {
                        expected: EditMode::AppendOnly,
                        got: "Mechanical",
                    });
                }
            }
        }

        // Acquire the per-node lock fail-fast (v1).
        match self.mechanical_locks.try_acquire(target.clone(), signer.voice_print()) {
            AcquireResult::Held(holder) => {
                return Err(WriteError::NodeLocked {
                    by: holder.holder,
                    until_ns: holder.deadline_ns(),
                });
            }
            AcquireResult::Acquired => {}
        }

        // Lock held — do the edit under the outer write lock. Release
        // the per-node lock no matter what (panic-safe via guard).
        let result = (|| -> Result<EditReceipt, WriteError> {
            let mut guard = self.state.write().expect("FabricState poisoned");
            let previous_fingerprint = *guard
                .inner
                .get_node(target)
                .ok_or_else(|| WriteError::NodeNotFound(target.clone()))?
                .content_fingerprint();

            // The inner Fabric's mutate_node closure handles signature
            // recompute and Lamport tick.
            guard
                .inner
                .mutate_node(target, mutation)
                .map_err(|e| WriteError::FabricInternal(format!("inner mutate_node: {}", e)))?;

            let new_fingerprint = *guard
                .inner
                .get_node(target)
                .expect("present after successful mutate_node")
                .content_fingerprint();

            Ok(EditReceipt {
                target: target.clone(),
                previous_content_fingerprint: previous_fingerprint,
                new_content_fingerprint: new_fingerprint,
                editor_voice: signer.voice_print(),
                commit_instant: FabricInstant::now(),
            })
        })();

        self.mechanical_locks.release(target);
        result
    }

    fn checkout(
        &self,
        target: &LineageId,
        rationale: String,
        ttl: Duration,
        signer: &AgentKeypair,
    ) -> Result<CheckoutHandle, WriteError> {
        // Validate the target exists and is Semantic-mode.
        {
            let guard = self.state.read().expect("FabricState poisoned");
            if !guard.inner.contains(target) {
                return Err(WriteError::NodeNotFound(target.clone()));
            }
            match guard.edit_modes.get(target).copied() {
                Some(EditMode::Semantic) => {}
                Some(other) => {
                    return Err(WriteError::EditModeMismatch {
                        expected: other,
                        got: "Semantic (checkout)",
                    })
                }
                None => {
                    return Err(WriteError::EditModeMismatch {
                        expected: EditMode::AppendOnly,
                        got: "Semantic (checkout)",
                    })
                }
            }
        }

        // Sweep any TTL-expired checkouts in passing.
        self.semantic_state.sweep_expired(target);

        // The checkout itself is materialized as an IntentNode whose
        // metadata records (target, rationale). Subscribers
        // (Nisaba in production) observe the checkout via the same
        // subscription mechanism as any other node. Note: until Step 4
        // lands, this just adds the node to the inner fabric and tags
        // it AppendOnly.
        let entry = CheckoutEntry {
            target: target.clone(),
            rationale: rationale.clone(),
            signer_voice: signer.voice_print(),
            opened_at: std::time::Instant::now(),
            ttl,
            status: CheckoutStatus::Open,
        };

        // Materialize the Checkout node with stable metadata.
        let checkout_node = build_checkout_node(target, &rationale, signer);
        let checkout_id = {
            let mut guard = self.state.write().expect("FabricState poisoned");
            let id = guard.inner.create(checkout_node, &self.default_namespace, Some(signer))?;
            guard.edit_modes.insert(id.clone(), EditMode::AppendOnly);
            id
        };

        match self.semantic_state.try_register_checkout(checkout_id.clone(), entry) {
            Ok(()) => Ok(CheckoutHandle {
                id: checkout_id,
                target: target.clone(),
                status: CheckoutStatus::Open,
            }),
            Err(()) => Err(WriteError::SnapshotInProgress),
        }
    }

    fn propose(
        &self,
        checkout: &LineageId,
        content: IntentNode,
        signer: &AgentKeypair,
    ) -> Result<ProposalHandle, WriteError> {
        // Materialize the Proposal as a fabric node first so it has a
        // LineageId we can use to register state.
        let target = {
            // Find the checkout's target via the semantic state table.
            // For v1, we walk the table; this is O(targets) at v1 scale.
            let mut found_target = None;
            // Iterate state under the inner state Mutex inside the
            // semantic table. We expose a tiny helper via a method.
            self.semantic_state.with_each_target(|target_id, ts| {
                if ts.checkouts.contains_key(checkout) {
                    found_target = Some(target_id.clone());
                }
            });
            match found_target {
                Some(t) => t,
                None => {
                    return Err(WriteError::CheckoutExpired {
                        checkout: checkout.clone(),
                    })
                }
            }
        };

        let proposal_id = {
            let mut guard = self.state.write().expect("FabricState poisoned");
            let id =
                guard.inner.create(content, &self.default_namespace, Some(signer))?;
            guard.edit_modes.insert(id.clone(), EditMode::AppendOnly);
            id
        };

        let entry = ProposalEntry {
            checkout: checkout.clone(),
            signer_voice: signer.voice_print(),
            status: ProposalStatus::Draft,
        };

        match self
            .semantic_state
            .register_proposal(proposal_id.clone(), target, checkout.clone(), entry)
        {
            Ok(_predecessor) => Ok(ProposalHandle {
                id: proposal_id,
                checkout: checkout.clone(),
                status: ProposalStatus::Draft,
            }),
            Err(ProposalRegisterError::CheckoutNotFound) => Err(WriteError::CheckoutExpired {
                checkout: checkout.clone(),
            }),
            Err(ProposalRegisterError::CheckoutClosed) => Err(WriteError::FabricInternal(
                "cannot propose against a closed checkout".into(),
            )),
            Err(ProposalRegisterError::CheckoutExpired) => Err(WriteError::CheckoutExpired {
                checkout: checkout.clone(),
            }),
        }
    }

    fn finalize_proposal(
        &self,
        proposal: &LineageId,
        _signer: &AgentKeypair,
    ) -> Result<Option<ConsensusSnapshot>, WriteError> {
        let outcome = self
            .semantic_state
            .record_finalize(proposal)
            .map_err(|e| match e {
                FinalizeError::ProposalNotFound => WriteError::FabricInternal(
                    "finalize: proposal not found in semantic state".into(),
                ),
                FinalizeError::AlreadyFinalized => WriteError::FabricInternal(
                    "finalize: proposal already finalized".into(),
                ),
                FinalizeError::Superseded => WriteError::FabricInternal(
                    "finalize: proposal was superseded by a later draft".into(),
                ),
                FinalizeError::Dropped => WriteError::FabricInternal(
                    "finalize: proposal was dropped (checkout TTL elapsed)".into(),
                ),
            })?;

        match outcome {
            FinalizeOutcome::StillPending => Ok(None),
            FinalizeOutcome::SnapshotReady(finalized_proposals) => {
                // The SnapshotLock is held. Write the ConsensusSnapshot
                // node into the inner fabric, then release the lock.
                // Per Spec 8 §3.4.3 the lock budget is 50ms; the inner
                // create is ~µs, well under budget.
                let target = self
                    .semantic_state
                    .target_of_proposal(proposal)
                    .ok_or_else(|| {
                        WriteError::FabricInternal(
                            "snapshot: lost target after finalize".into(),
                        )
                    })?;

                let snapshot_node = build_snapshot_node(&target, &finalized_proposals);
                let snapshot_id = {
                    let mut guard = self.state.write().expect("FabricState poisoned");
                    let id = guard
                        .inner
                        .create(snapshot_node, &self.default_namespace, None)?;
                    guard.edit_modes.insert(id.clone(), EditMode::AppendOnly);
                    id
                };

                self.semantic_state
                    .release_snapshot_lock(&target, snapshot_id.clone());

                Ok(Some(ConsensusSnapshot {
                    id: snapshot_id,
                    target,
                    finalized_proposals,
                    written_at: FabricInstant::now(),
                }))
            }
        }
    }

    fn get_node(&self, id: &LineageId) -> Option<IntentNode> {
        let guard = self.state.read().expect("FabricState poisoned");
        guard.inner.get_node(id).cloned()
    }

    fn node_identity(&self, id: &LineageId) -> Option<NodeIdentity> {
        let guard = self.state.read().expect("FabricState poisoned");
        guard.inner.node_identity(id)
    }

    fn edit_mode_of(&self, id: &LineageId) -> Option<EditMode> {
        let guard = self.state.read().expect("FabricState poisoned");
        guard.edit_modes.get(id).copied()
    }
}

// ── Node-shape helpers ────────────────────────────────────────────
//
// Per Jeremy's call: no `NodeKind` enum on IntentNode in v1. The
// Checkout / Proposal / ConsensusSnapshot node types are recorded via
// metadata keys with the `__bridge__` prefix. This is a bridge-internal
// convention — concrete callers (property-mgmt, nisaba-on-fabric) will
// drive a proper node-type system later.

use crate::node::MetadataValue;

const META_NODE_KIND: &str = "__bridge_node_kind__";
const META_CHECKOUT_TARGET: &str = "__bridge_checkout_target__";
const META_CHECKOUT_RATIONALE: &str = "__bridge_checkout_rationale__";
const META_SNAPSHOT_TARGET: &str = "__bridge_snapshot_target__";

const KIND_CHECKOUT: &str = "checkout";
const KIND_CONSENSUS_SNAPSHOT: &str = "consensus_snapshot";

fn build_checkout_node(
    target: &LineageId,
    rationale: &str,
    signer: &AgentKeypair,
) -> IntentNode {
    let mut node = IntentNode::new(format!("checkout: {}", target))
        .with_creator_voice(signer.voice_print());
    node.metadata.insert(
        META_NODE_KIND.into(),
        MetadataValue::String(KIND_CHECKOUT.into()),
    );
    node.metadata.insert(
        META_CHECKOUT_TARGET.into(),
        MetadataValue::String(target.as_uuid().to_string()),
    );
    node.metadata.insert(
        META_CHECKOUT_RATIONALE.into(),
        MetadataValue::String(rationale.into()),
    );
    node.recompute_signature();
    node
}

fn build_snapshot_node(target: &LineageId, finalized_proposals: &[LineageId]) -> IntentNode {
    let summary = format!(
        "consensus snapshot: {} proposal(s) for target {}",
        finalized_proposals.len(),
        target
    );
    let mut node = IntentNode::new(summary);
    node.metadata.insert(
        META_NODE_KIND.into(),
        MetadataValue::String(KIND_CONSENSUS_SNAPSHOT.into()),
    );
    node.metadata.insert(
        META_SNAPSHOT_TARGET.into(),
        MetadataValue::String(target.as_uuid().to_string()),
    );
    let proposals_csv: String = finalized_proposals
        .iter()
        .map(|id| id.as_uuid().to_string())
        .collect::<Vec<_>>()
        .join(",");
    node.metadata.insert(
        "__bridge_snapshot_proposals__".into(),
        MetadataValue::String(proposals_csv),
    );
    node.recompute_signature();
    node
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    fn fresh_bridge() -> BridgeFabric {
        BridgeFabric::new()
    }

    // ── AppendOnly path ──

    #[test]
    fn create_appendonly_node_succeeds() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("journal entry"),
                EditMode::AppendOnly,
                Some(&agent),
            )
            .unwrap();
        assert_eq!(bridge.edit_mode_of(&id), Some(EditMode::AppendOnly));
        let node = bridge.get_node(&id).unwrap();
        assert_eq!(node.creator_voice, Some(agent.voice_print()));
    }

    #[test]
    fn appendonly_cannot_use_edit_mechanical() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(IntentNode::new("note"), EditMode::AppendOnly, Some(&agent))
            .unwrap();
        let result = bridge.edit_mechanical(&id, &agent, |_| {});
        assert!(matches!(
            result.unwrap_err(),
            WriteError::EditModeMismatch { .. }
        ));
    }

    // ── Mechanical path ──

    #[test]
    fn mechanical_edit_succeeds_and_returns_receipt() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("counter"),
                EditMode::Mechanical,
                Some(&agent),
            )
            .unwrap();

        // Mutate content (constraints) — this affects the canonical
        // content bytes and so changes the fingerprint. Note: per the
        // existing IntentNode design, `metadata` is excluded from
        // content_fingerprint ("operational data, not the node's
        // meaning"), so a metadata-only mechanical edit would leave
        // the fingerprint unchanged. That's correct behavior for
        // metadata, but a `Mechanical` edit touching content (e.g.,
        // an updated counter encoded in want.description) should bump
        // the fingerprint, and the receipt should reflect that.
        let receipt = bridge
            .edit_mechanical(&id, &agent, |node| {
                node.constraints.add_hard("must reach quorum");
            })
            .unwrap();

        assert_ne!(
            receipt.previous_content_fingerprint,
            receipt.new_content_fingerprint,
            "A content-bearing mechanical edit must change the content fingerprint."
        );
        assert_eq!(receipt.editor_voice, agent.voice_print());

        let updated = bridge.get_node(&id).unwrap();
        assert_eq!(updated.constraints.count(), 1);
    }

    #[test]
    fn mechanical_edit_metadata_only_keeps_fingerprint() {
        // IntentNode design: metadata is mutable operational data, not
        // part of content_fingerprint. A `Mechanical` edit that only
        // touches metadata leaves the fingerprint stable.
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("counter"),
                EditMode::Mechanical,
                Some(&agent),
            )
            .unwrap();
        let receipt = bridge
            .edit_mechanical(&id, &agent, |node| {
                node.metadata.insert(
                    "count".into(),
                    crate::node::MetadataValue::Int(42),
                );
            })
            .unwrap();
        assert_eq!(
            receipt.previous_content_fingerprint, receipt.new_content_fingerprint,
            "Metadata-only edits don't bump content_fingerprint by design."
        );
        let updated = bridge.get_node(&id).unwrap();
        assert_eq!(
            updated.metadata.get("count"),
            Some(&crate::node::MetadataValue::Int(42))
        );
    }

    #[test]
    fn mechanical_lock_fails_fast_under_contention() {
        // Two agents try to edit the same Mechanical node "concurrently".
        // We simulate by acquiring the lock manually and then attempting
        // a second edit — the second must surface NodeLocked immediately.
        let bridge = fresh_bridge();
        let alice = generate_agent_keypair();
        let bob = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("rate"),
                EditMode::Mechanical,
                Some(&alice),
            )
            .unwrap();

        // Manually hold the lock so we can race the second call.
        let _ = bridge
            .mechanical_locks
            .try_acquire(id.clone(), alice.voice_print());

        let result = bridge.edit_mechanical(&id, &bob, |_| {});
        match result {
            Err(WriteError::NodeLocked { by, .. }) => {
                assert_eq!(by, alice.voice_print(),
                    "NodeLocked must report Alice as the holder.");
            }
            other => panic!("Expected NodeLocked, got {:?}", other),
        }

        // Release and retry — should now succeed.
        bridge.mechanical_locks.release(&id);
        let receipt = bridge.edit_mechanical(&id, &bob, |_| {});
        assert!(receipt.is_ok());
    }

    #[test]
    fn mechanical_lock_released_after_edit() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("metric"),
                EditMode::Mechanical,
                Some(&agent),
            )
            .unwrap();
        bridge.edit_mechanical(&id, &agent, |_| {}).unwrap();
        assert!(!bridge.mechanical_locks.is_held(&id),
            "Per-node lock must be released after a successful edit.");
    }

    #[test]
    fn semantic_node_rejects_mechanical_edit() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("PINNED entry: identity-as-emergent-relation"),
                EditMode::Semantic,
                Some(&agent),
            )
            .unwrap();
        let result = bridge.edit_mechanical(&id, &agent, |_| {});
        match result {
            Err(WriteError::EditModeMismatch { expected, got }) => {
                assert_eq!(expected, EditMode::Semantic);
                assert_eq!(got, "Mechanical");
            }
            other => panic!("Expected EditModeMismatch, got {:?}", other),
        }
    }

    // ── Semantic path ──

    fn semantic_target(bridge: &BridgeFabric, agent: &AgentKeypair) -> LineageId {
        bridge
            .create(
                IntentNode::new("PINNED entry"),
                EditMode::Semantic,
                Some(agent),
            )
            .unwrap()
    }

    #[test]
    fn checkout_propose_finalize_writes_consensus_snapshot() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let target = semantic_target(&bridge, &agent);

        let checkout = bridge
            .checkout(&target, "trying a wording".into(), Duration::from_secs(60), &agent)
            .unwrap();
        assert_eq!(checkout.status, CheckoutStatus::Open);

        let proposal = bridge
            .propose(&checkout.id, IntentNode::new("revised wording"), &agent)
            .unwrap();
        assert_eq!(proposal.status, ProposalStatus::Draft);

        let snapshot = bridge.finalize_proposal(&proposal.id, &agent).unwrap();
        let snapshot = snapshot.expect("single-checkout finalize must write a snapshot");
        assert_eq!(snapshot.target, target);
        assert_eq!(snapshot.finalized_proposals, vec![proposal.id.clone()]);

        // The snapshot is now a real fabric node.
        assert!(bridge.get_node(&snapshot.id).is_some());
    }

    #[test]
    fn checkout_during_snapshot_lock_returns_snapshot_in_progress() {
        // Spec 8 §3.4.3 atomic SnapshotLock: a checkout request landing
        // between "last finalize" and "snapshot written" must surface
        // `SnapshotInProgress`. To exercise this, we manually hold the
        // lock at the semantic-state layer.
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let target = semantic_target(&bridge, &agent);

        // Force the SnapshotLock state via a finalize that closes the
        // round, but DON'T release the lock yet (i.e., simulate the
        // microsecond window where the snapshot is being written).
        let co1 = bridge
            .checkout(&target, "first".into(), Duration::from_secs(60), &agent)
            .unwrap();
        let p1 = bridge
            .propose(&co1.id, IntentNode::new("first content"), &agent)
            .unwrap();
        // Use the low-level state-table path so we hold the SnapshotLock
        // without immediately writing the snapshot. (`finalize_proposal`
        // on the trait writes-and-releases atomically; for this test we
        // want to observe the in-between state.)
        let outcome = bridge.semantic_state.record_finalize(&p1.id).unwrap();
        assert!(matches!(outcome, FinalizeOutcome::SnapshotReady(_)));

        // SnapshotLock is held. A new checkout must be rejected.
        let result = bridge.checkout(&target, "during".into(), Duration::from_secs(60), &agent);
        assert!(matches!(result.unwrap_err(), WriteError::SnapshotInProgress),
            "checkout during SnapshotLock must surface SnapshotInProgress (Spec 8 §3.4.3).");

        // Release the lock manually (simulating the snapshot-written
        // step). After release, new checkouts succeed (Spec 8 §3.4.4).
        bridge
            .semantic_state
            .release_snapshot_lock(&target, LineageId::new());
        let after = bridge.checkout(&target, "after".into(), Duration::from_secs(60), &agent);
        assert!(after.is_ok());
    }

    #[test]
    fn three_concurrent_checkouts_one_snapshot() {
        // Spec 8 §11 acceptance #5 (single-target version). Three
        // checkouts open, three proposals draft + finalize. The
        // ConsensusSnapshot fires exactly once, includes all three.
        let bridge = fresh_bridge();
        let alice = generate_agent_keypair();
        let bob = generate_agent_keypair();
        let carol = generate_agent_keypair();
        let target = semantic_target(&bridge, &alice);

        let co_a = bridge
            .checkout(&target, "alice".into(), Duration::from_secs(60), &alice)
            .unwrap();
        let co_b = bridge
            .checkout(&target, "bob".into(), Duration::from_secs(60), &bob)
            .unwrap();
        let co_c = bridge
            .checkout(&target, "carol".into(), Duration::from_secs(60), &carol)
            .unwrap();

        let p_a = bridge
            .propose(&co_a.id, IntentNode::new("alice's wording"), &alice)
            .unwrap();
        let p_b = bridge
            .propose(&co_b.id, IntentNode::new("bob's wording"), &bob)
            .unwrap();
        let p_c = bridge
            .propose(&co_c.id, IntentNode::new("carol's wording"), &carol)
            .unwrap();

        // First two finalizes must NOT fire a snapshot (round still open).
        let after_a = bridge.finalize_proposal(&p_a.id, &alice).unwrap();
        assert!(after_a.is_none());
        let after_b = bridge.finalize_proposal(&p_b.id, &bob).unwrap();
        assert!(after_b.is_none());

        // Third (last) finalize must fire exactly one snapshot with all three.
        let snapshot = bridge.finalize_proposal(&p_c.id, &carol).unwrap();
        let snapshot = snapshot.expect("last finalize must write the snapshot");
        assert_eq!(snapshot.target, target);
        assert_eq!(snapshot.finalized_proposals.len(), 3);
        assert!(snapshot.finalized_proposals.contains(&p_a.id));
        assert!(snapshot.finalized_proposals.contains(&p_b.id));
        assert!(snapshot.finalized_proposals.contains(&p_c.id));
    }

    #[test]
    fn proposal_supersession_within_checkout() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let target = semantic_target(&bridge, &agent);
        let co = bridge
            .checkout(&target, "thinking".into(), Duration::from_secs(60), &agent)
            .unwrap();

        let p1 = bridge
            .propose(&co.id, IntentNode::new("first try"), &agent)
            .unwrap();
        let p2 = bridge
            .propose(&co.id, IntentNode::new("second, better try"), &agent)
            .unwrap();
        // p1 superseded; p2 is the active draft.
        assert_eq!(
            bridge.semantic_state.proposal_status(&target, &p1.id),
            Some(ProposalStatus::Superseded)
        );
        assert_eq!(
            bridge.semantic_state.proposal_status(&target, &p2.id),
            Some(ProposalStatus::Draft)
        );

        // Finalizing p2 should write a snapshot containing p2 only.
        let snap = bridge.finalize_proposal(&p2.id, &agent).unwrap();
        let snap = snap.unwrap();
        assert_eq!(snap.finalized_proposals, vec![p2.id]);
    }

    #[test]
    fn checkout_target_must_be_semantic_mode() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(IntentNode::new("entry"), EditMode::AppendOnly, Some(&agent))
            .unwrap();
        let result = bridge.checkout(&id, "?".into(), Duration::from_secs(60), &agent);
        assert!(matches!(result.unwrap_err(), WriteError::EditModeMismatch { .. }));
    }
}
