// IMMUNE HEALTH METRICS — Spec 6 CANTRILL.1 fold
//
// The immune system itself needs observable health metrics. These
// are emitted via the `metrics` crate and surfaced on Prometheus by
// nabu's `/metrics` route (the same mechanism Spec 8 §8.5 uses for
// fabric metrics).
//
// Metric family names are stable strings — operator dashboards key
// off them.

use crate::identity::NamespaceId;
use metrics::{counter, gauge};

use super::aggregation::ImmuneResponseMode;

// ── Metric name constants ─────────────────────────────────────────

pub const METRIC_CELL_AGENTS_TOTAL: &str = "immune_cell_agents_total";
pub const METRIC_ANOMALY_OBSERVATIONS_TOTAL: &str = "immune_anomaly_observations_total";
pub const METRIC_DAMAGE_OBSERVATIONS_TOTAL: &str = "immune_damage_observations_total";
pub const METRIC_CONVERGENCE_RATE: &str = "immune_convergence_rate";
pub const METRIC_BASELINE_AGE_SECONDS: &str = "immune_baseline_age_seconds";
pub const METRIC_POPULATION_DIVERSITY_SCORE: &str = "immune_population_diversity_score";
pub const METRIC_RESPONSE_MODE: &str = "immune_response_mode";
pub const METRIC_MAINTENANCE_DEFENSE_RATIO: &str = "immune_maintenance_defense_ratio";

/// Cell-agent state used as a label value on
/// `immune_cell_agents_total`.
pub mod cell_agent_state {
    pub const HEALTHY: &str = "healthy";
    pub const STALE: &str = "stale";
    pub const MISFIRING: &str = "misfiring";
    pub const RETIRED: &str = "retired";
}

// ── Emit helpers ─────────────────────────────────────────────────

pub fn record_anomaly_observation(specialization: &str, region: &NamespaceId) {
    counter!(
        METRIC_ANOMALY_OBSERVATIONS_TOTAL,
        "specialization" => specialization.to_string(),
        "region" => region.name.clone(),
    )
    .increment(1);
}

pub fn record_damage_observation(specialization: &str, region: &NamespaceId) {
    counter!(
        METRIC_DAMAGE_OBSERVATIONS_TOTAL,
        "specialization" => specialization.to_string(),
        "region" => region.name.clone(),
    )
    .increment(1);
}

pub fn record_baseline_healthy_age(specialization: &str, region: &NamespaceId, age_secs: f64) {
    gauge!(
        METRIC_BASELINE_AGE_SECONDS,
        "specialization" => specialization.to_string(),
        "region" => region.name.clone(),
    )
    .set(age_secs);
}

pub fn record_response_mode(region: &NamespaceId, mode: ImmuneResponseMode) {
    let label = match mode {
        ImmuneResponseMode::Active => "Active",
        ImmuneResponseMode::AlertOnly => "AlertOnly",
        ImmuneResponseMode::Disabled => "Disabled",
    };
    gauge!(
        METRIC_RESPONSE_MODE,
        "region" => region.name.clone(),
        "mode" => label,
    )
    .set(1.0);
}

pub fn record_cell_agent_state(specialization: &str, region: &NamespaceId, state: &str, count: u64) {
    gauge!(
        METRIC_CELL_AGENTS_TOTAL,
        "specialization" => specialization.to_string(),
        "region" => region.name.clone(),
        "state" => state.to_string(),
    )
    .set(count as f64);
}

pub fn record_maintenance_defense_ratio(region: &NamespaceId, ratio: f64) {
    gauge!(
        METRIC_MAINTENANCE_DEFENSE_RATIO,
        "region" => region.name.clone(),
    )
    .set(ratio);
}

pub fn record_convergence_rate(observations_per_converged: f64) {
    gauge!(METRIC_CONVERGENCE_RATE).set(observations_per_converged);
}

pub fn record_population_diversity(region: &NamespaceId, score: f64) {
    gauge!(
        METRIC_POPULATION_DIVERSITY_SCORE,
        "region" => region.name.clone(),
    )
    .set(score);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_run_without_panic_when_no_recorder_installed() {
        // With no metrics-exporter installed, every macro is a no-op.
        // Smoke-test that each emit helper accepts its inputs.
        let region = NamespaceId::fresh("test");
        record_anomaly_observation("RateObserver", &region);
        record_damage_observation("AttestationObserver", &region);
        record_baseline_healthy_age("DecayObserver", &region, 60.0);
        record_response_mode(&region, ImmuneResponseMode::Active);
        record_cell_agent_state(
            "RateObserver",
            &region,
            cell_agent_state::HEALTHY,
            1,
        );
        record_maintenance_defense_ratio(&region, 0.92);
        record_convergence_rate(7.5);
        record_population_diversity(&region, 0.8);
    }
}
