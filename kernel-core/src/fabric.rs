//! In-memory fabric: nodes, edges, lamport time, weights.
//!
//! Phase 1 keeps this deliberately small. Every observable thing — a CPU
//! feature, a memory region, a PCI device, an operator intent — is a node
//! identified by a BLAKE3 fingerprint of its canonical content. Edges
//! carry topology (this CPU contains this feature, this PCI device is on
//! this bus). Decay is approximated by a `weight` field that retrieval
//! reinforces and time eats away.

use alloc::{format, string::String, vec::Vec};
use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let h = blake3::hash(bytes);
        NodeId(*h.as_bytes())
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0[..6] {
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0[..6] {
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    Genesis {
        fabric_lamport: u64,
        observed: u32,
    },
    HwCpu {
        vendor: String,
        brand: String,
    },
    HwCpuFeature(String),
    HwMemoryRegion {
        start: u64,
        end: u64,
        kind: String,
    },
    HwPciDevice {
        bus: u8,
        device: u8,
        function: u8,
        vendor_id: u16,
        device_id: u16,
        class: u8,
        subclass: u8,
        prog_if: u8,
    },
    HwAcpiTable {
        signature: [u8; 4],
        address: u64,
        length: u32,
    },
    HwFramebuffer {
        width: u32,
        height: u32,
        bytes_per_pixel: u8,
        format: String,
    },
    HwStorage {
        kind: String,
        sectors: u64,
        sector_size: u32,
    },
    OperatorIntent {
        text: String,
        lamport: u64,
    },
    FabricResponse {
        text: String,
        lamport: u64,
    },
    SystemEvent {
        text: String,
        lamport: u64,
    },
    /// Serialised state of an active-inference hardware agent. The
    /// `params` blob is the agent's matrices + scalars in
    /// `kernel_core::model::DiscreteModel::serialize_to_bytes` format.
    /// Each agent class declares its own `kind` ("storage", "net", ...).
    /// On reboot the kernel scans for the latest LearnedDriver of each
    /// kind and re-initialises the corresponding agent from it instead
    /// of spawning fresh from spec-derived priors.
    LearnedDriver {
        kind: String,
        observations: u64,
        avg_surprise_x1000: u32,
        params: Vec<u8>,
    },
}

impl NodeKind {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // Cheap canonical form: tag byte + Debug rendering. Good enough
        // to give every distinct kind+content a distinct fingerprint.
        let s = format!("{:?}", self);
        let mut bytes = Vec::with_capacity(s.len() + 1);
        bytes.push(self.tag());
        bytes.extend_from_slice(s.as_bytes());
        bytes
    }

    pub fn tag(&self) -> u8 {
        match self {
            NodeKind::Genesis { .. } => 0,
            NodeKind::HwCpu { .. } => 1,
            NodeKind::HwCpuFeature(_) => 2,
            NodeKind::HwMemoryRegion { .. } => 3,
            NodeKind::HwPciDevice { .. } => 4,
            NodeKind::HwAcpiTable { .. } => 5,
            NodeKind::HwFramebuffer { .. } => 6,
            NodeKind::HwStorage { .. } => 7,
            NodeKind::OperatorIntent { .. } => 8,
            NodeKind::FabricResponse { .. } => 9,
            NodeKind::SystemEvent { .. } => 10,
            NodeKind::LearnedDriver { .. } => 11,
        }
    }

    pub fn short_label(&self) -> String {
        match self {
            NodeKind::Genesis { observed, .. } => format!("genesis ({} observed)", observed),
            NodeKind::HwCpu { vendor, brand } => format!("cpu {} / {}", vendor, brand),
            NodeKind::HwCpuFeature(f) => format!("cpu-feature {}", f),
            NodeKind::HwMemoryRegion { start, end, kind } => format!(
                "mem {:#x}..{:#x} ({})",
                start, end, kind
            ),
            NodeKind::HwPciDevice {
                bus,
                device,
                function,
                vendor_id,
                device_id,
                class,
                subclass,
                ..
            } => format!(
                "pci {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}:{:02x}",
                bus, device, function, vendor_id, device_id, class, subclass
            ),
            NodeKind::HwAcpiTable { signature, address, length } => {
                let sig = core::str::from_utf8(signature).unwrap_or("????");
                format!("acpi {} @ {:#x} ({} bytes)", sig, address, length)
            }
            NodeKind::HwFramebuffer {
                width,
                height,
                bytes_per_pixel,
                format,
            } => format!(
                "fb {}x{} {} bpp {}",
                width, height, bytes_per_pixel, format
            ),
            NodeKind::HwStorage {
                kind,
                sectors,
                sector_size,
            } => format!(
                "storage {} {} sectors x {} bytes",
                kind, sectors, sector_size
            ),
            NodeKind::OperatorIntent { text, .. } => format!("> {}", text),
            NodeKind::FabricResponse { text, .. } => format!("< {}", text),
            NodeKind::SystemEvent { text, .. } => format!("~ {}", text),
            NodeKind::LearnedDriver {
                kind,
                observations,
                avg_surprise_x1000,
                ..
            } => format!(
                "learned-driver {} obs={} surp={:.3}",
                kind,
                observations,
                (*avg_surprise_x1000 as f32) / 1000.0
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Containment — CPU contains feature, host contains PCI device, etc.
    Contains,
    /// Topology — device on bus, region within memory map.
    OnBus,
    /// Description — ACPI table describes hardware.
    Describes,
    /// Causation — intent caused response.
    Causes,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub created_at: u64, // lamport
    pub weight: f32,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
}

pub struct Fabric {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub lamport: u64,
}

impl Fabric {
    pub const fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            lamport: 0,
        }
    }

    pub fn tick(&mut self) -> u64 {
        self.lamport += 1;
        self.lamport
    }

    pub fn create(&mut self, kind: NodeKind) -> NodeId {
        let id = NodeId::from_bytes(&kind.canonical_bytes());
        // de-dup by id
        if !self.nodes.iter().any(|n| n.id == id) {
            let lamport = self.tick();
            self.nodes.push(Node {
                id,
                kind,
                created_at: lamport,
                weight: 1.0,
            });
        }
        id
    }

    pub fn link(&mut self, source: NodeId, target: NodeId, kind: EdgeKind) {
        if !self
            .edges
            .iter()
            .any(|e| e.source == source && e.target == target && e.kind == kind)
        {
            self.edges.push(Edge {
                source,
                target,
                kind,
            });
        }
    }

    pub fn count_by_tag(&self, tag: u8) -> usize {
        self.nodes.iter().filter(|n| n.kind.tag() == tag).count()
    }

    pub fn iter_kind(&self, tag: u8) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(move |n| n.kind.tag() == tag)
    }

    pub fn find(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn touch(&mut self, id: NodeId) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.weight += 0.1;
        }
    }

    pub fn decay(&mut self, factor: f32) {
        for n in self.nodes.iter_mut() {
            n.weight *= factor;
        }
    }
}

pub static FABRIC: spin::Mutex<Fabric> = spin::Mutex::new(Fabric::new());
