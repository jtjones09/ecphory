//! Substrate-agnostic genesis. The arch entry point hands us its CPU
//! and PCI observations (gathered via CPUID/MIDR_EL1 and port-I/O/ECAM
//! respectively); everything else (memory map → nodes, framebuffer →
//! node, ACPI tables → nodes) is portable and lives here.

use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::fabric::{EdgeKind, Fabric, NodeId, NodeKind};
use crate::framebuffer::FbInfo;
use crate::{CpuObservation, MemoryRegion, PciObservation};

pub struct Inventory {
    pub genesis: NodeId,
    pub cpu: NodeId,
    pub features: Vec<NodeId>,
    pub memory_regions: Vec<NodeId>,
    pub pci_devices: Vec<NodeId>,
    pub acpi_tables: Vec<NodeId>,
    pub framebuffer: Option<NodeId>,
}

pub struct GenesisInput<'a> {
    pub cpu: CpuObservation,
    pub pci: Vec<PciObservation>,
    pub memory: &'a [MemoryRegion],
    pub rsdp: Option<u64>,
    pub physical_memory_offset: Option<u64>,
    pub framebuffer_info: Option<FbInfo>,
}

pub fn genesis(fabric: &mut Fabric, input: GenesisInput<'_>) -> Inventory {
    let cpu = observe_cpu(fabric, &input.cpu);
    let features = observe_cpu_features(fabric, &input.cpu);
    let memory_regions = observe_memory(fabric, input.memory);
    let framebuffer = input
        .framebuffer_info
        .map(|info| observe_framebuffer(fabric, info));
    let pci_devices = observe_pci(fabric, &input.pci);
    let acpi_tables = observe_acpi(fabric, input.physical_memory_offset, input.rsdp);

    let observed = (1
        + features.len()
        + memory_regions.len()
        + pci_devices.len()
        + acpi_tables.len()
        + framebuffer.map(|_| 1).unwrap_or(0)) as u32;

    let genesis = fabric.create(NodeKind::Genesis {
        fabric_lamport: fabric.lamport,
        observed,
    });

    fabric.link(genesis, cpu, EdgeKind::Contains);
    for f in &features {
        fabric.link(cpu, *f, EdgeKind::Contains);
    }
    for r in &memory_regions {
        fabric.link(genesis, *r, EdgeKind::Contains);
    }
    for p in &pci_devices {
        fabric.link(genesis, *p, EdgeKind::Contains);
    }
    for a in &acpi_tables {
        fabric.link(genesis, *a, EdgeKind::Describes);
    }
    if let Some(fb) = framebuffer {
        fabric.link(genesis, fb, EdgeKind::Contains);
    }

    Inventory {
        genesis,
        cpu,
        features,
        memory_regions,
        pci_devices,
        acpi_tables,
        framebuffer,
    }
}

fn observe_cpu(fabric: &mut Fabric, obs: &CpuObservation) -> NodeId {
    fabric.create(NodeKind::HwCpu {
        vendor: obs.vendor.clone(),
        brand: obs.brand.clone(),
    })
}

fn observe_cpu_features(fabric: &mut Fabric, obs: &CpuObservation) -> Vec<NodeId> {
    obs.features
        .iter()
        .map(|name| fabric.create(NodeKind::HwCpuFeature(name.clone())))
        .collect()
}

fn observe_memory(fabric: &mut Fabric, regions: &[MemoryRegion]) -> Vec<NodeId> {
    let mut nodes = Vec::new();
    for r in regions {
        if r.end - r.start < 4096 {
            continue;
        }
        nodes.push(fabric.create(NodeKind::HwMemoryRegion {
            start: r.start,
            end: r.end,
            kind: r.kind.label().to_string(),
        }));
    }
    nodes
}

fn observe_framebuffer(fabric: &mut Fabric, info: FbInfo) -> NodeId {
    let format = format!("{:?}", info.pixel_format);
    fabric.create(NodeKind::HwFramebuffer {
        width: info.width as u32,
        height: info.height as u32,
        bytes_per_pixel: info.bytes_per_pixel as u8,
        format,
    })
}

fn observe_pci(fabric: &mut Fabric, devices: &[PciObservation]) -> Vec<NodeId> {
    devices
        .iter()
        .map(|d| {
            fabric.create(NodeKind::HwPciDevice {
                bus: d.bus,
                device: d.device,
                function: d.function,
                vendor_id: d.vendor_id,
                device_id: d.device_id,
                class: d.class,
                subclass: d.subclass,
                prog_if: d.prog_if,
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
struct AcpiHandlerImpl {
    physical_memory_offset: u64,
}

impl acpi::AcpiHandler for AcpiHandlerImpl {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> acpi::PhysicalMapping<Self, T> {
        let virt = self.physical_memory_offset as usize + physical_address;
        let ptr = core::ptr::NonNull::new(virt as *mut T).unwrap();
        unsafe { acpi::PhysicalMapping::new(physical_address, ptr, size, size, *self) }
    }
    fn unmap_physical_region<T>(_region: &acpi::PhysicalMapping<Self, T>) {}
}

fn observe_acpi(
    fabric: &mut Fabric,
    physical_memory_offset: Option<u64>,
    rsdp: Option<u64>,
) -> Vec<NodeId> {
    let mut nodes = Vec::new();
    let physical_memory_offset = match physical_memory_offset {
        Some(o) => o,
        None => return nodes,
    };
    let rsdp = match rsdp {
        Some(a) => a as usize,
        None => return nodes,
    };
    let handler = AcpiHandlerImpl {
        physical_memory_offset,
    };
    let tables = match unsafe { acpi::AcpiTables::from_rsdp(handler, rsdp) } {
        Ok(t) => t,
        Err(_) => return nodes,
    };

    for header in tables.headers() {
        let signature_str = header.signature.as_str();
        let mut sig = [0u8; 4];
        for (i, b) in signature_str.as_bytes().iter().take(4).enumerate() {
            sig[i] = *b;
        }
        nodes.push(fabric.create(NodeKind::HwAcpiTable {
            signature: sig,
            address: 0,
            length: header.length,
        }));
    }
    if let Ok(dsdt) = tables.dsdt() {
        nodes.push(fabric.create(NodeKind::HwAcpiTable {
            signature: *b"DSDT",
            address: dsdt.address as u64,
            length: dsdt.length,
        }));
    }
    for ssdt in tables.ssdts() {
        nodes.push(fabric.create(NodeKind::HwAcpiTable {
            signature: *b"SSDT",
            address: ssdt.address as u64,
            length: ssdt.length,
        }));
    }
    nodes
}
