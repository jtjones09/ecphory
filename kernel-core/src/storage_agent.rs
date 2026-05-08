//! Active inference agent for storage controllers.
//!
//! The agent's generative model:
//! - Hidden states `s` (the controller's internal state):
//!     0: idle      — controller ready, no command pending
//!     1: busy      — controller mid-command, can't accept
//!     2: drq       — data transfer ready (read/write phase)
//!     3: error     — fault flagged
//!     4: degraded  — measurable but not catastrophic deviation from baseline
//! - Observations `o` (what the substrate measured after a command):
//!     0: ok-fast   — completion below median latency
//!     1: ok-slow   — completion above median, no error
//!     2: drq-set   — controller asks for data
//!     3: timeout   — completion did not arrive in window
//!     4: device-error — ERR bit set
//! - Actions `a` (one-step policies the agent can pick):
//!     0: identify  — cheap, exploratory probe
//!     1: read      — cost: latency, info: throughput baseline
//!     2: write     — cost: latency + wear, info: write path baseline
//!     3: flush     — cost: durability barrier, info: cache state
//!     4: wait      — pass time, observe natural decay
//!
//! Initial A is the spec-derived prior: a healthy idle controller emits
//! ok-fast on read/write, drq-set during transfer phases, low error
//! rate. The agent then learns the SPECIFIC controller's deviations
//! from that prior — the "learned driver" the position paper specifies.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::agent::DiscreteModel;
use crate::ops::{BLOCK_SIZE, BlockResult, Op, OpResult, Shim};

pub const N_STATES: usize = 5;
pub const N_OBSERVATIONS: usize = 5;
pub const N_ACTIONS: usize = 5;

pub const STATE_IDLE: usize = 0;
pub const STATE_BUSY: usize = 1;
pub const STATE_DRQ: usize = 2;
pub const STATE_ERROR: usize = 3;
pub const STATE_DEGRADED: usize = 4;

pub const OBS_OK_FAST: usize = 0;
pub const OBS_OK_SLOW: usize = 1;
pub const OBS_DRQ: usize = 2;
pub const OBS_TIMEOUT: usize = 3;
pub const OBS_DEV_ERROR: usize = 4;

pub const ACT_IDENTIFY: usize = 0;
pub const ACT_READ: usize = 1;
pub const ACT_WRITE: usize = 2;
pub const ACT_FLUSH: usize = 3;
pub const ACT_WAIT: usize = 4;

const STATE_LABELS: &[&str] = &["idle", "busy", "drq", "error", "degraded"];
const ACTION_LABELS: &[&str] = &["identify", "read", "write", "flush", "wait"];

pub struct StorageAgent {
    pub model: DiscreteModel,
    pub last_action: usize,
    pub last_observation: usize,
    pub last_latency_ticks: u64,
    pub median_latency_ticks: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub completions: u64,
    pub timeouts: u64,
    pub device_errors: u64,
    pub probe_lba: u32,
    pub label: String,
}

impl StorageAgent {
    pub fn new(label: String) -> Self {
        let a = build_a_prior();
        let b = build_b_prior();
        let c = build_c_preferences();
        let d = build_d_prior();
        let model = DiscreteModel::new(N_STATES, N_OBSERVATIONS, N_ACTIONS, a, b, c, d);
        Self {
            model,
            last_action: ACT_WAIT,
            last_observation: OBS_OK_FAST,
            last_latency_ticks: 0,
            median_latency_ticks: 0,
            bytes_read: 0,
            bytes_written: 0,
            completions: 0,
            timeouts: 0,
            device_errors: 0,
            probe_lba: 8, // first block past the snapshot superblock
            label,
        }
    }

    pub fn map_state_label(&self) -> &'static str {
        STATE_LABELS[self.model.map_state()]
    }

    pub fn last_action_label(&self) -> &'static str {
        ACTION_LABELS[self.last_action]
    }

    /// Run one observe → predict → act → update cycle.
    /// Returns a short human-readable summary line.
    pub fn step<'a, S: Shim + ?Sized>(&mut self, shim: &'a mut S) -> StepReport {
        let (action, _g) = self.model.select_action();
        let t_before = match shim.execute(Op::GetTime) {
            OpResult::Time(t) => t,
            _ => 0,
        };

        let block_result = self.execute_action(shim, action);

        let t_after = match shim.execute(Op::GetTime) {
            OpResult::Time(t) => t,
            _ => t_before,
        };
        let latency = t_after.saturating_sub(t_before);
        self.last_latency_ticks = latency;
        if self.median_latency_ticks == 0 {
            self.median_latency_ticks = latency;
        } else {
            // Slow-decaying running median proxy.
            self.median_latency_ticks = (self.median_latency_ticks * 31 + latency) / 32;
        }

        let observation = classify(&block_result, latency, self.median_latency_ticks);

        let surprise = self.model.observe(action, observation);
        self.model.learn(observation);

        match observation {
            OBS_TIMEOUT => self.timeouts += 1,
            OBS_DEV_ERROR => self.device_errors += 1,
            _ => self.completions += 1,
        }

        self.last_action = action;
        self.last_observation = observation;

        StepReport {
            action,
            observation,
            latency_ticks: latency,
            map_state: self.model.map_state(),
            surprise,
            avg_surprise: self.model.average_surprise(),
        }
    }

    fn execute_action<'a, S: Shim + ?Sized>(
        &mut self,
        shim: &'a mut S,
        action: usize,
    ) -> BlockResult {
        match action {
            ACT_IDENTIFY => BlockResult::Ok, // shim has already identified; the prior reflects that
            ACT_READ => {
                let mut buf = [0u8; BLOCK_SIZE];
                let res = shim.execute(Op::ReadBlock {
                    lba: self.probe_lba,
                    into: &mut buf,
                });
                self.advance_probe();
                self.bytes_read += BLOCK_SIZE as u64;
                match res {
                    OpResult::Block(b) => b,
                    _ => BlockResult::NoDevice,
                }
            }
            ACT_WRITE => {
                // Phase 2 keeps writes safe: write back what we just
                // read, so the block on disk is unchanged. The agent
                // still gets the real latency profile of a write.
                let mut buf = [0u8; BLOCK_SIZE];
                let _ = shim.execute(Op::ReadBlock {
                    lba: self.probe_lba,
                    into: &mut buf,
                });
                let res = shim.execute(Op::WriteBlock {
                    lba: self.probe_lba,
                    from: &buf,
                });
                self.advance_probe();
                self.bytes_written += BLOCK_SIZE as u64;
                match res {
                    OpResult::Block(b) => b,
                    _ => BlockResult::NoDevice,
                }
            }
            ACT_FLUSH => BlockResult::Ok,
            _ => BlockResult::Ok, // ACT_WAIT
        }
    }

    fn advance_probe(&mut self) {
        // Wander through a small set of LBAs so the agent sees the
        // controller's behaviour across more than one block.
        self.probe_lba = (self.probe_lba + 7) % 128;
        if self.probe_lba < 8 {
            self.probe_lba = 8;
        }
    }

    /// Replace this agent's matrices and accumulated learning with
    /// state restored from a fabric snapshot. Used on reboot to
    /// continue learning from where the prior session left off.
    pub fn restore_from_bytes(&mut self, bytes: &[u8]) -> bool {
        if let Some(model) = crate::agent::DiscreteModel::deserialize_from_bytes(bytes) {
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

    /// Snapshot the current matrices + cumulative surprise so the
    /// kernel can write a `LearnedDriver` fabric node.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        self.model.serialize_to_bytes()
    }

    pub fn render_summary(&self) -> String {
        format!(
            "{}: {} → {}  state={}  obs={}  lat={}  surp={:.2}  done={}  err={}  to={}",
            self.label,
            ACTION_LABELS[self.last_action],
            obs_label(self.last_observation),
            STATE_LABELS[self.model.map_state()],
            self.completions + self.timeouts + self.device_errors,
            self.last_latency_ticks,
            self.model.average_surprise(),
            self.completions,
            self.device_errors,
            self.timeouts,
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StepReport {
    pub action: usize,
    pub observation: usize,
    pub latency_ticks: u64,
    pub map_state: usize,
    pub surprise: f32,
    pub avg_surprise: f32,
}

fn obs_label(o: usize) -> &'static str {
    match o {
        OBS_OK_FAST => "ok-fast",
        OBS_OK_SLOW => "ok-slow",
        OBS_DRQ => "drq-set",
        OBS_TIMEOUT => "timeout",
        OBS_DEV_ERROR => "dev-error",
        _ => "?",
    }
}

fn classify(result: &BlockResult, latency: u64, median: u64) -> usize {
    match result {
        BlockResult::Ok => {
            if median == 0 || latency <= median {
                OBS_OK_FAST
            } else {
                OBS_OK_SLOW
            }
        }
        BlockResult::Timeout => OBS_TIMEOUT,
        BlockResult::DeviceError(_) => OBS_DEV_ERROR,
        BlockResult::NoDevice => OBS_TIMEOUT,
    }
}

// --- spec-derived priors (the equivalent of innate reflexes) ---------

fn build_a_prior() -> Vec<f32> {
    // A[o, s] — likelihood of observation o given state s. Each column
    // (state) sums to 1.
    let mut a = alloc::vec![0.0f32; N_OBSERVATIONS * N_STATES];
    let set = |a: &mut [f32], o: usize, s: usize, v: f32| {
        a[o * N_STATES + s] = v;
    };
    // idle: usually quick OK
    set(&mut a, OBS_OK_FAST, STATE_IDLE, 0.85);
    set(&mut a, OBS_OK_SLOW, STATE_IDLE, 0.10);
    set(&mut a, OBS_DRQ, STATE_IDLE, 0.02);
    set(&mut a, OBS_TIMEOUT, STATE_IDLE, 0.01);
    set(&mut a, OBS_DEV_ERROR, STATE_IDLE, 0.02);
    // busy: should rarely be observed as ok-fast
    set(&mut a, OBS_OK_FAST, STATE_BUSY, 0.10);
    set(&mut a, OBS_OK_SLOW, STATE_BUSY, 0.55);
    set(&mut a, OBS_DRQ, STATE_BUSY, 0.30);
    set(&mut a, OBS_TIMEOUT, STATE_BUSY, 0.03);
    set(&mut a, OBS_DEV_ERROR, STATE_BUSY, 0.02);
    // drq: data ready
    set(&mut a, OBS_OK_FAST, STATE_DRQ, 0.10);
    set(&mut a, OBS_OK_SLOW, STATE_DRQ, 0.10);
    set(&mut a, OBS_DRQ, STATE_DRQ, 0.75);
    set(&mut a, OBS_TIMEOUT, STATE_DRQ, 0.03);
    set(&mut a, OBS_DEV_ERROR, STATE_DRQ, 0.02);
    // error: high error/timeout probability
    set(&mut a, OBS_OK_FAST, STATE_ERROR, 0.05);
    set(&mut a, OBS_OK_SLOW, STATE_ERROR, 0.05);
    set(&mut a, OBS_DRQ, STATE_ERROR, 0.05);
    set(&mut a, OBS_TIMEOUT, STATE_ERROR, 0.30);
    set(&mut a, OBS_DEV_ERROR, STATE_ERROR, 0.55);
    // degraded: ok but slow + occasional faults
    set(&mut a, OBS_OK_FAST, STATE_DEGRADED, 0.20);
    set(&mut a, OBS_OK_SLOW, STATE_DEGRADED, 0.55);
    set(&mut a, OBS_DRQ, STATE_DEGRADED, 0.10);
    set(&mut a, OBS_TIMEOUT, STATE_DEGRADED, 0.10);
    set(&mut a, OBS_DEV_ERROR, STATE_DEGRADED, 0.05);
    a
}

fn build_b_prior() -> Vec<f32> {
    // B[a, s_from, s_to] — transition probability after action a.
    let mut b = alloc::vec![0.0f32; N_ACTIONS * N_STATES * N_STATES];
    let set = |b: &mut [f32], a: usize, sf: usize, st: usize, v: f32| {
        b[a * N_STATES * N_STATES + sf * N_STATES + st] = v;
    };

    // Identify: idle stays idle mostly. From error/degraded, rarely heals.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_IDENTIFY, sf, st, if sf == st { 0.85 } else { 0.04 });
        }
    }

    // Read: idle → drq → idle path. busy stays busy. error sticky.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_READ, sf, st, 0.05);
        }
    }
    set(&mut b, ACT_READ, STATE_IDLE, STATE_IDLE, 0.40);
    set(&mut b, ACT_READ, STATE_IDLE, STATE_DRQ, 0.55);
    set(&mut b, ACT_READ, STATE_DRQ, STATE_IDLE, 0.85);
    set(&mut b, ACT_READ, STATE_BUSY, STATE_BUSY, 0.50);
    set(&mut b, ACT_READ, STATE_BUSY, STATE_IDLE, 0.40);
    set(&mut b, ACT_READ, STATE_ERROR, STATE_ERROR, 0.80);
    set(&mut b, ACT_READ, STATE_DEGRADED, STATE_IDLE, 0.50);
    set(&mut b, ACT_READ, STATE_DEGRADED, STATE_DEGRADED, 0.30);
    set(&mut b, ACT_READ, STATE_DEGRADED, STATE_ERROR, 0.10);

    // Write: similar to read but more exposed to failures.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_WRITE, sf, st, 0.05);
        }
    }
    set(&mut b, ACT_WRITE, STATE_IDLE, STATE_IDLE, 0.35);
    set(&mut b, ACT_WRITE, STATE_IDLE, STATE_BUSY, 0.30);
    set(&mut b, ACT_WRITE, STATE_IDLE, STATE_DRQ, 0.30);
    set(&mut b, ACT_WRITE, STATE_BUSY, STATE_IDLE, 0.55);
    set(&mut b, ACT_WRITE, STATE_DRQ, STATE_IDLE, 0.70);
    set(&mut b, ACT_WRITE, STATE_ERROR, STATE_ERROR, 0.85);
    set(&mut b, ACT_WRITE, STATE_DEGRADED, STATE_DEGRADED, 0.45);

    // Flush: cleans out busy/drq, leaves error untouched.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_FLUSH, sf, st, 0.05);
        }
    }
    set(&mut b, ACT_FLUSH, STATE_IDLE, STATE_IDLE, 0.85);
    set(&mut b, ACT_FLUSH, STATE_BUSY, STATE_IDLE, 0.85);
    set(&mut b, ACT_FLUSH, STATE_DRQ, STATE_IDLE, 0.85);
    set(&mut b, ACT_FLUSH, STATE_ERROR, STATE_ERROR, 0.80);
    set(&mut b, ACT_FLUSH, STATE_DEGRADED, STATE_DEGRADED, 0.65);

    // Wait: stays where it is. No information gain unless something
    // changes externally.
    for sf in 0..N_STATES {
        for st in 0..N_STATES {
            set(&mut b, ACT_WAIT, sf, st, if sf == st { 0.92 } else { 0.02 });
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

fn build_c_preferences() -> Vec<f32> {
    // ln-preferences. Higher = preferred.
    let mut c = alloc::vec![0.0f32; N_OBSERVATIONS];
    c[OBS_OK_FAST] = 2.0;
    c[OBS_OK_SLOW] = 0.5;
    c[OBS_DRQ] = 0.0;
    c[OBS_TIMEOUT] = -3.0;
    c[OBS_DEV_ERROR] = -4.0;
    c
}

fn build_d_prior() -> Vec<f32> {
    // Start strongly biased toward idle — controllers identify cleanly
    // out of reset.
    let mut d = alloc::vec![0.0f32; N_STATES];
    d[STATE_IDLE] = 0.70;
    d[STATE_BUSY] = 0.10;
    d[STATE_DRQ] = 0.05;
    d[STATE_ERROR] = 0.05;
    d[STATE_DEGRADED] = 0.10;
    d
}

#[allow(dead_code)]
pub fn state_label(s: usize) -> &'static str {
    STATE_LABELS.get(s).copied().unwrap_or("?")
}
#[allow(dead_code)]
pub fn action_label(a: usize) -> &'static str {
    ACTION_LABELS.get(a).copied().unwrap_or("?")
}
