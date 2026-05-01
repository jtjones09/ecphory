// MECHANICAL EDITS — Per-node lock with 500ms target (Spec 8 §3.2.2)
//
// Per spec §3.2.2:
// > Updating a counter, a timestamp, a weight, a last-observed-at field.
// > The specific new value matters less than the fact that *some* agent
// > has updated the value. These take an exclusive per-node lock with a
// > short timeout (default 500ms). First writer wins, loser receives
// > `WriteError::NodeLocked { by, until_ns }` and retries.
//
// v1 strategy (per Jeremy's call): try-lock fail-fast. The 500ms is
// communicated to the caller via `until_ns` as an SLA hint; the actual
// hold time is bounded by the operation, which for simple mutations is
// microseconds. v1.5 may add active spin-wait if real contention shows.

use crate::identity::{ContentFingerprint, VoicePrint};
use crate::signature::LineageId;
use crate::temporal::FabricInstant;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Receipt issued by `edit_mechanical` on success (Spec 8 §7).
#[derive(Debug, Clone)]
pub struct EditReceipt {
    pub target: LineageId,
    pub previous_content_fingerprint: ContentFingerprint,
    pub new_content_fingerprint: ContentFingerprint,
    pub editor_voice: VoicePrint,
    pub commit_instant: FabricInstant,
}

/// State of one currently-held per-node lock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LockHolder {
    pub holder: VoicePrint,
    /// Wall-clock instant when the lock was acquired. Used to compute
    /// the `until_ns` deadline reported to callers.
    pub acquired_at: Instant,
}

impl LockHolder {
    /// 500ms deadline per Spec 8 §3.2.2. Returned to callers as a hint.
    pub fn deadline_ns(&self) -> i128 {
        const TARGET_HOLD_NS: u128 = 500_000_000; // 500ms in nanoseconds
        // Approximate wall-clock ns since UNIX epoch from the
        // `acquired_at` snapshot is not directly available from `Instant`
        // (monotonic-only). Use the system time at the call site instead.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        let elapsed_ns = self.acquired_at.elapsed().as_nanos() as i128;
        // deadline ≈ now - elapsed + target = "when the holder must release"
        now - elapsed_ns + TARGET_HOLD_NS as i128
    }
}

/// Tracks active per-node locks for `EditMode::Mechanical` writes.
///
/// Implemented as `Mutex<HashMap<LineageId, LockHolder>>`. The outer
/// Mutex is held only briefly during acquire/release; per-node serial
/// access is enforced by the entry's presence (try-lock fail-fast).
#[derive(Default)]
pub(crate) struct MechanicalLockTable {
    locks: Mutex<HashMap<LineageId, LockHolder>>,
}

/// Outcome of a try-acquire on a per-node mechanical lock.
pub(crate) enum AcquireResult {
    Acquired,
    Held(LockHolder),
}

impl MechanicalLockTable {
    pub(crate) fn new() -> Self {
        Self { locks: Mutex::new(HashMap::new()) }
    }

    /// Try to acquire the per-node lock. Fail-fast on contention (v1).
    pub(crate) fn try_acquire(&self, target: LineageId, holder: VoicePrint) -> AcquireResult {
        let mut table = self.locks.lock().expect("mechanical lock table poisoned");
        if let Some(existing) = table.get(&target) {
            return AcquireResult::Held(*existing);
        }
        table.insert(
            target,
            LockHolder { holder, acquired_at: Instant::now() },
        );
        AcquireResult::Acquired
    }

    pub(crate) fn release(&self, target: &LineageId) {
        let mut table = self.locks.lock().expect("mechanical lock table poisoned");
        table.remove(target);
    }

    #[cfg(test)]
    pub(crate) fn is_held(&self, target: &LineageId) -> bool {
        self.locks.lock().unwrap().contains_key(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    #[test]
    fn first_acquire_succeeds() {
        let table = MechanicalLockTable::new();
        let id = LineageId::new();
        let agent = generate_agent_keypair();
        match table.try_acquire(id.clone(), agent.voice_print()) {
            AcquireResult::Acquired => {}
            AcquireResult::Held(_) => panic!("first acquire must succeed"),
        }
        assert!(table.is_held(&id));
    }

    #[test]
    fn second_acquire_on_same_node_reports_held() {
        let table = MechanicalLockTable::new();
        let id = LineageId::new();
        let alice = generate_agent_keypair();
        let bob = generate_agent_keypair();

        let _ = table.try_acquire(id.clone(), alice.voice_print());
        match table.try_acquire(id.clone(), bob.voice_print()) {
            AcquireResult::Held(holder) => {
                assert_eq!(holder.holder, alice.voice_print(),
                    "Lock must report Alice as the holder so Bob knows whom he's contending with.");
            }
            AcquireResult::Acquired => panic!("contention should fail-fast"),
        }
    }

    #[test]
    fn release_allows_subsequent_acquire() {
        let table = MechanicalLockTable::new();
        let id = LineageId::new();
        let alice = generate_agent_keypair();
        let bob = generate_agent_keypair();

        let _ = table.try_acquire(id.clone(), alice.voice_print());
        table.release(&id);

        match table.try_acquire(id.clone(), bob.voice_print()) {
            AcquireResult::Acquired => {}
            AcquireResult::Held(_) => panic!("after release, next acquire must succeed"),
        }
    }

    #[test]
    fn locks_are_per_node() {
        let table = MechanicalLockTable::new();
        let a = LineageId::new();
        let b = LineageId::new();
        let alice = generate_agent_keypair();
        let bob = generate_agent_keypair();

        let _ = table.try_acquire(a.clone(), alice.voice_print());
        match table.try_acquire(b.clone(), bob.voice_print()) {
            AcquireResult::Acquired => {}
            AcquireResult::Held(_) => panic!("locks must be per-node, not global"),
        }
    }
}
