//! Short-window observation history — a fixed-capacity ring of recent
//! free-energy values plus the most-surprising recent observation. Read
//! by the meta-region to compute ΔF/Δt and observation entropy, and by
//! the `surprise` command to show what surprised the agent recently.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

const FE_WINDOW: usize = 100;
const SURPRISE_LOG: usize = 16;

#[derive(Clone, Debug)]
pub struct SurpriseEntry {
    pub lamport: u64,
    pub region: String,
    pub surprise: f32,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct ObservationHistory {
    pub free_energy: VecDeque<f32>,
    pub surprises: VecDeque<SurpriseEntry>,
}

impl ObservationHistory {
    pub fn new() -> Self {
        Self {
            free_energy: VecDeque::with_capacity(FE_WINDOW),
            surprises: VecDeque::with_capacity(SURPRISE_LOG),
        }
    }

    pub fn push_fe(&mut self, fe: f32) {
        if self.free_energy.len() == FE_WINDOW {
            self.free_energy.pop_front();
        }
        self.free_energy.push_back(fe);
    }

    pub fn push_surprise(&mut self, entry: SurpriseEntry) {
        if self.surprises.len() == SURPRISE_LOG {
            self.surprises.pop_front();
        }
        self.surprises.push_back(entry);
    }

    /// ΔF/Δt over the most recent window — positive means free energy
    /// is climbing (model losing fit), negative means it is falling
    /// (model learning). Returns `None` if the window is too short.
    pub fn delta_fe(&self) -> Option<f32> {
        if self.free_energy.len() < 4 {
            return None;
        }
        let n = self.free_energy.len();
        let half = n / 2;
        let early: f32 =
            self.free_energy.iter().take(half).sum::<f32>() / (half as f32);
        let late: f32 =
            self.free_energy.iter().skip(half).sum::<f32>() / ((n - half) as f32);
        Some(late - early)
    }

    pub fn current_fe(&self) -> Option<f32> {
        self.free_energy.back().copied()
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.free_energy.len() as u32).to_le_bytes());
        for &v in &self.free_energy {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.surprises.len() as u32).to_le_bytes());
        for s in &self.surprises {
            out.extend_from_slice(&s.lamport.to_le_bytes());
            put_str(out, &s.region);
            out.extend_from_slice(&s.surprise.to_le_bytes());
            put_str(out, &s.note);
        }
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        let fe_n = read_u32(bytes, off)? as usize;
        let mut free_energy = VecDeque::with_capacity(fe_n);
        for _ in 0..fe_n {
            free_energy.push_back(read_f32(bytes, off)?);
        }
        let s_n = read_u32(bytes, off)? as usize;
        let mut surprises = VecDeque::with_capacity(s_n);
        for _ in 0..s_n {
            let lamport = read_u64(bytes, off)?;
            let region = read_str(bytes, off)?;
            let surprise = read_f32(bytes, off)?;
            let note = read_str(bytes, off)?;
            surprises.push_back(SurpriseEntry {
                lamport,
                region,
                surprise,
                note,
            });
        }
        Some(Self {
            free_energy,
            surprises,
        })
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
fn read_str(b: &[u8], off: &mut usize) -> Option<String> {
    let n = read_u16(b, off)? as usize;
    if *off + n > b.len() {
        return None;
    }
    let s = core::str::from_utf8(&b[*off..*off + n]).ok()?.into();
    *off += n;
    Some(s)
}
