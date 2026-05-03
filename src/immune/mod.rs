// IMMUNE SYSTEM — The fabric's primary trust mechanism (Spec 6)
//
// Per Spec 5 v2.1: behavioral observation REPLACES per-write signature
// verification for normal regions. The cell-agent population's
// assessment of an agent's pattern IS what determines trust. This is
// not a secondary security monitor — the immune system is the trust
// mechanism (Spec 6 §1).
//
// Architecture (Spec 6 §2 — two layers):
// - Layer 1 (substrate): every fabric action carries identifiable
//   provenance via Spec 8 §13's content-fingerprint + voice-print
//   surface. Already shipped.
// - Layer 2 (cognitive): a population of cell-agent observers, six
//   specializations per region, that maintain behavioral baselines
//   and surface anomaly / damage signals. THIS module.
//
// Maintenance / defense ratio (COHEN.1 fold): in steady state,
// ≥80% of cell-agent output is `BaselineHealthy` from maintenance
// specializations; ≤20% is anomaly / damage from defense
// specializations. If the ratio inverts, this is an IDS, not a
// real immune system.

pub mod aggregation;
pub mod baseline;
pub mod bootstrap;
pub mod cell_agent;
pub mod cognitive_map;
pub mod health;
pub mod inheritance;
pub mod multidim;
pub mod opacity;
pub mod specialization;

pub use baseline::WelfordTracker;
pub use aggregation::{
    AggregationConfig, AggregationLayer, AggregationOutcome, ConvergedAnomalyRecord,
    ImmuneResponseMode, ObservationRecord,
};
pub use bootstrap::{
    bootstrap_region, enforce_population, CellAgentManifest, MissingPopulation,
    RegionProvisionReport, V1_SPECIALIZATIONS,
};
pub use cognitive_map::{AnomalyCluster, CognitiveMap, StateVector};
pub use inheritance::BaselineSnapshot;
pub use multidim::{
    chi_squared_critical, ledoit_wolf_shrink, trust_modulated_threshold, MatrixError,
    MultivariateBaseline,
};
pub use cell_agent::{
    AnomalyObservation, BaselineHealthy, CellAgent, CellAgentHealth, CellAgentId,
    DamageObservation, ImmunePattern, ObservationContext, ObservationOutcome,
    ObservationSeverity, ObservedEvent, RetuneReport,
};
pub use specialization::{
    AttestationObserver, ConsensusObserver, DecayObserver, RateObserver, RelationObserver,
    SilenceObserver, Specialization,
};
pub use opacity::{
    OpacityObserver, OperatorObservedSet, DEFAULT_OBSERVATION_WINDOW, DEFAULT_OPACITY_THRESHOLD,
};
