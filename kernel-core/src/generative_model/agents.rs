//! Agents — the fabric's specialists, expressed as scoped views over
//! the shared `GenerativeModel`. Per `handoff-cc-fabric-v1.md` §2-3:
//! every agent owns a `Scope` declaring which regions it advances on
//! its tick, but the regions themselves stay in one `GenerativeModel`.
//! This preserves the single-snapshot persistence guarantees from
//! 25d/25e while introducing differentiation.
//!
//! v1 starts with exactly one agent at nucleation: `nucleation`, id 0,
//! full scope. Spawning lifecycle actions land in commit 3; until then
//! the kernel is effectively single-agent in behavior (the registry
//! has one entry, the inference loop runs that entry's scope).
//!
//! NAMING NOTE: the handoff's file map places this module at
//! `kernel-core/src/fabric/mod.rs` with the GenerativeModel field
//! named `fabric: AgentRegistry`. CC put it here at
//! `generative_model/agents.rs` with field `agents` to avoid
//! conflicting with the existing `kernel-core/src/fabric.rs` (the
//! node-store module that owns `FABRIC: Mutex<Fabric>`). Same
//! architectural intent, clearer naming against the existing module
//! layout. Spawn/dissolve handlers in commit 3 will follow the same
//! convention.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Identifier for an agent in the registry. Stable across reboots
/// (persisted as part of the snapshot). Allocated by `AgentRegistry`
/// monotonically — once given out, never reused. Aligns with the
/// fabric's general "nothing is deleted" discipline; a dissolved
/// agent's id is gone, replacement specialists get fresh ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AgentId(pub u32);

/// Lifecycle state. v1 needs three values; commit 3 may add more
/// (e.g., Quarantined when the immune system flags an agent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    /// Stepping normally each cycle that matches its cadence.
    Healthy,
    /// Selected `act_dissolve_self` on its last step; `reap_dissolving_agents`
    /// at the end of this loop iteration removes it from the registry.
    /// v1 commit 3 reads this; commit 2 declares it for forward
    /// compatibility but never enters this state.
    Dying,
    /// Suspended — does not step. v1 reserves this state but does not
    /// use it.
    Suspended,
}

/// Which regions of the shared `GenerativeModel` this agent advances
/// on its tick. v1 uses a fixed bitset over the four named regions
/// plus the meta region; v2 can grow this as new region kinds appear.
///
/// `ScopeFlags::ALL` is the nucleation agent's scope and gives full
/// authority including meta (the spawn-action authority lives on
/// agents whose scope includes META per the handoff §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeFlags(pub u8);

impl ScopeFlags {
    pub const META: u8 = 1 << 0;
    pub const OPERATOR: u8 = 1 << 1;
    pub const DEVICES: u8 = 1 << 2;
    pub const PERSISTENCE: u8 = 1 << 3;

    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(Self::META | Self::OPERATOR | Self::DEVICES | Self::PERSISTENCE);

    pub fn contains(self, region: u8) -> bool {
        (self.0 & region) != 0
    }

    pub fn labels(self) -> &'static [&'static str] {
        // Pre-computed all-regions list. v1 only uses ALL so a single
        // path is enough; commit 3 can replace this with a per-scope
        // computation when partial scopes become real.
        if self.0 == Self::ALL.0 {
            &["META", "OPERATOR", "DEVICES", "PERSISTENCE"]
        } else if self.0 == 0 {
            &[]
        } else {
            // Fallback for partial scopes that may appear in commit 3.
            // Render order: META, OPERATOR, DEVICES, PERSISTENCE.
            // The caller can read individual flags via `contains`.
            &[]
        }
    }
}

/// Per-agent scope: regions + which shim ops the agent can invoke.
/// Shim ops aren't enforced in v1 (the nucleation agent has all of
/// them); commit 3 wires per-agent shim filtering when spawned
/// specialists land. Declared here so the snapshot format is stable
/// from v3 onward.
#[derive(Clone, Copy, Debug)]
pub struct Scope {
    pub regions: ScopeFlags,
    pub shim_ops: ShimOpFlags,
}

impl Scope {
    pub const FULL: Self = Self {
        regions: ScopeFlags::ALL,
        shim_ops: ShimOpFlags::ALL,
    };
}

/// Which shim operations this agent may issue. v1 only checks this on
/// nucleation (always ALL); v2 enforces per-agent restrictions.
#[derive(Clone, Copy, Debug)]
pub struct ShimOpFlags(pub u16);

impl ShimOpFlags {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0xFFFF);
}

const SURPRISE_WINDOW: usize = 64;
const CONTRIBUTION_WINDOW: usize = 64;

/// Ring buffer of recent surprise values for this agent. The mean
/// over the window is the agent's read-out of its own "how much novel
/// information am I processing." Commit 3 reads this for the spawn
/// action's structural prior on novelty.
#[derive(Clone, Debug)]
pub struct SurpriseWindow {
    pub samples: [f32; SURPRISE_WINDOW],
    pub head: u8,
    pub filled: u8,
}

impl SurpriseWindow {
    pub const fn new() -> Self {
        Self {
            samples: [0.0; SURPRISE_WINDOW],
            head: 0,
            filled: 0,
        }
    }

    pub fn push(&mut self, v: f32) {
        self.samples[self.head as usize] = v;
        self.head = ((self.head as usize + 1) % SURPRISE_WINDOW) as u8;
        if (self.filled as usize) < SURPRISE_WINDOW {
            self.filled += 1;
        }
    }

    pub fn mean(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for i in 0..self.filled as usize {
            sum += self.samples[i];
        }
        sum / self.filled as f32
    }
}

/// Ring buffer of this agent's per-cycle free-energy contribution.
/// The handoff §4 says `act_dissolve_self` reads the mean of this
/// window: if the agent has been pulling its weight, dissolution is
/// dispreferred; if it's been dead weight, dissolution is preferred.
/// Same shape as `SurpriseWindow`.
#[derive(Clone, Debug)]
pub struct ContributionWindow {
    pub samples: [f32; CONTRIBUTION_WINDOW],
    pub head: u8,
    pub filled: u8,
}

impl ContributionWindow {
    pub const fn new() -> Self {
        Self {
            samples: [0.0; CONTRIBUTION_WINDOW],
            head: 0,
            filled: 0,
        }
    }

    pub fn push(&mut self, v: f32) {
        self.samples[self.head as usize] = v;
        self.head = ((self.head as usize + 1) % CONTRIBUTION_WINDOW) as u8;
        if (self.filled as usize) < CONTRIBUTION_WINDOW {
            self.filled += 1;
        }
    }

    pub fn mean(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for i in 0..self.filled as usize {
            sum += self.samples[i];
        }
        sum / self.filled as f32
    }
}

/// One agent in the fabric. The nucleation agent at boot has id 0,
/// name "nucleation", full scope. Spawned specialists in commit 3 get
/// fresh ids and narrowed scopes; the structure here doesn't change.
#[derive(Clone, Debug)]
pub struct Agent {
    pub id: AgentId,
    pub name: String,
    pub scope: Scope,
    pub tick_cadence: u32,
    pub last_tick: u64,
    pub spawned_at_lamport: u64,
    pub spawned_by: Option<AgentId>,
    pub state: AgentState,
    pub surprise_window: SurpriseWindow,
    pub contribution_window: ContributionWindow,
}

impl Agent {
    /// The boot-time nucleation agent. Full scope, cadence 1 (steps
    /// every loop iteration). Spawned by nobody; id 0; name "nucleation".
    pub fn nucleation() -> Self {
        Self {
            id: AgentId(0),
            name: "nucleation".into(),
            scope: Scope::FULL,
            tick_cadence: 1,
            last_tick: 0,
            spawned_at_lamport: 0,
            spawned_by: None,
            state: AgentState::Healthy,
            surprise_window: SurpriseWindow::new(),
            contribution_window: ContributionWindow::new(),
        }
    }

    /// One-line render for the FABRIC panel.
    pub fn render_summary(&self) -> String {
        let regions = self.scope.regions.labels();
        let regions_str = if regions.is_empty() {
            String::from("?")
        } else {
            let mut s = String::new();
            for (i, r) in regions.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(r);
            }
            s
        };
        let state_str = match self.state {
            AgentState::Healthy => "healthy",
            AgentState::Dying => "dying",
            AgentState::Suspended => "suspended",
        };
        format!(
            "agent[{}] regions=[{}] cadence={} surp={:.2} contrib={:+.2} state={}",
            self.name,
            regions_str,
            self.tick_cadence,
            self.surprise_window.mean(),
            self.contribution_window.mean(),
            state_str
        )
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.id.0.to_le_bytes());
        put_str(out, &self.name);
        out.extend_from_slice(&[self.scope.regions.0]);
        out.extend_from_slice(&self.scope.shim_ops.0.to_le_bytes());
        out.extend_from_slice(&self.tick_cadence.to_le_bytes());
        out.extend_from_slice(&self.last_tick.to_le_bytes());
        out.extend_from_slice(&self.spawned_at_lamport.to_le_bytes());
        out.extend_from_slice(&self.spawned_by.map_or(u32::MAX, |a| a.0).to_le_bytes());
        let state_byte: u8 = match self.state {
            AgentState::Healthy => 0,
            AgentState::Dying => 1,
            AgentState::Suspended => 2,
        };
        out.push(state_byte);
        // Surprise window
        out.push(self.surprise_window.head);
        out.push(self.surprise_window.filled);
        for v in &self.surprise_window.samples {
            out.extend_from_slice(&v.to_le_bytes());
        }
        // Contribution window
        out.push(self.contribution_window.head);
        out.push(self.contribution_window.filled);
        for v in &self.contribution_window.samples {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        let id = AgentId(read_u32(bytes, off)?);
        let name = read_str(bytes, off)?;
        let regions = ScopeFlags(read_u8(bytes, off)?);
        let shim_ops = ShimOpFlags(read_u16(bytes, off)?);
        let tick_cadence = read_u32(bytes, off)?;
        let last_tick = read_u64(bytes, off)?;
        let spawned_at_lamport = read_u64(bytes, off)?;
        let parent_id = read_u32(bytes, off)?;
        let spawned_by = if parent_id == u32::MAX {
            None
        } else {
            Some(AgentId(parent_id))
        };
        let state = match read_u8(bytes, off)? {
            0 => AgentState::Healthy,
            1 => AgentState::Dying,
            2 => AgentState::Suspended,
            _ => return None,
        };
        let mut surprise_window = SurpriseWindow::new();
        surprise_window.head = read_u8(bytes, off)?;
        surprise_window.filled = read_u8(bytes, off)?;
        for v in surprise_window.samples.iter_mut() {
            *v = read_f32(bytes, off)?;
        }
        let mut contribution_window = ContributionWindow::new();
        contribution_window.head = read_u8(bytes, off)?;
        contribution_window.filled = read_u8(bytes, off)?;
        for v in contribution_window.samples.iter_mut() {
            *v = read_f32(bytes, off)?;
        }
        Some(Self {
            id,
            name,
            scope: Scope { regions, shim_ops },
            tick_cadence,
            last_tick,
            spawned_at_lamport,
            spawned_by,
            state,
            surprise_window,
            contribution_window,
        })
    }
}

/// The kernel's agent registry. v1 caps at 8 agents (heapless-style
/// bounded growth). nucleation always occupies index 0. Commit 3 may
/// raise the cap when spawning becomes possible; the cap is a
/// performance bound, not an architectural one.
#[derive(Clone, Debug)]
pub struct AgentRegistry {
    pub agents: Vec<Agent>,
    pub next_id: u32,
}

impl AgentRegistry {
    pub fn nucleation() -> Self {
        let nucleation = Agent::nucleation();
        Self {
            agents: alloc::vec![nucleation],
            next_id: 1,
        }
    }

    pub fn find(&self, id: AgentId) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn find_mut(&mut self, id: AgentId) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|a| a.id == id)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == name)
    }

    pub fn count(&self) -> usize {
        self.agents.len()
    }

    /// Render the FABRIC panel for the Tesseract: one line per agent,
    /// in id order.
    pub fn render_panel(&self) -> Vec<String> {
        self.agents.iter().map(|a| a.render_summary()).collect()
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.agents.len() as u32).to_le_bytes());
        for a in &self.agents {
            a.serialize(out);
        }
        out.extend_from_slice(&self.next_id.to_le_bytes());
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        let n = read_u32(bytes, off)? as usize;
        let mut agents = Vec::with_capacity(n);
        for _ in 0..n {
            agents.push(Agent::deserialize(bytes, off)?);
        }
        let next_id = read_u32(bytes, off)?;
        Some(Self { agents, next_id })
    }
}

// ---- byte helpers ----

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn read_u8(b: &[u8], off: &mut usize) -> Option<u8> {
    if *off + 1 > b.len() {
        return None;
    }
    let v = b[*off];
    *off += 1;
    Some(v)
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
fn read_str(b: &[u8], off: &mut usize) -> Option<String> {
    let n = read_u16(b, off)? as usize;
    if *off + n > b.len() {
        return None;
    }
    let s = core::str::from_utf8(&b[*off..*off + n]).ok()?.into();
    *off += n;
    Some(s)
}
