//! Ecphory kernel-core: the substrate-agnostic fabric.
//!
//! This crate is the load-bearing claim of the substrate-agnosticism
//! principle: nothing in here knows whether it's running on x86_64
//! silicon, ARM silicon, or some future neuromorphic substrate. Every
//! byte that depends on a specific instruction set lives in the
//! per-architecture binary crate, not here.
//!
//! What this crate provides:
//!  - `fabric`     — Node, Edge, Fabric, NodeId (BLAKE3-fingerprinted)
//!  - `framebuffer`— substrate-agnostic FbInfo + colour text writer
//!  - `tesseract`  — two-pane fabric/log renderer
//!  - `intent`     — operator intents → command handlers → fabric responses
//!  - `heap`       — static-region allocator initializer
//!  - `BootHandoff`— the data each arch entry point must collect from
//!                    its firmware before calling into the fabric

#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod fabric;
pub mod framebuffer;
pub mod generative_model;
pub mod heap;
pub mod inference;
pub mod intent;
pub mod keyboard;
pub mod model;
pub mod observe;
pub mod ops;
pub mod snapshot;
pub mod storage_agent;
pub mod storage_inventory;
pub mod tesseract;

/// Pre-Step-2 compatibility alias. Pre-nucleation code referenced
/// `kernel_core::agent::DiscreteModel`. The math primitive lives at
/// `kernel_core::model::discrete::DiscreteModel` now. This alias lets
/// the existing storage_agent and external references keep working
/// while the rest of the GenerativeModel comes online. Will be dropped
/// when Step 3 retires storage_agent.rs.
pub mod agent {
    pub use crate::model::discrete::*;
}

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;

/// Free-running tick counter. The substrate's IRQ handler (timer on
/// x86, generic timer on aarch64) increments this. Read by `intent::
/// command_uptime` and shown in the Tesseract.
pub static UPTIME_TICKS: AtomicU64 = AtomicU64::new(0);

/// What an entry point must hand to the fabric. Each architecture
/// collects this from its firmware before calling [`run_genesis`].
pub struct BootHandoff {
    pub substrate: &'static str,
    pub fb_buffer: Option<&'static mut [u8]>,
    pub fb_info: Option<framebuffer::FbInfo>,
    pub memory_regions: Vec<MemoryRegion>,
    pub rsdp: Option<u64>,
    pub physical_memory_offset: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub enum MemoryKind {
    Usable,
    Reserved,
    Bootloader,
    Acpi,
    Mmio,
    Other,
}

impl MemoryKind {
    pub fn label(self) -> &'static str {
        match self {
            MemoryKind::Usable => "usable",
            MemoryKind::Reserved => "reserved",
            MemoryKind::Bootloader => "bootloader",
            MemoryKind::Acpi => "acpi",
            MemoryKind::Mmio => "mmio",
            MemoryKind::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub kind: MemoryKind,
}

/// CPU descriptor returned by per-arch CPU observers.
#[derive(Clone, Debug)]
pub struct CpuObservation {
    pub vendor: String,
    pub brand: String,
    pub features: Vec<String>,
}

/// PCI(e) device descriptor returned by per-arch enumerators.
#[derive(Clone, Copy, Debug)]
pub struct PciObservation {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

// Convenience re-exports.
pub use fabric::{Edge, EdgeKind, Fabric, Node, NodeId, NodeKind, FABRIC};
pub use framebuffer::{FbInfo, FrameBufferWriter, PixelFormat};
pub use tesseract::{LogKind, TESSERACT, Tesseract, render};

/// Re-export of blake3's hash function so binaries don't need a direct
/// dep on the blake3 crate.
pub fn blake3_hash(bytes: &[u8]) -> blake3::Hash {
    blake3::hash(bytes)
}
