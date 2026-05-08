//! The substrate boundary.
//!
//! kernel-core knows nothing about CPUID, port-I/O, MIDR_EL1, ATA PIO,
//! 8259 PICs, GICs, PS/2, or USB-HID. It runs over a [`Shim`] trait
//! that translates substrate-agnostic [`Op`]s into substrate-specific
//! hardware actions.
//!
//! This is the eForth/Smalltalk move: the fabric is interpreted by a
//! thin per-arch translator. Add a new substrate (RISC-V, neuromorphic,
//! whatever) by writing a new Shim. The fabric snapshot — the world
//! model, learned matrices, immune baselines — boots unchanged on any
//! of them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::framebuffer::FbInfo;
use crate::{CpuObservation, MemoryRegion, PciObservation};

/// Block size for [`Op::ReadBlock`] / [`Op::WriteBlock`]. We standardise
/// on 512 bytes — every substrate of interest can give us this granule.
pub const BLOCK_SIZE: usize = 512;

/// One scancode/keycode/HID byte from the input source. The exact
/// encoding is substrate-specific (PS/2 set-1 on x86, HID usage on
/// USB), but the fabric just sees a stream of `u8`s and decodes them
/// in [`crate::keyboard`] (substrate-agnostic).
pub type InputByte = u8;

/// What the fabric asks the substrate to do.
///
/// Phase 2 keeps the surface small (~10 ops). Each op is the kind of
/// thing the substrate can do but the fabric *knowledge of how* lives
/// in the prior — class-level protocol awareness, not vendor-specific
/// driver code.
#[allow(dead_code)]
#[derive(Debug)]
pub enum Op<'a> {
    /// Substrate identifies its CPU. Implementor reads CPUID / MIDR_EL1
    /// / equivalent and packs the result.
    ObserveCpu,

    /// Substrate enumerates its memory map.
    ObserveMemory,

    /// Substrate enumerates its bus-attached devices (PCI / PCIe / DT).
    ObserveDevices,

    /// Read one 512-byte block from the storage controller.
    ReadBlock { lba: u32, into: &'a mut [u8] },

    /// Write one 512-byte block to the storage controller.
    WriteBlock { lba: u32, from: &'a [u8] },

    /// Flush the storage controller's write cache. Required for
    /// durability across power cycles or VM stops — UEFI BlockIO
    /// writes can be cached and the firmware doesn't always flush on
    /// shutdown (AAVMF in QEMU does; Parallels Desktop on M2 doesn't).
    /// Caller pairs this with the atomic-commit pattern in
    /// `kernel_core::snapshot::persist`.
    FlushStorage,

    /// Pop one input byte from the substrate's input buffer (non-
    /// blocking). `None` if no input is queued.
    PollInput,

    /// Monotonic tick count from the substrate's free-running timer.
    /// 18.2 Hz on x86 PIT, ~62.5 MHz on aarch64 generic timer — we
    /// don't claim a specific unit, only monotonicity.
    GetTime,

    /// BLAKE3 hash of the given bytes. Substrate-specific only because
    /// we may want to use hardware accelerators (AES-NI, ARMv8 crypto)
    /// where available.
    Hash(&'a [u8]),

    /// Park the CPU until the next interrupt. `wfi` on aarch64,
    /// `hlt` on x86, equivalent elsewhere.
    Halt,
}

/// Output of an op. Errors are encoded as `Err` variants — the fabric
/// observes them and feeds them to the active-inference agents.
#[derive(Debug)]
pub enum OpResult {
    Cpu(CpuObservation),
    Memory(Vec<MemoryRegion>),
    Devices(Vec<PciObservation>),
    Block(BlockResult),
    Input(Option<InputByte>),
    Time(u64),
    Hash([u8; 32]),
    Done,
    Unsupported,
}

#[derive(Debug, Clone)]
pub enum BlockResult {
    Ok,
    NoDevice,
    Timeout,
    DeviceError(u8),
}

/// What the substrate gives the kernel-core at boot. Same handoff for
/// both arches; the per-arch code converts whatever its firmware /
/// bootloader delivered into this canonical form.
pub struct BootHandoff {
    pub substrate: &'static str,
    pub fb_info: Option<FbInfo>,
    pub cpu: CpuObservation,
    pub pci: Vec<PciObservation>,
    pub memory: Vec<MemoryRegion>,
    pub rsdp: Option<u64>,
    pub physical_memory_offset: Option<u64>,
    pub storage_label: Option<String>,
    pub storage_sectors: Option<u64>,
}

/// The substrate's translation table.
///
/// Implementations live in the per-arch binary crates. Their job is
/// to translate each `Op` into the substrate's native instructions.
/// Everything above this layer is portable.
pub trait Shim {
    fn substrate_label(&self) -> &'static str;

    fn execute<'a>(&mut self, op: Op<'a>) -> OpResult;

    /// Optional: present a frame to the display. Some substrates write
    /// directly to a memory-mapped buffer (x86); others need to push
    /// via UEFI Blt (aarch64 in Blt-only mode). The fabric calls this
    /// after rendering into a logical buffer.
    fn present_frame(&mut self) {}
}
