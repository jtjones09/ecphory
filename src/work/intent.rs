// WorkIntent + IntentDuration — Spec 9 §2.3
//
// A WorkIntent is an attractor in the fabric's state space, not a
// ticket on a board. It carries the description of what's wanted, the
// SuccessCheck predicates that determine completion, and a duration
// kind (discrete with deadline or standing with expected cadence).
//
// `to_intent_node` materializes the WorkIntent as an IntentNode ready
// for `Fabric::create()`. Metadata keys (defined in `work::mod`) let
// downstream parsers (visibility query, WorkObserver) recover the
// structured fields without re-rendering the body.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::comms::message::{SuccessCheck, Urgency};
use crate::identity::{NodeIdentity, VoicePrint};
use crate::node::{IntentNode, MetadataValue};

use super::*;

#[derive(Debug, Clone)]
pub enum IntentDuration {
    /// Work that completes. The deadline is optional — many tasks have
    /// "should be done soon" without a hard date.
    ///
    /// Spec text says `Option<FabricInstant>` but FabricInstant is a
    /// process-local monotonic wrapper (no wall-clock anchor), so it
    /// can't represent an absolute deadline. v1 uses `SystemTime`;
    /// the spec text needs a v1.2 patch.
    Discrete { deadline: Option<SystemTime> },
    /// Work that never completes — maintain the journal, monitor
    /// fabric health, keep packages current. Stalls are detected when
    /// evidence cadence drops below the immune-baseline-calibrated
    /// threshold (see Spec 9 §3.3 + §6.1 G.2 fold).
    Standing { expected_cadence: Duration },
}

impl IntentDuration {
    pub fn is_standing(&self) -> bool {
        matches!(self, IntentDuration::Standing { .. })
    }

    pub fn is_discrete(&self) -> bool {
        matches!(self, IntentDuration::Discrete { .. })
    }
}

#[derive(Debug, Clone)]
pub struct WorkIntent {
    pub description: String,
    pub intended_outcome: String,
    pub constraints: Vec<String>,
    /// Reused from Spec 7 §4.2. Predicate evaluation against the
    /// fabric is the visibility query's job (Step 3).
    pub success_checks: Vec<SuccessCheck>,
    pub requested_by: VoicePrint,
    pub urgency: Urgency,
    /// Related nodes the worker should observe — context for the
    /// attractor's "relevance" dimension in the gravity function.
    pub context: Vec<NodeIdentity>,
    pub duration: IntentDuration,
}

impl WorkIntent {
    /// Materialize as an `IntentNode` ready for `Fabric::create()`.
    pub fn to_intent_node(&self, creator: VoicePrint) -> IntentNode {
        let body = render_body(self);
        let mut node = IntentNode::new(body).with_creator_voice(creator);

        node.metadata.insert(
            META_KIND.into(),
            MetadataValue::String(KIND_WORK_INTENT.into()),
        );
        node.metadata.insert(
            META_INTENT_OUTCOME.into(),
            MetadataValue::String(self.intended_outcome.clone()),
        );
        node.metadata.insert(
            META_INTENT_URGENCY.into(),
            MetadataValue::String(self.urgency.label().into()),
        );
        node.metadata.insert(
            META_INTENT_REQUESTED_BY.into(),
            MetadataValue::String(self.requested_by.to_hex()),
        );
        node.metadata.insert(
            META_INTENT_CHECK_COUNT.into(),
            MetadataValue::Int(self.success_checks.len() as i64),
        );

        if !self.constraints.is_empty() {
            node.metadata.insert(
                META_INTENT_CONSTRAINTS.into(),
                MetadataValue::String(self.constraints.join("\n")),
            );
        }

        match &self.duration {
            IntentDuration::Discrete { deadline } => {
                node.metadata.insert(
                    META_INTENT_DURATION_KIND.into(),
                    MetadataValue::String(DURATION_DISCRETE.into()),
                );
                if let Some(d) = deadline {
                    let nanos = d
                        .duration_since(UNIX_EPOCH)
                        .map(|dur| dur.as_nanos() as i64)
                        .unwrap_or(0);
                    node.metadata.insert(
                        META_INTENT_DEADLINE_NS.into(),
                        MetadataValue::Int(nanos),
                    );
                }
            }
            IntentDuration::Standing { expected_cadence } => {
                node.metadata.insert(
                    META_INTENT_DURATION_KIND.into(),
                    MetadataValue::String(DURATION_STANDING.into()),
                );
                node.metadata.insert(
                    META_INTENT_CADENCE_SECS.into(),
                    MetadataValue::Int(expected_cadence.as_secs() as i64),
                );
            }
        }

        node.recompute_signature();
        node
    }

    /// True if a materialized node was a WorkIntent.
    pub fn is_intent_node(node: &IntentNode) -> bool {
        node.metadata
            .get(META_KIND)
            .map(|v| v.as_str_repr() == KIND_WORK_INTENT)
            .unwrap_or(false)
    }
}

fn render_body(intent: &WorkIntent) -> String {
    let mut out = format!(
        "[WorkIntent] {}\nOutcome: {}",
        intent.description, intent.intended_outcome
    );
    if !intent.constraints.is_empty() {
        out.push_str("\nConstraints: ");
        out.push_str(&intent.constraints.join("; "));
    }
    out.push_str(&format!("\nUrgency: {}", intent.urgency.label()));
    match &intent.duration {
        IntentDuration::Discrete { deadline } => {
            out.push_str("\nDuration: discrete");
            if let Some(d) = deadline {
                if let Ok(dur) = d.duration_since(UNIX_EPOCH) {
                    out.push_str(&format!(" (deadline {}s since epoch)", dur.as_secs()));
                }
            }
        }
        IntentDuration::Standing { expected_cadence } => {
            out.push_str(&format!(
                "\nDuration: standing ({}s cadence)",
                expected_cadence.as_secs()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    fn sample_intent() -> WorkIntent {
        let kp = generate_agent_keypair();
        WorkIntent {
            description: "Implement Spec 9 visibility query".into(),
            intended_outcome: "fabric_work_status returns 8 categories".into(),
            constraints: vec!["no set_status() method anywhere".into()],
            success_checks: vec![],
            requested_by: kp.voice_print(),
            urgency: Urgency::Prompt,
            context: vec![],
            duration: IntentDuration::Discrete { deadline: None },
        }
    }

    #[test]
    fn discrete_intent_materializes_with_kind_metadata() {
        let intent = sample_intent();
        let creator = generate_agent_keypair().voice_print();
        let node = intent.to_intent_node(creator);
        assert!(WorkIntent::is_intent_node(&node));
        assert_eq!(
            node.metadata
                .get(META_INTENT_DURATION_KIND)
                .unwrap()
                .as_str_repr(),
            DURATION_DISCRETE
        );
    }

    #[test]
    fn standing_intent_records_cadence() {
        let mut intent = sample_intent();
        intent.duration = IntentDuration::Standing {
            expected_cadence: Duration::from_secs(3600),
        };
        let node = intent.to_intent_node(intent.requested_by);
        assert_eq!(
            node.metadata
                .get(META_INTENT_DURATION_KIND)
                .unwrap()
                .as_str_repr(),
            DURATION_STANDING
        );
        let cadence = match node.metadata.get(META_INTENT_CADENCE_SECS).unwrap() {
            MetadataValue::Int(n) => *n,
            _ => panic!("cadence should be Int"),
        };
        assert_eq!(cadence, 3600);
    }

    #[test]
    fn urgency_round_trips_through_metadata() {
        let mut intent = sample_intent();
        intent.urgency = Urgency::Immediate;
        let node = intent.to_intent_node(intent.requested_by);
        assert_eq!(
            node.metadata.get(META_INTENT_URGENCY).unwrap().as_str_repr(),
            "Immediate"
        );
    }

    #[test]
    fn not_a_work_intent_when_kind_missing() {
        let node = IntentNode::new("just a regular node");
        assert!(!WorkIntent::is_intent_node(&node));
    }
}
