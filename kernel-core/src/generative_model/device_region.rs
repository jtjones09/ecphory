//! Device region — beliefs about hardware controllers, expressed as one
//! `DiscreteModel` per device. The storage controller is the only
//! populated device today; future device classes (network, USB,
//! sensors) plug in as additional entries here.
//!
//! For now the storage device wraps the existing `StorageAgent`
//! end-to-end via composition rather than reimplementing it. That keeps
//! the lifecycle path that's been validated on Mac (M2 Max via
//! Parallels) intact while the rest of the GenerativeModel comes
//! online. Step 3 of the nucleation plan rewires the inference loop to
//! drive this region instead of the bare `StorageAgent`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::storage_agent::StorageAgent;

/// One device's belief state. The label identifies the underlying
/// hardware ("vblk0", "ata0-slave", etc.); `kind` is the device class
/// ("storage", "network", ...). Future device classes carry their own
/// dimension parameters.
pub struct DeviceModel {
    pub label: String,
    pub kind: String,
    pub agent: StorageAgent,
}

impl DeviceModel {
    pub fn storage(label: String) -> Self {
        Self {
            kind: "storage".to_string(),
            agent: StorageAgent::new(label.clone()),
            label,
        }
    }

    pub fn average_surprise(&self) -> f32 {
        self.agent.model.average_surprise()
    }

    pub fn observations_seen(&self) -> u64 {
        self.agent.model.observations_seen
    }

    pub fn map_state_label(&self) -> &'static str {
        self.agent.map_state_label()
    }

    pub fn last_action_label(&self) -> &'static str {
        self.agent.last_action_label()
    }

    pub fn render_summary(&self) -> String {
        self.agent.render_summary()
    }

    pub fn snapshot_bytes(&self) -> Vec<u8> {
        self.agent.snapshot_bytes()
    }

    pub fn restore_from_bytes(&mut self, bytes: &[u8]) -> bool {
        self.agent.restore_from_bytes(bytes)
    }
}

/// Container for all device beliefs. Today this is a single storage
/// device; the API admits extension to N devices without changing
/// callers.
pub struct DeviceRegion {
    pub devices: Vec<DeviceModel>,
}

impl DeviceRegion {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    pub fn add(&mut self, device: DeviceModel) {
        self.devices.push(device);
    }

    pub fn storage_mut(&mut self) -> Option<&mut DeviceModel> {
        self.devices.iter_mut().find(|d| d.kind == "storage")
    }

    pub fn storage(&self) -> Option<&DeviceModel> {
        self.devices.iter().find(|d| d.kind == "storage")
    }

    /// Mean of per-device average surprise — the immune signal at the
    /// region level. A degraded device pulls this number up.
    pub fn aggregate_surprise(&self) -> f32 {
        if self.devices.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.devices.iter().map(|d| d.average_surprise()).sum();
        sum / self.devices.len() as f32
    }

    pub fn total_observations(&self) -> u64 {
        self.devices.iter().map(|d| d.observations_seen()).sum()
    }
}
