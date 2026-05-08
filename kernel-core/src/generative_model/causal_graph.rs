//! Causal graph engine — small directed graph of causes and effects
//! discovered through observation. The kernel adds an edge whenever an
//! action precedes an observation; backward-tracing from a surprising
//! observation yields candidate causes.
//!
//! The graph is intentionally tiny (target capacity ~50–200 nodes for
//! a small kernel; ~2000 if scaled). All operations are linear scans —
//! that's faster than maintaining an index for graphs this small.
//!
//! Step 2 lays down the data structures and serialization. Step 5
//! turns the inference loop into a producer that calls `record` on
//! every observation/action pair, and adds the `backward_trace` query
//! used by the `causal` command.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const MAX_NODES: usize = 256;
const MAX_EDGES: usize = 1024;

#[derive(Clone, Debug)]
pub struct CausalNode {
    pub id: u32,
    pub label: String,
    pub region: String,
    pub observations: u32,
}

#[derive(Clone, Debug)]
pub struct CausalEdge {
    pub cause: u32,
    pub effect: u32,
    pub strength: f32,  // exponential moving average of co-occurrence
    pub observations: u32,
}

#[derive(Clone, Debug)]
pub struct CausalCandidate {
    pub cause_id: u32,
    pub cause_label: String,
    pub strength: f32,
    pub observations: u32,
}

pub struct CausalGraph {
    pub nodes: Vec<CausalNode>,
    pub edges: Vec<CausalEdge>,
    pub label_index: BTreeMap<String, u32>,
    next_id: u32,
}

impl CausalGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            label_index: BTreeMap::new(),
            next_id: 0,
        }
    }

    /// Get-or-create a node by label. Returns the node id.
    pub fn intern(&mut self, label: &str, region: &str) -> u32 {
        if let Some(&id) = self.label_index.get(label) {
            // bump observation count
            if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
                n.observations = n.observations.saturating_add(1);
            }
            return id;
        }
        if self.nodes.len() >= MAX_NODES {
            // graph full — evict the lowest-observation node that has
            // no inbound or outbound edges (cheapest to remove).
            self.evict_one();
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.nodes.push(CausalNode {
            id,
            label: label.to_string(),
            region: region.to_string(),
            observations: 1,
        });
        self.label_index.insert(label.to_string(), id);
        id
    }

    /// Record `cause → effect`. If the edge exists, strengthen it; if
    /// not, create it.
    pub fn record(&mut self, cause: u32, effect: u32) {
        if cause == effect {
            return;
        }
        if let Some(e) = self
            .edges
            .iter_mut()
            .find(|e| e.cause == cause && e.effect == effect)
        {
            // EMA on co-occurrence: strength inches toward 1.0.
            e.observations = e.observations.saturating_add(1);
            e.strength = e.strength * 0.95 + 0.05;
            return;
        }
        if self.edges.len() >= MAX_EDGES {
            self.evict_weakest_edge();
        }
        self.edges.push(CausalEdge {
            cause,
            effect,
            strength: 0.10,
            observations: 1,
        });
    }

    /// Find candidate causes for `effect`, ordered by edge strength.
    pub fn backward_trace(&self, effect: u32, limit: usize) -> Vec<CausalCandidate> {
        let mut candidates: Vec<CausalCandidate> = self
            .edges
            .iter()
            .filter(|e| e.effect == effect)
            .filter_map(|e| {
                let n = self.nodes.iter().find(|n| n.id == e.cause)?;
                Some(CausalCandidate {
                    cause_id: e.cause,
                    cause_label: n.label.clone(),
                    strength: e.strength,
                    observations: e.observations,
                })
            })
            .collect();
        candidates.sort_by(|a, b| {
            b.strength
                .partial_cmp(&a.strength)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);
        candidates
    }

    pub fn node_by_label(&self, label: &str) -> Option<&CausalNode> {
        let id = self.label_index.get(label)?;
        self.nodes.iter().find(|n| n.id == *id)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Render top-N edges by strength as human-readable lines for the
    /// `causal` command output.
    pub fn render_top_edges(&self, limit: usize) -> Vec<String> {
        let mut sorted: Vec<&CausalEdge> = self.edges.iter().collect();
        sorted.sort_by(|a, b| {
            b.strength
                .partial_cmp(&a.strength)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        let mut out = Vec::new();
        for e in sorted.into_iter().take(limit) {
            let cause = self
                .nodes
                .iter()
                .find(|n| n.id == e.cause)
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            let effect = self
                .nodes
                .iter()
                .find(|n| n.id == e.effect)
                .map(|n| n.label.as_str())
                .unwrap_or("?");
            out.push(alloc::format!(
                "{} → {} (strength {:.2}, obs {})",
                cause,
                effect,
                e.strength,
                e.observations
            ));
        }
        out
    }

    fn evict_one(&mut self) {
        if let Some((idx, _)) = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                let id = n.id;
                !self
                    .edges
                    .iter()
                    .any(|e| e.cause == id || e.effect == id)
            })
            .min_by_key(|(_, n)| n.observations)
        {
            let evicted = self.nodes.remove(idx);
            self.label_index.remove(&evicted.label);
        }
    }

    fn evict_weakest_edge(&mut self) {
        if let Some((idx, _)) = self.edges.iter().enumerate().min_by(|(_, a), (_, b)| {
            a.strength
                .partial_cmp(&b.strength)
                .unwrap_or(core::cmp::Ordering::Equal)
        }) {
            self.edges.remove(idx);
        }
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        for n in &self.nodes {
            out.extend_from_slice(&n.id.to_le_bytes());
            put_str(out, &n.label);
            put_str(out, &n.region);
            out.extend_from_slice(&n.observations.to_le_bytes());
        }
        out.extend_from_slice(&(self.edges.len() as u32).to_le_bytes());
        for e in &self.edges {
            out.extend_from_slice(&e.cause.to_le_bytes());
            out.extend_from_slice(&e.effect.to_le_bytes());
            out.extend_from_slice(&e.strength.to_le_bytes());
            out.extend_from_slice(&e.observations.to_le_bytes());
        }
        out.extend_from_slice(&self.next_id.to_le_bytes());
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        let n = read_u32(bytes, off)? as usize;
        let mut nodes = Vec::with_capacity(n);
        let mut label_index = BTreeMap::new();
        for _ in 0..n {
            let id = read_u32(bytes, off)?;
            let label = read_str(bytes, off)?;
            let region = read_str(bytes, off)?;
            let observations = read_u32(bytes, off)?;
            label_index.insert(label.clone(), id);
            nodes.push(CausalNode {
                id,
                label,
                region,
                observations,
            });
        }
        let m = read_u32(bytes, off)? as usize;
        let mut edges = Vec::with_capacity(m);
        for _ in 0..m {
            let cause = read_u32(bytes, off)?;
            let effect = read_u32(bytes, off)?;
            let strength = read_f32(bytes, off)?;
            let observations = read_u32(bytes, off)?;
            edges.push(CausalEdge {
                cause,
                effect,
                strength,
                observations,
            });
        }
        let next_id = read_u32(bytes, off)?;
        Some(Self {
            nodes,
            edges,
            label_index,
            next_id,
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
