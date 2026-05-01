// REGION SENSITIVITY — Selective signing policy (Spec 5 §3.3)
//
// Regions are marked `sensitivity: high` to enforce per-node signing,
// or `sensitivity: normal` for the default content-fingerprint-only model.
//
// Per spec §3.3: "Regions are marked `sensitivity: high` via a fabric node
// (editable through Spec 8's semantic edit protocol). Default: `propmgmt` is
// `high`; `nisaba` is `normal`; immune system observations are `high`".
//
// Sensitivity changes are monitored by the immune system (Spec 6
// `ConsensusObserver`) — see spec §3.3 v2.1 fold note.

/// Sensitivity level of a fabric region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionSensitivity {
    /// Default: content fingerprint only, no per-write signing.
    /// Trust comes from the immune system's behavioral observation.
    Normal,
    /// High-sensitivity: per-node Ed25519 signature required.
    /// Verified on first read by an agent outside the creator's session.
    /// Cost: ~50µs per signed creation, ~100µs per first-read verification.
    High,
}

impl RegionSensitivity {
    /// Does this region require a per-node signature?
    pub fn requires_signature(&self) -> bool {
        matches!(self, RegionSensitivity::High)
    }
}

impl Default for RegionSensitivity {
    fn default() -> Self {
        RegionSensitivity::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_requires_signature() {
        assert!(RegionSensitivity::High.requires_signature());
    }

    #[test]
    fn normal_does_not_require_signature() {
        assert!(!RegionSensitivity::Normal.requires_signature());
    }

    #[test]
    fn default_is_normal() {
        assert_eq!(RegionSensitivity::default(), RegionSensitivity::Normal);
    }
}
