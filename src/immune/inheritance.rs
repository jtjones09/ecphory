// BASELINE INHERITANCE — Cohen COHEN.3 fold (Spec 6 §7.3.3)
//
// "A retiring cell-agent's accumulated baseline is a fabric node,
//  signed by the retiring cell-agent before retirement. The
//  replacement cell-agent loads this baseline node as its initial
//  state and continues learning from there."
//
// Concretely: each cell-agent serializes its Welford tracker (n,
// mean, m2) into a `BaselineSnapshot`. Replacement cell-agents
// accept an `Option<BaselineSnapshot>` at construction — `Some` for
// inheritance, `None` for fresh bootstrap.
//
// v1 keeps this structurally simple: the snapshot is a typed value;
// bridge wiring will materialize it as a fabric node (with creator
// voice = the retiring cell-agent's voice print) when the bridge's
// retire/replace path comes online.

use super::baseline::WelfordTracker;
use super::specialization::Specialization;
use crate::identity::{NamespaceId, VoicePrint};
use crate::temporal::FabricInstant;

/// Captured baseline state at the moment a cell-agent retires.
#[derive(Debug, Clone)]
pub struct BaselineSnapshot {
    pub region: NamespaceId,
    pub specialization: Specialization,
    pub retiring_voice: VoicePrint,
    pub retired_at: FabricInstant,
    /// Welford state — (n, mean, m2). Reconstructed by the
    /// replacement cell-agent via `WelfordTracker::from_snapshot`.
    pub n: u64,
    pub mean: f64,
    pub m2: f64,
}

impl BaselineSnapshot {
    /// Capture a snapshot for transmission as a fabric node.
    pub fn capture(
        region: NamespaceId,
        specialization: Specialization,
        retiring_voice: VoicePrint,
        tracker: &WelfordTracker,
    ) -> Self {
        let (n, mean, m2) = tracker.snapshot();
        Self {
            region,
            specialization,
            retiring_voice,
            retired_at: FabricInstant::now(),
            n,
            mean,
            m2,
        }
    }

    /// Reconstruct a Welford tracker from the captured snapshot.
    pub fn into_tracker(&self) -> WelfordTracker {
        WelfordTracker::from_snapshot(self.n, self.mean, self.m2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    #[test]
    fn snapshot_roundtrip_preserves_baseline_state() {
        let mut t = WelfordTracker::new();
        for x in [10.0, 12.0, 14.0, 16.0, 18.0, 9.5, 11.0] {
            t.observe(x);
        }
        let original_mean = t.mean();
        let original_var = t.variance();
        let original_n = t.count();

        let voice = generate_agent_keypair().voice_print();
        let snapshot = BaselineSnapshot::capture(
            NamespaceId::fresh("test"),
            Specialization::Rate,
            voice,
            &t,
        );

        // Replacement cell-agent reconstructs the tracker from the
        // snapshot — accumulated wisdom is durable across generations.
        let restored = snapshot.into_tracker();
        assert_eq!(restored.count(), original_n);
        assert!((restored.mean() - original_mean).abs() < 1e-12);
        assert!((restored.variance() - original_var).abs() < 1e-12);
    }

    #[test]
    fn replacement_continues_learning_from_inherited_baseline() {
        let mut original = WelfordTracker::new();
        for x in [10.0; 10] {
            original.observe(x);
        }
        let snapshot = BaselineSnapshot::capture(
            NamespaceId::fresh("test"),
            Specialization::Rate,
            generate_agent_keypair().voice_print(),
            &original,
        );
        let mut replacement = snapshot.into_tracker();
        // The replacement learns one new observation. The mean
        // shifts — but only slightly, because it's averaged against
        // the inherited n=10 history.
        replacement.observe(20.0);
        // Mean of 11 observations: (10*10 + 20)/11 ≈ 10.909
        assert!((replacement.mean() - 10.909_090_909_090_909).abs() < 1e-9);
        assert_eq!(replacement.count(), 11);
    }
}
