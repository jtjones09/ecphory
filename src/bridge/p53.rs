// P53 — Architectural mortality (Spec 8 §8.4)
//
// Per spec §8.4 and Cohen I.3 fold, p53 has three scopes with very
// different operational stories:
//
// • `Node`   — routine, expected. A node observes its own irrecoverable
//              corruption and self-terminates. No operator alert.
// • `Region` — serious. Region-wide compromise; drain subscriptions,
//              forensic-archive the region, refuse all further writes to
//              it, alert the operator.
// • `Fabric` — catastrophic, manual trigger only with the operator's
//              offline key. Drain, archive everything, enter terminated
//              state. Recovery requires a fresh installation.
//
// Per spec §8.4.5 every subscription receives a final event before its
// fabric goes silent — `RegionDying` / `FabricDying`. The drain has a
// 30-second budget; subscribers that don't drain within the budget are
// dropped without notification. v1 makes the budget configurable so
// tests can use ~100ms while production uses 30s.

use crate::identity::{NamespaceId, VoicePrint};
use crate::signature::LineageId;
use crate::temporal::FabricInstant;
use std::time::Duration;

/// What the p53 trigger applies to (Spec 8 §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P53Scope {
    /// Single-node self-termination — routine background activity.
    Node(LineageId),
    /// Region-wide collapse — drain, archive, refuse further writes.
    Region(NamespaceId),
    /// Full-fabric collapse — manual operator trigger only.
    Fabric,
}

impl P53Scope {
    pub fn label(&self) -> &'static str {
        match self {
            P53Scope::Node(_) => "Node",
            P53Scope::Region(_) => "Region",
            P53Scope::Fabric => "Fabric",
        }
    }
}

/// The operator's "offline" p53 key. Ed25519 keypair under the hood;
/// "offline" is an operational property (never carried into a running
/// fabric process for `Fabric` triggers) the bridge can't enforce.
pub type P53Key = crate::identity::AgentKeypair;

/// Receipt emitted on a successful p53 trigger.
#[derive(Debug, Clone)]
pub struct P53Receipt {
    pub scope_label: &'static str,
    /// `LineageId` of the `P53*Terminated` event node written into the
    /// fabric. Subscribers + the immune system observe this.
    pub event_node: LineageId,
    /// When the fabric committed the trigger.
    pub triggered_at: FabricInstant,
    /// Number of subscriptions that received a final dying event.
    pub subscriptions_drained: usize,
    /// `Some(path)` when a forensic archive was written (Region/Fabric).
    pub forensic_archive: Option<String>,
}

/// Errors the p53 path can return.
#[derive(Debug, Clone, PartialEq)]
pub enum SafetyError {
    /// The supplied scope already has a recorded p53 trigger; not
    /// re-entrant.
    P53AlreadyTriggered { scope_label: &'static str },
    /// The signer's voice print doesn't match the configured operator
    /// key (when one is set).
    InvalidP53Key,
    /// `Fabric` scope is reserved for manual-only triggers; running
    /// without an explicit operator-key match is refused unless test
    /// mode is enabled.
    ScopeNotPermittedAtRuntime,
    /// Forensic archive write failed.
    ArchiveFailed(String),
    /// Generic fabric error.
    FabricInternal(String),
}

impl std::fmt::Display for SafetyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafetyError::P53AlreadyTriggered { scope_label } => {
                write!(f, "p53 already triggered for scope {}", scope_label)
            }
            SafetyError::InvalidP53Key => write!(f, "p53 key not authorized"),
            SafetyError::ScopeNotPermittedAtRuntime => {
                write!(f, "fabric-scope p53 requires offline operator key + manual gate")
            }
            SafetyError::ArchiveFailed(reason) => write!(f, "forensic archive write failed: {}", reason),
            SafetyError::FabricInternal(reason) => write!(f, "fabric internal: {}", reason),
        }
    }
}

impl std::error::Error for SafetyError {}

/// Tunable knobs for the p53 mechanism. Defaults match Spec 8 §8.4.5.
#[derive(Debug, Clone)]
pub struct P53Config {
    /// Subscription-drain budget per scope. Spec default 30 seconds.
    pub drain_budget: Duration,
    /// Where region/fabric forensic archives are written. Defaults to
    /// `/var/lib/ecphory/p53-archive`. Tests override via env or config.
    pub archive_root: std::path::PathBuf,
    /// When `Some`, p53 triggers require the signer's voice print to
    /// match. `None` accepts any signer (test mode / pre-genesis).
    pub authorized_operator: Option<VoicePrint>,
    /// When true, `P53Scope::Fabric` is permitted in this process. v1
    /// production should leave this `false`; the operator runbook
    /// enables it explicitly via offline-key flow.
    pub fabric_scope_enabled: bool,
}

impl Default for P53Config {
    fn default() -> Self {
        Self {
            drain_budget: Duration::from_secs(30),
            archive_root: std::path::PathBuf::from("/var/lib/ecphory/p53-archive"),
            authorized_operator: None,
            // Disabled by default per spec §8.4.3 — fabric-wide p53
            // requires explicit operator action.
            fabric_scope_enabled: false,
        }
    }
}

/// Snapshot of a single node + its outbound edges, used by the
/// region/fabric forensic archive.
#[derive(Debug, Clone)]
pub struct ArchivedNode {
    pub lineage_id: String,
    pub want: String,
    pub content_fingerprint_hex: String,
    pub creator_voice_hex: Option<String>,
    pub edges_out: Vec<ArchivedEdge>,
}

#[derive(Debug, Clone)]
pub struct ArchivedEdge {
    pub target: String,
    pub weight: f64,
    pub kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::LineageId;

    #[test]
    fn scope_labels_are_distinct() {
        let n = P53Scope::Node(LineageId::new());
        let r = P53Scope::Region(NamespaceId::fresh("test"));
        let f = P53Scope::Fabric;
        let mut labels = std::collections::HashSet::new();
        labels.insert(n.label());
        labels.insert(r.label());
        labels.insert(f.label());
        assert_eq!(labels.len(), 3);
    }

    #[test]
    fn default_config_disables_fabric_scope() {
        let cfg = P53Config::default();
        assert!(!cfg.fabric_scope_enabled);
        assert_eq!(cfg.drain_budget, Duration::from_secs(30));
    }
}
