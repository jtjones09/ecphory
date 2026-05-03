// SEMANTIC EDITS — Checkout / Propose / ConsensusSnapshot (Spec 8 §3.4)
//
// Per spec §3.4, semantic edits proceed via a state machine, not free
// concurrent writes:
//
//   checkout(target, ttl)        → Checkout node, status: Open
//   propose(checkout, content)   → Proposal node referencing the checkout
//   finalize_proposal(proposal)  → marks the proposal Finalized
//
// When the *last* outstanding checkout on a target finalizes or expires,
// the fabric writes a `ConsensusSnapshot` referencing all `Finalized`
// proposals at that instant.
//
// The transition from "last finalize observed" → "snapshot written" is
// atomic with respect to the checkout set (Spec 8 §3.4.3, Kingsbury K.2
// FATAL fold). Under `SnapshotLock`:
// - new checkouts on this target are rejected with
//   `WriteError::SnapshotInProgress`
// - the set of `Finalized` proposals is read atomically
// - the `ConsensusSnapshot` is written
// - the lock is released
//
// Lock budget: 50ms (one extension to 100ms permitted before the immune
// system flags consensus mechanism as anomalous).

use crate::identity::VoicePrint;
use crate::signature::LineageId;
use crate::temporal::FabricInstant;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tunable knobs for the semantic edit state machine. Defaults match
/// Spec 8 §3.4.1 and §3.4.3.
#[derive(Debug, Clone, Copy)]
pub struct SemanticEditConfig {
    /// Default TTL for a checkout if the caller doesn't override.
    pub default_checkout_ttl: Duration,
    /// `SnapshotLock` budget. 50ms per Spec 8 §3.4.3.
    pub snapshot_lock_budget: Duration,
    /// One extension permitted before flagging an anomaly.
    pub snapshot_lock_extension: Duration,
}

impl Default for SemanticEditConfig {
    fn default() -> Self {
        Self {
            default_checkout_ttl: Duration::from_secs(15 * 60),
            snapshot_lock_budget: Duration::from_millis(50),
            snapshot_lock_extension: Duration::from_millis(100),
        }
    }
}

/// State of an outstanding checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutStatus {
    Open,
    Finalized,
    Expired,
}

/// State of a proposal under a checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    /// Caller may revise via further `propose` calls on the same checkout.
    Draft,
    /// Locked-in. Counts in the next `ConsensusSnapshot`.
    Finalized,
    /// Superseded by a newer proposal on the same checkout.
    Superseded,
    /// Dropped because the checkout TTL elapsed before finalization.
    Dropped,
}

/// Public-facing handle to a checkout the caller initiated.
#[derive(Debug, Clone)]
pub struct CheckoutHandle {
    pub id: LineageId,
    pub target: LineageId,
    pub status: CheckoutStatus,
}

/// Public-facing handle to a proposal under a checkout.
#[derive(Debug, Clone)]
pub struct ProposalHandle {
    pub id: LineageId,
    pub checkout: LineageId,
    pub status: ProposalStatus,
}

/// A consensus-snapshot record. Written by the fabric when the last
/// outstanding checkout on a target finalizes or expires. The
/// subscriber (Nisaba in production) processes the entire vector as a
/// batch with no observation-order dependency (Spec 8 §3.4.3).
#[derive(Debug, Clone)]
pub struct ConsensusSnapshot {
    pub id: LineageId,
    pub target: LineageId,
    pub finalized_proposals: Vec<LineageId>,
    pub written_at: FabricInstant,
}

// ── Internal entries ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct CheckoutEntry {
    pub target: LineageId,
    /// Surfaced to subscribers and to debug endpoints. The pattern-match
    /// engine keys on the materialized Checkout node's metadata, not on
    /// this in-memory copy, so the field is held but not read in v1.
    #[allow(dead_code)]
    pub rationale: String,
    /// Read by Step 4 (subscriptions) and Spec 6 immune system to
    /// baseline who is opening checkouts in which region.
    #[allow(dead_code)]
    pub signer_voice: VoicePrint,
    pub opened_at: Instant,
    pub ttl: Duration,
    pub status: CheckoutStatus,
}

impl CheckoutEntry {
    pub(crate) fn ttl_expired(&self) -> bool {
        self.opened_at.elapsed() >= self.ttl
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProposalEntry {
    pub checkout: LineageId,
    /// Read by Step 4 (subscriptions) so the immune system can baseline
    /// who is proposing what.
    #[allow(dead_code)]
    pub signer_voice: VoicePrint,
    pub status: ProposalStatus,
}

/// Per-target state for the semantic edit machine.
#[derive(Debug, Default)]
pub(crate) struct TargetState {
    /// Active checkouts on this target, keyed by checkout LineageId.
    pub checkouts: HashMap<LineageId, CheckoutEntry>,
    /// Active proposals on this target, keyed by proposal LineageId.
    /// Each proposal references one checkout via `proposal.checkout`.
    pub proposals: HashMap<LineageId, ProposalEntry>,
    /// True while a `SnapshotLock` is held (Spec 8 §3.4.3).
    /// New `checkout` calls during this window get
    /// `WriteError::SnapshotInProgress`.
    pub snapshot_in_progress: bool,
    /// LineageId of the most recent ConsensusSnapshot written for this
    /// target, if any.
    pub last_snapshot: Option<LineageId>,
}

/// Outcome of `record_finalize`: did this finalize close the round?
#[derive(Debug)]
pub(crate) enum FinalizeOutcome {
    /// Some checkouts on the target are still Open. No snapshot yet.
    StillPending,
    /// This was the last outstanding checkout. The caller must now
    /// take the SnapshotLock and write the ConsensusSnapshot.
    /// Returned vec is the set of currently-Finalized proposal IDs.
    /// The lock is already held when this is returned.
    SnapshotReady(Vec<LineageId>),
}

/// Tracks semantic-edit state across all targets in the fabric.
///
/// The outer Mutex is held briefly during state transitions. The
/// SnapshotLock is implemented by setting the per-target
/// `snapshot_in_progress = true` while the outer Mutex is dropped, so
/// other operations can run on other targets concurrently.
#[derive(Default)]
pub(crate) struct SemanticStateTable {
    /// Per-target state, keyed by the target LineageId.
    targets: Mutex<HashMap<LineageId, TargetState>>,
}

impl SemanticStateTable {
    pub(crate) fn new() -> Self {
        Self { targets: Mutex::new(HashMap::new()) }
    }

    /// Sweep expired checkouts on a target. Drops their unfinalized
    /// proposals (Spec 8 §3.4.5). Returns true if any expirations
    /// happened — caller may want to check whether the target is now
    /// snapshot-ready (no Open checkouts remain).
    pub(crate) fn sweep_expired(&self, target: &LineageId) -> bool {
        let mut targets = self.targets.lock().expect("semantic state poisoned");
        let entry = match targets.get_mut(target) {
            Some(e) => e,
            None => return false,
        };

        let mut expired_ids = Vec::new();
        for (id, checkout) in entry.checkouts.iter() {
            if checkout.status == CheckoutStatus::Open && checkout.ttl_expired() {
                expired_ids.push(id.clone());
            }
        }
        for id in &expired_ids {
            if let Some(co) = entry.checkouts.get_mut(id) {
                co.status = CheckoutStatus::Expired;
            }
            // §3.4.5: drop unfinalized proposals on expired checkouts.
            for p in entry.proposals.values_mut() {
                if &p.checkout == id && p.status == ProposalStatus::Draft {
                    p.status = ProposalStatus::Dropped;
                }
            }
        }
        !expired_ids.is_empty()
    }

    /// Try to register a new checkout. Fails with
    /// `Err(())` if the target is currently under SnapshotLock —
    /// caller translates to `WriteError::SnapshotInProgress`.
    pub(crate) fn try_register_checkout(
        &self,
        checkout_id: LineageId,
        entry: CheckoutEntry,
    ) -> Result<(), ()> {
        let mut targets = self.targets.lock().expect("semantic state poisoned");
        let target_state = targets
            .entry(entry.target.clone())
            .or_insert_with(TargetState::default);
        if target_state.snapshot_in_progress {
            return Err(());
        }
        target_state.checkouts.insert(checkout_id, entry);
        Ok(())
    }

    /// Register a new proposal under an existing Open checkout.
    /// Returns the predecessor proposal ID if the new proposal
    /// supersedes an earlier draft on the same checkout (caller may
    /// want to record the supersession in the fabric graph).
    pub(crate) fn register_proposal(
        &self,
        proposal_id: LineageId,
        target: LineageId,
        checkout: LineageId,
        entry: ProposalEntry,
    ) -> Result<Option<LineageId>, ProposalRegisterError> {
        let mut targets = self.targets.lock().expect("semantic state poisoned");
        let target_state = targets.get_mut(&target).ok_or(ProposalRegisterError::CheckoutNotFound)?;
        let co = target_state
            .checkouts
            .get(&checkout)
            .ok_or(ProposalRegisterError::CheckoutNotFound)?;
        match co.status {
            CheckoutStatus::Finalized => return Err(ProposalRegisterError::CheckoutClosed),
            CheckoutStatus::Expired => return Err(ProposalRegisterError::CheckoutExpired),
            CheckoutStatus::Open => {}
        }

        // Supersede any existing Draft proposal on this checkout.
        let mut superseded: Option<LineageId> = None;
        for (pid, p) in target_state.proposals.iter_mut() {
            if p.checkout == checkout && p.status == ProposalStatus::Draft {
                p.status = ProposalStatus::Superseded;
                superseded = Some(pid.clone());
            }
        }

        target_state.proposals.insert(proposal_id, entry);
        Ok(superseded)
    }

    /// Mark a proposal as Finalized AND mark its checkout as Finalized.
    /// Returns whether the round is now ready for snapshot.
    ///
    /// CRITICAL: if `SnapshotReady`, this method *also* sets
    /// `snapshot_in_progress = true` on the target before releasing
    /// the outer mutex. That's the start of the SnapshotLock window.
    /// The caller MUST follow up by writing the ConsensusSnapshot
    /// and calling `release_snapshot_lock`.
    pub(crate) fn record_finalize(
        &self,
        proposal_id: &LineageId,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let mut targets = self.targets.lock().expect("semantic state poisoned");

        // Find which target hosts this proposal.
        let target_id = targets
            .iter()
            .find(|(_, ts)| ts.proposals.contains_key(proposal_id))
            .map(|(id, _)| id.clone())
            .ok_or(FinalizeError::ProposalNotFound)?;

        let ts = targets.get_mut(&target_id).expect("found above");
        let proposal = ts.proposals.get_mut(proposal_id).expect("found above");
        match proposal.status {
            ProposalStatus::Finalized => return Err(FinalizeError::AlreadyFinalized),
            ProposalStatus::Superseded => return Err(FinalizeError::Superseded),
            ProposalStatus::Dropped => return Err(FinalizeError::Dropped),
            ProposalStatus::Draft => {
                proposal.status = ProposalStatus::Finalized;
            }
        }
        let checkout_id = proposal.checkout.clone();
        if let Some(co) = ts.checkouts.get_mut(&checkout_id) {
            if co.status == CheckoutStatus::Open {
                co.status = CheckoutStatus::Finalized;
            }
        }

        // Sweep any TTL-expired checkouts in passing — they no longer
        // count as "outstanding."
        let mut newly_expired = Vec::new();
        for (id, co) in ts.checkouts.iter() {
            if co.status == CheckoutStatus::Open && co.ttl_expired() {
                newly_expired.push(id.clone());
            }
        }
        for id in &newly_expired {
            if let Some(co) = ts.checkouts.get_mut(id) {
                co.status = CheckoutStatus::Expired;
            }
            for p in ts.proposals.values_mut() {
                if &p.checkout == id && p.status == ProposalStatus::Draft {
                    p.status = ProposalStatus::Dropped;
                }
            }
        }

        // Are any checkouts on this target still Open?
        let still_open = ts
            .checkouts
            .values()
            .any(|c| c.status == CheckoutStatus::Open);

        if still_open {
            return Ok(FinalizeOutcome::StillPending);
        }

        // Take the SnapshotLock atomically — the same Mutex region that
        // observed "no Open checkouts left" sets `snapshot_in_progress`.
        ts.snapshot_in_progress = true;

        let finalized_proposals: Vec<LineageId> = ts
            .proposals
            .iter()
            .filter(|(_, p)| p.status == ProposalStatus::Finalized)
            .map(|(id, _)| id.clone())
            .collect();

        Ok(FinalizeOutcome::SnapshotReady(finalized_proposals))
    }

    /// Release the SnapshotLock and record the ConsensusSnapshot's
    /// LineageId. Caller invokes this after writing the ConsensusSnapshot
    /// node into the inner fabric.
    pub(crate) fn release_snapshot_lock(&self, target: &LineageId, snapshot_id: LineageId) {
        let mut targets = self.targets.lock().expect("semantic state poisoned");
        if let Some(ts) = targets.get_mut(target) {
            ts.snapshot_in_progress = false;
            ts.last_snapshot = Some(snapshot_id);
            // Per Spec 8 §3.4.4: new checkouts arriving after the
            // snapshot start a new round on whatever was committed.
            // In practice that means we leave Finalized proposals in
            // place (they're the committed state) and accept new
            // checkouts going forward.
        }
    }

    /// Walk every (target, state) tuple briefly under the outer lock.
    /// Used by `propose` to find which target hosts a given checkout.
    pub(crate) fn with_each_target<F>(&self, mut f: F)
    where
        F: FnMut(&LineageId, &TargetState),
    {
        let targets = self.targets.lock().expect("semantic state poisoned");
        for (id, state) in targets.iter() {
            f(id, state);
        }
    }

    /// Find the target that hosts the given proposal. Returns the
    /// target's LineageId if found.
    pub(crate) fn target_of_proposal(&self, proposal: &LineageId) -> Option<LineageId> {
        let targets = self.targets.lock().expect("semantic state poisoned");
        for (target_id, state) in targets.iter() {
            if state.proposals.contains_key(proposal) {
                return Some(target_id.clone());
            }
        }
        None
    }

    /// Number of currently-Open checkouts on `target`. Spec 7 Step 5
    /// uses this to detect concurrent decision proposals: when comms
    /// would open a second checkout on a target that already has one
    /// Open, it writes a `ConflictDetected` marker to the thread.
    pub(crate) fn open_checkout_count(&self, target: &LineageId) -> usize {
        let targets = self.targets.lock().expect("semantic state poisoned");
        targets
            .get(target)
            .map(|ts| {
                ts.checkouts
                    .values()
                    .filter(|c| c.status == CheckoutStatus::Open)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Read-only accessor for tests and operators.
    #[cfg(test)]
    pub(crate) fn snapshot_in_progress(&self, target: &LineageId) -> bool {
        let targets = self.targets.lock().unwrap();
        targets
            .get(target)
            .map(|ts| ts.snapshot_in_progress)
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn checkout_status(&self, target: &LineageId, id: &LineageId) -> Option<CheckoutStatus> {
        let targets = self.targets.lock().unwrap();
        targets.get(target)?.checkouts.get(id).map(|c| c.status)
    }

    #[cfg(test)]
    pub(crate) fn proposal_status(&self, target: &LineageId, id: &LineageId) -> Option<ProposalStatus> {
        let targets = self.targets.lock().unwrap();
        targets.get(target)?.proposals.get(id).map(|p| p.status)
    }

    #[cfg(test)]
    pub(crate) fn last_snapshot(&self, target: &LineageId) -> Option<LineageId> {
        let targets = self.targets.lock().unwrap();
        targets.get(target).and_then(|ts| ts.last_snapshot.clone())
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum ProposalRegisterError {
    CheckoutNotFound,
    CheckoutClosed,
    CheckoutExpired,
}

#[derive(Debug, PartialEq)]
pub(crate) enum FinalizeError {
    ProposalNotFound,
    AlreadyFinalized,
    Superseded,
    Dropped,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    fn fresh_checkout_entry(target: LineageId, ttl: Duration) -> CheckoutEntry {
        let agent = generate_agent_keypair();
        CheckoutEntry {
            target,
            rationale: "test".into(),
            signer_voice: agent.voice_print(),
            opened_at: Instant::now(),
            ttl,
            status: CheckoutStatus::Open,
        }
    }

    fn fresh_proposal_entry(checkout: LineageId) -> ProposalEntry {
        let agent = generate_agent_keypair();
        ProposalEntry {
            checkout,
            signer_voice: agent.voice_print(),
            status: ProposalStatus::Draft,
        }
    }

    #[test]
    fn single_finalize_completes_round() {
        let table = SemanticStateTable::new();
        let target = LineageId::new();
        let checkout_id = LineageId::new();
        let proposal_id = LineageId::new();

        table
            .try_register_checkout(
                checkout_id.clone(),
                fresh_checkout_entry(target.clone(), Duration::from_secs(60)),
            )
            .unwrap();
        table
            .register_proposal(
                proposal_id.clone(),
                target.clone(),
                checkout_id.clone(),
                fresh_proposal_entry(checkout_id.clone()),
            )
            .unwrap();

        let outcome = table.record_finalize(&proposal_id).unwrap();
        match outcome {
            FinalizeOutcome::SnapshotReady(ids) => {
                assert_eq!(ids, vec![proposal_id.clone()]);
                assert!(table.snapshot_in_progress(&target),
                    "Last finalize must take the SnapshotLock atomically (Spec 8 §3.4.3).");
            }
            FinalizeOutcome::StillPending => {
                panic!("Single checkout that finalizes must complete the round.");
            }
        }

        let snapshot_id = LineageId::new();
        table.release_snapshot_lock(&target, snapshot_id.clone());
        assert!(!table.snapshot_in_progress(&target));
        assert_eq!(table.last_snapshot(&target), Some(snapshot_id));
    }

    #[test]
    fn checkout_during_snapshot_lock_is_rejected() {
        // Spec 8 §3.4.3 — atomic SnapshotLock: a new checkout arriving
        // during the lock window MUST be rejected.
        let table = SemanticStateTable::new();
        let target = LineageId::new();

        let co1 = LineageId::new();
        let p1 = LineageId::new();
        table
            .try_register_checkout(
                co1.clone(),
                fresh_checkout_entry(target.clone(), Duration::from_secs(60)),
            )
            .unwrap();
        table
            .register_proposal(p1.clone(), target.clone(), co1.clone(), fresh_proposal_entry(co1.clone()))
            .unwrap();
        let _ = table.record_finalize(&p1).unwrap();
        // SnapshotLock is now held.

        // A new checkout arriving in the lock window must fail.
        let co2 = LineageId::new();
        let result = table.try_register_checkout(
            co2.clone(),
            fresh_checkout_entry(target.clone(), Duration::from_secs(60)),
        );
        assert!(result.is_err(),
            "Concurrent checkout during SnapshotLock must be rejected with SnapshotInProgress.");

        // After release, new checkouts succeed.
        table.release_snapshot_lock(&target, LineageId::new());
        let co3 = LineageId::new();
        let result2 = table.try_register_checkout(
            co3.clone(),
            fresh_checkout_entry(target.clone(), Duration::from_secs(60)),
        );
        assert!(result2.is_ok(),
            "After SnapshotLock releases, new checkouts must be accepted (Spec 8 §3.4.4).");
    }

    #[test]
    fn multiple_checkouts_one_finalized_keeps_round_open() {
        let table = SemanticStateTable::new();
        let target = LineageId::new();
        let co1 = LineageId::new();
        let co2 = LineageId::new();
        let p1 = LineageId::new();
        let p2 = LineageId::new();

        table
            .try_register_checkout(co1.clone(), fresh_checkout_entry(target.clone(), Duration::from_secs(60)))
            .unwrap();
        table
            .try_register_checkout(co2.clone(), fresh_checkout_entry(target.clone(), Duration::from_secs(60)))
            .unwrap();
        table
            .register_proposal(p1.clone(), target.clone(), co1.clone(), fresh_proposal_entry(co1.clone()))
            .unwrap();
        table
            .register_proposal(p2.clone(), target.clone(), co2.clone(), fresh_proposal_entry(co2.clone()))
            .unwrap();

        // Finalize only p1. p2's checkout still Open.
        let outcome = table.record_finalize(&p1).unwrap();
        assert!(matches!(outcome, FinalizeOutcome::StillPending),
            "If any checkouts remain Open, the round is still pending.");
        assert!(!table.snapshot_in_progress(&target));

        // Finalize p2 → round closes.
        let outcome2 = table.record_finalize(&p2).unwrap();
        match outcome2 {
            FinalizeOutcome::SnapshotReady(ids) => {
                assert_eq!(ids.len(), 2,
                    "ConsensusSnapshot includes both finalized proposals.");
                assert!(ids.contains(&p1));
                assert!(ids.contains(&p2));
            }
            FinalizeOutcome::StillPending => panic!("All checkouts settled — should be ready."),
        }
    }

    #[test]
    fn proposal_supersession_marks_predecessor() {
        let table = SemanticStateTable::new();
        let target = LineageId::new();
        let co = LineageId::new();
        let p1 = LineageId::new();
        let p2 = LineageId::new();

        table
            .try_register_checkout(co.clone(), fresh_checkout_entry(target.clone(), Duration::from_secs(60)))
            .unwrap();
        table
            .register_proposal(p1.clone(), target.clone(), co.clone(), fresh_proposal_entry(co.clone()))
            .unwrap();

        let predecessor = table
            .register_proposal(p2.clone(), target.clone(), co.clone(), fresh_proposal_entry(co.clone()))
            .unwrap();
        assert_eq!(predecessor, Some(p1.clone()));
        assert_eq!(
            table.proposal_status(&target, &p1),
            Some(ProposalStatus::Superseded)
        );
        assert_eq!(
            table.proposal_status(&target, &p2),
            Some(ProposalStatus::Draft)
        );
    }

    #[test]
    fn expired_checkout_drops_unfinalized_proposals() {
        let table = SemanticStateTable::new();
        let target = LineageId::new();
        let co = LineageId::new();
        let p = LineageId::new();

        // Zero TTL → already expired.
        table
            .try_register_checkout(co.clone(), fresh_checkout_entry(target.clone(), Duration::from_nanos(1)))
            .unwrap();
        table
            .register_proposal(p.clone(), target.clone(), co.clone(), fresh_proposal_entry(co.clone()))
            .unwrap();

        // Let the TTL elapse.
        std::thread::sleep(Duration::from_millis(2));
        let any_expired = table.sweep_expired(&target);
        assert!(any_expired);
        assert_eq!(table.checkout_status(&target, &co), Some(CheckoutStatus::Expired));
        assert_eq!(table.proposal_status(&target, &p), Some(ProposalStatus::Dropped),
            "Spec 8 §3.4.5: unfinalized proposals on expired checkouts are dropped.");
    }

    #[test]
    fn cannot_finalize_already_finalized() {
        let table = SemanticStateTable::new();
        let target = LineageId::new();
        let co = LineageId::new();
        let p = LineageId::new();

        table
            .try_register_checkout(co.clone(), fresh_checkout_entry(target.clone(), Duration::from_secs(60)))
            .unwrap();
        table
            .register_proposal(p.clone(), target.clone(), co.clone(), fresh_proposal_entry(co.clone()))
            .unwrap();

        let _ = table.record_finalize(&p).unwrap();
        let again = table.record_finalize(&p);
        assert_eq!(again.unwrap_err(), FinalizeError::AlreadyFinalized);
    }
}
