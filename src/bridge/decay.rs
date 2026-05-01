// DECAY TICK — Periodic mortality (Spec 8 §7, §8.3)
//
// Per spec §7 the trait surface includes
//   fn decay_tick(&self) -> Result<DecayReport, DecayError>;
//
// Per spec §8.3 default behavior is exponential decay with a
// configurable half-life per node type. Default half-lives:
//   nisaba:journal:entry      → ∞ (no decay)
//   nisaba:pinned:entry       → 365 days
//   nisaba:decision:entry     → ∞
//   propmgmt:extraction:*     → 7 years (regulatory retention)
//   fabric:node:weight        → 30 days
//   fabric:subscription:state → 24 hours
//   default                   → 90 days
//
// v1 implementation honors only the `default` half-life — the
// per-node-type table lands when the node-type system arrives (per
// Jeremy's call: no NodeKind on IntentNode for v1). The bridge tracks
// last-access wall-clock time on the inner fabric (already there), so
// decay is just a sweep that fades nodes whose temporal weight fell
// below a configurable threshold.
//
// Per spec §11 acceptance #6: dissolved nodes are preserved for audit
// — we hand the dropped IntentNode back in the report so callers can
// forward to a forensic store if they care. Operationally Step 6's P53
// archive path is the same archive shape; integrating both is a v1.5
// follow-up.

use crate::signature::LineageId;
use crate::temporal::FabricInstant;
use std::time::Duration;

/// `decay_tick` summary per Spec 8 §7.
#[derive(Debug, Clone)]
pub struct DecayReport {
    /// Wall-clock instant the tick began.
    pub started_at: FabricInstant,
    /// Total nodes the tick evaluated.
    pub nodes_evaluated: u64,
    /// Nodes whose weight fell below the dissolution threshold.
    pub nodes_dissolved: u64,
    /// LineageIds of the dissolved nodes — caller can forward them to
    /// a forensic archive if desired.
    pub dissolved_ids: Vec<LineageId>,
    /// Wall-clock duration of the tick.
    pub duration: Duration,
    /// True if the tick was cut short by the budget; remaining nodes
    /// are left for the next tick (Spec 8 §2.6.2).
    pub deferred_to_next_tick: bool,
}

/// Errors `decay_tick` can return.
#[derive(Debug, Clone, PartialEq)]
pub enum DecayError {
    /// Tick exceeded the configured budget; partial work was committed,
    /// remaining nodes are deferred. The fabric is healthy but operator
    /// should consider increasing the budget or sweeping more often.
    BudgetExceeded,
    /// Generic fabric error.
    FabricInternal(String),
}

impl std::fmt::Display for DecayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecayError::BudgetExceeded => write!(f, "decay tick exceeded its budget"),
            DecayError::FabricInternal(reason) => write!(f, "fabric internal: {}", reason),
        }
    }
}

impl std::error::Error for DecayError {}

/// Tunable knobs for the decay tick.
#[derive(Debug, Clone, Copy)]
pub struct DecayConfig {
    /// Wall-clock budget for a single tick. Default 30s per Spec 8
    /// §2.6.2.
    pub tick_budget: Duration,
    /// Weight threshold below which a node is dissolved. Per spec §8.3
    /// "weight approaches zero, never reaches it" — but operationally
    /// a tiny weight is indistinguishable from zero. Default 1e-3.
    pub dissolution_threshold: f64,
    /// Default half-life used when no per-node-type table entry
    /// applies. Spec §8.3 default: 90 days.
    pub default_half_life: Duration,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            tick_budget: Duration::from_secs(30),
            dissolution_threshold: 1e-3,
            default_half_life: Duration::from_secs(90 * 86400),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let cfg = DecayConfig::default();
        assert_eq!(cfg.tick_budget, Duration::from_secs(30));
        assert_eq!(cfg.default_half_life, Duration::from_secs(90 * 86400));
        assert!(cfg.dissolution_threshold > 0.0);
    }
}
