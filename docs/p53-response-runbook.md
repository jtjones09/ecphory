# P53 Response Runbook

> **Final home:** `homelab/runbooks/p53-response.md` (Spec 8 §11 acceptance criterion #13).
> Drafted in this repo because the `homelab` repo is not yet present on Enki at the time of writing — when it is, this file moves verbatim. Cross-referenced from `projects/ecphory/handoffs/handoff-cc-spec-8-bridge.md` in nisaba.

This runbook covers operator response to a P53 trigger across the three scopes defined in Spec 8 §8.4 (Node / Region / Fabric). Required sections per Spec 8 §8.5 v3.1 fold (Cantrill C.1).

---

## 1. Alert response — what just paged

A P53 event reaches the operator through one or more of:

- **Structured log line** with `level=warn` or higher and one of the event names: `P53NodeTerminated`, `RegionDying`, `P53RegionTerminated`, `FabricDying`, `P53FabricTerminated`. Search logs for the literal event name to scope the incident.
- **Prometheus gauge** `fabric_p53_triggered{scope, region}` flipped to `1`. The gauge is set at trigger time and persists for the life of the fabric instance — `scope` label is `Node` / `Region` / `Fabric`, `region` is the region name when applicable.
- **Subscription panic events** can also masquerade as p53-adjacent if they trip an immune-system reaction. These appear as fabric-resident nodes with `__bridge_node_kind__ = "SubscriptionPanic"`.

**First step on alert:** confirm the alert is real by hitting `/debug/fabric/p53/status` (admin token required, 1 hour Ed25519-signed by the operator key per Spec 8 §8.5.3). If `Node` scope only and no other anomalies, this is routine — proceed to §4.1. If `Region` or `Fabric` scope, escalate immediately.

---

## 2. Scope identification

Pull the structured log entry for the trigger and read the `scope` field on the `fabric::p53_trigger` span:

| Scope    | Trigger signature                                                                          | Operator action |
|----------|--------------------------------------------------------------------------------------------|------|
| `Node`   | `P53NodeTerminated` event written; target node faded; no other writes affected.            | None required — routine maintenance. |
| `Region` | `RegionDying` then `P53RegionTerminated`; forensic archive path returned in receipt; further writes to that namespace return `WriteError::FabricDegraded`. | Page operator. Inspect archive. Decide recovery vs accept. |
| `Fabric` | `FabricDying` then `P53FabricTerminated`; the `fabric_terminated` flag flipped; ALL writes return `WriteError::FabricDegraded`. | Operator-only event — should not fire without explicit offline-key trigger. Treat as catastrophic. |

If you see `P53Scope::Fabric` in production WITHOUT having authorized it via the offline operator key, that is itself an incident: investigate how the trigger reached the fabric.

---

## 3. False-positive identification

Region p53 can fire on automated immune-system signals (when Spec 6 lands). Before committing to recovery, check whether the trigger is a true positive:

- **Inspect the dying event's signal payload.** The `RegionDying` node's metadata records why the region was dying. For v1, `__bridge_p53_scope__` and `__bridge_p53_target__` are present; v2 will add the immune-system signal vector.
- **Cross-check fabric writes against the immune system's baseline.** If the region's recent write rate, attestation failure rate, or topology distance from baseline are within ±2σ, the trigger is suspicious.
- **Confirm with a second observer.** Read the same archive timestamp via a fresh `BridgeFabric::debug_node` snapshot from a different operator session; if the picture differs, suspect transport or storage corruption.

A false-positive Region p53 is **not** undoable by the runbook — the region is terminated, archived, and refusing writes by the time you read this. Recovery is the same procedure as a true positive, but with a different post-mortem.

---

## 4. Recovery procedures

### 4.1 P53Scope::Node — accept the loss or re-attest

Routine. The single node has self-terminated; the immune system observed it as a normal `P53NodeTerminated` event. No operator action is required unless the same node identity (lineage_id) shows up multiple times in your alert stream — that pattern indicates a deeper bug, file an issue.

If the node was load-bearing for downstream readers (rare — append-only nodes mean readers should observe the absence as a soft signal, not a hard miss), re-create equivalent content via the normal `BridgeFabric::create` path. The new node has a fresh lineage_id; downstream subscribers receive it through the dispatch pool as a normal write.

### 4.2 P53Scope::Region — restore from forensic archive or accept loss

The region is terminated and refusing writes. Two paths:

**Path A — restore.** Read the JSONL archive at the path recorded in the `P53Receipt::forensic_archive` field of the trigger's structured log. Each line is one node with: `lineage_id`, `want`, `content_fingerprint`, `creator_voice`, `edges_out`. If you decide to restore:

1. Provision a fresh region with a new `NamespaceId` (do NOT reuse the terminated namespace's UUID — the bridge will refuse writes against the terminated entry forever).
2. Re-create each archived node via `BridgeFabric::create` in the new region. Verify each restored node's `content_fingerprint` matches the archive entry — any mismatch indicates corruption between the snapshot moment and now.
3. Re-create edges using the archived `edges_out` records. Edge identity is recomputed against the new lineage_ids; the original archive's edge UUIDs are reference-only.
4. After restore, write a `RegionRestored` audit node into nisaba (or the equivalent project log) recording archive path, restore timestamp, and the new namespace UUID. The immune system will see this node and bias its baseline on the restored region accordingly.

**Path B — accept.** If the region's data was non-load-bearing (e.g., an experiment), do nothing further. The forensic archive remains on disk for retention; consider compressing and rotating to long-term storage once your retention window has passed.

### 4.3 P53Scope::Fabric — fresh install

Recovery requires a fresh fabric installation:

1. **Verify the trigger.** Read the operator's offline-key signature on the `P53FabricTerminated` event. If the signature doesn't match a known operator key, the fabric was compromised — STOP and escalate to the security incident process before any recovery action.
2. **Snapshot the forensic archive.** The `archive_root/_fabric_/<timestamp>/region.jsonl` file contains every node + edge in the fabric at termination time. Copy it off the host before starting recovery.
3. **Provision a fresh fabric instance.** Per Spec 5 §4 this requires a new genesis tuple — instance public key, witness commitment, state root, optional lineage parent. The new fabric's lineage_parent should point to the terminated fabric's genesis tuple, preserving the species tree (Spec 5 §2.4).
4. **Selectively import.** Replay archive nodes via `BridgeFabric::create` in the new fabric. Spec 8 §11 acceptance criterion #9 says fabric-scope p53 forces "fresh install with archive import" — this is that import. The bridge does not provide an automatic importer in v1; the operator runs a one-off Rust binary that streams the JSONL through `create()`.
5. **Audit the import.** After import, run `BridgeFabric::verify_all_content_fingerprints` (already exists per Spec 5 §3.1 boot-time check) to confirm every imported node's fingerprint matches its content. Any mismatches must be triaged before the fabric is opened to agents.

---

## 5. Post-mortem template

Within 7 days of any Region or Fabric trigger, file a post-mortem in nisaba's `JOURNAL.md` and (if it represents a recurring failure mode) update `PINNED.md`. Use this structure:

```markdown
## P53 Post-Mortem — <date> — <scope> — <region or 'fabric'>

### What happened
<1-2 paragraphs: what triggered, what the operator observed, what
recovery was chosen.>

### Why it happened
<Root cause. If this was an immune-system trigger, name which
specialization (Rate / Attestation / Decay / Consensus / Relation /
Silence per Spec 6 §3.3) fired and what threshold was crossed. If
this was a manual trigger, name the operator and the underlying
incident.>

### What changed in the fabric or its operating procedure
<Spec / config / threshold / runbook / monitoring updates committed
as a result. If nothing changed, justify why ('this was expected
routine maintenance').>

### What we'd do differently next time
<Honest assessment. Surfaces drift in the recovery procedure faster
than waiting for the next incident.>

### Forensic archive disposition
<Path of the archive on disk; retention decision; whether content
was migrated to long-term storage or left in place.>
```

---

## 6. Quick reference

| Event                       | Scope    | Affects                       | Operator action |
|-----------------------------|----------|-------------------------------|----|
| `P53NodeTerminated`         | Node     | One node faded                | None — routine |
| `RegionDying`               | Region   | Subscription drain begins     | Watch logs |
| `P53RegionTerminated`       | Region   | Region writes refused; archived | §4.2 Path A or B |
| `FabricDying`               | Fabric   | Subscription drain begins     | Confirm authorized; otherwise escalate |
| `P53FabricTerminated`       | Fabric   | All writes refused; archived  | §4.3 fresh install |
| `SubscriptionPanic`         | Subscription | Single subscription marked panicked | Inspect `subscription_id`; consider unsubscribing |

---

## 7. Related references

- Spec 8 §8.4 — P53 mechanism specification (`projects/ecphory/specs/spec-8-phase-f-bridge.md` in nisaba)
- Spec 8 §8.5 — Observability surface (the structured log fields and metric names this runbook depends on)
- Spec 5 §3.1 — Content fingerprint invariant (used by step 4.3.5 audit)
- Spec 6 — Immune system (when it lands, the `RegionDying` payload will carry the cell-agent specialization signals this runbook describes in §3)

This runbook updates whenever any of the above specs are revised. Major-version spec changes require a runbook revision alongside.
