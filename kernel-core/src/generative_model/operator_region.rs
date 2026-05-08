//! Operator region — beliefs about the operator-fabric exchange. The
//! storage region predicts hardware behavior; the operator region
//! predicts operator behavior. Today it's a thin wrapper around an
//! intent counter; the structural priors are intentionally weak so the
//! agent learns from observation rather than imposing assumptions about
//! the operator's command vocabulary.
//!
//! State space (4): no_operator, exploring, focused_command, idle.
//! Observation space (5): no_input, known_command, unknown_command,
//! help_request, persist_request.
//! Action space (3): respond_terse, respond_explain, suggest_help.

use alloc::vec::Vec;

use crate::model::discrete::DiscreteModel;

pub const N_STATES: usize = 4;
pub const N_OBSERVATIONS: usize = 5;
pub const N_ACTIONS: usize = 3;

pub const STATE_NO_OPERATOR: usize = 0;
pub const STATE_EXPLORING: usize = 1;
pub const STATE_FOCUSED: usize = 2;
pub const STATE_IDLE: usize = 3;

pub const OBS_NO_INPUT: usize = 0;
pub const OBS_KNOWN: usize = 1;
pub const OBS_UNKNOWN: usize = 2;
pub const OBS_HELP: usize = 3;
pub const OBS_PERSIST: usize = 4;

pub const ACT_TERSE: usize = 0;
pub const ACT_EXPLAIN: usize = 1;
pub const ACT_SUGGEST_HELP: usize = 2;

const STATE_LABELS: &[&str] = &["no-op", "exploring", "focused", "idle"];

pub struct OperatorRegion {
    pub model: DiscreteModel,
    pub intents_seen: u64,
    pub help_requests: u64,
    pub unknown_commands: u64,
}

impl OperatorRegion {
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
            intents_seen: 0,
            help_requests: 0,
            unknown_commands: 0,
        }
    }

    pub fn observe_intent(&mut self, classification: usize) -> f32 {
        self.intents_seen += 1;
        match classification {
            OBS_HELP => self.help_requests += 1,
            OBS_UNKNOWN => self.unknown_commands += 1,
            _ => {}
        }
        let action = match classification {
            OBS_HELP => ACT_EXPLAIN,
            OBS_UNKNOWN => ACT_SUGGEST_HELP,
            _ => ACT_TERSE,
        };
        let s = self.model.observe(action, classification);
        self.model.learn(classification);
        s
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

/// Classify a parsed command into an observation tag the region
/// understands. The kernel calls this in `intent::submit` after the
/// command has been recognised.
pub fn classify_command(known: bool, lower_first_word: &str) -> usize {
    if lower_first_word.is_empty() {
        OBS_NO_INPUT
    } else if lower_first_word == "help" || lower_first_word == "?" {
        OBS_HELP
    } else if lower_first_word == "persist" || lower_first_word == "save" {
        OBS_PERSIST
    } else if known {
        OBS_KNOWN
    } else {
        OBS_UNKNOWN
    }
}

fn build_a() -> Vec<f32> {
    let mut a = alloc::vec![0.0f32; N_OBSERVATIONS * N_STATES];
    let set = |a: &mut [f32], o: usize, s: usize, v: f32| {
        a[o * N_STATES + s] = v;
    };
    // no_operator: silence
    set(&mut a, OBS_NO_INPUT, STATE_NO_OPERATOR, 0.95);
    set(&mut a, OBS_KNOWN, STATE_NO_OPERATOR, 0.02);
    set(&mut a, OBS_UNKNOWN, STATE_NO_OPERATOR, 0.01);
    set(&mut a, OBS_HELP, STATE_NO_OPERATOR, 0.01);
    set(&mut a, OBS_PERSIST, STATE_NO_OPERATOR, 0.01);
    // exploring: lots of help, mix of known/unknown
    set(&mut a, OBS_HELP, STATE_EXPLORING, 0.30);
    set(&mut a, OBS_UNKNOWN, STATE_EXPLORING, 0.30);
    set(&mut a, OBS_KNOWN, STATE_EXPLORING, 0.30);
    set(&mut a, OBS_PERSIST, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_NO_INPUT, STATE_EXPLORING, 0.05);
    // focused: mostly known commands, some persist
    set(&mut a, OBS_KNOWN, STATE_FOCUSED, 0.65);
    set(&mut a, OBS_PERSIST, STATE_FOCUSED, 0.20);
    set(&mut a, OBS_HELP, STATE_FOCUSED, 0.05);
    set(&mut a, OBS_UNKNOWN, STATE_FOCUSED, 0.05);
    set(&mut a, OBS_NO_INPUT, STATE_FOCUSED, 0.05);
    // idle: silence dominates
    set(&mut a, OBS_NO_INPUT, STATE_IDLE, 0.85);
    set(&mut a, OBS_KNOWN, STATE_IDLE, 0.10);
    set(&mut a, OBS_HELP, STATE_IDLE, 0.02);
    set(&mut a, OBS_UNKNOWN, STATE_IDLE, 0.02);
    set(&mut a, OBS_PERSIST, STATE_IDLE, 0.01);
    a
}

fn build_b() -> Vec<f32> {
    let mut b = alloc::vec![0.0f32; N_ACTIONS * N_STATES * N_STATES];
    let set = |b: &mut [f32], a: usize, sf: usize, st: usize, v: f32| {
        b[a * N_STATES * N_STATES + sf * N_STATES + st] = v;
    };
    // For all actions: weak transitions. The operator's own state
    // changes through their own choices; this model is mostly an
    // observer. Same diagonal pattern across all actions, with small
    // perturbations indicating what the action "encourages".
    for ai in 0..N_ACTIONS {
        for sf in 0..N_STATES {
            for st in 0..N_STATES {
                set(&mut b, ai, sf, st, if sf == st { 0.85 } else { 0.05 });
            }
        }
    }
    // suggest_help nudges UNKNOWN-emitting state toward EXPLORING
    set(&mut b, ACT_SUGGEST_HELP, STATE_EXPLORING, STATE_FOCUSED, 0.20);
    // explain nudges EXPLORING toward FOCUSED
    set(&mut b, ACT_EXPLAIN, STATE_EXPLORING, STATE_FOCUSED, 0.15);
    // terse nudges FOCUSED to stay FOCUSED
    set(&mut b, ACT_TERSE, STATE_FOCUSED, STATE_FOCUSED, 0.92);

    // Renormalise.
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
    c[OBS_KNOWN] = 1.5;       // operator engaged, intent recognised
    c[OBS_PERSIST] = 1.0;     // explicit save = good signal
    c[OBS_HELP] = 0.5;        // exploration is neutral-positive
    c[OBS_NO_INPUT] = 0.0;    // silence is neutral
    c[OBS_UNKNOWN] = -1.0;    // discontinuity preference
    c
}

fn build_d() -> Vec<f32> {
    let mut d = alloc::vec![0.0f32; N_STATES];
    d[STATE_NO_OPERATOR] = 0.50;
    d[STATE_EXPLORING] = 0.30;
    d[STATE_FOCUSED] = 0.10;
    d[STATE_IDLE] = 0.10;
    d
}
