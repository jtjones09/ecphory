// Split / merge / emerge topology helpers (Spec 9 §2.3 + K.S2 fold)
//
// **Split** — when an intent turns out to be N different things, write
// the new child intents and link them to the parent via `SplitFrom`
// edges. Original parent is preserved as provenance; its visibility
// status becomes `WorkStatus::Split`. Evidence produced after the
// split links to whichever child it addresses (caller's job, not
// the helper's).
//
// **Merge** — when two independent intents converge on the same
// problem, link them with mutual `ConvergedWith` edges. Both
// originals are preserved; neither is primary. Caller decides
// whether to also create a new unified WorkIntent for the joint
// SuccessChecks.
//
// **Emerge** — no helper. An agent writing a new WorkIntent into
// the fabric while already claimed on another is "emergence" by
// definition; the WorkObserver (Step 7) is what flags it. No edge
// type is required at write time — the temporal proximity and the
// agent's claim history are the signals.

use crate::bridge::BridgeFabric;
use crate::context::RelationshipKind;
use crate::fabric::FabricError;
use crate::signature::LineageId;

/// Wire `child` → `parent` via a `SplitFrom` edge. The parent's
/// visibility-query status will reflect this on the next query.
pub fn split_intent(
    bridge: &BridgeFabric,
    parent: &LineageId,
    child: &LineageId,
) -> Result<(), FabricError> {
    bridge.add_edge(child, parent, 1.0, RelationshipKind::SplitFrom)
}

/// Wire mutual `ConvergedWith` edges between two intents that
/// converged on the same problem. Both originals are preserved;
/// neither is primary.
pub fn merge_intents(
    bridge: &BridgeFabric,
    a: &LineageId,
    b: &LineageId,
) -> Result<(), FabricError> {
    bridge.add_edge(a, b, 1.0, RelationshipKind::ConvergedWith)?;
    bridge.add_edge(b, a, 1.0, RelationshipKind::ConvergedWith)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::message::Urgency;
    use crate::fabric::Fabric;
    use crate::identity::generate_agent_keypair;
    use crate::work::intent::{IntentDuration, WorkIntent};
    use crate::work::{hotash_work, query_visibility, VisibilityConfig, WorkStatus};

    fn intent(description: &str) -> WorkIntent {
        WorkIntent {
            description: description.into(),
            intended_outcome: "ok".into(),
            constraints: vec![],
            success_checks: vec![],
            requested_by: generate_agent_keypair().voice_print(),
            urgency: Urgency::Normal,
            context: vec![],
            duration: IntentDuration::Discrete { deadline: None },
        }
    }

    #[test]
    fn split_marks_parent_as_split_and_keeps_children_separate() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let parent_id = fabric
            .create(
                intent("publish session 21 capture").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let child_a = fabric
            .create(
                intent("write narrative section").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let child_b = fabric
            .create(
                intent("ship code review").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        fabric
            .add_edge(&child_a, &parent_id, 1.0, RelationshipKind::SplitFrom)
            .unwrap();
        fabric
            .add_edge(&child_b, &parent_id, 1.0, RelationshipKind::SplitFrom)
            .unwrap();

        let bridge = BridgeFabric::wrap(fabric);
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());

        assert_eq!(snap.split.len(), 1, "snapshot: {:?}", snap);
        assert_eq!(snap.split[0].lineage_id, parent_id);
        assert_eq!(snap.split[0].status, WorkStatus::Split);
        // Children land in their own categories (Unstarted by topology).
        assert_eq!(snap.unstarted.len(), 2);
    }

    #[test]
    fn merge_writes_mutual_converged_with_edges() {
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let a = fabric
            .create(
                intent("approach A").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let b = fabric
            .create(
                intent("approach B").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();

        let bridge = BridgeFabric::wrap(fabric);
        merge_intents(&bridge, &a, &b).unwrap();

        // Both directions exist.
        bridge.read_inner(|inner| {
            let from_a = inner.edges_from(&a);
            assert!(from_a
                .iter()
                .any(|e| e.target == b && matches!(e.kind, RelationshipKind::ConvergedWith)));
            let from_b = inner.edges_from(&b);
            assert!(from_b
                .iter()
                .any(|e| e.target == a && matches!(e.kind, RelationshipKind::ConvergedWith)));
        });

        // Neither is split; both are still discoverable in their
        // topology-only category (Unstarted).
        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.split.len(), 0);
        assert_eq!(snap.unstarted.len(), 2);
    }

    #[test]
    fn split_helper_via_bridge_lands_correctly() {
        // Same as the first test, but using the helper rather than
        // raw add_edge calls.
        let mut fabric = Fabric::new();
        let creator = generate_agent_keypair();
        let parent_id = fabric
            .create(
                intent("parent").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();
        let child = fabric
            .create(
                intent("child").to_intent_node(creator.voice_print()),
                &hotash_work(),
                None,
            )
            .unwrap();

        let bridge = BridgeFabric::wrap(fabric);
        split_intent(&bridge, &parent_id, &child).unwrap();

        let snap = query_visibility(&bridge, &VisibilityConfig::default(), |_| Vec::new());
        assert_eq!(snap.split.len(), 1);
        assert_eq!(snap.split[0].lineage_id, parent_id);
    }
}
