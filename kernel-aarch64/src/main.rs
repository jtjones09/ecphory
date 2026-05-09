//! Ecphory OS — aarch64 UEFI entry point.
//!
//! Mirror of kernel-x86_64. Both binaries delegate UEFI service calls
//! to `kernel-uefi-common`; the only arch-specific code here is CPU
//! observation via MIDR_EL1 / ID_AA64* system registers (and a
//! placeholder for PCI enumeration which on aarch64 needs ECAM walking
//! — Phase 2.5 leaves that empty since UEFI BlockIO already gave us
//! the storage controller).

#![no_std]
#![no_main]
#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use uefi::Status;
use uefi::prelude::*;

use kernel_core::FABRIC;
use kernel_core::generative_model::{submit_event, LogCandidate};
use kernel_core::inference::LoopConfig;
use kernel_core::observe::{GenesisInput, genesis};
use kernel_core::ops::{Op, OpResult, Shim};
use kernel_core::tesseract::TESSERACT;
use kernel_core::{CpuObservation, PciObservation};

#[entry]
fn efi_main() -> Status {
    let sub = kernel_uefi_common::init();

    submit_event(LogCandidate::Boot("aarch64: kernel-aarch64 entry".into()));
    if let Some(info) = sub.fb_info {
        submit_event(LogCandidate::DeviceDiscovery(format!(
            "gop: {}x{} format={:?}",
            info.width, info.height, info.pixel_format
        )));
    }
    if sub.storage_present {
        submit_event(LogCandidate::DeviceDiscovery(format!(
            "storage: {} ({} blocks x {} B)",
            sub.storage_label, sub.storage_blocks, sub.storage_block_size
        )));
    } else {
        submit_event(LogCandidate::DeviceDiscovery(
            "storage: none discovered (persistence disabled)".into(),
        ));
    }
    TESSERACT.lock().dirty = true;

    let mut shim = ArmShim;
    let mut restored = false;
    if sub.storage_present {
        match kernel_core::snapshot::restore(&mut shim) {
            Ok(snap) => {
                let lamport = snap.lamport;
                let nodes = snap.nodes.len();
                let edges = snap.edges.len();
                {
                    let mut f = FABRIC.lock();
                    kernel_core::snapshot::apply(&mut f, snap);
                }
                submit_event(LogCandidate::RestoreOk {
                    nodes,
                    edges,
                    lamport,
                });
                restored = true;
            }
            Err(e) => {
                submit_event(LogCandidate::NoSnapshot(format!(
                    "no prior snapshot ({}); fresh genesis",
                    e
                )));
            }
        }
    }

    if !restored {
        let cpu = observe_cpu_aarch64();
        let pci: Vec<PciObservation> = Vec::new();
        let memory = kernel_uefi_common::observe_memory();
        let rsdp = kernel_uefi_common::find_rsdp();

        submit_event(LogCandidate::Genesis(format!(
            "observed: cpu={} mem-regions={} rsdp={}",
            cpu.vendor,
            memory.len(),
            rsdp.map(|_| "yes").unwrap_or("none"),
        )));

        {
            let mut f = FABRIC.lock();
            let _inv = genesis(
                &mut f,
                GenesisInput {
                    cpu,
                    pci,
                    memory: &memory,
                    rsdp,
                    physical_memory_offset: Some(0),
                    framebuffer_info: sub.fb_info,
                },
            );
            if sub.storage_present {
                f.create(kernel_core::NodeKind::HwStorage {
                    kind: sub.storage_label.clone(),
                    sectors: sub.storage_blocks,
                    sector_size: sub.storage_block_size as u32,
                });
            }
        }
    }

    if sub.storage_present {
        let result = {
            let f = FABRIC.lock();
            kernel_core::snapshot::persist(&f, &mut shim)
        };
        submit_event(LogCandidate::PersistOutcome {
            ok: result.is_ok(),
            bytes: result.as_ref().copied().unwrap_or(0),
            error: result.as_ref().err().map(|e| format!("{}", e)),
        });
    }

    submit_event(LogCandidate::DeviceDiscovery(
        "type help to list intents".into(),
    ));

    let cfg = LoopConfig {
        agent_label: String::from("vblk0"),
        agent_enabled: sub.storage_present,
        persist_enabled: sub.storage_present,
    };
    kernel_core::inference::run(&mut shim, &kernel_uefi_common::FB, cfg)
}

struct ArmShim;

impl Shim for ArmShim {
    fn substrate_label(&self) -> &'static str {
        "aarch64"
    }

    fn execute<'a>(&mut self, op: Op<'a>) -> OpResult {
        match op {
            Op::ObserveCpu => OpResult::Cpu(observe_cpu_aarch64()),
            Op::ObserveMemory => OpResult::Memory(kernel_uefi_common::observe_memory()),
            Op::ObserveDevices => OpResult::Devices(Vec::new()),
            Op::ReadBlock { lba, into } => kernel_uefi_common::handle_read_block(lba, into),
            Op::WriteBlock { lba, from } => kernel_uefi_common::handle_write_block(lba, from),
            Op::FlushStorage => kernel_uefi_common::handle_flush_storage(),
            Op::PollInput => kernel_uefi_common::handle_poll_input(),
            Op::GetTime => kernel_uefi_common::handle_get_time(),
            Op::Hash(b) => kernel_uefi_common::handle_hash(b),
            Op::Halt => {
                unsafe { core::arch::asm!("wfi", options(nostack, preserves_flags)) }
                OpResult::Done
            }
        }
    }

    fn present_frame(&mut self) {
        kernel_uefi_common::present_frame()
    }
}

fn observe_cpu_aarch64() -> CpuObservation {
    let midr: u64;
    unsafe { core::arch::asm!("mrs {}, MIDR_EL1", out(reg) midr, options(nostack, preserves_flags)) };

    let implementer = ((midr >> 24) & 0xFF) as u8;
    let variant = ((midr >> 20) & 0xF) as u8;
    let architecture = ((midr >> 16) & 0xF) as u8;
    let part_num = ((midr >> 4) & 0xFFF) as u16;
    let revision = (midr & 0xF) as u8;

    let vendor = match implementer {
        0x41 => "ARM Limited".to_string(),
        0x42 => "Broadcom".to_string(),
        0x43 => "Cavium".to_string(),
        0x44 => "DEC".to_string(),
        0x46 => "Fujitsu".to_string(),
        0x48 => "HiSilicon".to_string(),
        0x49 => "Infineon".to_string(),
        0x4D => "Motorola/Freescale".to_string(),
        0x4E => "NVIDIA".to_string(),
        0x50 => "Applied Micro".to_string(),
        0x51 => "Qualcomm".to_string(),
        0x56 => "Marvell".to_string(),
        0x61 => "Apple".to_string(),
        0x69 => "Intel".to_string(),
        0xC0 => "Ampere".to_string(),
        _ => format!("vendor 0x{:02X}", implementer),
    };
    let brand = format!(
        "aarch64 part 0x{:03X} variant {} arch {} rev {}",
        part_num, variant, architecture, revision
    );

    let pfr0: u64;
    let isar0: u64;
    let isar1: u64;
    unsafe {
        core::arch::asm!("mrs {}, ID_AA64PFR0_EL1", out(reg) pfr0, options(nostack, preserves_flags));
        core::arch::asm!("mrs {}, ID_AA64ISAR0_EL1", out(reg) isar0, options(nostack, preserves_flags));
        core::arch::asm!("mrs {}, ID_AA64ISAR1_EL1", out(reg) isar1, options(nostack, preserves_flags));
    }

    let mut features: Vec<String> = Vec::new();
    if (pfr0 & 0xF) == 1 || (pfr0 & 0xF) == 2 { features.push("el0-aarch64".to_string()); }
    if ((pfr0 >> 16) & 0xF) >= 1 { features.push("fp".to_string()); }
    if ((pfr0 >> 20) & 0xF) >= 1 { features.push("advsimd".to_string()); }
    if ((pfr0 >> 32) & 0xF) >= 1 { features.push("sve".to_string()); }
    if ((isar0 >> 4) & 0xF) >= 1 { features.push("aes".to_string()); }
    if ((isar0 >> 8) & 0xF) >= 1 { features.push("sha1".to_string()); }
    if ((isar0 >> 12) & 0xF) >= 1 { features.push("sha2".to_string()); }
    if ((isar0 >> 16) & 0xF) >= 1 { features.push("crc32".to_string()); }
    if ((isar0 >> 20) & 0xF) >= 2 { features.push("atomics".to_string()); }
    if ((isar0 >> 28) & 0xF) >= 1 { features.push("rdm".to_string()); }
    if ((isar1 >> 0) & 0xF) >= 1 { features.push("dpb".to_string()); }
    if ((isar1 >> 4) & 0xF) >= 1 { features.push("apa".to_string()); }
    if ((isar1 >> 12) & 0xF) >= 1 { features.push("jscvt".to_string()); }
    if ((isar1 >> 20) & 0xF) >= 1 { features.push("lrcpc".to_string()); }
    features.push("uefi".to_string());

    CpuObservation { vendor, brand, features }
}
