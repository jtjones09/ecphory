// IMMUNE-SYSTEM BOOTSTRAP — Provision the v1 minimum cell-agent
// population (Spec 6 §7.2)
//
// Per Spec 6 §7.1, the fabric refuses to start without a registered
// cell-agent population. Bootstrap provisions one of each
// specialization per region: RateObserver, AttestationObserver,
// DecayObserver, ConsensusObserver, RelationObserver, SilenceObserver.
//
// v1 implementation runs as both a library entry point
// (`bootstrap_population`) and a binary (`src/bin/immune-bootstrap.rs`).
// State is written to `~/.ecphory/immune/<region>/<specialization>/`
// per Spec 6 §7.2; v1 v1 ships a minimal manifest containing the
// cell-agent's voice print + specialization metadata. Full encrypted
// keypair persistence is deferred per Spec 5 §3.2.1 (the same pattern
// used for agent keypairs).

use crate::identity::NamespaceId;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::cell_agent::CellAgent;
use super::specialization::{
    AttestationObserver, ConsensusObserver, DecayObserver, RateObserver, RelationObserver,
    SilenceObserver, Specialization,
};

/// All six v1 specializations in canonical order. The bootstrap path
/// + acceptance criterion 5 use this order so manifests are stable.
pub const V1_SPECIALIZATIONS: [Specialization; 6] = [
    Specialization::Rate,
    Specialization::Attestation,
    Specialization::Decay,
    Specialization::Consensus,
    Specialization::Relation,
    Specialization::Silence,
];

/// Per-cell-agent manifest written into
/// `<root>/<region>/<specialization>/manifest.txt`.
#[derive(Debug, Clone)]
pub struct CellAgentManifest {
    pub region_name: String,
    pub specialization: Specialization,
    pub voice_print_hex: String,
    pub cell_agent_id: String,
}

impl CellAgentManifest {
    pub fn render(&self) -> String {
        format!(
            "# Cell-agent manifest (Spec 6 §7.2)\n\
             region: {}\n\
             specialization: {}\n\
             voice_print: {}\n\
             cell_agent_id: {}\n",
            self.region_name,
            self.specialization.as_str(),
            self.voice_print_hex,
            self.cell_agent_id
        )
    }
}

/// Result of provisioning one region's full cell-agent population.
pub struct RegionProvisionReport {
    pub region: NamespaceId,
    pub manifests: Vec<CellAgentManifest>,
    pub state_dir: PathBuf,
}

/// Provision the v1 minimum cell-agent population for a single
/// region. Generates one cell-agent of each specialization, writes
/// the per-agent manifest, returns the manifest list for caller-side
/// registration with `Fabric::register_cell_agent`.
///
/// `state_root` is the directory under which per-region subdirs are
/// created (default: `~/.ecphory/immune/`). Tests pass a tmpdir.
pub fn bootstrap_region(
    region: NamespaceId,
    state_root: &Path,
) -> Result<RegionProvisionReport, std::io::Error> {
    let region_dir = state_root.join(&region.name);
    std::fs::create_dir_all(&region_dir)?;

    let mut manifests = Vec::with_capacity(V1_SPECIALIZATIONS.len());
    for spec in V1_SPECIALIZATIONS {
        let dir = region_dir.join(spec.as_str());
        std::fs::create_dir_all(&dir)?;
        let (voice_hex, agent_id_str) = match spec {
            Specialization::Rate => {
                let a = RateObserver::new(region.clone(), Duration::from_secs(60));
                (a.voice_print().to_hex(), a.id().to_string())
            }
            Specialization::Attestation => {
                let a = AttestationObserver::new(region.clone());
                (a.voice_print().to_hex(), a.id().to_string())
            }
            Specialization::Decay => {
                let a = DecayObserver::new(region.clone());
                (a.voice_print().to_hex(), a.id().to_string())
            }
            Specialization::Consensus => {
                let a = ConsensusObserver::new(region.clone());
                (a.voice_print().to_hex(), a.id().to_string())
            }
            Specialization::Relation => {
                let a = RelationObserver::new(region.clone(), Duration::from_secs(60));
                (a.voice_print().to_hex(), a.id().to_string())
            }
            Specialization::Silence => {
                let a = SilenceObserver::new(region.clone(), Duration::from_secs(60));
                (a.voice_print().to_hex(), a.id().to_string())
            }
        };
        let manifest = CellAgentManifest {
            region_name: region.name.clone(),
            specialization: spec,
            voice_print_hex: voice_hex,
            cell_agent_id: agent_id_str,
        };
        std::fs::write(dir.join("manifest.txt"), manifest.render())?;
        manifests.push(manifest);
    }
    Ok(RegionProvisionReport {
        region,
        manifests,
        state_dir: region_dir,
    })
}

/// Use Spec 6 §7.1's "fabric refuses to start without an immune
/// system" check. Returns `Err` if no manifest is found for any
/// region in `expected_regions`.
pub fn enforce_population(
    expected_regions: &[NamespaceId],
    state_root: &Path,
) -> Result<(), MissingPopulation> {
    let mut missing = Vec::new();
    for region in expected_regions {
        let region_dir = state_root.join(&region.name);
        for spec in V1_SPECIALIZATIONS {
            let manifest = region_dir.join(spec.as_str()).join("manifest.txt");
            if !manifest.exists() {
                missing.push(format!("{}::{}", region.name, spec.as_str()));
            }
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(MissingPopulation { missing })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MissingPopulation {
    pub missing: Vec<String>,
}

impl std::fmt::Display for MissingPopulation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fabric refuses to start: missing immune cell-agents for {}",
            self.missing.join(", ")
        )
    }
}

impl std::error::Error for MissingPopulation {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ecphory-immune-bootstrap-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        p
    }

    #[test]
    fn bootstrap_writes_six_manifests_per_region() {
        let root = tmpdir();
        let region = NamespaceId::fresh("propmgmt");
        let report = bootstrap_region(region.clone(), &root).unwrap();
        assert_eq!(report.manifests.len(), 6);
        for m in &report.manifests {
            let path = root
                .join(&region.name)
                .join(m.specialization.as_str())
                .join("manifest.txt");
            assert!(path.exists(), "missing manifest at {:?}", path);
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(body.contains(m.specialization.as_str()));
            assert!(body.contains(&m.voice_print_hex));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enforce_population_passes_after_bootstrap() {
        let root = tmpdir();
        let r1 = NamespaceId::fresh("r1");
        let r2 = NamespaceId::fresh("r2");
        let _ = bootstrap_region(r1.clone(), &root).unwrap();
        let _ = bootstrap_region(r2.clone(), &root).unwrap();
        assert!(enforce_population(&[r1, r2], &root).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enforce_population_fails_when_specialization_missing() {
        let root = tmpdir();
        let r = NamespaceId::fresh("r");
        // Provision but then remove one specialization's manifest.
        let _ = bootstrap_region(r.clone(), &root).unwrap();
        let removed = root.join(&r.name).join("RateObserver").join("manifest.txt");
        std::fs::remove_file(&removed).unwrap();
        let err = enforce_population(&[r.clone()], &root).unwrap_err();
        assert!(err.missing.iter().any(|m| m.contains("RateObserver")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn voice_prints_are_distinct_across_specializations() {
        let root = tmpdir();
        let region = NamespaceId::fresh("test");
        let report = bootstrap_region(region, &root).unwrap();
        let mut voices = std::collections::HashSet::new();
        for m in &report.manifests {
            assert!(voices.insert(m.voice_print_hex.clone()));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
