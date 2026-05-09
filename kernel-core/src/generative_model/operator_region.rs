//! Operator region — beliefs about the operator-fabric exchange and
//! curator of what the operator sees on the interaction log. The
//! storage region predicts hardware behavior; the operator region
//! predicts operator behavior AND decides which model events should
//! become visible to the operator.
//!
//! State space (4): no_operator, exploring, focused_command, idle.
//! Observation space (12): operator-driven {no_input, known, unknown,
//! help, persist} + system-event categories {failure, persist_ok,
//! agent_step, meta, genesis, restore_ok, device_discovery}.
//! Action space (5): operator-response {terse, explain, suggest_help}
//! plus log-curation {render_to_log, absorb}.
//!
//! Render decision (v1): model-driven threshold over the C vector.
//! `select_render_action(obs)` returns RENDER iff `|c[obs]| >
//! RENDER_THRESHOLD`. The C vector IS the model's preference, so the
//! decision is grounded in model parameters. Refining the C vector
//! changes what gets surfaced. Belief still updates on every event
//! regardless of render outcome — the model learns from everything,
//! it just doesn't show everything.

use alloc::vec::Vec;

use crate::model::discrete::DiscreteModel;

pub const N_STATES: usize = 4;
pub const N_OBSERVATIONS: usize = 12;
pub const N_ACTIONS: usize = 5;

pub const STATE_NO_OPERATOR: usize = 0;
pub const STATE_EXPLORING: usize = 1;
pub const STATE_FOCUSED: usize = 2;
pub const STATE_IDLE: usize = 3;

// Operator-driven observations (the operator typed something).
pub const OBS_NO_INPUT: usize = 0;
pub const OBS_KNOWN: usize = 1;
pub const OBS_UNKNOWN: usize = 2;
pub const OBS_HELP: usize = 3;
pub const OBS_PERSIST: usize = 4;

// System-event observations (the kernel produced something the
// operator might want to see).
pub const OBS_EVENT_FAILURE: usize = 5;
pub const OBS_EVENT_PERSIST_OK: usize = 6;
pub const OBS_EVENT_AGENT_STEP: usize = 7;
pub const OBS_EVENT_META: usize = 8;
pub const OBS_EVENT_GENESIS: usize = 9;
pub const OBS_EVENT_RESTORE_OK: usize = 10;
pub const OBS_EVENT_DEVICE_DISCOVERY: usize = 11;

// Operator-response actions.
pub const ACT_TERSE: usize = 0;
pub const ACT_EXPLAIN: usize = 1;
pub const ACT_SUGGEST_HELP: usize = 2;

// Log-curation actions.
pub const ACT_RENDER_TO_LOG: usize = 3;
pub const ACT_ABSORB: usize = 4;

/// Render-or-absorb threshold. Events whose `|C[obs]| > RENDER_THRESHOLD`
/// render to the interaction log; events with `|C[obs]| <= threshold`
/// absorb silently. The model learns from both.
pub const RENDER_THRESHOLD: f32 = 0.5;

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

    /// Operator typed a line. Updates belief and counters; the action
    /// chosen is informational (the inference loop already sends the
    /// actual response text via `submit_event(LogCandidate::Fabric
    /// Response)`).
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

    /// Bayesian belief update for any event. Used by `submit_event` to
    /// route system events through the operator region's learning,
    /// regardless of whether the curator decides to render. The model
    /// learns from every event; only display is gated.
    pub fn observe_event(&mut self, observation: usize) -> f32 {
        let s = self.model.observe(ACT_ABSORB, observation);
        self.model.learn(observation);
        s
    }

    /// Render-or-absorb decision. v1: model-driven threshold over the
    /// C vector. Salient (|C| > RENDER_THRESHOLD) renders; routine
    /// (|C| <= RENDER_THRESHOLD) absorbs. The C vector IS the model's
    /// preference, so this rule is grounded in model parameters, not
    /// hardcoded categories. Refining the C vector changes what gets
    /// surfaced.
    pub fn select_render_action(&self, observation: usize) -> usize {
        let c = self.model.c.get(observation).copied().unwrap_or(0.0);
        if c.abs() > RENDER_THRESHOLD {
            ACT_RENDER_TO_LOG
        } else {
            ACT_ABSORB
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

    // STATE_NO_OPERATOR — silence dominates; rare boot-time events.
    set(&mut a, OBS_NO_INPUT, STATE_NO_OPERATOR, 0.85);
    set(&mut a, OBS_KNOWN, STATE_NO_OPERATOR, 0.01);
    set(&mut a, OBS_UNKNOWN, STATE_NO_OPERATOR, 0.005);
    set(&mut a, OBS_HELP, STATE_NO_OPERATOR, 0.005);
    set(&mut a, OBS_PERSIST, STATE_NO_OPERATOR, 0.005);
    set(&mut a, OBS_EVENT_FAILURE, STATE_NO_OPERATOR, 0.01);
    set(&mut a, OBS_EVENT_PERSIST_OK, STATE_NO_OPERATOR, 0.02);
    set(&mut a, OBS_EVENT_AGENT_STEP, STATE_NO_OPERATOR, 0.02);
    set(&mut a, OBS_EVENT_META, STATE_NO_OPERATOR, 0.01);
    set(&mut a, OBS_EVENT_GENESIS, STATE_NO_OPERATOR, 0.04);
    set(&mut a, OBS_EVENT_RESTORE_OK, STATE_NO_OPERATOR, 0.005);
    set(&mut a, OBS_EVENT_DEVICE_DISCOVERY, STATE_NO_OPERATOR, 0.02);

    // STATE_EXPLORING — operator is poking around. Help, unknown, and
    // genesis-level events are common.
    set(&mut a, OBS_NO_INPUT, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_KNOWN, STATE_EXPLORING, 0.20);
    set(&mut a, OBS_UNKNOWN, STATE_EXPLORING, 0.20);
    set(&mut a, OBS_HELP, STATE_EXPLORING, 0.20);
    set(&mut a, OBS_PERSIST, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_EVENT_FAILURE, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_EVENT_PERSIST_OK, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_EVENT_AGENT_STEP, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_EVENT_META, STATE_EXPLORING, 0.05);
    set(&mut a, OBS_EVENT_GENESIS, STATE_EXPLORING, 0.04);
    set(&mut a, OBS_EVENT_RESTORE_OK, STATE_EXPLORING, 0.03);
    set(&mut a, OBS_EVENT_DEVICE_DISCOVERY, STATE_EXPLORING, 0.03);

    // STATE_FOCUSED — operator is engaged. Salient events dominate.
    set(&mut a, OBS_NO_INPUT, STATE_FOCUSED, 0.05);
    set(&mut a, OBS_KNOWN, STATE_FOCUSED, 0.40);
    set(&mut a, OBS_UNKNOWN, STATE_FOCUSED, 0.05);
    set(&mut a, OBS_HELP, STATE_FOCUSED, 0.05);
    set(&mut a, OBS_PERSIST, STATE_FOCUSED, 0.15);
    set(&mut a, OBS_EVENT_FAILURE, STATE_FOCUSED, 0.10);
    set(&mut a, OBS_EVENT_PERSIST_OK, STATE_FOCUSED, 0.02);
    set(&mut a, OBS_EVENT_AGENT_STEP, STATE_FOCUSED, 0.02);
    set(&mut a, OBS_EVENT_META, STATE_FOCUSED, 0.04);
    set(&mut a, OBS_EVENT_GENESIS, STATE_FOCUSED, 0.04);
    set(&mut a, OBS_EVENT_RESTORE_OK, STATE_FOCUSED, 0.05);
    set(&mut a, OBS_EVENT_DEVICE_DISCOVERY, STATE_FOCUSED, 0.03);

    // STATE_IDLE — silence and routine events dominate.
    set(&mut a, OBS_NO_INPUT, STATE_IDLE, 0.55);
    set(&mut a, OBS_KNOWN, STATE_IDLE, 0.05);
    set(&mut a, OBS_UNKNOWN, STATE_IDLE, 0.01);
    set(&mut a, OBS_HELP, STATE_IDLE, 0.01);
    set(&mut a, OBS_PERSIST, STATE_IDLE, 0.01);
    set(&mut a, OBS_EVENT_FAILURE, STATE_IDLE, 0.01);
    set(&mut a, OBS_EVENT_PERSIST_OK, STATE_IDLE, 0.15);
    set(&mut a, OBS_EVENT_AGENT_STEP, STATE_IDLE, 0.15);
    set(&mut a, OBS_EVENT_META, STATE_IDLE, 0.03);
    set(&mut a, OBS_EVENT_GENESIS, STATE_IDLE, 0.005);
    set(&mut a, OBS_EVENT_RESTORE_OK, STATE_IDLE, 0.005);
    set(&mut a, OBS_EVENT_DEVICE_DISCOVERY, STATE_IDLE, 0.005);

    // Renormalise each column (state) so every state's emission row
    // sums to 1 — the math substrate doesn't enforce this, but
    // belief-update assumes it.
    for s in 0..N_STATES {
        let mut sum = 0.0f32;
        for o in 0..N_OBSERVATIONS {
            sum += a[o * N_STATES + s];
        }
        if sum > 0.0 {
            for o in 0..N_OBSERVATIONS {
                a[o * N_STATES + s] /= sum;
            }
        }
    }
    a
}

fn build_b() -> Vec<f32> {
    let mut b = alloc::vec![0.0f32; N_ACTIONS * N_STATES * N_STATES];
    let set = |b: &mut [f32], a: usize, sf: usize, st: usize, v: f32| {
        b[a * N_STATES * N_STATES + sf * N_STATES + st] = v;
    };

    // Operator-response actions: weak transitions, mostly stationary.
    // The operator's own state changes through their own choices; this
    // model is mostly an observer.
    for ai in 0..N_ACTIONS {
        for sf in 0..N_STATES {
            for st in 0..N_STATES {
                set(&mut b, ai, sf, st, if sf == st { 0.85 } else { 0.05 });
            }
        }
    }

    // ACT_SUGGEST_HELP nudges EXPLORING → FOCUSED.
    set(&mut b, ACT_SUGGEST_HELP, STATE_EXPLORING, STATE_FOCUSED, 0.20);
    // ACT_EXPLAIN nudges EXPLORING → FOCUSED.
    set(&mut b, ACT_EXPLAIN, STATE_EXPLORING, STATE_FOCUSED, 0.15);
    // ACT_TERSE keeps FOCUSED in FOCUSED.
    set(&mut b, ACT_TERSE, STATE_FOCUSED, STATE_FOCUSED, 0.92);

    // Log-curation actions: rendering pulls operator toward FOCUSED;
    // absorbing pulls toward IDLE. These are the model's hypothesis
    // about how showing / not showing events shapes the operator's
    // engagement state. Used by EFE in v2; in v1 the render decision
    // is the C-vector threshold heuristic above, so these B entries
    // exist for the math's sanity (B must be a proper transition
    // matrix) but don't drive selection.
    for sf in 0..N_STATES {
        // ACT_RENDER_TO_LOG → STATE_FOCUSED with high prob.
        set(&mut b, ACT_RENDER_TO_LOG, sf, STATE_FOCUSED, 0.75);
        for st in 0..N_STATES {
            if st != STATE_FOCUSED {
                set(&mut b, ACT_RENDER_TO_LOG, sf, st, 0.05);
            }
        }
        // ACT_ABSORB → STATE_IDLE with high prob.
        set(&mut b, ACT_ABSORB, sf, STATE_IDLE, 0.75);
        for st in 0..N_STATES {
            if st != STATE_IDLE {
                set(&mut b, ACT_ABSORB, sf, st, 0.05);
            }
        }
    }

    // Renormalise each (action, source-state) row to 1.
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
    // Operator-driven events.
    c[OBS_KNOWN] = 1.5; // operator engaged, intent recognised
    c[OBS_PERSIST] = 1.0; // explicit save = good signal
    c[OBS_HELP] = 0.5; // exploration, neutral-positive
    c[OBS_NO_INPUT] = 0.0; // silence, neutral
    c[OBS_UNKNOWN] = -1.0; // discontinuity, dispreferred
    // System-event preferences. The render decision uses |C| >
    // RENDER_THRESHOLD (=0.5), so:
    c[OBS_EVENT_FAILURE] = -3.0; // |C|=3.0 → RENDER (operator must know)
    c[OBS_EVENT_PERSIST_OK] = 0.0; // routine, ABSORB
    c[OBS_EVENT_AGENT_STEP] = 0.0; // routine, ABSORB
    c[OBS_EVENT_META] = 0.4; // borderline, ABSORB (≤ threshold)
    c[OBS_EVENT_GENESIS] = 1.5; // boot summary, RENDER
    c[OBS_EVENT_RESTORE_OK] = 2.0; // load-bearing, RENDER
    c[OBS_EVENT_DEVICE_DISCOVERY] = 1.0; // boot-time info, RENDER
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
