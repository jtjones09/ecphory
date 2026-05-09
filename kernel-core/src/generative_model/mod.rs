//! The unified generative model — one struct, one inference path. The
//! kernel IS this model plus a Shim. Each named region (devices,
//! persistence, operator, meta) holds a [`DiscreteModel`] of its own
//! domain; the causal graph and pattern engine compose across regions
//! to produce reasoning beyond reflex.
//!
//! Per `nisaba/positions/nucleation-architecture.md` the rule is:
//! everything the OS does is a consequence of one inference loop
//! reading and writing this struct. Today (Step 2 of nucleation) the
//! struct exists alongside the legacy `StorageAgent` driver; Step 3
//! retires the bare driver in favor of `DeviceRegion::storage_mut`.

pub mod causal_graph;
pub mod device_region;
pub mod history;
pub mod meta_region;
pub mod operator_region;
pub mod pattern_engine;
pub mod persistence_region;
pub mod preferences;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::tesseract::{LogKind, TESSERACT};

pub use causal_graph::{CausalCandidate, CausalEdge, CausalGraph, CausalNode};
pub use device_region::{DeviceModel, DeviceRegion};
pub use history::{ObservationHistory, SurpriseEntry};
pub use meta_region::{MetaAssessment, MetaRegion};
pub use operator_region::OperatorRegion;
pub use pattern_engine::{Hypothesis, PatternEngine};
pub use persistence_region::PersistenceRegion;
pub use preferences::Preferences;

const SERIAL_MAGIC: u64 = 0xEC0_C0DE_FAB_C0FFE;
// v2 adds:
//   - cumulative_surprise_since_last_persist (top-level)
//   - persist_threshold (persistence region)
//   - extended observation/action spaces in the operator region
// v1 snapshots fail magic-check via the version field and the kernel
// falls through to fresh genesis. Acceptable: pre-v2 snapshots are at
// most a few hours old and were captured during the noisy-log era.
const SERIAL_VERSION: u32 = 2;

pub struct GenerativeModel {
    pub devices: DeviceRegion,
    pub persistence: PersistenceRegion,
    pub operator: OperatorRegion,
    pub meta: MetaRegion,
    pub causal_graph: CausalGraph,
    pub pattern_engine: PatternEngine,
    pub preferences: Preferences,
    pub history: ObservationHistory,
    pub lamport: u64,
    pub boot_count: u64,
    pub total_observations: u64,
    pub cumulative_surprise: f32,
    /// Accumulated surprise across all regions since the last successful
    /// persist. Read by `persistence.should_persist_now()` to gate
    /// snapshot writes — when this exceeds `persistence.persist_threshold`,
    /// the inference loop persists. Reset by `note_persist_success()`.
    /// Replaces the old hardcoded `PERSIST_EVERY_LAMPORT = 5` cadence
    /// with a model-resident, learnable parameter.
    pub cumulative_surprise_since_last_persist: f32,
}

impl GenerativeModel {
    pub fn nucleation() -> Self {
        Self {
            devices: DeviceRegion::new(),
            persistence: PersistenceRegion::nucleation(),
            operator: OperatorRegion::nucleation(),
            meta: MetaRegion::nucleation(),
            causal_graph: CausalGraph::new(),
            pattern_engine: PatternEngine::new(),
            preferences: Preferences::nucleation(),
            history: ObservationHistory::new(),
            lamport: 0,
            boot_count: 0,
            total_observations: 0,
            cumulative_surprise: 0.0,
            cumulative_surprise_since_last_persist: 0.0,
        }
    }

    /// Reset the unsaved-information accumulator. Called by the
    /// inference loop after `snapshot::persist` returns Ok — the
    /// successful write means everything currently in the model is
    /// durable. Combined with `should_persist_now`, this gives the
    /// kernel an information-driven persist cadence: persist when
    /// there's information to lose, skip when there isn't.
    pub fn note_persist_success(&mut self) {
        self.cumulative_surprise_since_last_persist = 0.0;
    }

    /// Region-level summary string for the Tesseract overview line.
    pub fn render_overview(&self) -> alloc::string::String {
        alloc::format!(
            "model: boots={} obs={} F̄={:.2} | devs={} persist={} op={} meta={}",
            self.boot_count,
            self.total_observations,
            self.average_surprise(),
            self.devices.devices.len(),
            self.persistence.map_state_label(),
            self.operator.map_state_label(),
            self.meta.label(),
        )
    }

    pub fn average_surprise(&self) -> f32 {
        if self.total_observations == 0 {
            0.0
        } else {
            self.cumulative_surprise / self.total_observations as f32
        }
    }

    /// Account a fresh observation against the global counters and the
    /// short-window history. Per-region observe is the region's own
    /// responsibility.
    pub fn account_observation(&mut self, region: &str, surprise: f32, note: alloc::string::String) {
        self.total_observations = self.total_observations.saturating_add(1);
        self.cumulative_surprise += surprise;
        // Information-since-last-save accumulator. The persist gate
        // reads this and triggers when it exceeds the model's
        // persist_threshold — replaces the old hardcoded lamport
        // cadence with a learning-volume cadence.
        self.cumulative_surprise_since_last_persist += surprise;
        self.history.push_fe(surprise);
        if surprise > 1.5 {
            self.history.push_surprise(SurpriseEntry {
                lamport: self.lamport,
                region: region.into(),
                surprise,
                note,
            });
        }
    }

    pub fn tick(&mut self) -> u64 {
        self.lamport = self.lamport.saturating_add(1);
        self.lamport
    }

    pub fn note_boot(&mut self) {
        self.boot_count = self.boot_count.saturating_add(1);
    }

    /// Serialise the entire model to a byte stream. Used by Step 7's
    /// dedicated snapshot (and during Step 2 already validated by
    /// round-trip in unit tests). The format is deliberately simple so
    /// it can grow without breaking forward compatibility — version is
    /// in the header.
    pub fn serialize_to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 * 1024);
        out.extend_from_slice(&SERIAL_MAGIC.to_le_bytes());
        out.extend_from_slice(&SERIAL_VERSION.to_le_bytes());
        out.extend_from_slice(&self.lamport.to_le_bytes());
        out.extend_from_slice(&self.boot_count.to_le_bytes());
        out.extend_from_slice(&self.total_observations.to_le_bytes());
        out.extend_from_slice(&self.cumulative_surprise.to_le_bytes());
        out.extend_from_slice(&self.cumulative_surprise_since_last_persist.to_le_bytes());

        // Devices.
        out.extend_from_slice(&(self.devices.devices.len() as u32).to_le_bytes());
        for d in &self.devices.devices {
            put_str(&mut out, &d.label);
            put_str(&mut out, &d.kind);
            let bytes = d.snapshot_bytes();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }

        // Persistence.
        let p_bytes = self.persistence.snapshot_bytes();
        out.extend_from_slice(&(p_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&p_bytes);
        out.extend_from_slice(&self.persistence.successful_persists.to_le_bytes());
        out.extend_from_slice(&self.persistence.failed_persists.to_le_bytes());
        out.extend_from_slice(&self.persistence.successful_restores.to_le_bytes());
        out.extend_from_slice(&self.persistence.failed_restores.to_le_bytes());
        out.extend_from_slice(&self.persistence.persist_threshold.to_le_bytes());

        // Operator.
        let o_bytes = self.operator.snapshot_bytes();
        out.extend_from_slice(&(o_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&o_bytes);
        out.extend_from_slice(&self.operator.intents_seen.to_le_bytes());
        out.extend_from_slice(&self.operator.help_requests.to_le_bytes());
        out.extend_from_slice(&self.operator.unknown_commands.to_le_bytes());

        // Meta.
        let m_bytes = self.meta.snapshot_bytes();
        out.extend_from_slice(&(m_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&m_bytes);
        out.extend_from_slice(&self.meta.assessments_taken.to_le_bytes());
        out.extend_from_slice(&(self.meta.last_state as u32).to_le_bytes());

        self.causal_graph.serialize(&mut out);
        self.pattern_engine.serialize(&mut out);
        self.preferences.serialize(&mut out);
        self.history.serialize(&mut out);

        out
    }

    pub fn deserialize_from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut off = 0usize;
        let magic = read_u64(bytes, &mut off)?;
        if magic != SERIAL_MAGIC {
            return None;
        }
        let version = read_u32(bytes, &mut off)?;
        if version != SERIAL_VERSION {
            return None;
        }
        let lamport = read_u64(bytes, &mut off)?;
        let boot_count = read_u64(bytes, &mut off)?;
        let total_observations = read_u64(bytes, &mut off)?;
        let cumulative_surprise = read_f32(bytes, &mut off)?;
        let cumulative_surprise_since_last_persist = read_f32(bytes, &mut off)?;

        // Devices.
        let n_dev = read_u32(bytes, &mut off)? as usize;
        let mut devices = DeviceRegion::new();
        for _ in 0..n_dev {
            let label = read_str(bytes, &mut off)?;
            let kind = read_str(bytes, &mut off)?;
            let nb = read_u32(bytes, &mut off)? as usize;
            if off + nb > bytes.len() {
                return None;
            }
            let blob = &bytes[off..off + nb];
            off += nb;
            if kind == "storage" {
                let mut d = DeviceModel::storage(label);
                let _ = d.restore_from_bytes(blob);
                devices.add(d);
            }
        }

        // Persistence.
        let np = read_u32(bytes, &mut off)? as usize;
        if off + np > bytes.len() {
            return None;
        }
        let p_blob = bytes[off..off + np].to_vec();
        off += np;
        let mut persistence = PersistenceRegion::nucleation();
        let _ = persistence.restore_from_bytes(&p_blob);
        persistence.successful_persists = read_u64(bytes, &mut off)?;
        persistence.failed_persists = read_u64(bytes, &mut off)?;
        persistence.successful_restores = read_u64(bytes, &mut off)?;
        persistence.failed_restores = read_u64(bytes, &mut off)?;
        persistence.persist_threshold = read_f32(bytes, &mut off)?;

        // Operator.
        let no = read_u32(bytes, &mut off)? as usize;
        if off + no > bytes.len() {
            return None;
        }
        let o_blob = bytes[off..off + no].to_vec();
        off += no;
        let mut operator = OperatorRegion::nucleation();
        let _ = operator.restore_from_bytes(&o_blob);
        operator.intents_seen = read_u64(bytes, &mut off)?;
        operator.help_requests = read_u64(bytes, &mut off)?;
        operator.unknown_commands = read_u64(bytes, &mut off)?;

        // Meta.
        let nm = read_u32(bytes, &mut off)? as usize;
        if off + nm > bytes.len() {
            return None;
        }
        let m_blob = bytes[off..off + nm].to_vec();
        off += nm;
        let mut meta = MetaRegion::nucleation();
        let _ = meta.restore_from_bytes(&m_blob);
        meta.assessments_taken = read_u64(bytes, &mut off)?;
        meta.last_state = read_u32(bytes, &mut off)? as usize;

        let causal_graph = CausalGraph::deserialize(bytes, &mut off)?;
        let pattern_engine = PatternEngine::deserialize(bytes, &mut off)?;
        let preferences = Preferences::deserialize(bytes, &mut off)?;
        let history = ObservationHistory::deserialize(bytes, &mut off)?;

        Some(Self {
            devices,
            persistence,
            operator,
            meta,
            causal_graph,
            pattern_engine,
            preferences,
            history,
            lamport,
            boot_count,
            total_observations,
            cumulative_surprise,
            cumulative_surprise_since_last_persist,
        })
    }
}

pub static MODEL: spin::Mutex<Option<GenerativeModel>> = spin::Mutex::new(None);

// ---------- log candidates / submit_event ----------

/// A potential interaction-log entry. The kernel produces these
/// instead of calling `TESSERACT.lock().log_*` directly. The operator
/// region's curator decides whether to render or absorb.
#[derive(Clone, Debug)]
pub enum LogCandidate {
    /// Operator's own typed line. Always renders (force_render).
    OperatorInput(String),
    /// One line of a fabric command response. Always renders
    /// (force_render).
    FabricResponse(String),
    /// Architecture entry markers, "genesis complete", etc.
    Boot(String),
    /// `observed: cpu=... mem-regions=N pci=N rsdp=...`
    Genesis(String),
    /// GOP info, storage info, observe-only notice, type-help prompt.
    DeviceDiscovery(String),
    /// Per-cycle persist outcome. ok → routine (absorbs); !ok →
    /// failure (renders).
    PersistOutcome {
        ok: bool,
        bytes: usize,
        error: Option<String>,
    },
    /// Storage-agent step trace. Routine (absorbs).
    AgentStep(String),
    /// Meta-region self-assessment line. Borderline (absorbs).
    MetaAssessment(String),
    /// `restored N nodes / M edges from disk (lamport L)`. Load-bearing
    /// (renders).
    RestoreOk {
        nodes: usize,
        edges: usize,
        lamport: u64,
    },
    /// `no prior snapshot (...); fresh genesis`. Boot-time (renders).
    NoSnapshot(String),
    /// Catch-all warning. Always renders (failure-class).
    Failure(String),
}

impl LogCandidate {
    fn classify(&self) -> usize {
        use operator_region::*;
        match self {
            LogCandidate::OperatorInput(_) => OBS_KNOWN,
            LogCandidate::FabricResponse(_) => OBS_KNOWN,
            LogCandidate::Boot(_) => OBS_EVENT_GENESIS,
            LogCandidate::Genesis(_) => OBS_EVENT_GENESIS,
            LogCandidate::DeviceDiscovery(_) => OBS_EVENT_DEVICE_DISCOVERY,
            LogCandidate::PersistOutcome { ok: true, .. } => OBS_EVENT_PERSIST_OK,
            LogCandidate::PersistOutcome { ok: false, .. } => OBS_EVENT_FAILURE,
            LogCandidate::AgentStep(_) => OBS_EVENT_AGENT_STEP,
            LogCandidate::MetaAssessment(_) => OBS_EVENT_META,
            LogCandidate::RestoreOk { .. } => OBS_EVENT_RESTORE_OK,
            LogCandidate::NoSnapshot(_) => OBS_EVENT_GENESIS,
            LogCandidate::Failure(_) => OBS_EVENT_FAILURE,
        }
    }

    fn render(&self) -> (LogKind, String) {
        match self {
            LogCandidate::OperatorInput(s) => (LogKind::Operator, s.clone()),
            LogCandidate::FabricResponse(s) => (LogKind::Fabric, s.clone()),
            LogCandidate::Boot(s) => (LogKind::System, s.clone()),
            LogCandidate::Genesis(s) => (LogKind::System, s.clone()),
            LogCandidate::DeviceDiscovery(s) => (LogKind::System, s.clone()),
            LogCandidate::PersistOutcome { ok: true, bytes, .. } => {
                (LogKind::System, format!("persisted {} bytes", bytes))
            }
            LogCandidate::PersistOutcome { ok: false, error, .. } => (
                LogKind::Warning,
                format!(
                    "persist failed: {}",
                    error.as_deref().unwrap_or("unknown")
                ),
            ),
            LogCandidate::AgentStep(s) => (LogKind::System, s.clone()),
            LogCandidate::MetaAssessment(s) => (LogKind::System, s.clone()),
            LogCandidate::RestoreOk {
                nodes,
                edges,
                lamport,
            } => (
                LogKind::System,
                format!(
                    "restored {} nodes / {} edges from disk (lamport {})",
                    nodes, edges, lamport
                ),
            ),
            LogCandidate::NoSnapshot(s) => (LogKind::System, s.clone()),
            LogCandidate::Failure(s) => (LogKind::Warning, s.clone()),
        }
    }

    fn force_render(&self) -> bool {
        // Operator I/O is never a "candidate" — it's literal. Failures
        // also force-render so the operator never misses one even if
        // the agent's belief about C drifts.
        matches!(
            self,
            LogCandidate::OperatorInput(_)
                | LogCandidate::FabricResponse(_)
                | LogCandidate::Failure(_)
                | LogCandidate::PersistOutcome { ok: false, .. }
        )
    }
}

/// Submit a log candidate to the model. ALWAYS updates the operator
/// region's beliefs (the model learns from every event, regardless of
/// whether the curator decides to render). Renders to the Tesseract
/// iff the operator region's `select_render_action` returns
/// `ACT_RENDER_TO_LOG`, OR if the candidate force-renders (operator
/// I/O and failures).
///
/// Pre-nucleation (model is `None`): renders unconditionally — the
/// kernel hasn't built its mind yet, so we can't ask it.
pub fn submit_event(candidate: LogCandidate) {
    let render = {
        let mut slot = MODEL.lock();
        match slot.as_mut() {
            Some(model) => {
                let obs = candidate.classify();
                let _ = model.operator.observe_event(obs);
                candidate.force_render()
                    || model.operator.select_render_action(obs)
                        == operator_region::ACT_RENDER_TO_LOG
            }
            None => true,
        }
    };
    if render {
        let (kind, text) = candidate.render();
        let mut t = TESSERACT.lock();
        match kind {
            LogKind::System => t.log_system(text),
            LogKind::Operator => t.log_operator(text),
            LogKind::Fabric => t.log_fabric(text),
            LogKind::Warning => t.log_warning(text),
        }
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}
fn read_u16(b: &[u8], off: &mut usize) -> Option<u16> {
    if *off + 2 > b.len() {
        return None;
    }
    let v = u16::from_le_bytes(b[*off..*off + 2].try_into().ok()?);
    *off += 2;
    Some(v)
}
fn read_u32(b: &[u8], off: &mut usize) -> Option<u32> {
    if *off + 4 > b.len() {
        return None;
    }
    let v = u32::from_le_bytes(b[*off..*off + 4].try_into().ok()?);
    *off += 4;
    Some(v)
}
fn read_u64(b: &[u8], off: &mut usize) -> Option<u64> {
    if *off + 8 > b.len() {
        return None;
    }
    let v = u64::from_le_bytes(b[*off..*off + 8].try_into().ok()?);
    *off += 8;
    Some(v)
}
fn read_f32(b: &[u8], off: &mut usize) -> Option<f32> {
    if *off + 4 > b.len() {
        return None;
    }
    let v = f32::from_le_bytes(b[*off..*off + 4].try_into().ok()?);
    *off += 4;
    Some(v)
}
fn read_str(b: &[u8], off: &mut usize) -> Option<alloc::string::String> {
    let n = read_u16(b, off)? as usize;
    if *off + n > b.len() {
        return None;
    }
    let s = core::str::from_utf8(&b[*off..*off + n]).ok()?.into();
    *off += n;
    Some(s)
}
