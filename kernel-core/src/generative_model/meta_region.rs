//! Meta region — self-assessment over the rest of the model. Watches
//! ΔF/Δt, current free energy, and observation entropy; classifies
//! whether the model is healthy_learning, healthy_calibrated, stuck,
//! or overfitted.
//!
//! Step 4 of the nucleation plan populates this region with the full
//! 4-state DiscreteModel; for now the region carries the placeholder
//! state and a simple heuristic assessment so Step 2's compile is
//! green and the inference loop can render it. The DiscreteModel-based
//! self-assessment lights up when MetaRegion::assess is wired in.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::model::discrete::DiscreteModel;

pub const N_STATES: usize = 4;
pub const N_OBSERVATIONS: usize = 3;
pub const N_ACTIONS: usize = 5;

pub const STATE_HEALTHY_LEARNING: usize = 0;
pub const STATE_HEALTHY_CALIBRATED: usize = 1;
pub const STATE_STUCK: usize = 2;
pub const STATE_OVERFITTED: usize = 3;

pub const OBS_DELTA_F_NEGATIVE: usize = 0;
pub const OBS_DELTA_F_FLAT: usize = 1;
pub const OBS_DELTA_F_POSITIVE: usize = 2;

pub const ACT_INCREASE_LR: usize = 0;
pub const ACT_DECREASE_LR: usize = 1;
pub const ACT_WIDEN_PRIORS: usize = 2;
pub const ACT_RESET_TO_STRUCTURAL: usize = 3;
pub const ACT_CONTINUE: usize = 4;

const STATE_LABELS: &[&str] = &[
    "healthy-learning",
    "healthy-calibrated",
    "stuck",
    "overfitted",
];

#[derive(Clone, Copy, Debug)]
pub struct MetaAssessment {
    pub state: usize,
    pub free_energy: f32,
    pub delta_f: f32,
    pub action_recommendation: usize,
}

impl MetaAssessment {
    pub fn label(&self) -> &'static str {
        STATE_LABELS[self.state]
    }
    pub fn render(&self) -> String {
        let action = match self.action_recommendation {
            ACT_INCREASE_LR => "increase-lr",
            ACT_DECREASE_LR => "decrease-lr",
            ACT_WIDEN_PRIORS => "widen-priors",
            ACT_RESET_TO_STRUCTURAL => "reset-to-structural",
            ACT_CONTINUE => "continue",
            _ => "?",
        };
        alloc::format!(
            "meta: {} | F={:.2} ΔF={:.3} → {}",
            self.label(),
            self.free_energy,
            self.delta_f,
            action
        )
    }
}

pub struct MetaRegion {
    pub model: DiscreteModel,
    pub last_assessment: Option<MetaAssessment>,
    pub assessments_taken: u64,
    pub last_state: usize,
}

impl MetaRegion {
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
            last_assessment: None,
            assessments_taken: 0,
            last_state: STATE_HEALTHY_LEARNING,
        }
    }

    /// Classify a (delta_f, current_fe, observation_entropy) triple
    /// into the meta state space using simple thresholds, then run the
    /// DiscreteModel.observe step so the meta-region is also a learnt
    /// model. Returns the assessment.
    pub fn assess(&mut self, delta_f: f32, current_fe: f32) -> MetaAssessment {
        let obs = if delta_f < -0.05 {
            OBS_DELTA_F_NEGATIVE
        } else if delta_f > 0.05 {
            OBS_DELTA_F_POSITIVE
        } else {
            OBS_DELTA_F_FLAT
        };

        let heuristic_state = if delta_f < -0.05 {
            STATE_HEALTHY_LEARNING
        } else if current_fe < 1.5 && delta_f.abs() < 0.05 {
            STATE_HEALTHY_CALIBRATED
        } else if current_fe > 3.0 && delta_f.abs() < 0.05 {
            STATE_STUCK
        } else if delta_f > 0.05 {
            STATE_OVERFITTED
        } else {
            STATE_HEALTHY_LEARNING
        };

        let _ = self.model.observe(ACT_CONTINUE, obs);
        self.model.learn(obs);
        self.assessments_taken += 1;
        self.last_state = heuristic_state;

        let action_recommendation = match heuristic_state {
            STATE_HEALTHY_LEARNING => ACT_CONTINUE,
            STATE_HEALTHY_CALIBRATED => ACT_CONTINUE,
            STATE_STUCK => ACT_INCREASE_LR,
            STATE_OVERFITTED => ACT_WIDEN_PRIORS,
            _ => ACT_CONTINUE,
        };

        let assessment = MetaAssessment {
            state: heuristic_state,
            free_energy: current_fe,
            delta_f,
            action_recommendation,
        };
        self.last_assessment = Some(assessment);
        assessment
    }

    pub fn label(&self) -> &'static str {
        STATE_LABELS[self.last_state]
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

    pub fn render_summary(&self) -> String {
        match self.last_assessment {
            Some(a) => a.render(),
            None => "meta: pre-assessment".to_string(),
        }
    }
}

#[allow(dead_code)]
pub fn state_label(s: usize) -> &'static str {
    STATE_LABELS.get(s).copied().unwrap_or("?")
}

fn build_a() -> Vec<f32> {
    // Each meta-state biases toward a particular ΔF observation.
    let mut a = alloc::vec![0.0f32; N_OBSERVATIONS * N_STATES];
    let set = |a: &mut [f32], o: usize, s: usize, v: f32| {
        a[o * N_STATES + s] = v;
    };
    set(&mut a, OBS_DELTA_F_NEGATIVE, STATE_HEALTHY_LEARNING, 0.80);
    set(&mut a, OBS_DELTA_F_FLAT, STATE_HEALTHY_LEARNING, 0.15);
    set(&mut a, OBS_DELTA_F_POSITIVE, STATE_HEALTHY_LEARNING, 0.05);

    set(&mut a, OBS_DELTA_F_FLAT, STATE_HEALTHY_CALIBRATED, 0.85);
    set(&mut a, OBS_DELTA_F_NEGATIVE, STATE_HEALTHY_CALIBRATED, 0.10);
    set(&mut a, OBS_DELTA_F_POSITIVE, STATE_HEALTHY_CALIBRATED, 0.05);

    set(&mut a, OBS_DELTA_F_FLAT, STATE_STUCK, 0.85);
    set(&mut a, OBS_DELTA_F_NEGATIVE, STATE_STUCK, 0.05);
    set(&mut a, OBS_DELTA_F_POSITIVE, STATE_STUCK, 0.10);

    set(&mut a, OBS_DELTA_F_POSITIVE, STATE_OVERFITTED, 0.75);
    set(&mut a, OBS_DELTA_F_FLAT, STATE_OVERFITTED, 0.15);
    set(&mut a, OBS_DELTA_F_NEGATIVE, STATE_OVERFITTED, 0.10);
    a
}

fn build_b() -> Vec<f32> {
    let mut b = alloc::vec![0.0f32; N_ACTIONS * N_STATES * N_STATES];
    let set = |b: &mut [f32], a: usize, sf: usize, st: usize, v: f32| {
        b[a * N_STATES * N_STATES + sf * N_STATES + st] = v;
    };
    // Identity-ish for all actions; the meta-region's actions affect
    // the primary models via inference-loop coupling, not via its own
    // B matrix. A simple stationary B keeps the math well-defined.
    for ai in 0..N_ACTIONS {
        for sf in 0..N_STATES {
            for st in 0..N_STATES {
                set(&mut b, ai, sf, st, if sf == st { 0.85 } else { 0.05 });
            }
        }
    }
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
    c[OBS_DELTA_F_NEGATIVE] = 1.5;  // model is learning → preferred
    c[OBS_DELTA_F_FLAT] = 0.5;      // calibrated
    c[OBS_DELTA_F_POSITIVE] = -1.0; // free energy climbing → dispreferred
    c
}

fn build_d() -> Vec<f32> {
    let mut d = alloc::vec![0.0f32; N_STATES];
    d[STATE_HEALTHY_LEARNING] = 0.60;
    d[STATE_HEALTHY_CALIBRATED] = 0.20;
    d[STATE_STUCK] = 0.10;
    d[STATE_OVERFITTED] = 0.10;
    d
}
