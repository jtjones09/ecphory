//! Persistence region — beliefs about whether the snapshot pathway is
//! healthy. The Parallels lesson lives here as a structural prior:
//! `p(broken_cache | untested, persist_naive) > p(broken_cache |
//! untested, persist_atomic)`. The agent starts with the safer
//! atomic-commit-with-flush behavior.
//!
//! State space (7): no_storage, untested, working, broken_cache,
//! broken_format, broken_hardware, degraded.
//! Observation space (7): no_device, fresh_nucleation, restored_ok,
//! checksum_mismatch, partial_restore, corrupt_data, io_error.
//! Action space (6): persist_naive, persist_atomic, persist_verify,
//! persist_double_flush, diagnose, skip.

use alloc::vec::Vec;

use crate::model::discrete::DiscreteModel;

pub const N_STATES: usize = 7;
pub const N_OBSERVATIONS: usize = 7;
pub const N_ACTIONS: usize = 6;

pub const STATE_NO_STORAGE: usize = 0;
pub const STATE_UNTESTED: usize = 1;
pub const STATE_WORKING: usize = 2;
pub const STATE_BROKEN_CACHE: usize = 3;
pub const STATE_BROKEN_FORMAT: usize = 4;
pub const STATE_BROKEN_HARDWARE: usize = 5;
pub const STATE_DEGRADED: usize = 6;

pub const OBS_NO_DEVICE: usize = 0;
pub const OBS_FRESH_NUCLEATION: usize = 1;
pub const OBS_RESTORED_OK: usize = 2;
pub const OBS_CHECKSUM_MISMATCH: usize = 3;
pub const OBS_PARTIAL_RESTORE: usize = 4;
pub const OBS_CORRUPT_DATA: usize = 5;
pub const OBS_IO_ERROR: usize = 6;

pub const ACT_PERSIST_NAIVE: usize = 0;
pub const ACT_PERSIST_ATOMIC: usize = 1;
pub const ACT_PERSIST_VERIFY: usize = 2;
pub const ACT_PERSIST_DOUBLE_FLUSH: usize = 3;
pub const ACT_DIAGNOSE: usize = 4;
pub const ACT_SKIP: usize = 5;

const STATE_LABELS: &[&str] = &[
    "no-storage",
    "untested",
    "working",
    "broken-cache",
    "broken-format",
    "broken-hw",
    "degraded",
];
const OBS_LABELS: &[&str] = &[
    "no-device",
    "fresh-nucleation",
    "restored-ok",
    "checksum-mismatch",
    "partial-restore",
    "corrupt-data",
    "io-error",
];
const ACTION_LABELS: &[&str] = &[
    "persist-naive",
    "persist-atomic",
    "persist-verify",
    "persist-double-flush",
    "diagnose",
    "skip",
];

pub struct PersistenceRegion {
    pub model: DiscreteModel,
    pub last_action: usize,
    pub last_observation: usize,
    pub successful_persists: u64,
    pub failed_persists: u64,
    pub successful_restores: u64,
    pub failed_restores: u64,
    /// Persist gate: when the model has accumulated more than this much
    /// surprise since the last successful persist, the inference loop
    /// triggers a snapshot. Replaces the old hardcoded
    /// `PERSIST_EVERY_LAMPORT` cadence with a model-resident parameter
    /// the agent can later learn to tune. Default 5.0; lower → persist
    /// more often, higher → persist less often. Operator-typed
    /// `> persist` bumps the accumulator above any reasonable threshold
    /// so the next cycle persists regardless.
    pub persist_threshold: f32,
}

impl PersistenceRegion {
    pub fn nucleation() -> Self {
        let model = DiscreteModel::new(
            N_STATES,
            N_OBSERVATIONS,
            N_ACTIONS,
            build_a(),
            build_b(),
            build_c(),
            build_d(),
        );
        Self {
            model,
            last_action: ACT_SKIP,
            last_observation: OBS_FRESH_NUCLEATION,
            successful_persists: 0,
            failed_persists: 0,
            successful_restores: 0,
            failed_restores: 0,
            persist_threshold: 5.0,
        }
    }

    /// The persist gate: triggers when the model has accumulated more
    /// surprise since the last save than `persist_threshold` allows.
    /// Caller is the inference loop; it passes
    /// `model.cumulative_surprise_since_last_persist` as `info_at_risk`.
    pub fn should_persist_now(&self, info_at_risk: f32) -> bool {
        info_at_risk > self.persist_threshold
    }

    /// The action the agent prefers right now, given current beliefs.
    pub fn select_action(&self) -> usize {
        let (a, _) = self.model.select_action();
        a
    }

    /// Integrate an observation about the persistence pathway.
    pub fn observe(&mut self, action_taken: usize, observation: usize) -> f32 {
        self.last_action = action_taken;
        self.last_observation = observation;
        match observation {
            OBS_RESTORED_OK => self.successful_restores += 1,
            OBS_CHECKSUM_MISMATCH | OBS_PARTIAL_RESTORE | OBS_CORRUPT_DATA => {
                self.failed_restores += 1
            }
            _ => {}
        }
        let s = self.model.observe(action_taken, observation);
        self.model.learn(observation);
        s
    }

    pub fn note_persist_outcome(&mut self, success: bool) {
        if success {
            self.successful_persists += 1;
        } else {
            self.failed_persists += 1;
        }
    }

    pub fn map_state_label(&self) -> &'static str {
        STATE_LABELS[self.model.map_state()]
    }

    pub fn observations_seen(&self) -> u64 {
        self.model.observations_seen
    }

    pub fn average_surprise(&self) -> f32 {
        self.model.average_surprise()
    }

    pub fn snapshot_bytes(&self) -> Vec<u8> {
        self.model.serialize_to_bytes()
    }

    pub fn restore_from_bytes(&mut self, bytes: &[u8]) -> bool {
        if let Some(model) = DiscreteModel::deserialize_from_bytes(bytes) {
            if model.n_states == N_STATES
                && model.n_observations == N_OBSERVATIONS
                && model.n_actions == N_ACTIONS
            {
                self.model = model;
                return true;
            }
        }
        false
    }
}

#[allow(dead_code)]
pub fn state_label(s: usize) -> &'static str {
    STATE_LABELS.get(s).copied().unwrap_or("?")
}
#[allow(dead_code)]
pub fn obs_label(o: usize) -> &'static str {
    OBS_LABELS.get(o).copied().unwrap_or("?")
}
pub fn action_label(a: usize) -> &'static str {
    ACTION_LABELS.get(a).copied().unwrap_or("?")
}

// --- nucleation priors ---

fn build_a() -> Vec<f32> {
    // A[o, s] — what observations to expect in each state. Each column
    // (state) sums to 1.
    let mut a = alloc::vec![0.0f32; N_OBSERVATIONS * N_STATES];
    let set = |a: &mut [f32], o: usize, s: usize, v: f32| {
        a[o * N_STATES + s] = v;
    };
    // no_storage: only OBS_NO_DEVICE
    set(&mut a, OBS_NO_DEVICE, STATE_NO_STORAGE, 0.95);
    set(&mut a, OBS_FRESH_NUCLEATION, STATE_NO_STORAGE, 0.05);
    // untested: fresh nucleation likely; a few odd outcomes possible
    set(&mut a, OBS_FRESH_NUCLEATION, STATE_UNTESTED, 0.55);
    set(&mut a, OBS_RESTORED_OK, STATE_UNTESTED, 0.20);
    set(&mut a, OBS_CHECKSUM_MISMATCH, STATE_UNTESTED, 0.10);
    set(&mut a, OBS_PARTIAL_RESTORE, STATE_UNTESTED, 0.05);
    set(&mut a, OBS_CORRUPT_DATA, STATE_UNTESTED, 0.05);
    set(&mut a, OBS_IO_ERROR, STATE_UNTESTED, 0.05);
    // working: nearly always restored_ok
    set(&mut a, OBS_RESTORED_OK, STATE_WORKING, 0.90);
    set(&mut a, OBS_FRESH_NUCLEATION, STATE_WORKING, 0.05);
    set(&mut a, OBS_CHECKSUM_MISMATCH, STATE_WORKING, 0.02);
    set(&mut a, OBS_IO_ERROR, STATE_WORKING, 0.03);
    // broken_cache: characteristic = checksum_mismatch (the Parallels bug)
    set(&mut a, OBS_CHECKSUM_MISMATCH, STATE_BROKEN_CACHE, 0.65);
    set(&mut a, OBS_PARTIAL_RESTORE, STATE_BROKEN_CACHE, 0.20);
    set(&mut a, OBS_FRESH_NUCLEATION, STATE_BROKEN_CACHE, 0.10);
    set(&mut a, OBS_RESTORED_OK, STATE_BROKEN_CACHE, 0.05);
    // broken_format: corrupt_data dominates
    set(&mut a, OBS_CORRUPT_DATA, STATE_BROKEN_FORMAT, 0.70);
    set(&mut a, OBS_CHECKSUM_MISMATCH, STATE_BROKEN_FORMAT, 0.20);
    set(&mut a, OBS_PARTIAL_RESTORE, STATE_BROKEN_FORMAT, 0.10);
    // broken_hardware: io_error dominates
    set(&mut a, OBS_IO_ERROR, STATE_BROKEN_HARDWARE, 0.65);
    set(&mut a, OBS_NO_DEVICE, STATE_BROKEN_HARDWARE, 0.20);
    set(&mut a, OBS_PARTIAL_RESTORE, STATE_BROKEN_HARDWARE, 0.10);
    set(&mut a, OBS_CORRUPT_DATA, STATE_BROKEN_HARDWARE, 0.05);
    // degraded: mostly works, occasional issue
    set(&mut a, OBS_RESTORED_OK, STATE_DEGRADED, 0.55);
    set(&mut a, OBS_PARTIAL_RESTORE, STATE_DEGRADED, 0.15);
    set(&mut a, OBS_CHECKSUM_MISMATCH, STATE_DEGRADED, 0.10);
    set(&mut a, OBS_IO_ERROR, STATE_DEGRADED, 0.10);
    set(&mut a, OBS_CORRUPT_DATA, STATE_DEGRADED, 0.10);
    a
}

fn build_b() -> Vec<f32> {
    let mut b = alloc::vec![0.0f32; N_ACTIONS * N_STATES * N_STATES];
    let set = |b: &mut [f32], a: usize, sf: usize, st: usize, v: f32| {
        b[a * N_STATES * N_STATES + sf * N_STATES + st] = v;
    };

    // persist_naive: untested + naive → broken_cache (the lesson the
    // prior encodes; the agent starts with this stronger than 50%).
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_PERSIST_NAIVE, sf, st, 0.05);
        }
    }
    set(&mut b, ACT_PERSIST_NAIVE, STATE_NO_STORAGE, STATE_NO_STORAGE, 0.85);
    set(&mut b, ACT_PERSIST_NAIVE, STATE_UNTESTED, STATE_BROKEN_CACHE, 0.60);
    set(&mut b, ACT_PERSIST_NAIVE, STATE_UNTESTED, STATE_WORKING, 0.30);
    set(&mut b, ACT_PERSIST_NAIVE, STATE_WORKING, STATE_WORKING, 0.55);
    set(&mut b, ACT_PERSIST_NAIVE, STATE_WORKING, STATE_BROKEN_CACHE, 0.30);
    set(&mut b, ACT_PERSIST_NAIVE, STATE_BROKEN_CACHE, STATE_BROKEN_CACHE, 0.85);
    set(
        &mut b,
        ACT_PERSIST_NAIVE,
        STATE_BROKEN_FORMAT,
        STATE_BROKEN_FORMAT,
        0.85,
    );
    set(
        &mut b,
        ACT_PERSIST_NAIVE,
        STATE_BROKEN_HARDWARE,
        STATE_BROKEN_HARDWARE,
        0.85,
    );
    set(&mut b, ACT_PERSIST_NAIVE, STATE_DEGRADED, STATE_DEGRADED, 0.65);
    set(&mut b, ACT_PERSIST_NAIVE, STATE_DEGRADED, STATE_BROKEN_CACHE, 0.20);

    // persist_atomic: untested → working (the safer behavior; this is
    // what the kernel actually does today).
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_PERSIST_ATOMIC, sf, st, 0.05);
        }
    }
    set(&mut b, ACT_PERSIST_ATOMIC, STATE_NO_STORAGE, STATE_NO_STORAGE, 0.85);
    set(&mut b, ACT_PERSIST_ATOMIC, STATE_UNTESTED, STATE_WORKING, 0.75);
    set(&mut b, ACT_PERSIST_ATOMIC, STATE_UNTESTED, STATE_BROKEN_CACHE, 0.05);
    set(&mut b, ACT_PERSIST_ATOMIC, STATE_WORKING, STATE_WORKING, 0.85);
    set(
        &mut b,
        ACT_PERSIST_ATOMIC,
        STATE_BROKEN_CACHE,
        STATE_WORKING,
        0.55,
    );
    set(
        &mut b,
        ACT_PERSIST_ATOMIC,
        STATE_BROKEN_CACHE,
        STATE_BROKEN_CACHE,
        0.30,
    );
    set(
        &mut b,
        ACT_PERSIST_ATOMIC,
        STATE_BROKEN_FORMAT,
        STATE_BROKEN_FORMAT,
        0.85,
    );
    set(
        &mut b,
        ACT_PERSIST_ATOMIC,
        STATE_BROKEN_HARDWARE,
        STATE_BROKEN_HARDWARE,
        0.85,
    );
    set(&mut b, ACT_PERSIST_ATOMIC, STATE_DEGRADED, STATE_WORKING, 0.50);
    set(&mut b, ACT_PERSIST_ATOMIC, STATE_DEGRADED, STATE_DEGRADED, 0.40);

    // persist_verify, persist_double_flush: similar to atomic but
    // dampen risk a bit further.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_PERSIST_VERIFY, sf, st, 0.04);
            set(&mut b, ACT_PERSIST_DOUBLE_FLUSH, sf, st, 0.04);
        }
    }
    set(&mut b, ACT_PERSIST_VERIFY, STATE_UNTESTED, STATE_WORKING, 0.80);
    set(&mut b, ACT_PERSIST_VERIFY, STATE_WORKING, STATE_WORKING, 0.90);
    set(&mut b, ACT_PERSIST_VERIFY, STATE_BROKEN_FORMAT, STATE_WORKING, 0.30);
    set(
        &mut b,
        ACT_PERSIST_VERIFY,
        STATE_BROKEN_FORMAT,
        STATE_BROKEN_FORMAT,
        0.55,
    );
    set(
        &mut b,
        ACT_PERSIST_DOUBLE_FLUSH,
        STATE_UNTESTED,
        STATE_WORKING,
        0.85,
    );
    set(
        &mut b,
        ACT_PERSIST_DOUBLE_FLUSH,
        STATE_BROKEN_CACHE,
        STATE_WORKING,
        0.65,
    );
    set(
        &mut b,
        ACT_PERSIST_DOUBLE_FLUSH,
        STATE_WORKING,
        STATE_WORKING,
        0.90,
    );

    // diagnose: never changes the world, just refines belief. We model
    // this as near-stationary.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(
                &mut b,
                ACT_DIAGNOSE,
                sf,
                st,
                if sf == st { 0.92 } else { 0.012 },
            );
        }
    }

    // skip: identity transition.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_SKIP, sf, st, if sf == st { 0.95 } else { 0.008 });
        }
    }

    // Renormalise each (a, sf) row to 1.
    for ai in 0..N_ACTIONS {
        for sf in 0..N_STATES {
            let mut sum = 0.0f32;
            for st in 0..N_STATES {
                sum += b[ai * N_STATES * N_STATES + sf * N_STATES + st];
            }
            if sum > 0.0 {
                for st in 0..N_STATES {
                    b[ai * N_STATES * N_STATES + sf * N_STATES + st] /= sum;
                }
            }
        }
    }

    b
}

fn build_c() -> Vec<f32> {
    let mut c = alloc::vec![0.0f32; N_OBSERVATIONS];
    c[OBS_RESTORED_OK] = 2.5;        // strongly preferred
    c[OBS_FRESH_NUCLEATION] = 0.0;   // neutral
    c[OBS_NO_DEVICE] = -1.0;
    c[OBS_PARTIAL_RESTORE] = -2.0;
    c[OBS_CHECKSUM_MISMATCH] = -3.5; // information_loss preference
    c[OBS_CORRUPT_DATA] = -4.0;      // corruption preference
    c[OBS_IO_ERROR] = -3.0;          // nonresponse preference
    c
}

fn build_d() -> Vec<f32> {
    // Bias toward "untested" at nucleation: the kernel doesn't yet
    // know whether the substrate's persistence path works.
    let mut d = alloc::vec![0.0f32; N_STATES];
    d[STATE_NO_STORAGE] = 0.05;
    d[STATE_UNTESTED] = 0.70;
    d[STATE_WORKING] = 0.10;
    d[STATE_BROKEN_CACHE] = 0.05;
    d[STATE_BROKEN_FORMAT] = 0.03;
    d[STATE_BROKEN_HARDWARE] = 0.02;
    d[STATE_DEGRADED] = 0.05;
    d
}
