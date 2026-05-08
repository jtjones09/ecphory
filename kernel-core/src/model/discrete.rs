//! Discrete active inference primitive — `DiscreteModel`.
//!
//! Generic POMDP factor parameterised by four matrices A, B, C, D over
//! configurable `n_states × n_observations × n_actions`. Used as the
//! math substrate for every region of the nucleation `GenerativeModel`:
//! one `DiscreteModel` per device controller, one for the persistence
//! region, one for the operator-interaction region, one for the meta
//! self-assessment level. Each region picks its own dimensions and
//! seeds its own priors; the math here is identical for all of them.
//!
//! Implements pymdp's structure (Heins et al. 2022) in `no_std + alloc`:
//!
//!   variational free energy: F[q] = D_KL[q(s) || p(s|o)] − ln p(o)
//!   belief update:           q(s) = softmax(ln A^T·o + ln B^T·s_{t-1})
//!   expected free energy:    G(π) = −E[ln p~(o)] − E[D_KL[q(s|o,π) || q(s|π)]]
//!   policy posterior:        q(π) = softmax(−γ·G(π))
//!
//! All operations are over `Vec<f32>` (not NumPy or matrix crates) so
//! we stay no_std-friendly and don't pay an alloc on every infer step.
//!
//! See `nisaba/positions/nucleation-architecture.md` and
//! `nisaba/research/ecphory-kernel-world-model-math.md` for the
//! architecture this primitive serves.

use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct DiscreteModel {
    pub n_states: usize,
    pub n_observations: usize,
    pub n_actions: usize,

    /// A: p(o | s). Row-major `[n_observations × n_states]`.
    pub a: Vec<f32>,

    /// B: p(s' | s, a). Row-major `[n_actions × n_states × n_states]`.
    /// `B[a][s' from][s to]` — index as `a*n_states*n_states + sf*n_states + st`.
    pub b: Vec<f32>,

    /// C: ln-preference over observations. `[n_observations]`.
    pub c: Vec<f32>,

    /// D: prior over states. `[n_states]`.
    pub d: Vec<f32>,

    /// Current posterior belief. `[n_states]`.
    pub belief: Vec<f32>,

    /// Inverse temperature for policy selection.
    pub gamma: f32,

    /// Total observations integrated. Used by the immune system as
    /// "experience" — a freshly-spawned agent has weight ≈ priors only.
    pub observations_seen: u64,

    /// Cumulative surprise (−ln p(o)) integrated over the agent's life.
    /// Healthy controllers see low cumulative surprise; a controller
    /// whose A no longer predicts its observations sees this number
    /// climb. The immune system reads this to flag damage.
    pub cumulative_surprise: f32,
}

const EPSILON: f32 = 1e-6;
const PRIOR_NUDGE: f32 = 0.05;

impl DiscreteModel {
    pub fn new(
        n_states: usize,
        n_observations: usize,
        n_actions: usize,
        a: Vec<f32>,
        b: Vec<f32>,
        c: Vec<f32>,
        d: Vec<f32>,
    ) -> Self {
        debug_assert_eq!(a.len(), n_observations * n_states);
        debug_assert_eq!(b.len(), n_actions * n_states * n_states);
        debug_assert_eq!(c.len(), n_observations);
        debug_assert_eq!(d.len(), n_states);
        let mut s = Self {
            n_states,
            n_observations,
            n_actions,
            a,
            b,
            c,
            d: d.clone(),
            belief: d,
            gamma: 4.0,
            observations_seen: 0,
            cumulative_surprise: 0.0,
        };
        normalize(&mut s.belief);
        s
    }

    /// Integrate one observation. Returns the surprise of this
    /// observation given the prior belief — useful as an immune signal.
    pub fn observe(&mut self, action_taken: usize, observation: usize) -> f32 {
        // 1. Predicted next state given last action: s_pred = B[a]^T · belief
        let predicted = self.transition(action_taken, &self.belief);

        // 2. Likelihood of the observation under each state: lik = A[o, :]
        let mut posterior = vec![0.0f32; self.n_states];
        for s in 0..self.n_states {
            let p_o_given_s = self.a[observation * self.n_states + s];
            posterior[s] = predicted[s] * p_o_given_s;
        }
        // marginal p(o) = sum over s of A[o,s]·predicted[s]
        let p_o: f32 = posterior.iter().sum::<f32>().max(EPSILON);
        let surprise = -libm::logf(p_o);
        normalize(&mut posterior);

        self.belief = posterior;
        self.observations_seen += 1;
        self.cumulative_surprise += surprise;
        surprise
    }

    /// Slow learning of A — gently shift the row corresponding to the
    /// observed (o, s) pair toward the actual joint occurrence. This
    /// is what makes the agent specifically a *learned* interaction
    /// model rather than a static prior.
    pub fn learn(&mut self, observation: usize) {
        // The expected state distribution is `belief`. Nudge A's column
        // for the observed o toward the belief mass.
        for s in 0..self.n_states {
            let idx = observation * self.n_states + s;
            self.a[idx] += PRIOR_NUDGE * self.belief[s];
        }
        // Renormalise each column of A so each state's emission row sums to 1.
        for s in 0..self.n_states {
            let mut col_sum = 0.0;
            for o in 0..self.n_observations {
                col_sum += self.a[o * self.n_states + s];
            }
            if col_sum > 0.0 {
                for o in 0..self.n_observations {
                    self.a[o * self.n_states + s] /= col_sum;
                }
            }
        }
    }

    fn transition(&self, action: usize, state: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; self.n_states];
        for s_to in 0..self.n_states {
            for s_from in 0..self.n_states {
                let p = self.b[action * self.n_states * self.n_states
                    + s_from * self.n_states
                    + s_to];
                out[s_to] += p * state[s_from];
            }
        }
        normalize(&mut out);
        out
    }

    /// Expected free energy of each one-step policy (just an action,
    /// not a multi-step plan). Lower is better. Returns the chosen
    /// action and the EFE for each.
    pub fn select_action(&self) -> (usize, Vec<f32>) {
        let mut g = vec![0.0f32; self.n_actions];
        for a in 0..self.n_actions {
            let q_s = self.transition(a, &self.belief);

            // Predicted observations under this policy: q_o = A · q_s
            let mut q_o = vec![0.0f32; self.n_observations];
            for o in 0..self.n_observations {
                for s in 0..self.n_states {
                    q_o[o] += self.a[o * self.n_states + s] * q_s[s];
                }
            }

            // Pragmatic value: −E_q[ln p~(o)] = −sum q_o · C
            let pragmatic: f32 = q_o
                .iter()
                .zip(self.c.iter())
                .map(|(qo, co)| qo * co)
                .sum::<f32>();

            // Epistemic value: information gain about s from o under π.
            // Approximate as entropy of q_o (ambiguity) — full
            // computation would be E_q[D_KL[q(s|o,π)||q(s|π)]] but the
            // ambiguity term is the dominant contribution and cheaper.
            let ambiguity: f32 = q_o
                .iter()
                .map(|&qo| {
                    if qo > EPSILON {
                        -qo * libm::logf(qo)
                    } else {
                        0.0
                    }
                })
                .sum();

            // G = −pragmatic − epistemic. We minimise.
            g[a] = -pragmatic - ambiguity;
        }
        let best_action = argmin(&g);
        (best_action, g)
    }

    /// Most likely current state under the posterior.
    pub fn map_state(&self) -> usize {
        argmax(&self.belief)
    }

    /// Average surprise per observation — the immune signal. Healthy
    /// controllers stay low; failing ones climb.
    pub fn average_surprise(&self) -> f32 {
        if self.observations_seen == 0 {
            0.0
        } else {
            self.cumulative_surprise / (self.observations_seen as f32)
        }
    }

    /// Serialise the entire generative model + learned state into a
    /// flat little-endian byte stream. Used by the fabric snapshot to
    /// persist the agent across reboots.
    pub fn serialize_to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            32 + 4 * (self.a.len() + self.b.len() + self.c.len() + self.d.len() + self.belief.len()),
        );
        // Header: version, dimensions
        out.extend_from_slice(&1u32.to_le_bytes()); // version
        out.extend_from_slice(&(self.n_states as u32).to_le_bytes());
        out.extend_from_slice(&(self.n_observations as u32).to_le_bytes());
        out.extend_from_slice(&(self.n_actions as u32).to_le_bytes());
        out.extend_from_slice(&self.gamma.to_le_bytes());
        out.extend_from_slice(&self.observations_seen.to_le_bytes());
        out.extend_from_slice(&self.cumulative_surprise.to_le_bytes());
        write_floats(&mut out, &self.a);
        write_floats(&mut out, &self.b);
        write_floats(&mut out, &self.c);
        write_floats(&mut out, &self.d);
        write_floats(&mut out, &self.belief);
        out
    }

    pub fn deserialize_from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut p = 0usize;
        if bytes.len() < 4 + 4 + 4 + 4 + 4 + 8 + 4 {
            return None;
        }
        let version = read_u32(bytes, &mut p)?;
        if version != 1 {
            return None;
        }
        let n_states = read_u32(bytes, &mut p)? as usize;
        let n_observations = read_u32(bytes, &mut p)? as usize;
        let n_actions = read_u32(bytes, &mut p)? as usize;
        let gamma = read_f32(bytes, &mut p)?;
        let observations_seen = read_u64(bytes, &mut p)?;
        let cumulative_surprise = read_f32(bytes, &mut p)?;
        let a = read_floats(bytes, &mut p, n_observations * n_states)?;
        let b = read_floats(bytes, &mut p, n_actions * n_states * n_states)?;
        let c = read_floats(bytes, &mut p, n_observations)?;
        let d = read_floats(bytes, &mut p, n_states)?;
        let belief = read_floats(bytes, &mut p, n_states)?;
        Some(Self {
            n_states,
            n_observations,
            n_actions,
            a,
            b,
            c,
            d,
            belief,
            gamma,
            observations_seen,
            cumulative_surprise,
        })
    }
}

fn write_floats(out: &mut Vec<u8>, v: &[f32]) {
    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for &x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

fn read_u32(b: &[u8], p: &mut usize) -> Option<u32> {
    if *p + 4 > b.len() {
        return None;
    }
    let v = u32::from_le_bytes(b[*p..*p + 4].try_into().ok()?);
    *p += 4;
    Some(v)
}
fn read_u64(b: &[u8], p: &mut usize) -> Option<u64> {
    if *p + 8 > b.len() {
        return None;
    }
    let v = u64::from_le_bytes(b[*p..*p + 8].try_into().ok()?);
    *p += 8;
    Some(v)
}
fn read_f32(b: &[u8], p: &mut usize) -> Option<f32> {
    if *p + 4 > b.len() {
        return None;
    }
    let v = f32::from_le_bytes(b[*p..*p + 4].try_into().ok()?);
    *p += 4;
    Some(v)
}
fn read_floats(b: &[u8], p: &mut usize, expected: usize) -> Option<Vec<f32>> {
    let n = read_u32(b, p)? as usize;
    if n != expected {
        return None;
    }
    if *p + 4 * n > b.len() {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(read_f32(b, p)?);
    }
    Some(out)
}

fn normalize(v: &mut [f32]) {
    let mut sum = 0.0f32;
    for x in v.iter() {
        sum += *x;
    }
    if sum > 0.0 {
        for x in v.iter_mut() {
            *x /= sum;
        }
    } else {
        let n = v.len() as f32;
        for x in v.iter_mut() {
            *x = 1.0 / n;
        }
    }
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &x) in v.iter().enumerate() {
        if x > best_v {
            best = i;
            best_v = x;
        }
    }
    best
}

fn argmin(v: &[f32]) -> usize {
    let mut best = 0;
    let mut best_v = f32::INFINITY;
    for (i, &x) in v.iter().enumerate() {
        if x < best_v {
            best = i;
            best_v = x;
        }
    }
    best
}
