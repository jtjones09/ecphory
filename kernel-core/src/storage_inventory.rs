//! Substrate-agnostic record of every block device the substrate
//! observed at boot. Populated by per-arch shims (today: kernel-
//! uefi-common::init). Read by the `disks` command and by future
//! tooling that wants to inspect what was visible without writing
//! anything.
//!
//! Per the hardware-interaction position paper, the controller's
//! register interface IS the Markov blanket between fabric and
//! hardware. This module is the substrate-agnostic surface where the
//! list of observable controllers (the BlockIO instances UEFI
//! exposes) shows up before the kernel decides which one to write to.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct BlockDeviceInfo {
    pub index: usize,
    pub label: String,
    pub blocks: u64,
    pub block_size: u64,
    pub is_logical_partition: bool,
    pub is_boot_device: bool,
    pub is_boot_ancestor: bool,
    pub picked_for_storage: bool,
}

impl BlockDeviceInfo {
    pub fn total_bytes(&self) -> u64 {
        self.blocks.saturating_mul(self.block_size)
    }

    pub fn render_summary(&self) -> String {
        let bytes = self.total_bytes();
        let mib = bytes / (1024 * 1024);
        let mut tags: Vec<&str> = Vec::new();
        if self.picked_for_storage {
            tags.push("PICKED");
        }
        if self.is_boot_device {
            tags.push("BOOT");
        }
        if self.is_boot_ancestor {
            tags.push("boot-ancestor");
        }
        if self.is_logical_partition {
            tags.push("partition");
        }
        let tag_str = if tags.is_empty() {
            String::new()
        } else {
            alloc::format!(" [{}]", tags.join(","))
        };
        // ASCII-only — the framebuffer's bitmap font doesn't render
        // unicode dashes/arrows reliably, and Nexus screenshots will
        // be read by humans who shouldn't have to guess what the
        // garbled glyph is.
        alloc::format!(
            "[{}] {} - {} blocks x {} B = {} MiB{}",
            self.index, self.label, self.blocks, self.block_size, mib, tag_str
        )
    }
}

pub static INVENTORY: spin::Mutex<Vec<BlockDeviceInfo>> = spin::Mutex::new(Vec::new());

pub fn record(devices: Vec<BlockDeviceInfo>) {
    let mut slot = INVENTORY.lock();
    *slot = devices;
}

pub fn snapshot() -> Vec<BlockDeviceInfo> {
    INVENTORY.lock().clone()
}
