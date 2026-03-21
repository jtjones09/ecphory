// CONSTRAINTS
//
// Design decisions:
// 1. Constraints BOUND, they never PRESCRIBE (Law 2)
// 2. Two kinds: Hard (inviolable) and Soft (negotiable with weight)
// 3. Constraints carry semantic meaning — they describe WHAT is bounded,
//    not how to implement the boundary
// 4. A constraint is itself a semantic shape — "must be private" is a region
//    in meaning-space that excludes non-private resolutions
// 5. For Phase 1, we represent semantic content as a String descriptor.
//    This is a PLACEHOLDER. Phase 2 replaces it with embedding vectors.
//    The String is the shovel, not the building.
//
// Open questions:
// - How do soft constraints negotiate? What's the protocol?
//   Decision: Soft constraints carry a weight [0,1]. Resolution engine
//   can violate soft constraints if the cost of satisfying them exceeds
//   the weight. The violation is recorded, not hidden.
// - Can constraints conflict? Yes. That's surfaced as low resolution confidence.

use std::fmt;

/// How a constraint limits the resolution space.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintKind {
    /// Inviolable. Violation = intent failed. No negotiation.
    /// "Must be private." "Must not contact this person." "Must complete before deadline."
    Hard,

    /// Negotiable. Carries weight indicating importance.
    /// Resolution engine can flex on these when perfect resolution isn't available.
    /// Weight 1.0 = practically hard. Weight 0.1 = nice to have.
    /// Violation is recorded and visible, never hidden.
    Soft { weight: f64 },
}

/// A single constraint on an intent's resolution space.
///
/// Constraints eliminate paths — they define what the resolution
/// CANNOT do or MUST satisfy. They never specify HOW.
#[derive(Debug, Clone, PartialEq)]
pub struct Constraint {
    /// Semantic description of what is bounded.
    /// Phase 1: String placeholder.
    /// Phase 2: Embedding vector — a region in meaning-space.
    pub semantic: String,

    /// Hard or soft, with negotiation weight for soft.
    pub kind: ConstraintKind,

    /// Has this constraint been checked during resolution?
    /// Unchecked constraints are a signal — the system hasn't verified them yet.
    pub verified: bool,

    /// Was this constraint violated during resolution?
    /// Only meaningful after resolution attempt.
    /// None = not yet resolved. Some(true) = violated. Some(false) = satisfied.
    pub violated: Option<bool>,
}

impl Constraint {
    pub fn hard(semantic: impl Into<String>) -> Self {
        Self {
            semantic: semantic.into(),
            kind: ConstraintKind::Hard,
            verified: false,
            violated: None,
        }
    }

    pub fn soft(semantic: impl Into<String>, weight: f64) -> Self {
        assert!((0.0..=1.0).contains(&weight), "Soft constraint weight must be in [0, 1]");
        Self {
            semantic: semantic.into(),
            kind: ConstraintKind::Soft { weight },
            verified: false,
            violated: None,
        }
    }

    pub fn is_hard(&self) -> bool {
        matches!(self.kind, ConstraintKind::Hard)
    }

    pub fn weight(&self) -> f64 {
        match self.kind {
            ConstraintKind::Hard => 1.0,
            ConstraintKind::Soft { weight } => weight,
        }
    }

    /// Mark this constraint as satisfied.
    pub fn satisfy(&mut self) {
        self.verified = true;
        self.violated = Some(false);
    }

    /// Mark this constraint as violated.
    /// For hard constraints, this means intent resolution failed.
    pub fn violate(&mut self) {
        self.verified = true;
        self.violated = Some(true);
    }
}

/// The complete constraint field for an intent node.
/// Holds all constraints and provides aggregate operations.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConstraintField {
    pub constraints: Vec<Constraint>,
}

impl ConstraintField {
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }

    pub fn add_hard(&mut self, semantic: impl Into<String>) {
        self.constraints.push(Constraint::hard(semantic));
    }

    pub fn add_soft(&mut self, semantic: impl Into<String>, weight: f64) {
        self.constraints.push(Constraint::soft(semantic, weight));
    }

    /// Are any hard constraints violated?
    /// If so, the intent resolution has FAILED.
    pub fn has_hard_violation(&self) -> bool {
        self.constraints.iter().any(|c| c.is_hard() && c.violated == Some(true))
    }

    /// Total cost of soft constraint violations.
    /// Higher = more compromises were made.
    pub fn soft_violation_cost(&self) -> f64 {
        self.constraints
            .iter()
            .filter(|c| !c.is_hard() && c.violated == Some(true))
            .map(|c| c.weight())
            .sum()
    }

    /// Are all constraints verified (checked)?
    pub fn fully_verified(&self) -> bool {
        self.constraints.iter().all(|c| c.verified)
    }

    /// How many constraints exist?
    pub fn count(&self) -> usize {
        self.constraints.len()
    }

    /// How many hard constraints?
    pub fn hard_count(&self) -> usize {
        self.constraints.iter().filter(|c| c.is_hard()).count()
    }
}

impl fmt::Display for ConstraintField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hard = self.hard_count();
        let soft = self.count() - hard;
        write!(f, "Constraints [{} hard, {} soft]", hard, soft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_constraint_weight_is_one() {
        let c = Constraint::hard("must be private");
        assert_eq!(c.weight(), 1.0);
        assert!(c.is_hard());
    }

    #[test]
    fn soft_constraint_carries_weight() {
        let c = Constraint::soft("prefer fast delivery", 0.7);
        assert_eq!(c.weight(), 0.7);
        assert!(!c.is_hard());
    }

    #[test]
    fn hard_violation_is_failure() {
        let mut field = ConstraintField::new();
        field.add_hard("must be private");
        field.add_soft("prefer JSON format", 0.3);

        field.constraints[0].violate();
        field.constraints[1].satisfy();

        assert!(field.has_hard_violation());
    }

    #[test]
    fn soft_violation_has_cost() {
        let mut field = ConstraintField::new();
        field.add_soft("prefer fast", 0.7);
        field.add_soft("prefer cheap", 0.4);

        field.constraints[0].violate();
        field.constraints[1].satisfy();

        assert!(!field.has_hard_violation());
        assert!((field.soft_violation_cost() - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn no_violations_when_all_satisfied() {
        let mut field = ConstraintField::new();
        field.add_hard("must be private");
        field.add_soft("prefer fast", 0.5);

        field.constraints[0].satisfy();
        field.constraints[1].satisfy();

        assert!(!field.has_hard_violation());
        assert_eq!(field.soft_violation_cost(), 0.0);
        assert!(field.fully_verified());
    }

    #[test]
    #[should_panic]
    fn soft_weight_out_of_range_panics() {
        Constraint::soft("bad", 1.5);
    }
}
