// Intent gravity — the attractor strength function (Spec 9 §2.3 K.S1)
//
// `gravity(intent, agent) = urgency × relevance × evidence_gap`
//
// - `urgency` maps the Spec 7 enum: Background=1, Normal=2, Prompt=4,
//   Immediate=8 (each step doubling its predecessor — the same shape
//   the comms log projection uses for "should this wake the operator")
// - `relevance` is similarity between the intent's terms (description
//   + outcome) and the agent's behavioral profile keywords. v1 uses
//   Jaccard similarity over case-folded word sets — cosine similarity
//   over learned embeddings is staged for v1.5 (Wolpert W.S1 fold).
// - `evidence_gap` is `1.0 - (passing_checks / total_checks)`. An
//   intent with zero SuccessChecks is treated as gap=1.0 (everything
//   is unfinished).
//
// Switching cost: `0.3` constant. An agent stays on its current intent
// unless `gravity(new) > gravity(current) + SWITCHING_COST`. Per the
// spec note, this prevents thrashing every time a high-urgency intent
// appears.

use std::collections::HashSet;

use crate::comms::message::Urgency;

use super::intent::WorkIntent;

/// Per the spec K.S1 fold. Tunable in v2 once the WorkObserver
/// surfaces empirical thrash rates.
pub const SWITCHING_COST: f64 = 0.3;

/// An agent's behavioral profile for the v1 keyword-overlap relevance
/// score. Seeded at agent provisioning; refined by the immune system's
/// behavioral baseline once it has enough observations.
///
/// v1.5 will replace this with embedding vectors derived from the
/// agent's evidence history.
#[derive(Debug, Clone, Default)]
pub struct AgentProfile {
    pub keywords: HashSet<String>,
}

impl AgentProfile {
    pub fn new<I, S>(words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let keywords = words
            .into_iter()
            .map(|w| w.into().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        Self { keywords }
    }
}

/// Numeric weight for an `Urgency` band — each step doubles the
/// previous so a Prompt intent is half a Background and a quarter of
/// an Immediate.
fn urgency_weight(u: Urgency) -> f64 {
    match u {
        Urgency::Background => 1.0,
        Urgency::Normal => 2.0,
        Urgency::Prompt => 4.0,
        Urgency::Immediate => 8.0,
    }
}

/// Jaccard similarity between two word sets. Returns 0.0 when both are
/// empty (no shared semantics yet).
fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Tokenize the intent's description + outcome into the same case-
/// folded word set the agent profile holds. Strips punctuation by
/// keeping only alphanumeric characters.
fn intent_terms(intent: &WorkIntent) -> HashSet<String> {
    let mut all = String::new();
    all.push_str(&intent.description);
    all.push(' ');
    all.push_str(&intent.intended_outcome);
    all.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Evidence gap: 1.0 means everything is unfinished, 0.0 means
/// every SuccessCheck has passed against linked evidence.
///
/// Step 1 uses the success_checks length as a proxy: with no
/// linked-evidence map yet, gap = 1.0 if there are any checks at all,
/// else 1.0 (still attract — you can't tell whether the work is done).
/// Step 3's visibility query feeds the real `passing_checks` count
/// derived from evaluating predicates against linked evidence.
pub fn evidence_gap(passing_checks: usize, total_checks: usize) -> f64 {
    if total_checks == 0 {
        return 1.0;
    }
    let passing = passing_checks.min(total_checks);
    1.0 - (passing as f64 / total_checks as f64)
}

/// Compute the gravity an `intent` exerts on an `agent`, given the
/// current `passing_checks` count against the intent's SuccessChecks
/// (defaults to 0 when the visibility query hasn't evaluated yet).
pub fn gravity(intent: &WorkIntent, agent: &AgentProfile, passing_checks: usize) -> f64 {
    let urgency = urgency_weight(intent.urgency);
    let relevance = jaccard(&intent_terms(intent), &agent.keywords);
    let gap = evidence_gap(passing_checks, intent.success_checks.len());
    urgency * relevance * gap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;
    use crate::work::intent::{IntentDuration, WorkIntent};

    fn intent(description: &str, urgency: Urgency) -> WorkIntent {
        WorkIntent {
            description: description.into(),
            intended_outcome: "done".into(),
            constraints: vec![],
            success_checks: vec![],
            requested_by: generate_agent_keypair().voice_print(),
            urgency,
            context: vec![],
            duration: IntentDuration::Discrete { deadline: None },
        }
    }

    #[test]
    fn higher_relevance_yields_higher_gravity() {
        // Per the Step 1 acceptance test: agent with higher relevance
        // should have higher gravity for the same intent.
        let i = intent("implement the visibility substrate", Urgency::Prompt);
        let infra_agent = AgentProfile::new(["visibility", "substrate", "fabric"]);
        let prose_agent = AgentProfile::new(["copywriting", "marketing", "voice"]);
        let g_infra = gravity(&i, &infra_agent, 0);
        let g_prose = gravity(&i, &prose_agent, 0);
        assert!(
            g_infra > g_prose,
            "infra agent ({}) should be more attracted than prose agent ({})",
            g_infra,
            g_prose
        );
        assert!(g_infra > 0.0);
        assert_eq!(g_prose, 0.0); // zero overlap → zero gravity
    }

    #[test]
    fn higher_urgency_yields_higher_gravity() {
        let agent = AgentProfile::new(["fabric"]);
        let normal = intent("touch the fabric", Urgency::Normal);
        let immediate = intent("touch the fabric", Urgency::Immediate);
        let g_normal = gravity(&normal, &agent, 0);
        let g_immediate = gravity(&immediate, &agent, 0);
        assert_eq!(g_immediate / g_normal, 4.0); // 8 / 2
    }

    #[test]
    fn evidence_gap_collapses_when_complete() {
        let mut i = intent("close out spec 9", Urgency::Normal);
        i.success_checks = vec![
            crate::comms::message::SuccessCheck::NodeExists {
                reference: "x".into(),
            };
            4
        ];
        let agent = AgentProfile::new(["close", "spec"]);
        let unstarted = gravity(&i, &agent, 0);
        let half_done = gravity(&i, &agent, 2);
        let done = gravity(&i, &agent, 4);
        assert!(unstarted > half_done);
        assert!(half_done > 0.0);
        assert_eq!(done, 0.0); // zero gap → zero gravity
    }

    #[test]
    fn switching_cost_protects_current_work() {
        // Concrete inertia check — the spec rule is:
        //   agent drifts iff gravity(new) > gravity(current) + SWITCHING_COST
        let agent = AgentProfile::new(["fabric", "spec"]);
        let current = intent("polish the fabric spec", Urgency::Normal);
        let shiny_distraction = intent("polish the fabric demo", Urgency::Normal);
        let g_current = gravity(&current, &agent, 0);
        let g_new = gravity(&shiny_distraction, &agent, 0);
        // Roughly equal-relevance work shouldn't trigger a switch.
        assert!(g_new <= g_current + SWITCHING_COST);
    }

    #[test]
    fn jaccard_returns_zero_for_empty() {
        let empty: HashSet<String> = HashSet::new();
        let some: HashSet<String> = ["a".into()].into_iter().collect();
        assert_eq!(jaccard(&empty, &some), 0.0);
        assert_eq!(jaccard(&empty, &empty), 0.0);
    }

    #[test]
    fn evidence_gap_handles_no_checks() {
        assert_eq!(evidence_gap(0, 0), 1.0);
        assert_eq!(evidence_gap(5, 0), 1.0); // safety: no checks → full gap
    }

    #[test]
    fn evidence_gap_handles_overcount() {
        // Defensive: more passing than total → gap = 0, not negative.
        assert_eq!(evidence_gap(10, 3), 0.0);
    }
}
