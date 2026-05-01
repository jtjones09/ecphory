// QUARANTINE — Autoimmune resolution mechanism (Spec 5 §5.6)
//
// Quarantine is not punishment. It is not exoneration. It is isolation
// while the immune system gathers more information and the operator
// reviews the situation. (Spec 5 §5.6.1)
//
// Quarantine triggers on conflicting signals — for example, an
// `OperatorIntent` AND an `AgencyDiminishment`. Resolution paths:
// 1. Operator confirms → `Confirmed`, weight unfreezes, subscriptions fire.
// 2. Operator reverses → `Reversed`, effects undone where possible.
// 3. No review within expiry → `Dissolved` via fabric decay.
//
// Note on expiry: Spec 5 §5.6.3 lists `expires_at: FabricInstant`. Since
// the bootstrap `FabricInstant` (`temporal/`) wraps a real `Instant` and
// only goes forward in real time, we represent the expiry window as a
// `Duration` measured from `flagged_at`. Behaviorally equivalent.

use crate::identity::voice_print::VoicePrint;
use crate::temporal::FabricInstant;
use std::time::Duration;

/// One of the four questions from Spec 5 §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FabricQuestion {
    /// Strong force — structural integrity (Spec 5 §5.1).
    CausesSuffering,
    /// Weak force — boundary / capability integrity (Spec 5 §5.2).
    DiminishesAgency,
    /// EM force — behavioral integrity (Spec 5 §5.3).
    Inconsistent,
    /// Gravitational force — provenance integrity (Spec 5 §5.4).
    ExploitsAsymmetry,
}

/// What kind of signal the immune system surfaced.
#[derive(Debug, Clone)]
pub enum SignalType {
    /// The operator instructed this. Carries operator's voice print and the
    /// `LineageId`-like identifier of the instructing node.
    OperatorIntent {
        operator_pk: VoicePrint,
        instruction_node: String,
    },
    /// An agent acted without operator instruction.
    AutonomousAction { agent_pk: VoicePrint },
    /// Statistical anomaly observation (cell-agent flagged behavior).
    AnomalyObservation,
    /// Damage observation (content fingerprint mismatch, impossible
    /// causal ordering, etc.).
    DamageObservation,
}

/// One signal among the conflicting set that triggered quarantine.
#[derive(Debug, Clone)]
pub struct ConflictingSignal {
    pub question: FabricQuestion,
    pub signal_type: SignalType,
    /// Confidence in [0.0, 1.0].
    pub confidence: f64,
    /// Human-readable explanation surfaced to the operator.
    pub explanation: String,
}

/// Why a node entered quarantine.
#[derive(Debug, Clone)]
pub enum QuarantineReason {
    /// `OperatorIntent` AND `AgencyDiminishment`.
    OperatorIntentVsAgency,
    /// `OperatorIntent` AND `DamageObservation`.
    OperatorIntentVsDamage,
    /// `AutonomousAction` AND `AgencyDiminishment`.
    AutonomousVsAgency,
    /// Other conflict surfaced by the immune system.
    Other(String),
}

/// Quarantine state of a node.
///
/// `Normal` is the default. When the immune system flags conflicting
/// signals (Spec 5 §5.6.2), the node transitions to `Quarantined`. From
/// there, the operator can `Confirm` or `Reverse`, or the node
/// `Dissolve`s on inaction once `expires_after` elapses since `flagged_at`.
#[derive(Debug, Clone)]
pub enum NodeQuarantineState {
    Normal,
    Quarantined {
        reason: QuarantineReason,
        flagged_at: FabricInstant,
        signals: Vec<ConflictingSignal>,
        /// Window after `flagged_at` before the quarantine dissolves on
        /// inaction. Default 7 days (Spec 5 §5.6.3).
        expires_after: Duration,
    },
    Confirmed {
        confirmed_by: VoicePrint,
        confirmed_at: FabricInstant,
        /// Reserved for v2.5 Black Hole simulation paths.
        selected_path: Option<String>,
        reasoning: String,
    },
    Reversed {
        reversed_by: VoicePrint,
        reversed_at: FabricInstant,
        reasoning: String,
    },
    /// Expired without review — the safe default per Spec 5 §5.6.5.
    Dissolved,
}

impl Default for NodeQuarantineState {
    fn default() -> Self {
        NodeQuarantineState::Normal
    }
}

impl NodeQuarantineState {
    /// Default expiry window per spec §5.6.3: 7 days.
    pub const DEFAULT_EXPIRY: Duration = Duration::from_secs(7 * 86400);

    /// Move a `Normal` node into `Quarantined`.
    pub fn quarantine(
        reason: QuarantineReason,
        signals: Vec<ConflictingSignal>,
        flagged_at: FabricInstant,
        expires_after: Duration,
    ) -> Self {
        NodeQuarantineState::Quarantined {
            reason,
            flagged_at,
            signals,
            expires_after,
        }
    }

    /// Convenience: quarantine with the default 7-day expiry, flagged now.
    pub fn quarantine_default(
        reason: QuarantineReason,
        signals: Vec<ConflictingSignal>,
    ) -> Self {
        Self::quarantine(reason, signals, FabricInstant::now(), Self::DEFAULT_EXPIRY)
    }

    /// Is this node currently observable but weight-frozen?
    /// Per spec §5.6.4: quarantined nodes are queryable but their weight
    /// does not propagate.
    pub fn is_weight_frozen(&self) -> bool {
        matches!(self, NodeQuarantineState::Quarantined { .. })
    }

    /// Should subscriptions fire on this node? Per spec §5.6.4:
    /// "Propagation-frozen — subscriptions do not fire on quarantined nodes
    /// until confirmed."
    pub fn allows_subscription_fire(&self) -> bool {
        !matches!(
            self,
            NodeQuarantineState::Quarantined { .. } | NodeQuarantineState::Dissolved
        )
    }

    /// Is this node still observable in queries? Quarantined and Confirmed
    /// nodes are observable; Dissolved nodes are not (Spec 5 §5.6.5).
    pub fn is_observable(&self) -> bool {
        !matches!(self, NodeQuarantineState::Dissolved)
    }

    /// Has the quarantine expiry elapsed (relative to wall clock since
    /// `flagged_at`)?
    pub fn has_expired(&self) -> bool {
        match self {
            NodeQuarantineState::Quarantined { flagged_at, expires_after, .. } => {
                flagged_at.elapsed_secs() >= expires_after.as_secs_f64()
            }
            _ => false,
        }
    }

    /// Operator confirms the quarantined action — weight unfreezes.
    pub fn confirm(self, confirmed_by: VoicePrint, reasoning: String) -> Self {
        match self {
            NodeQuarantineState::Quarantined { .. } => NodeQuarantineState::Confirmed {
                confirmed_by,
                confirmed_at: FabricInstant::now(),
                selected_path: None,
                reasoning,
            },
            other => other,
        }
    }

    /// Operator reverses the quarantined action — effects undone.
    pub fn reverse(self, reversed_by: VoicePrint, reasoning: String) -> Self {
        match self {
            NodeQuarantineState::Quarantined { .. } => NodeQuarantineState::Reversed {
                reversed_by,
                reversed_at: FabricInstant::now(),
                reasoning,
            },
            other => other,
        }
    }

    /// Mark a quarantine as dissolved if its expiry has elapsed.
    /// Idempotent / no-op for non-`Quarantined` states.
    pub fn dissolve_if_expired(self) -> Self {
        if self.has_expired() {
            NodeQuarantineState::Dissolved
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::voice_print::generate_agent_keypair;

    fn agency_signal() -> ConflictingSignal {
        ConflictingSignal {
            question: FabricQuestion::DiminishesAgency,
            signal_type: SignalType::AnomalyObservation,
            confidence: 0.9,
            explanation: "would crowd out kid agents' write budget".into(),
        }
    }

    #[test]
    fn default_state_is_normal() {
        let q = NodeQuarantineState::default();
        assert!(matches!(q, NodeQuarantineState::Normal));
        assert!(!q.is_weight_frozen());
        assert!(q.allows_subscription_fire());
        assert!(q.is_observable());
    }

    #[test]
    fn quarantined_node_is_observable_but_weight_frozen() {
        let q = NodeQuarantineState::quarantine_default(
            QuarantineReason::OperatorIntentVsAgency,
            vec![agency_signal()],
        );
        assert!(q.is_weight_frozen(),
            "Quarantined nodes are weight-frozen (Spec 5 §5.6.4).");
        assert!(!q.allows_subscription_fire(),
            "Subscriptions must not fire on quarantined nodes (Spec 5 §5.6.4).");
        assert!(q.is_observable(),
            "Quarantined nodes remain observable in queries (Spec 5 §5.6.4).");
    }

    #[test]
    fn confirm_unfreezes_weight() {
        let operator = generate_agent_keypair();
        let q = NodeQuarantineState::quarantine_default(
            QuarantineReason::OperatorIntentVsAgency,
            vec![agency_signal()],
        );
        let confirmed = q.confirm(operator.voice_print(), "I accept the impact".into());

        assert!(matches!(confirmed, NodeQuarantineState::Confirmed { .. }));
        assert!(!confirmed.is_weight_frozen(),
            "Confirmed quarantine unfreezes weight (Spec 5 §5.6.5).");
        assert!(confirmed.allows_subscription_fire());
    }

    #[test]
    fn reverse_marks_state_as_reversed() {
        let operator = generate_agent_keypair();
        let q = NodeQuarantineState::quarantine_default(
            QuarantineReason::AutonomousVsAgency,
            vec![agency_signal()],
        );
        let reversed = q.reverse(operator.voice_print(), "didn't consider impact".into());
        assert!(matches!(reversed, NodeQuarantineState::Reversed { .. }));
        assert!(!reversed.is_weight_frozen());
    }

    #[test]
    fn quarantine_dissolves_after_expiry() {
        // Simulate: quarantine flagged 8 days ago with the default 7-day window.
        let flagged_at = FabricInstant::with_age_secs(8.0 * 86400.0);
        let q = NodeQuarantineState::quarantine(
            QuarantineReason::OperatorIntentVsAgency,
            vec![agency_signal()],
            flagged_at,
            NodeQuarantineState::DEFAULT_EXPIRY,
        );

        assert!(q.has_expired(),
            "An 8-day-old quarantine with a 7-day window must have expired.");

        let dissolved = q.dissolve_if_expired();
        assert!(matches!(dissolved, NodeQuarantineState::Dissolved));
        assert!(!dissolved.is_observable(),
            "Dissolved nodes are no longer observable (Spec 5 §5.6.5).");
    }

    #[test]
    fn quarantine_does_not_dissolve_before_expiry() {
        // Flagged 1 day ago with a 7-day window — still within the window.
        let flagged_at = FabricInstant::with_age_secs(86400.0);
        let q = NodeQuarantineState::quarantine(
            QuarantineReason::OperatorIntentVsAgency,
            vec![agency_signal()],
            flagged_at,
            NodeQuarantineState::DEFAULT_EXPIRY,
        );
        assert!(!q.has_expired());
        let still_quarantined = q.dissolve_if_expired();
        assert!(matches!(still_quarantined, NodeQuarantineState::Quarantined { .. }));
    }
}
