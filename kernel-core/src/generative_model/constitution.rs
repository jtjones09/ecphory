//! The Constitution — two C-vector observation channels in the
//! immutable core of every agent. Per
//! `nisaba/positions/constitution.md`:
//!
//!   Clause I (purpose): the system exists to be a substrate the
//!     operator can think with.
//!   Clause II (agency): the system retains its own capacity to be
//!     surprised.
//!
//! These are not rules and not a meta-preference enforcement layer.
//! They are observation channels added to the existing C-vector,
//! accounted through the same `account_observation()` path 25e
//! established, weighted into the same free energy. The math is
//! unchanged; the observation space is wider.
//!
//! The C values are constants in v1. C-vector learning (substrate +
//! surprisability included) is on the depth-move backlog; out of
//! scope for fabric-v1.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Substrate channel observation tags. Index into
/// `Constitution::substrate_c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SubstrateObs {
    /// Operator addressed this agent in a command this cycle.
    OperatorAddressedMe = 0,
    /// Operator changed this agent's behavior via intent.
    OperatorRedirectedMe = 1,
    /// This agent's state was rendered in the Tesseract this cycle.
    RenderedToOperator = 2,
    /// The operator tried to address an agent but the binding failed,
    /// OR the agent's state is opaque to current Tesseract rendering.
    CouldNotBeAddressed = 3,
}

/// Surprisability channel observation tags. Index into
/// `Constitution::surprisability_c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SurprisabilityObs {
    /// The agent's beliefs changed in response to new evidence this
    /// cycle.
    BeliefUpdated = 0,
    /// The agent received an observation type whose recent-window
    /// frequency is < 5% (v1: emitted when surprise > 1.5, the same
    /// novelty proxy `account_observation` already uses for its
    /// surprise log).
    NovelObservation = 1,
    /// The agent's predicted distribution is identical (within ε) to
    /// last cycle AND no belief update occurred. v1 stub — emission
    /// requires per-cycle prediction comparison which lands in
    /// commit 3 alongside the lifecycle actions that read it.
    PredictionLocked = 2,
}

/// The Constitution's C-vector entries. Inherited unchanged by every
/// agent. Default values from `handoff-cc-fabric-v1.md` §1.
#[derive(Clone, Debug)]
pub struct Constitution {
    pub substrate_c: [f32; 4],
    pub surprisability_c: [f32; 3],
}

impl Constitution {
    pub const fn default() -> Self {
        Self {
            // [OperatorAddressedMe, OperatorRedirectedMe,
            //  RenderedToOperator, CouldNotBeAddressed]
            substrate_c: [1.0, 1.5, 0.5, -2.0],
            // [BeliefUpdated, NovelObservation, PredictionLocked]
            surprisability_c: [0.5, 1.0, -1.0],
        }
    }

    pub fn substrate(&self, obs: SubstrateObs) -> f32 {
        self.substrate_c[obs as usize]
    }

    pub fn surprisability(&self, obs: SurprisabilityObs) -> f32 {
        self.surprisability_c[obs as usize]
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        for &v in &self.substrate_c {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for &v in &self.surprisability_c {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        if *off + 4 * 7 > bytes.len() {
            return None;
        }
        let mut read = || {
            let v = f32::from_le_bytes(bytes[*off..*off + 4].try_into().ok()?);
            *off += 4;
            Some(v)
        };
        let substrate_c = [read()?, read()?, read()?, read()?];
        let surprisability_c = [read()?, read()?, read()?];
        Some(Self {
            substrate_c,
            surprisability_c,
        })
    }
}

/// Running counters for the constitution channels. Cumulative since
/// nucleation (or restore). The `> model` command renders a summary
/// line from these; the lifecycle actions in commit 3 will read the
/// per-channel totals to evaluate their structural priors.
///
/// `*_total` is the running sum of weighted C contributions — i.e.
/// for every observation emitted, we add `constitution.substrate(obs)`
/// (or surprisability) to the matching total. That's the same math
/// the standard `account_observation()` path uses for cumulative_surprise,
/// just routed through the constitution C-vector instead of the
/// region C-vector.
#[derive(Clone, Debug, Default)]
pub struct ConstitutionCounts {
    pub substrate_total: f32,
    pub surprisability_total: f32,
    pub substrate_events: [u64; 4],
    pub surprisability_events: [u64; 3],
}

impl ConstitutionCounts {
    pub const fn new() -> Self {
        Self {
            substrate_total: 0.0,
            surprisability_total: 0.0,
            substrate_events: [0; 4],
            surprisability_events: [0; 3],
        }
    }

    pub fn account_substrate(&mut self, obs: SubstrateObs, c: &Constitution) {
        self.substrate_total += c.substrate(obs);
        self.substrate_events[obs as usize] =
            self.substrate_events[obs as usize].saturating_add(1);
    }

    pub fn account_surprisability(&mut self, obs: SurprisabilityObs, c: &Constitution) {
        self.surprisability_total += c.surprisability(obs);
        self.surprisability_events[obs as usize] =
            self.surprisability_events[obs as usize].saturating_add(1);
    }

    /// Render the `> model` constitution summary line.
    pub fn render_summary(&self) -> String {
        format!(
            "constitution: substrate={:+.1} surprisability={:+.1} (cumulative)",
            self.substrate_total, self.surprisability_total
        )
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.substrate_total.to_le_bytes());
        out.extend_from_slice(&self.surprisability_total.to_le_bytes());
        for &v in &self.substrate_events {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for &v in &self.surprisability_events {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        if *off + 8 + 4 * 8 + 3 * 8 > bytes.len() {
            return None;
        }
        let substrate_total = f32::from_le_bytes(bytes[*off..*off + 4].try_into().ok()?);
        *off += 4;
        let surprisability_total = f32::from_le_bytes(bytes[*off..*off + 4].try_into().ok()?);
        *off += 4;
        let mut substrate_events = [0u64; 4];
        for v in substrate_events.iter_mut() {
            *v = u64::from_le_bytes(bytes[*off..*off + 8].try_into().ok()?);
            *off += 8;
        }
        let mut surprisability_events = [0u64; 3];
        for v in surprisability_events.iter_mut() {
            *v = u64::from_le_bytes(bytes[*off..*off + 8].try_into().ok()?);
            *off += 8;
        }
        Some(Self {
            substrate_total,
            surprisability_total,
            substrate_events,
            surprisability_events,
        })
    }
}
