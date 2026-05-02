# Immune-System Anomaly Response Runbook

> **Final home:** `homelab/runbooks/immune-anomaly-response.md` (Spec 6 §10 acceptance criterion 9, CANTRILL.3 fold).
> Drafted in this repo because the `homelab` repo isn't on this dev VM at the time of writing — moves verbatim when it is. Cross-referenced from the Spec 6 handoff in nisaba.

This runbook covers operator response to immune-system signals: `AnomalyObservation`, `DamageObservation`, `ConvergedAnomaly`, and auto-`P53Scope::Region` escalations. Required sections per Cantrill C.3 fold (Spec 6 §6.3 v1.1 warning).

---

## 1. Alert response — what just paged

The immune system surfaces anomaly signals via:

- **Prometheus metrics** at `/metrics`:
  - `immune_anomaly_observations_total{specialization, region}` — counter
  - `immune_damage_observations_total{specialization, region}` — counter
  - `immune_convergence_rate` — gauge (anomaly observations per converged anomaly)
  - `immune_response_mode{region, mode}` — gauge (1 for the active mode)
  - `fabric_p53_triggered{scope="Region", region="..."}` — gauge flipped to 1 by an auto-escalation
- **Structured tracing events** with `level=warn` on `fabric::p53_trigger` spans when auto-escalation fires
- **Fabric-resident nodes** with metadata `__bridge_node_kind__` ∈ `{"AnomalyObservation", "DamageObservation", "ConvergedAnomaly"}`

**First step on alert:** identify which signal type fired. Search structured logs for the event name, or query the fabric for the most recent observation node:

```text
# Quick triage
GET /debug/fabric/state                              # global fabric health
GET /debug/fabric/subscriptions                      # cell-agent activity
GET /debug/fabric/node/<reference>                   # specific observation
```

The cognitive map's per-region state vector + recent cluster history give the moment-to-moment context any anomaly needs to be triaged against.

---

## 2. False-positive identification — operational drift vs real attack vs threshold miscalibration

Three things look like an anomaly but require different responses:

### Operational drift

Jeremy reorganized the `nisaba` vault, or a new agent came online, or a region's natural traffic pattern shifted. Per the Matzinger fold, pure-anomaly convergence (no damage among the signals) DOES NOT auto-escalate to P53 — the immune system surfaces the deviation and waits for human review.

**Indicator:** `ConvergedAnomaly` node lands but no `DamageObservation` is among its source observations. `had_damage = false` in the `ConvergedAnomalyRecord`.

**Action:** review the cognitive map's history for this region. If the drift is intentional (tagged change), set the region to `ImmuneResponseMode::AlertOnly` for the duration of the change so further convergence doesn't auto-escalate, then revert to `Active` once the change settles and baselines re-tune.

### Real attack

Damage observations are present (signature failures, content fingerprint failures, impossible causal ordering). At least one `DamageObservation` is among the convergent signals. The aggregation layer auto-escalates to `P53Scope::Region`.

**Indicator:** `EscalateP53Region` fired AND the source observations include `damage_kind` values like `attestation_failed` or `content_fingerprint_failed`.

**Action:** the auto-p53 path is correct. Inspect the forensic archive at the path returned in the `P53Receipt::forensic_archive` field and follow the P53 Region recovery path in `p53-response-runbook.md` §4.2.

### Threshold miscalibration

The anomaly is real but operationally insignificant — the threshold is too tight for the region's actual variance. Baseline was learned during an unrepresentative period.

**Indicator:** convergence fires repeatedly on minor variations; `immune_convergence_rate` very low (high anomalies per ConvergedAnomaly); no `DamageObservation` ever among the signals.

**Action:** propose a threshold change via the semantic edit protocol. The aggregation thresholds (N, T, M, U) are themselves fabric nodes editable through `BridgeFabric::checkout` → `propose` → `finalize_proposal`. Tune the convergence_n, convergence_window, escalation_m, or escalation_window. The immune system observes the tuning operation as a fabric event — audit trail is built in.

---

## 3. Threshold tuning — how to propose a change without consensus deadlock

The aggregation thresholds are stored as fabric nodes in the `hotash:immune:config` namespace. Tuning is a Spec 8 §3.4 semantic edit:

```text
1. fabric.checkout(threshold_node, "tighten convergence_n from 3 to 4 in propmgmt", ttl=15min, signer=operator)
2. fabric.propose(checkout, new_threshold_node_content, signer=operator)
3. fabric.finalize_proposal(proposal, signer=operator)
4. ConsensusSnapshot writes; immune system's ConsensusObserver baselines this as a config-change event.
```

**Avoiding consensus deadlock:** if multiple operators or agents try to tune simultaneously, the SnapshotLock ensures at most one round commits per atomic transition. New checkouts during a snapshot window get `WriteError::SnapshotInProgress` — back off 100ms and retry. Per Spec 8 §3.4.3 the lock window is bounded at 50ms.

**If the threshold change itself is contentious** (e.g., an agent proposes loosening thresholds in a way Jeremy doesn't approve), Nisaba escalates to Jeremy per Spec 8 §3.4.6 v3.1 fold (immune-system thresholds are tagged `requires-human-approval`).

---

## 4. Recovery from auto-p53 — restoring a region killed by false-positive convergence

When `EscalateP53Region` fires on a false-positive convergence (rare — requires both convergence AND a damage signal that turned out to be benign), the region is terminated and forensic-archived per Spec 8 §8.4.2.

**Recovery flow:**

1. **Diagnose first.** Pull the forensic archive at the path in `P53Receipt::forensic_archive`. Inspect each archived node's `content_fingerprint` against its content; if all match, the convergence was truly false-positive. If any mismatch, the damage signal was real.

2. **Switch the region to AlertOnly BEFORE restoring.** Otherwise the same convergence pattern will re-fire on the restored region:
   ```text
   aggregation.set_response_mode(&region, ImmuneResponseMode::AlertOnly)
   ```
   This auto-reverts to `Active` after 4 hours per Spec 6 §5.2.2 CANTRILL.4 fold. If you want longer protection, set `Disabled` (also auto-reverts to AlertOnly after 4 hours).

3. **Provision a fresh region** with a NEW NamespaceId (do not reuse the terminated namespace's UUID — the bridge will refuse writes against a terminated entry forever).

4. **Replay archived nodes** into the new region via `BridgeFabric::create`. Verify each restored node's content_fingerprint.

5. **Re-bootstrap the cell-agent population for the new region:**
   ```text
   immune-bootstrap <new-region-name>
   ```
   The fresh cell-agents start with empty baselines unless `BaselineSnapshot` nodes from the prior population survive (per Spec 6 COHEN.3 fold) — in which case load the snapshot via `BaselineSnapshot::into_tracker` so accumulated wisdom carries forward.

6. **Switch back to Active** once the cell-agents have re-warmed (default warmup: ~5 observation cycles per specialization).

---

## 5. Escalation criteria — when to invoke `P53Scope::Fabric` vs accept region-level damage

Auto-escalation only ever triggers `P53Scope::Region`. Fabric-wide p53 is operator-only per Spec 8 §8.4.3 — it requires `P53Config::fabric_scope_enabled = true` AND the operator's offline key.

**Escalate to Fabric scope when:**

- Multiple regions show `ConvergedAnomaly` with `had_damage = true` simultaneously (the immune system writes a `FabricCompromiseSuspected` node when M regions are concurrently anomalous per Spec 6 §5.2.3 — your alert)
- Forensic inspection of multiple regions shows the same root-cause damage kind (e.g., the same compromised key signed nodes across regions)
- The substrate-level identity primitives themselves are compromised (Spec 5 violations)

**Do NOT escalate to Fabric when:**

- A single region's auto-p53 fired and the damage is contained
- Multiple regions show anomalies but none with `had_damage = true`
- The damage kind is regional (e.g., `propmgmt` extraction errors don't justify killing the whole fabric)

The Fabric-p53 path is defined in `p53-response-runbook.md` §4.3. Recovery requires fresh installation — there is no in-place restart from a Fabric-terminated state.

---

## 6. Cell-agent retirement & misfire handling

Cell-agents that report `CellAgentHealth::Misfiring` (high false-positive rate) or `Retired` are automatically replaced.

**Replacement flow (Spec 6 §7.3.3):**

1. Cell-agent reports `Misfiring` with a `false_positive_rate` over the last sliding window.
2. Bridge captures the cell-agent's current `BaselineSnapshot` (Spec 6 COHEN.3 fold) — the accumulated wisdom is preserved.
3. Replacement cell-agent is provisioned via `bootstrap_region` with the same specialization. The new agent's constructor accepts the inherited `BaselineSnapshot` so it doesn't start from scratch.
4. Retired cell-agent's keypair stays on its observation history but is no longer accepted as an active observer.

Per the KING.3 fold (Spec 6 §7.3.3), observations made by a cell-agent BEFORE it was marked Misfiring still count toward convergence — the messenger isn't punished retroactively.

**Indicator that cell-agent replacement isn't happening:** `immune_cell_agents_total{state="misfiring"}` stays positive for >60 seconds (spec acceptance #8: replacement within 60s).

---

## 7. Quick reference — what each signal means

| Signal | Meaning | Default operator action |
|--------|---------|-------------------------|
| `BaselineHealthy` | Cell-agent is alive; baseline within ±1σ | None — routine, expected |
| `AnomalyObservation` | Cell-agent saw a deviation | None — wait for convergence |
| `DamageObservation` | Cell-agent saw evidence of harm | Investigate — high confidence |
| `ConvergedAnomaly` (no damage) | N specializations agree on deviation | Review cognitive map |
| `ConvergedAnomaly` (with damage) | Convergence + damage | Forensic archive expected, P53Region likely |
| `EscalateP53Region` (Active) | Auto-P53 fired | Recovery per `p53-response-runbook.md` §4.2 |
| `EscalateP53Region` (AlertOnly) | Convergence + damage but no auto-p53 | Manual decision: confirm or accept |
| `FabricCompromiseSuspected` | Multiple regions converging | Operator-only Fabric scope decision |
| `SubscriptionPanic` | Cell-agent or other callback panicked | Cell-agent replacement triggered |

---

## 8. Post-mortem template

Within 7 days of any auto-p53 region escalation OR ImmuneResponseMode::Disabled invocation, file a post-mortem:

```markdown
## Immune-System Post-Mortem — <date> — <region> — <signal>

### What happened
<Which cell-agents fired, what convergence emerged, whether damage
observations were present, what the auto-escalation did.>

### Why it happened
<Operational drift / real attack / threshold miscalibration. Cite
the cognitive-map history and forensic archive evidence.>

### What changed
<Threshold tuning committed via semantic edit, region restored from
archive, cell-agent population adjusted, etc.>

### Spec deviations or weaknesses surfaced
<Did this incident reveal a gap in Spec 6's coverage? File as a
v1.1 fold candidate.>

### Forensic archive disposition
<Path on disk; retention decision.>
```

---

## 9. Related references

- Spec 6 §3 — Cell-agent specifications
- Spec 6 §5 — Aggregation layer
- Spec 6 §7.3 — Operating the immune system
- Spec 5 §5.5 — Mathematical formalization of behavioral trust
- Spec 8 §8.4 — P53 mechanism (the escalation target)
- `p53-response-runbook.md` — Adjacent runbook for the P53 path
