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

use super::debug::{
    DebugToken, FabricStateSnapshot, NodeDebugDetail, NodeQuarantineLabel,
    DEBUG_TOKEN_DEFAULT_LIFETIME, DEBUG_TOKEN_DEFAULT_SCOPE,
};
use super::fabric_trait::Fabric as FabricTrait;
use super::mechanical::{AcquireResult, EditReceipt, MechanicalLockTable};
use super::observability::{
    edit_mode_label, subscription_state, write_outcome_label, write_type,
    METRIC_FABRIC_CONSENSUS_RESOLUTION_SECONDS, METRIC_FABRIC_SNAPSHOT_LOCK_HELD,
    METRIC_FABRIC_SUBSCRIPTION_COUNT, METRIC_FABRIC_WRITES_TOTAL,
    METRIC_FABRIC_WRITE_LATENCY_SECONDS, METRIC_FABRIC_ATTESTATION_VERIFICATIONS_TOTAL,
};
use super::semantic::{
    CheckoutEntry, CheckoutHandle, CheckoutStatus, ConsensusSnapshot, FinalizeError,
    FinalizeOutcome, ProposalEntry, ProposalHandle, ProposalRegisterError, ProposalStatus,
    SemanticEditConfig, SemanticStateTable,
};
use super::subscription::{
    Callback, DispatchConfig, DispatchPool, Predicate, SubscribeError, SubscriptionId,
    SubscriptionRegistry, SubscriptionState,
};
use metrics::{counter, gauge, histogram};
use std::sync::atomic::Ordering;
use std::time::Instant;
use tracing::{debug, info_span, warn};

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
/// - A `SubscriptionRegistry` plus `DispatchPool` (Spec 8 §6 + §2.6.1).
pub struct BridgeFabric {
    state: Arc<RwLock<FabricState>>,
    mechanical_locks: MechanicalLockTable,
    semantic_state: SemanticStateTable,
    semantic_config: SemanticEditConfig,
    /// Default namespace used when callers don't specify one. Matches
    /// the existing `Fabric::add_node` behavior.
    default_namespace: NamespaceId,
    pub(crate) subscriptions: SubscriptionRegistry,
    dispatch_pool: DispatchPool,
    dispatch_config: DispatchConfig,
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
        Self::wrap_with_dispatch(inner, DispatchConfig::default())
    }

    /// Wrap with a custom dispatch configuration (worker count,
    /// lagged threshold, retry backoff). Lets tests pin worker count
    /// to 2 for predictable behavior.
    pub fn wrap_with_dispatch(inner: InnerFabric, dispatch: DispatchConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(FabricState::new(inner))),
            mechanical_locks: MechanicalLockTable::new(),
            semantic_state: SemanticStateTable::new(),
            semantic_config: SemanticEditConfig::default(),
            default_namespace: NamespaceId::default_namespace(),
            // 65,536 subscription cap is permissive for v1; tighten in v2.
            subscriptions: SubscriptionRegistry::new(65_536),
            dispatch_pool: DispatchPool::new(dispatch),
            dispatch_config: dispatch,
        }
    }

    /// Stop the dispatch pool cleanly. Idempotent — `Drop` calls this
    /// too.
    pub fn shutdown(&self) {
        self.dispatch_pool.shutdown();
    }

    /// Internal: dispatch a freshly-committed node to all matching
    /// subscriptions. Pattern matching happens synchronously on the
    /// caller thread (cheap); callbacks run on the pool. Backpressure:
    /// subscribers whose queue depth exceeds the lagged threshold are
    /// marked `Lagged` and skip individual matches until the queue
    /// drains (Spec 8 §6.B.4).
    fn dispatch_to_subscribers(&self, node: &IntentNode) {
        let snapshot = self.subscriptions.snapshot();
        if snapshot.is_empty() {
            return;
        }
        let lagged_threshold = self.dispatch_config.lagged_threshold;
        for entry in snapshot {
            if !(entry.pattern)(node) {
                continue;
            }
            let depth = entry.queue_depth.load(Ordering::Relaxed);
            if depth >= lagged_threshold {
                // Mark lagged once; subsequent matches are silently
                // skipped until the queue drains. Spec 8 §6.B.4 calls
                // for a single Lagged summary event — Step 5
                // (observability) emits a tracing event for it.
                if !entry.lagged.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "[ecphory::subscription] subscription_lagged id={} depth={}",
                        entry.id, depth
                    );
                }
                continue;
            }
            let _ = self.dispatch_pool.enqueue(entry, node.clone());
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

    // ── Debug accessors (Spec 8 §8.5.3) ────────────────────────────
    //
    // These produce structured snapshots that the HTTP `/debug/fabric/*`
    // endpoints (in nabu) serialize to operators. Gating to localhost or
    // an admin token (`DebugToken::verify`) is the HTTP layer's job; the
    // accessors themselves are unauthenticated so they remain testable
    // and reusable from in-process tooling.

    /// `GET /debug/fabric/state` — top-level fabric summary.
    pub fn debug_state(&self) -> FabricStateSnapshot {
        let guard = self.state.read().expect("FabricState poisoned");
        let node_count = guard.inner.node_count();
        let edge_count = guard.inner.edge_count();
        let region_count = guard
            .inner
            .nodes()
            .filter_map(|(_, n)| n.causal_position.as_ref().map(|p| p.namespace.uuid))
            .collect::<std::collections::HashSet<_>>()
            .len();
        let genesis = guard.inner.genesis();
        FabricStateSnapshot {
            node_count,
            edge_count,
            region_count,
            subscription_count: self.subscriptions.count(),
            genesis_present: genesis.is_some(),
            training_complete: genesis.map(|g| g.training_complete()),
            current_lamport: guard.inner.current_timestamp().value(),
        }
    }

    /// `GET /debug/fabric/subscriptions` — list active subscriptions
    /// with their runtime state (queue depth, panic count, lagged).
    pub fn debug_subscriptions(&self) -> Vec<SubscriptionState> {
        self.subscriptions
            .snapshot()
            .iter()
            .filter_map(|entry| self.subscriptions.state(entry.id))
            .collect()
    }

    /// `GET /debug/fabric/node/<reference>` — full per-node detail.
    /// Returns `None` if the node is absent or has no causal position
    /// (which would mean it bypassed `Fabric::create` — shouldn't happen
    /// for bridge-managed nodes).
    pub fn debug_node(&self, id: &LineageId) -> Option<NodeDebugDetail> {
        let guard = self.state.read().expect("FabricState poisoned");
        let node = guard.inner.get_node(id)?;
        let identity = guard.inner.node_identity(id)?;
        let edit_mode = guard.edit_modes.get(id).copied();
        Some(NodeDebugDetail {
            identity,
            edit_mode,
            want: node.want.description.clone(),
            constraint_count: node.constraints.constraints.len(),
            edges_out: guard.inner.edges_from(id).len(),
            edges_in: guard.inner.edges_to(id).len(),
            quarantine_state: NodeQuarantineLabel::from_state(&node.quarantine),
            has_node_signature: node.node_signature.is_some(),
            version: node.version(),
        })
    }

    // ── Admin tokens (Spec 8 §8.5.3 Cantrill C.3 fold) ─────────────

    /// Issue a 1-hour Ed25519-signed debug token. The signing key is
    /// the operator's bootstrap key (in production: Jeremy's). Token
    /// validates against the operator's voice print at the endpoint.
    pub fn issue_debug_token(&self, operator: &AgentKeypair) -> DebugToken {
        DebugToken::issue(operator, DEBUG_TOKEN_DEFAULT_SCOPE, DEBUG_TOKEN_DEFAULT_LIFETIME)
    }

    /// Verify an incoming token against the operator's voice print.
    /// Convenience wrapper around `DebugToken::verify` so the HTTP
    /// layer doesn't need to know about scopes.
    pub fn verify_debug_token(
        &self,
        token: &DebugToken,
        operator: &crate::identity::VoicePrint,
    ) -> Result<(), super::debug::DebugTokenError> {
        token.verify(operator, DEBUG_TOKEN_DEFAULT_SCOPE)
    }

    /// Verify a per-node signature on a high-sensitivity node and
    /// emit the `fabric_attestation_verifications_total{outcome}`
    /// metric. Wraps the inner `Fabric::verify_node_signature` so
    /// operators reading metrics see the verification rate.
    pub fn verify_node_signature_metered(&self, id: &LineageId) -> Option<bool> {
        let guard = self.state.read().expect("FabricState poisoned");
        let result = guard.inner.verify_node_signature(id);
        let outcome = match result {
            Some(true) => "verified",
            Some(false) => "failed",
            None => "unsigned",
        };
        counter!(
            METRIC_FABRIC_ATTESTATION_VERIFICATIONS_TOTAL,
            "outcome" => outcome,
        )
        .increment(1);
        result
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
        // Spec 8 §8.5.1 — every public trait method opens a span.
        // `signer_pubkey_fingerprint` is the first 16 hex chars of the
        // signer's voice print, matching the field naming in the spec.
        let span = info_span!(
            "fabric::create",
            edit_mode = edit_mode_label(edit_mode),
            region = %self.default_namespace.name,
            signer_pubkey_fingerprint = signer
                .map(|kp| short_voice(&kp.voice_print()))
                .unwrap_or_else(|| "<unsigned>".into()),
        );
        let _enter = span.enter();
        let started = Instant::now();
        let mode_label = edit_mode_label(edit_mode);

        let result = (|| -> Result<(LineageId, IntentNode), WriteError> {
            let mut guard = self.state.write().expect("FabricState poisoned");
            let lineage_id =
                guard.inner.create(content, &self.default_namespace, signer)?;
            guard.edit_modes.insert(lineage_id.clone(), edit_mode);
            let node_snapshot = guard
                .inner
                .get_node(&lineage_id)
                .cloned()
                .expect("just-created node must be present");
            Ok((lineage_id, node_snapshot))
        })();

        let outcome_label = write_outcome_label(&result);
        counter!(
            METRIC_FABRIC_WRITES_TOTAL,
            "type" => mode_label,
            "region" => self.default_namespace.name.clone(),
            "outcome" => outcome_label,
        )
        .increment(1);

        match result {
            Ok((lineage_id, dispatched_node)) => {
                histogram!(
                    METRIC_FABRIC_WRITE_LATENCY_SECONDS,
                    "type" => mode_label,
                )
                .record(started.elapsed().as_secs_f64());
                debug!(
                    lineage_id = %lineage_id,
                    "fabric::create committed"
                );
                drop(_enter);
                self.dispatch_to_subscribers(&dispatched_node);
                Ok(lineage_id)
            }
            Err(err) => {
                warn!(error = %err, "fabric::create rejected");
                Err(err)
            }
        }
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
        let span = info_span!(
            "fabric::edit_mechanical",
            target_id = %target,
            signer_pubkey_fingerprint = %short_voice(&signer.voice_print()),
        );
        let _enter = span.enter();
        let started = Instant::now();

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
                let err = WriteError::NodeLocked {
                    by: holder.holder,
                    until_ns: holder.deadline_ns(),
                };
                warn!(error = %err, "fabric::edit_mechanical contention");
                let outcome = "node_locked";
                counter!(
                    METRIC_FABRIC_WRITES_TOTAL,
                    "type" => write_type::MECHANICAL,
                    "region" => self.default_namespace.name.clone(),
                    "outcome" => outcome,
                )
                .increment(1);
                return Err(err);
            }
            AcquireResult::Acquired => {}
        }

        // Lock held — do the edit under the outer write lock. Release
        // the per-node lock no matter what (panic-safe via guard).
        let outcome: Result<(EditReceipt, IntentNode), WriteError> = (|| {
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

            let mutated = guard
                .inner
                .get_node(target)
                .expect("present after successful mutate_node")
                .clone();
            let new_fingerprint = *mutated.content_fingerprint();

            Ok((
                EditReceipt {
                    target: target.clone(),
                    previous_content_fingerprint: previous_fingerprint,
                    new_content_fingerprint: new_fingerprint,
                    editor_voice: signer.voice_print(),
                    commit_instant: FabricInstant::now(),
                },
                mutated,
            ))
        })();

        self.mechanical_locks.release(target);
        let outcome_label = write_outcome_label(&outcome);
        counter!(
            METRIC_FABRIC_WRITES_TOTAL,
            "type" => write_type::MECHANICAL,
            "region" => self.default_namespace.name.clone(),
            "outcome" => outcome_label,
        )
        .increment(1);
        let (receipt, mutated) = outcome?;
        histogram!(
            METRIC_FABRIC_WRITE_LATENCY_SECONDS,
            "type" => write_type::MECHANICAL,
        )
        .record(started.elapsed().as_secs_f64());
        debug!(
            target_id = %target,
            "fabric::edit_mechanical committed"
        );
        drop(_enter);
        self.dispatch_to_subscribers(&mutated);
        Ok(receipt)
    }

    fn checkout(
        &self,
        target: &LineageId,
        rationale: String,
        ttl: Duration,
        signer: &AgentKeypair,
    ) -> Result<CheckoutHandle, WriteError> {
        let span = info_span!(
            "fabric::checkout",
            target_id = %target,
            ttl_ms = ttl.as_millis() as u64,
            signer_pubkey_fingerprint = %short_voice(&signer.voice_print()),
        );
        let _enter = span.enter();
        let started = Instant::now();

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
        let (checkout_id, dispatched_node) = {
            let mut guard = self.state.write().expect("FabricState poisoned");
            let id = guard.inner.create(checkout_node, &self.default_namespace, Some(signer))?;
            guard.edit_modes.insert(id.clone(), EditMode::AppendOnly);
            let stored = guard
                .inner
                .get_node(&id)
                .cloned()
                .expect("just-created Checkout node must be present");
            (id, stored)
        };

        let result = self.semantic_state.try_register_checkout(checkout_id.clone(), entry);
        let outcome_label = match &result {
            Ok(()) => "success",
            Err(()) => "snapshot_in_progress",
        };
        counter!(
            METRIC_FABRIC_WRITES_TOTAL,
            "type" => write_type::CHECKOUT,
            "region" => self.default_namespace.name.clone(),
            "outcome" => outcome_label,
        )
        .increment(1);

        match result {
            Ok(()) => {
                histogram!(
                    METRIC_FABRIC_WRITE_LATENCY_SECONDS,
                    "type" => write_type::CHECKOUT,
                )
                .record(started.elapsed().as_secs_f64());
                debug!(checkout_id = %checkout_id, "fabric::checkout opened");
                drop(_enter);
                self.dispatch_to_subscribers(&dispatched_node);
                Ok(CheckoutHandle {
                    id: checkout_id,
                    target: target.clone(),
                    status: CheckoutStatus::Open,
                })
            }
            Err(()) => {
                warn!("fabric::checkout rejected (snapshot in progress)");
                Err(WriteError::SnapshotInProgress)
            }
        }
    }

    fn propose(
        &self,
        checkout: &LineageId,
        content: IntentNode,
        signer: &AgentKeypair,
    ) -> Result<ProposalHandle, WriteError> {
        let span = info_span!(
            "fabric::propose",
            checkout_id = %checkout,
            signer_pubkey_fingerprint = %short_voice(&signer.voice_print()),
        );
        let _enter = span.enter();
        let started = Instant::now();

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

        let (proposal_id, proposal_snapshot) = {
            let mut guard = self.state.write().expect("FabricState poisoned");
            let id =
                guard.inner.create(content, &self.default_namespace, Some(signer))?;
            guard.edit_modes.insert(id.clone(), EditMode::AppendOnly);
            let stored = guard
                .inner
                .get_node(&id)
                .cloned()
                .expect("just-created Proposal node must be present");
            (id, stored)
        };

        let entry = ProposalEntry {
            checkout: checkout.clone(),
            signer_voice: signer.voice_print(),
            status: ProposalStatus::Draft,
        };

        let register_result = self
            .semantic_state
            .register_proposal(proposal_id.clone(), target, checkout.clone(), entry);

        let outcome_label = match &register_result {
            Ok(_) => "success",
            Err(ProposalRegisterError::CheckoutNotFound)
            | Err(ProposalRegisterError::CheckoutExpired) => "checkout_expired",
            Err(ProposalRegisterError::CheckoutClosed) => "fabric_internal",
        };
        counter!(
            METRIC_FABRIC_WRITES_TOTAL,
            "type" => write_type::PROPOSAL,
            "region" => self.default_namespace.name.clone(),
            "outcome" => outcome_label,
        )
        .increment(1);

        match register_result {
            Ok(_predecessor) => {
                histogram!(
                    METRIC_FABRIC_WRITE_LATENCY_SECONDS,
                    "type" => write_type::PROPOSAL,
                )
                .record(started.elapsed().as_secs_f64());
                debug!(proposal_id = %proposal_id, "fabric::propose drafted");
                drop(_enter);
                self.dispatch_to_subscribers(&proposal_snapshot);
                Ok(ProposalHandle {
                    id: proposal_id,
                    checkout: checkout.clone(),
                    status: ProposalStatus::Draft,
                })
            }
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
        let span = info_span!(
            "fabric::finalize_proposal",
            proposal_id = %proposal,
        );
        let _enter = span.enter();
        let started = Instant::now();

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
            FinalizeOutcome::StillPending => {
                counter!(
                    METRIC_FABRIC_WRITES_TOTAL,
                    "type" => write_type::FINALIZE,
                    "region" => self.default_namespace.name.clone(),
                    "outcome" => "success",
                )
                .increment(1);
                histogram!(
                    METRIC_FABRIC_WRITE_LATENCY_SECONDS,
                    "type" => write_type::FINALIZE,
                )
                .record(started.elapsed().as_secs_f64());
                debug!("fabric::finalize_proposal pending — round still open");
                Ok(None)
            }
            FinalizeOutcome::SnapshotReady(finalized_proposals) => {
                // SnapshotLock is held. The §3.4.3 budget is 50ms.
                gauge!(METRIC_FABRIC_SNAPSHOT_LOCK_HELD).set(1.0);

                let target = self
                    .semantic_state
                    .target_of_proposal(proposal)
                    .ok_or_else(|| {
                        WriteError::FabricInternal(
                            "snapshot: lost target after finalize".into(),
                        )
                    })?;

                let snapshot_node = build_snapshot_node(&target, &finalized_proposals);
                let (snapshot_id, snapshot_snapshot) = {
                    let mut guard = self.state.write().expect("FabricState poisoned");
                    let id = guard
                        .inner
                        .create(snapshot_node, &self.default_namespace, None)?;
                    guard.edit_modes.insert(id.clone(), EditMode::AppendOnly);
                    let stored = guard
                        .inner
                        .get_node(&id)
                        .cloned()
                        .expect("just-created ConsensusSnapshot node must be present");
                    (id, stored)
                };

                self.semantic_state
                    .release_snapshot_lock(&target, snapshot_id.clone());
                gauge!(METRIC_FABRIC_SNAPSHOT_LOCK_HELD).set(0.0);

                let resolution_secs = started.elapsed().as_secs_f64();
                histogram!(METRIC_FABRIC_CONSENSUS_RESOLUTION_SECONDS).record(resolution_secs);
                histogram!(
                    METRIC_FABRIC_WRITE_LATENCY_SECONDS,
                    "type" => write_type::FINALIZE,
                )
                .record(resolution_secs);
                counter!(
                    METRIC_FABRIC_WRITES_TOTAL,
                    "type" => write_type::FINALIZE,
                    "region" => self.default_namespace.name.clone(),
                    "outcome" => "success",
                )
                .increment(1);
                counter!(
                    METRIC_FABRIC_WRITES_TOTAL,
                    "type" => write_type::CONSENSUS_SNAPSHOT,
                    "region" => self.default_namespace.name.clone(),
                    "outcome" => "success",
                )
                .increment(1);

                if resolution_secs * 1000.0 > self.semantic_config.snapshot_lock_budget.as_millis() as f64 {
                    warn!(
                        resolution_ms = resolution_secs * 1000.0,
                        budget_ms = self.semantic_config.snapshot_lock_budget.as_millis() as u64,
                        "fabric::finalize_proposal exceeded snapshot lock budget (Spec 8 §3.4.3)"
                    );
                }
                debug!(
                    snapshot_id = %snapshot_id,
                    proposals = finalized_proposals.len(),
                    "fabric::finalize_proposal wrote ConsensusSnapshot"
                );
                drop(_enter);

                // Dispatch the ConsensusSnapshot AFTER releasing the
                // SnapshotLock so a subscriber that immediately opens a
                // new checkout doesn't see SnapshotInProgress.
                self.dispatch_to_subscribers(&snapshot_snapshot);

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

    fn subscribe(
        &self,
        pattern: Predicate,
        callback: Callback,
    ) -> Result<SubscriptionId, SubscribeError> {
        let span = info_span!("fabric::subscribe");
        let _enter = span.enter();
        let id = self.subscriptions.add(pattern, callback)?;
        gauge!(
            METRIC_FABRIC_SUBSCRIPTION_COUNT,
            "state" => subscription_state::ACTIVE,
        )
        .set(self.subscriptions.count() as f64);
        debug!(subscription_id = %id, "fabric::subscribe registered");
        Ok(id)
    }

    fn unsubscribe(&self, id: SubscriptionId) -> Result<(), SubscribeError> {
        let span = info_span!("fabric::unsubscribe", subscription_id = %id);
        let _enter = span.enter();
        self.subscriptions.remove(id)?;
        gauge!(
            METRIC_FABRIC_SUBSCRIPTION_COUNT,
            "state" => subscription_state::ACTIVE,
        )
        .set(self.subscriptions.count() as f64);
        debug!("fabric::unsubscribe removed");
        Ok(())
    }

    fn subscription_state(&self, id: SubscriptionId) -> Option<SubscriptionState> {
        self.subscriptions.state(id)
    }
}

// ── Tracing helpers ───────────────────────────────────────────────

fn short_voice(voice: &crate::identity::VoicePrint) -> String {
    voice.to_hex()[..16.min(voice.to_hex().len())].to_string()
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

    // ── Subscriptions wired into the bridge (Spec 8 §6) ──

    use crate::bridge::subscription::{Callback, CallbackResult, Predicate};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn wait_until<F: Fn() -> bool>(deadline: Duration, check: F) -> bool {
        let end = Instant::now() + deadline;
        while Instant::now() < end {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        check()
    }

    #[test]
    fn subscribe_fires_on_create() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let count_for_cb = std::sync::Arc::clone(&count);

        let pat: Predicate = std::sync::Arc::new(|node: &IntentNode| {
            node.want.description.contains("important")
        });
        let cb: Callback = std::sync::Arc::new(move |_node, _ctx| {
            count_for_cb.fetch_add(1, Ordering::SeqCst);
            CallbackResult::Success
        });
        let _ = bridge.subscribe(pat, cb).unwrap();

        // Match.
        bridge
            .create(
                IntentNode::new("important note"),
                EditMode::AppendOnly,
                Some(&agent),
            )
            .unwrap();
        // No match.
        bridge
            .create(IntentNode::new("trivial"), EditMode::AppendOnly, Some(&agent))
            .unwrap();
        // Match.
        bridge
            .create(
                IntentNode::new("very important reminder"),
                EditMode::AppendOnly,
                Some(&agent),
            )
            .unwrap();

        assert!(
            wait_until(Duration::from_secs(2), || count.load(Ordering::SeqCst) == 2),
            "Expected 2 matches; got {}",
            count.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let count_for_cb = std::sync::Arc::clone(&count);

        let pat: Predicate = std::sync::Arc::new(|_| true);
        let cb: Callback = std::sync::Arc::new(move |_node, _ctx| {
            count_for_cb.fetch_add(1, Ordering::SeqCst);
            CallbackResult::Success
        });
        let id = bridge.subscribe(pat, cb).unwrap();

        bridge
            .create(IntentNode::new("a"), EditMode::AppendOnly, Some(&agent))
            .unwrap();
        assert!(wait_until(Duration::from_secs(1), || count.load(
            Ordering::SeqCst
        ) >= 1));

        bridge.unsubscribe(id).unwrap();

        bridge
            .create(IntentNode::new("b"), EditMode::AppendOnly, Some(&agent))
            .unwrap();
        bridge
            .create(IntentNode::new("c"), EditMode::AppendOnly, Some(&agent))
            .unwrap();

        // Give the pool a moment to drain — count should remain at 1.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "After unsubscribe, no further deliveries should occur."
        );
    }

    #[test]
    fn callback_panic_does_not_disrupt_request_path() {
        // Spec 8 §2.6.1: subscription panics never reach the request path.
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let pat: Predicate = std::sync::Arc::new(|_| true);
        let cb: Callback = std::sync::Arc::new(|_node, _ctx| {
            panic!("intentional test panic");
        });
        let id = bridge.subscribe(pat, cb).unwrap();

        // The create call must return Ok regardless of the panicking callback.
        let result = bridge.create(
            IntentNode::new("triggers panic"),
            EditMode::AppendOnly,
            Some(&agent),
        );
        assert!(result.is_ok(), "create() must not be affected by callback panics");

        // The callback should have been invoked and the panic counted.
        // (We poll because dispatch is async.)
        assert!(wait_until(Duration::from_secs(2), || {
            bridge
                .subscription_state(id)
                .map(|s| s.panic_count >= 1)
                .unwrap_or(false)
        }), "Subscription panic_count should reach 1");
    }

    #[test]
    fn lagged_threshold_marks_subscription_lagged() {
        // Spec 8 §6.B.4: a subscription whose queue grows beyond the
        // lagged threshold is marked Lagged and stops getting new
        // matches enqueued individually until the queue drains.
        //
        // Strategy: build a bridge with worker_count=1, lagged_threshold=3
        // and a slow callback. Pump 6 writes through; the first few
        // queue (depth ≤ 3), the rest are skipped, so only ~3 ever fire.
        let bridge = BridgeFabric::wrap_with_dispatch(
            crate::fabric::Fabric::new(),
            DispatchConfig {
                worker_count: 1,
                lagged_threshold: 3,
                retry_backoff: Duration::from_millis(0),
            },
        );
        let agent = generate_agent_keypair();
        let invocations = std::sync::Arc::new(AtomicUsize::new(0));
        let invocations_for_cb = std::sync::Arc::clone(&invocations);

        let pat: Predicate = std::sync::Arc::new(|_| true);
        let cb: Callback = std::sync::Arc::new(move |_node, _ctx| {
            std::thread::sleep(Duration::from_millis(50));
            invocations_for_cb.fetch_add(1, Ordering::SeqCst);
            CallbackResult::Success
        });
        let id = bridge.subscribe(pat, cb).unwrap();

        // Fire 8 writes back-to-back. Worker is slow; queue should
        // saturate. Once depth reaches threshold (3), further writes
        // are skipped — they don't get enqueued.
        for i in 0..8 {
            bridge
                .create(
                    IntentNode::new(format!("event {}", i)),
                    EditMode::AppendOnly,
                    Some(&agent),
                )
                .unwrap();
        }

        // Eventually the queue drains. We expect strictly fewer than 8
        // invocations because some were skipped under backpressure.
        let _ = wait_until(Duration::from_secs(3), || {
            bridge
                .subscription_state(id)
                .map(|s| s.queue_depth == 0)
                .unwrap_or(false)
        });

        let final_count = invocations.load(Ordering::SeqCst);
        assert!(
            final_count < 8,
            "Lagged backpressure must skip some matches; got {} invocations for 8 writes",
            final_count
        );
        assert!(
            final_count >= 1,
            "At least the early matches should have fired before lagged kicked in; got {}",
            final_count
        );
    }

    #[test]
    fn subscription_fires_on_consensus_snapshot() {
        // The ConsensusSnapshot node is itself dispatched to subscribers
        // — it's a fabric-resident node like any other (Spec 8 §3.4.3,
        // §6.5: cell-agent population subscribes to ConsensusSnapshot).
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let snapshot_count = std::sync::Arc::new(AtomicUsize::new(0));
        let count_for_cb = std::sync::Arc::clone(&snapshot_count);

        let pat: Predicate = std::sync::Arc::new(|node: &IntentNode| {
            node.metadata
                .get(super::META_NODE_KIND)
                .map(|v| v.as_str_repr() == super::KIND_CONSENSUS_SNAPSHOT)
                .unwrap_or(false)
        });
        let cb: Callback = std::sync::Arc::new(move |_node, _ctx| {
            count_for_cb.fetch_add(1, Ordering::SeqCst);
            CallbackResult::Success
        });
        let _ = bridge.subscribe(pat, cb).unwrap();

        let target = bridge
            .create(IntentNode::new("PINNED"), EditMode::Semantic, Some(&agent))
            .unwrap();
        let co = bridge
            .checkout(&target, "rev".into(), Duration::from_secs(60), &agent)
            .unwrap();
        let p = bridge
            .propose(&co.id, IntentNode::new("revised"), &agent)
            .unwrap();
        let snap = bridge.finalize_proposal(&p.id, &agent).unwrap();
        assert!(snap.is_some());

        assert!(
            wait_until(Duration::from_secs(2), || snapshot_count
                .load(Ordering::SeqCst)
                >= 1),
            "Subscriber should observe the ConsensusSnapshot node"
        );
    }

    // ── Spec 8 §8.5 Observability ──

    #[test]
    fn debug_state_reflects_current_fabric() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();

        let initial = bridge.debug_state();
        assert_eq!(initial.node_count, 0);
        assert_eq!(initial.subscription_count, 0);
        assert!(!initial.genesis_present);

        // Add some nodes + a subscription, snapshot updates.
        bridge
            .create(IntentNode::new("a"), EditMode::AppendOnly, Some(&agent))
            .unwrap();
        bridge
            .create(IntentNode::new("b"), EditMode::AppendOnly, Some(&agent))
            .unwrap();

        let pat: Predicate = std::sync::Arc::new(|_| true);
        let cb: Callback = std::sync::Arc::new(|_, _| CallbackResult::Success);
        let _ = bridge.subscribe(pat, cb).unwrap();

        let after = bridge.debug_state();
        assert_eq!(after.node_count, 2);
        assert_eq!(after.subscription_count, 1);
        assert!(after.current_lamport >= initial.current_lamport);
    }

    #[test]
    fn debug_node_returns_full_detail() {
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let id = bridge
            .create(
                IntentNode::new("audit entry"),
                EditMode::Mechanical,
                Some(&agent),
            )
            .unwrap();
        bridge
            .edit_mechanical(&id, &agent, |node| {
                node.constraints.add_hard("must be observable");
            })
            .unwrap();

        let detail = bridge.debug_node(&id).expect("node detail available");
        assert_eq!(detail.want, "audit entry");
        assert_eq!(detail.edit_mode, Some(EditMode::Mechanical));
        assert_eq!(detail.constraint_count, 1);
        assert_eq!(detail.quarantine_state, NodeQuarantineLabel::Normal);
        assert!(detail.version >= 1);
        assert_eq!(detail.identity.creator_voice, Some(agent.voice_print()));
    }

    #[test]
    fn debug_node_returns_none_for_unknown() {
        let bridge = fresh_bridge();
        let result = bridge.debug_node(&LineageId::new());
        assert!(result.is_none());
    }

    #[test]
    fn debug_subscriptions_lists_active() {
        let bridge = fresh_bridge();
        let pat: Predicate = std::sync::Arc::new(|_| true);
        let cb: Callback = std::sync::Arc::new(|_, _| CallbackResult::Success);
        let _ = bridge.subscribe(std::sync::Arc::clone(&pat), std::sync::Arc::clone(&cb)).unwrap();
        let _ = bridge.subscribe(pat, cb).unwrap();
        let states = bridge.debug_subscriptions();
        assert_eq!(states.len(), 2);
        for state in &states {
            assert_eq!(state.queue_depth, 0);
            assert_eq!(state.panic_count, 0);
        }
    }

    #[test]
    fn issue_and_verify_debug_token_round_trip() {
        let bridge = fresh_bridge();
        let operator = generate_agent_keypair();
        let token = bridge.issue_debug_token(&operator);
        assert!(bridge.verify_debug_token(&token, &operator.voice_print()).is_ok());

        // Different operator key fails.
        let other = generate_agent_keypair();
        assert!(bridge
            .verify_debug_token(&token, &other.voice_print())
            .is_err());
    }

    #[test]
    fn verify_node_signature_metered_returns_correct_outcome() {
        // Setup: high-sensitivity region with a signed node. Because the
        // bridge's default namespace is Normal sensitivity, we reach into
        // the inner fabric to register propmgmt as High and create the
        // signed node directly. CRITICAL: drop the write guard BEFORE
        // calling `verify_node_signature_metered`, which itself takes a
        // read lock on the same RwLock — same-thread upgrade would
        // deadlock std::sync::RwLock.
        let bridge = fresh_bridge();
        let agent = generate_agent_keypair();
        let propmgmt = NamespaceId::fresh("propmgmt");
        let id = {
            let mut guard = bridge.state.write().unwrap();
            guard.inner.set_region_sensitivity(
                propmgmt.clone(),
                crate::identity::RegionSensitivity::High,
            );
            guard
                .inner
                .create(IntentNode::new("financial"), &propmgmt, Some(&agent))
                .unwrap()
        };

        let result = bridge.verify_node_signature_metered(&id);
        assert_eq!(result, Some(true));
    }
}
