// OBSERVABILITY — Tracing + metrics primitives (Spec 8 §8.5)
//
// Per spec §8.5: "Every public API call produces a structured tracing
// span via the `tracing` crate. Every state transition produces a
// discrete event via the `metrics` crate. Every panic produces a
// structured event with full context."
//
// This module declares metric name constants and label helpers so the
// bridge surface can emit consistent telemetry. The exporter (e.g.
// `metrics-exporter-prometheus`) is wired up by Nabu, not here — when
// no exporter is registered, the macros are no-ops, which keeps this
// crate drop-in for callers that don't care about telemetry.
//
// Spec §8.5.2 lists the metric families. v1 emits the writes / latency
// / subscription / panic / consensus / snapshot families. The decay
// and p53 families land with Step 6 (P53 mechanism).

// ── Metric names (Spec 8 §8.5.2) ──────────────────────────────────

/// Counter, labeled `{type, region, outcome}`. Increments on every
/// attempted write.
pub const METRIC_FABRIC_WRITES_TOTAL: &str = "fabric_writes_total";

/// Histogram, labeled `{type}`. Observes wall-clock duration of each
/// successful write in seconds.
pub const METRIC_FABRIC_WRITE_LATENCY_SECONDS: &str = "fabric_write_latency_seconds";

/// Gauge, labeled `{state}` where state ∈ `{active, lagged, panicked}`.
pub const METRIC_FABRIC_SUBSCRIPTION_COUNT: &str = "fabric_subscription_count";

/// Histogram. Observes wall-clock duration of each subscription
/// callback invocation in seconds.
pub const METRIC_FABRIC_SUBSCRIPTION_CALLBACK_LATENCY_SECONDS: &str =
    "fabric_subscription_callback_latency_seconds";

/// Histogram. Observes wall-clock duration of consensus snapshot
/// resolution (from "last finalize observed" to "snapshot written").
pub const METRIC_FABRIC_CONSENSUS_RESOLUTION_SECONDS: &str =
    "fabric_consensus_resolution_seconds";

/// Counter, labeled `{outcome}`. Increments on every per-node
/// signature verification attempt (Spec 5 §3.3).
pub const METRIC_FABRIC_ATTESTATION_VERIFICATIONS_TOTAL: &str =
    "fabric_attestation_verifications_total";

/// Counter, labeled `{location}`. Increments on every caught panic.
pub const METRIC_FABRIC_PANIC_TOTAL: &str = "fabric_panic_total";

/// Gauge. 1 when the SnapshotLock is held on at least one target,
/// else 0. Tracks the §3.4.3 atomic-transition window.
pub const METRIC_FABRIC_SNAPSHOT_LOCK_HELD: &str = "fabric_snapshot_lock_held";

// ── Outcome labels ────────────────────────────────────────────────

/// Outcome label string for `fabric_writes_total`. Stable strings make
/// downstream Prometheus alerts predictable.
pub fn write_outcome_label(result: &Result<impl Sized, crate::identity::WriteError>) -> &'static str {
    use crate::identity::WriteError;
    match result {
        Ok(_) => "success",
        Err(WriteError::SignatureRequired) => "signature_required",
        Err(WriteError::InvalidSignature) => "invalid_signature",
        Err(WriteError::UnknownNamespace) => "unknown_namespace",
        Err(WriteError::NodeLocked { .. }) => "node_locked",
        Err(WriteError::CheckoutExpired { .. }) => "checkout_expired",
        Err(WriteError::EditModeMismatch { .. }) => "edit_mode_mismatch",
        Err(WriteError::SnapshotInProgress) => "snapshot_in_progress",
        Err(WriteError::FabricCongested) => "fabric_congested",
        Err(WriteError::FabricDegraded) => "fabric_degraded",
        Err(WriteError::FabricInternal(_)) => "fabric_internal",
        Err(WriteError::NodeNotFound(_)) => "node_not_found",
    }
}

/// Subscription-count gauge label values.
pub mod subscription_state {
    pub const ACTIVE: &str = "active";
    pub const LAGGED: &str = "lagged";
    pub const PANICKED: &str = "panicked";
}

/// Write-type labels per Spec 8 §8.5.2 (`fabric_writes_total{type,...}`).
pub mod write_type {
    pub const APPEND_ONLY: &str = "AppendOnly";
    pub const MECHANICAL: &str = "Mechanical";
    pub const SEMANTIC: &str = "Semantic";
    pub const CHECKOUT: &str = "Checkout";
    pub const PROPOSAL: &str = "Proposal";
    pub const FINALIZE: &str = "Finalize";
    pub const CONSENSUS_SNAPSHOT: &str = "ConsensusSnapshot";
}

/// String form of `EditMode` for metric labels (matches `EditMode::as_str`).
pub fn edit_mode_label(mode: crate::identity::EditMode) -> &'static str {
    match mode {
        crate::identity::EditMode::AppendOnly => write_type::APPEND_ONLY,
        crate::identity::EditMode::Mechanical => write_type::MECHANICAL,
        crate::identity::EditMode::Semantic => write_type::SEMANTIC,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{EditMode, WriteError};
    use crate::signature::LineageId;

    #[test]
    fn write_outcome_labels_cover_all_variants() {
        // Use a typed unit `Ok` so the function signature is satisfiable.
        let success: Result<(), WriteError> = Ok(());
        assert_eq!(write_outcome_label(&success), "success");

        let cases: Vec<(WriteError, &str)> = vec![
            (WriteError::SignatureRequired, "signature_required"),
            (WriteError::InvalidSignature, "invalid_signature"),
            (WriteError::UnknownNamespace, "unknown_namespace"),
            (
                WriteError::CheckoutExpired { checkout: LineageId::new() },
                "checkout_expired",
            ),
            (
                WriteError::EditModeMismatch {
                    expected: EditMode::AppendOnly,
                    got: "Mechanical",
                },
                "edit_mode_mismatch",
            ),
            (WriteError::SnapshotInProgress, "snapshot_in_progress"),
            (WriteError::FabricCongested, "fabric_congested"),
            (WriteError::FabricDegraded, "fabric_degraded"),
            (WriteError::FabricInternal("boom".into()), "fabric_internal"),
            (WriteError::NodeNotFound(LineageId::new()), "node_not_found"),
        ];
        for (err, expected) in cases {
            let res: Result<(), WriteError> = Err(err);
            assert_eq!(write_outcome_label(&res), expected);
        }
    }

    #[test]
    fn edit_mode_labels_are_distinct() {
        assert_eq!(edit_mode_label(EditMode::AppendOnly), "AppendOnly");
        assert_eq!(edit_mode_label(EditMode::Mechanical), "Mechanical");
        assert_eq!(edit_mode_label(EditMode::Semantic), "Semantic");
    }
}
