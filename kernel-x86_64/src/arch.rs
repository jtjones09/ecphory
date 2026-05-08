//! x86_64-specific hardware probes.
//!
//! Substrate boundary: this module knows about CPUID and port I/O.
//! It returns kernel-core's substrate-agnostic observation types so
//! `kernel_core::observe::genesis` can build the fabric without
//! knowing it's on x86.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use kernel_core::{CpuObservation, PciObservation};

pub fn observe_cpu() -> CpuObservation {
    let cpuid = raw_cpuid::CpuId::new();
    let vendor = cpuid
        .get_vendor_info()
        .map(|v| v.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let brand = cpuid
        .get_processor_brand_string()
        .map(|b| b.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let mut features: Vec<String> = Vec::new();
    if let Some(info) = cpuid.get_feature_info() {
        macro_rules! feat {
            ($flag:ident, $name:literal) => {
                if info.$flag() {
                    features.push($name.to_string());
                }
            };
        }
        feat!(has_fpu, "fpu");
        feat!(has_tsc, "tsc");
        feat!(has_msr, "msr");
        feat!(has_pae, "pae");
        feat!(has_apic, "apic");
        feat!(has_mtrr, "mtrr");
        feat!(has_mmx, "mmx");
        feat!(has_sse, "sse");
        feat!(has_sse2, "sse2");
        feat!(has_sse3, "sse3");
        feat!(has_ssse3, "ssse3");
        feat!(has_sse41, "sse4.1");
        feat!(has_sse42, "sse4.2");
        feat!(has_avx, "avx");
        feat!(has_aesni, "aes-ni");
        feat!(has_rdrand, "rdrand");
        feat!(has_x2apic, "x2apic");
        feat!(has_pcid, "pcid");
        feat!(has_xsave, "xsave");
        feat!(has_hypervisor, "hypervisor");
    }
    if let Some(ext) = cpuid.get_extended_feature_info() {
        if ext.has_avx2() {
            features.push("avx2".to_string());
        }
        if ext.has_bmi1() {
            features.push("bmi1".to_string());
        }
        if ext.has_bmi2() {
            features.push("bmi2".to_string());
        }
        if ext.has_rdseed() {
            features.push("rdseed".to_string());
        }
    }

    CpuObservation {
        vendor,
        brand,
        features,
    }
}

mod pci {
    use x86_64::instructions::port::Port;

    const CONFIG_ADDRESS: u16 = 0xCF8;
    const CONFIG_DATA: u16 = 0xCFC;

    fn enable_bit(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        0x8000_0000
            | ((bus as u32) << 16)
            | ((device as u32) << 11)
            | ((function as u32) << 8)
            | ((offset as u32) & 0xFC)
    }
    pub fn read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        let addr = enable_bit(bus, device, function, offset);
        unsafe {
            let mut a: Port<u32> = Port::new(CONFIG_ADDRESS);
            let mut d: Port<u32> = Port::new(CONFIG_DATA);
            a.write(addr);
            d.read()
        }
    }
    pub fn read_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
        let v = read_u32(bus, device, function, offset & 0xFC);
        ((v >> ((offset & 0x2) * 8)) & 0xFFFF) as u16
    }
    pub fn read_u8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
        let v = read_u32(bus, device, function, offset & 0xFC);
        ((v >> ((offset & 0x3) * 8)) & 0xFF) as u8
    }
}

pub fn observe_pci() -> Vec<PciObservation> {
    let mut nodes = Vec::new();
    for bus in 0u16..256 {
        for device in 0u8..32 {
            for function in 0u8..8 {
                let bus = bus as u8;
                let vendor_id = pci::read_u16(bus, device, function, 0x00);
                if vendor_id == 0xFFFF {
                    if function == 0 {
                        break;
                    }
                    continue;
                }
                let device_id = pci::read_u16(bus, device, function, 0x02);
                let prog_if = pci::read_u8(bus, device, function, 0x09);
                let subclass = pci::read_u8(bus, device, function, 0x0A);
                let class = pci::read_u8(bus, device, function, 0x0B);
                nodes.push(PciObservation {
                    bus,
                    device,
                    function,
                    vendor_id,
                    device_id,
                    class,
                    subclass,
                    prog_if,
                });
                if function == 0 {
                    let header_type = pci::read_u8(bus, device, 0, 0x0E);
                    if header_type & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
    }
    nodes
}
