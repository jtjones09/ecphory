//! Ecphory OS — x86_64 UEFI entry point.
//!
//! Phase 2.5: x86 joined the UEFI club. Same boot path as aarch64,
//! same shim helpers via `kernel-uefi-common`. The fabric runs on
//! x86_64 silicon with CPUID + port-I/O PCI for arch-specific
//! observation, and UEFI services for everything else (framebuffer
//! Blt, BlockIO storage, Simple Text Input keyboard, ACPI tables,
//! memory map). No more bootloader crate, no ATA PIO driver, no
//! 8259 PIC, no PS/2 scancode translator.

#![no_std]
#![no_main]
#![allow(dead_code)]

extern crate alloc;

mod arch;

use alloc::format;
use alloc::string::{String, ToString};

use uefi::Status;
use uefi::prelude::*;

use kernel_core::FABRIC;
use kernel_core::inference::LoopConfig;
use kernel_core::observe::{GenesisInput, genesis};
use kernel_core::ops::{Op, OpResult, Shim};
use kernel_core::tesseract::TESSERACT;

#[entry]
fn efi_main() -> Status {
    let sub = kernel_uefi_common::init();

    {
        let mut t = TESSERACT.lock();
        t.log_system("x86_64: kernel-x86_64 entry");
        if let Some(info) = sub.fb_info {
            t.log_system(format!(
                "gop: {}x{} format={:?}",
                info.width, info.height, info.pixel_format
            ));
        }
        if sub.storage_present {
            t.log_system(format!(
                "storage: {} ({} blocks x {} B)",
                sub.storage_label, sub.storage_blocks, sub.storage_block_size
            ));
        } else {
            t.log_system("storage: none discovered (persistence disabled)");
        }
        t.dirty = true;
    }

    // Try to restore prior fabric snapshot via the shim.
    let mut shim = X86Shim;
    let mut restored = false;
    if sub.storage_present {
        match kernel_core::snapshot::restore(&mut shim) {
            Ok(snap) => {
                let lamport = snap.lamport;
                let nodes = snap.nodes.len();
                let edges = snap.edges.len();
                let mut f = FABRIC.lock();
                kernel_core::snapshot::apply(&mut f, snap);
                drop(f);
                let mut t = TESSERACT.lock();
                t.log_system(format!(
                    "restored {} nodes / {} edges from disk (lamport {})",
                    nodes, edges, lamport
                ));
                restored = true;
            }
            Err(e) => {
                let mut t = TESSERACT.lock();
                t.log_system(format!("no prior snapshot ({}); fresh genesis", e));
            }
        }
    }

    if !restored {
        let cpu_obs = arch::observe_cpu();
        let pci_obs = arch::observe_pci();
        let memory = kernel_uefi_common::observe_memory();
        let rsdp = kernel_uefi_common::find_rsdp();

        {
            let mut t = TESSERACT.lock();
            t.log_system(format!(
                "observed: cpu={} mem-regions={} pci={} rsdp={}",
                cpu_obs.vendor,
                memory.len(),
                pci_obs.len(),
                rsdp.map(|_| "yes").unwrap_or("none"),
            ));
        }

        {
            let mut f = FABRIC.lock();
            let _inv = genesis(
                &mut f,
                GenesisInput {
                    cpu: cpu_obs,
                    pci: pci_obs,
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
        let f = FABRIC.lock();
        match kernel_core::snapshot::persist(&f, &mut shim) {
            Ok(bytes) => {
                drop(f);
                TESSERACT
                    .lock()
                    .log_system(format!("persisted {} bytes to disk", bytes));
            }
            Err(e) => {
                drop(f);
                TESSERACT
                    .lock()
                    .log_warning(format!("persist failed: {}", e));
            }
        }
    }

    {
        let mut t = TESSERACT.lock();
        t.log_system("type help to list intents");
    }

    let cfg = LoopConfig {
        agent_label: String::from("vblk0"),
        agent_enabled: sub.storage_present,
        persist_enabled: sub.storage_present,
    };
    kernel_core::inference::run(&mut shim, &kernel_uefi_common::FB, cfg)
}

struct X86Shim;

impl Shim for X86Shim {
    fn substrate_label(&self) -> &'static str {
        "x86_64"
    }

    fn execute<'a>(&mut self, op: Op<'a>) -> OpResult {
        match op {
            Op::ObserveCpu => OpResult::Cpu(arch::observe_cpu()),
            Op::ObserveDevices => OpResult::Devices(arch::observe_pci()),
            Op::ObserveMemory => OpResult::Memory(kernel_uefi_common::observe_memory()),
            Op::ReadBlock { lba, into } => kernel_uefi_common::handle_read_block(lba, into),
            Op::WriteBlock { lba, from } => kernel_uefi_common::handle_write_block(lba, from),
            Op::FlushStorage => kernel_uefi_common::handle_flush_storage(),
            Op::PollInput => kernel_uefi_common::handle_poll_input(),
            Op::GetTime => kernel_uefi_common::handle_get_time(),
            Op::Hash(b) => kernel_uefi_common::handle_hash(b),
            Op::Halt => {
                unsafe { core::arch::asm!("hlt", options(nostack, preserves_flags)) }
                OpResult::Done
            }
        }
    }

    fn present_frame(&mut self) {
        kernel_uefi_common::present_frame()
    }
}
