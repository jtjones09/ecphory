// IMMUNE-BOOTSTRAP — Provision the v1 cell-agent population for a
// fabric installation (Spec 6 §7.2)
//
// Usage:
//   immune-bootstrap <region1> [<region2> …]
//
// Optional env vars:
//   ECPHORY_IMMUNE_STATE_ROOT — defaults to ~/.ecphory/immune/
//
// Per Spec 6 §7.1, the fabric refuses to start without a registered
// cell-agent population. This binary provisions one of each
// specialization (Rate, Attestation, Decay, Consensus, Relation,
// Silence) per supplied region.

use ecphory::immune::{bootstrap_region, V1_SPECIALIZATIONS};
use ecphory::NamespaceId;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args();
    let _ = args.next(); // bin name
    let regions: Vec<String> = args.collect();
    if regions.is_empty() {
        eprintln!("usage: immune-bootstrap <region1> [<region2> …]");
        eprintln!();
        eprintln!("Provisions one of each v1 specialization per region:");
        for spec in V1_SPECIALIZATIONS {
            eprintln!("  - {}", spec.as_str());
        }
        std::process::exit(2);
    }

    let state_root = match std::env::var("ECPHORY_IMMUNE_STATE_ROOT") {
        Ok(s) => PathBuf::from(s),
        Err(_) => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".ecphory").join("immune")
        }
    };
    if let Err(e) = std::fs::create_dir_all(&state_root) {
        eprintln!("immune-bootstrap: cannot create {}: {}", state_root.display(), e);
        std::process::exit(1);
    }

    let mut total_manifests = 0usize;
    for name in &regions {
        let region = NamespaceId::fresh(name);
        match bootstrap_region(region.clone(), &state_root) {
            Ok(report) => {
                println!(
                    "provisioned {} cell-agents for region {} at {}",
                    report.manifests.len(),
                    region.name,
                    report.state_dir.display()
                );
                total_manifests += report.manifests.len();
            }
            Err(e) => {
                eprintln!(
                    "immune-bootstrap: failed to provision region {}: {}",
                    region.name, e
                );
                std::process::exit(1);
            }
        }
    }
    println!(
        "immune-bootstrap: {} cell-agents across {} regions written to {}",
        total_manifests,
        regions.len(),
        state_root.display()
    );
}
