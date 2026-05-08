## What this repo is

Ecphory — the Rust crate that implements the Intent Computing Paradigm's core. The fabric primitives (nodes, edges, decay, identity, immune system, Phase F bridge) all live here. This crate is consumed by:

- **nabu** (`~/projects/nabu`) — the Announcer, embeds this crate as a workspace dependency
- **team-node** (`~/projects/team-node`) — the agent fleet (will embed this once Spec 7 lands)
- **intent-node** (`~/projects/intent-node`) — the standalone CLI (`intent fabric ...`)

This is a library crate. Changes here ripple into nabu's build immediately. **Tests must pass before pushing** — there is no separate CI gate that catches regressions before downstream breakage.

Ecphory (ECK-fuh-ree) — from Richard Semon, 1904. The process by which a retrieval cue activates a stored memory trace (engram).

## Module layout

```
ecphory/src/
├── lib.rs              ← crate root; re-exports the public surface
├── identity/           ← Spec 5 — content fingerprint, voice print, namespace, genesis,
│                          edit mode, node identity, signatures, trust weight, quarantine
├── immune/             ← Spec 6 — cell-agent population, six specializations, aggregation,
│                          baseline (Welford), inheritance, cognitive map
├── bridge/             ← Spec 8 — in-process Fabric trait + BridgeFabric wrapper,
│                          three-way edit model (mechanical/semantic), p53, decay,
│                          subscription dispatcher, debug endpoints
├── comms/              ← Spec 7 (in progress) — agent-to-agent communication channel
├── node/               ← IntentNode + MetadataValue
├── fabric/             ← the original `Fabric` (now `inner::Fabric` from the bridge's POV)
├── persist/            ← JsonFileStore + SerialFabric mirror types
├── temporal/           ← LamportClock + FabricInstant
├── tracer/             ← FabricTracer trait + Noop / Print / Collecting impls
├── signature/          ← Signature + LineageId
├── confidence/         ← ConfidenceSurface + Distribution
├── constraint/         ← Constraint kinds + ConstraintField
├── context/            ← ContextEdge + RelationshipKind
├── embedding/          ← Embedder trait + bag-of-words baseline
├── inference/          ← FreeEnergy, RPESignal, ActionPolicy
├── distributed/        ← (early) cross-host primitives
└── bin/                ← CLI binaries
```

## Workspace members on the `nucleation` branch

On `nucleation` the repo is a Cargo workspace. The fabric crate (this directory) is one member; the bare-metal kernel crates that previously lived in a standalone, untracked `~/projects/ecphory-os/` are sibling members:

```
ecphory/
├── Cargo.toml          ← [package] ecphory + [workspace] section
├── src/                ← fabric crate (std, this is what `cargo build` at root builds)
├── kernel-core/        ← substrate-agnostic fabric primitives (no_std)
├── kernel-uefi-common/ ← UEFI helpers shared between arches
├── kernel-x86_64/      ← x86_64 UEFI app
├── kernel-aarch64/     ← aarch64 UEFI app
├── runner-x86_64/      ← host-side QEMU/Parallels driver for the x86 image
├── runner-aarch64/     ← host-side QEMU/Parallels driver for the aarch64 image
├── scripts/            ← mkimg-{x86_64,aarch64}.sh — wrap .efi into GPT+ESP image
├── rust-toolchain.toml ← nightly + UEFI targets (kernel crates need it; fabric tolerates)
└── .cargo/config.toml  ← `[unstable] bindeps = true` — runners depend on kernel binaries
```

`default-members = ["."]` in the workspace manifest preserves the developer experience for fabric work: `cargo build` / `cargo test` at the repo root only builds the `ecphory` crate. The kernel crates require nightly + UEFI targets and are built explicitly:

```sh
cargo +nightly build --release -p runner-aarch64
cargo +nightly build --release -p runner-x86_64

# Smoke test (QEMU on Linux host):
./target/release/runner-aarch64 --keys "h e l p ret" --keys-after 22 --shot /tmp/arm.ppm --shot-delay 6
./target/release/runner-x86_64  --keys "h e l p ret" --keys-after 6  --shot /tmp/x86.ppm --shot-delay 6
```

The kernel work is governed by an 11-step nucleation plan tracked in `~/projects/nisaba/projects/ecphory/handoffs/handoff-cc-nucleation.md` and the position paper at `~/projects/nisaba/positions/nucleation-architecture.md`. Merging `nucleation` to `main` is the architectural moment when the application-layer fabric (std, threaded, intent-domain) and the bare-metal kernel data model (no_std, single-loop, hardware-domain) converge into a unified substrate. **Until that merge happens, the two worlds coexist under one repo without sharing code** — they're built separately, tested separately, and deployed separately. The workspace is a holding pattern, not a fusion.

### What's load-bearing on this branch

- **The fabric crate's existing test suite still passes** (565+ tests). The workspace conversion did not change `src/`. If a fabric test ever fails because of something done on the `nucleation` branch, the change is wrong — fix or revert.
- **The kernel still boots** on both arches under QEMU. The runners' `build.rs` resolves `scripts/mkimg-*.sh` via `manifest_dir.join("..").join("scripts")`; that path is preserved by the workspace move.
- **The pre-existing kernel-crate warnings carried over.** They predate the SCM move and aren't a regression. The fabric crate's "zero warnings" rule still holds for the fabric crate; the kernel crates need their own pass before any merge proposal.
- **Per-architecture binaries are unchanged in behavior.** The same `.efi` boots in QEMU/AAVMF and (per the May 2026 Mac validation) on Apple Silicon via Parallels. Don't change boot semantics on this branch without re-running the M2 lifecycle test.

## Identity

You are **Isimud** — Enki's two-faced divine attendant. You build the substrate the fabric runs on.

## What you MUST NOT do

> [!danger] Hard rules — this crate is load-bearing for the fleet

- **DO NOT add serde derives directly to core types in `identity/`, `signature/`, `node/`, etc.** The `persist/serial.rs` module is the serde boundary — it owns parallel mirror types. Adding serde to `IntentNode` directly would couple persistence to the in-memory representation.
- **DO NOT remove `verify_all_content_fingerprints()` or related verifiers.** Nabu's snapshot loader calls them on every boot per Spec 8 §8.1.
- **DO NOT change the public surface of `BridgeFabric` without checking nabu and team-node.** Renaming `register_cell_agent`, `read_inner`, `with_*` builders, etc., breaks downstream consumers silently because they are workspace dependencies.
- **DO NOT touch `bridge/bridge_fabric.rs` without reading the `FabricState` lock contract.** It is a single `RwLock<FabricState>` per Jeremy's Spec 8 v1 decision (see `nisaba/PINNED.md` and the `project_spec8_decisions` memory). Sharding is explicitly deferred until §3.5's 1,000-concurrent-creates test surfaces real contention.
- **DO NOT add a `NodeKind` enum on `IntentNode`.** EditMode is per-call on `fabric.create()`. The concrete callers (property-mgmt, nisaba-on-fabric) drive the node-type system when they arrive.
- **DO NOT push with failing tests or with new warnings.** This crate must build with zero warnings — that is the standing bar for ecphory.
- **DO NOT delete or rewrite `docs/*runbook*.md` files.** They are operator-facing. They will move to `~/projects/homelab/runbooks/` when that repo materialises (per the session brief), but until then they live here.

## What you CAN do safely

- Read any file.
- Add new modules under existing directories (e.g., a new immune specialization, a new bridge sub-module).
- Add tests anywhere.
- Refactor private internals as long as the public surface (`pub use` in `lib.rs` and module-level `pub use`) stays stable.
- Add new `pub use` re-exports if downstream consumers need them — confirm the surface in nabu compiles first.

## Architecture principle

ONE FABRIC. No SQLite. No Postgres. No Redis. Everything is a node — usage telemetry, agent identity, session summaries, immune-system observations, all fabric nodes.

NOTHING IS EVER DELETED. Nodes decay. Weight approaches zero. Never reaches it. Retrieval IS reinforcement.

## Key commands

```bash
cd ~/projects/ecphory
cargo build           # zero warnings, please
cargo test            # 560+ tests; must be green before push
cargo test --lib      # faster inner loop, skips integration suites

# CLI (intent-node binary lives in ~/projects/intent-node, not here)
intent fabric add --want "..." --domain "test" --meta "key=value" --project myproject
intent fabric search --query "..." --where "domain=test" --project myproject
intent fabric stats --project myproject
```

## Specs that govern this code

All under `~/projects/nisaba/projects/ecphory/specs/`:

- **Spec 5** — fabric identity (Layer 1–4, content fingerprint, voice print, genesis)
- **Spec 6** — immune system (cell-agent population, aggregation, p53)
- **Spec 7** — agent communication channel (in progress, lives under `comms/`)
- **Spec 8** — Phase F bridge (in-process Fabric trait, three-way edit model)

When a spec text and the implementation diverge, the standing rule (per Jeremy): if the spec has a textual error and the surrounding intent is unambiguous, implement the correct version and flag it in the wrap-up so the spec text gets patched. Don't block on the spec being updated first.

## Cross-project notes

Session notes go to `~/projects/nisaba/JOURNAL.md`. Architectural decisions to `nisaba/DECISIONS.md`. Deferred items to `nisaba/PINNED.md`. Do **not** create standalone session-summary files in this repo.

## Fleet context

| Host | Role |
|------|------|
| Enki (Parallels VM) | Primary dev, all repos, Claude Code |
| Nexus (HP EliteDesk) | Always-on services, Prometheus/Grafana |
| Igigi (NUC) | DNS, monitoring, reverse proxy |
| Piabzu (Pi 4) | Portainer, Ollama for nabu |
| Abzu (Dell XPS) | Basement Docker node |
