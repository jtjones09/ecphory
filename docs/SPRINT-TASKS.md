# TASKS.md — Ecphory Sprint: Unified Fabric + Self-Monitoring
# For Claude Code execution on intent-node, ecphory, and team-node repos
# Priority: TOP → BOTTOM. Each task is self-contained.
# Expected execution: CC on Opus, one task at a time, git pull before each.

---

## TASK 1: Structured Metadata on Intent Nodes (ecphory repo)
**Repo:** ecphory (~/projects/ecphory)
**Branch:** main
**Priority:** CRITICAL — everything else depends on this

The IntentNode's `constraints` field currently holds semantic constraints.
Add a `metadata` field to IntentNode that holds arbitrary typed key-value pairs.
This is NOT a new node type — it's an extension of every node.

### What to implement:
1. Add `metadata: HashMap<String, MetadataValue>` to IntentNode
2. MetadataValue enum: `String(String)`, `Float(f64)`, `Int(i64)`, `Bool(bool)`
3. Serialization: metadata serializes as JSON object in fabric.json
4. CLI: `intent fabric add --want "..." --meta "key=value" --meta "cost=0.42"`
5. CLI parsing: auto-detect type (numbers → Float/Int, true/false → Bool, else String)
6. Update fabric.json reader/writer to handle metadata field
7. Backward compatible: existing fabric.json files without metadata field still load

### Tests:
- Add node with metadata, verify it persists in fabric.json
- Add node without metadata, verify backward compatibility
- Verify type detection: "42" → Int, "3.14" → Float, "true" → Bool, "hello" → String
- Search still works with metadata-bearing nodes
- List output includes metadata when present

### Do NOT:
- Change the signature computation (metadata is mutable, signature is intrinsic)
- Change the want, constraints, or confidence fields
- Add metadata to the signature hash

---

## TASK 2: Predicate Filtering on Fabric Search (ecphory repo)
**Repo:** ecphory (~/projects/ecphory)
**Branch:** main
**Priority:** CRITICAL — enables usage tracking as fabric nodes
**Depends on:** Task 1

Add `--where` flag to `intent fabric search` that filters results by metadata predicates.

### What to implement:
1. `intent fabric search --query "api calls" --where "domain=marketing"`
2. `intent fabric search --query "usage" --where "cost>0.1"`
3. `intent fabric search --where "project=reallycoons"` (no semantic query, pure filter)
4. Support operators: `=`, `!=`, `>`, `<`, `>=`, `<=`
5. Support AND: `--where "domain=marketing AND cost>0.1"`
6. Results still sorted by resonance score when --query is present
7. When only --where is present (no --query), return all matching nodes sorted by recency

### Predicate parser:
- Simple: split on " AND ", parse each as "key operator value"
- Type-aware comparison: compare Float to Float, String to String, etc.
- Missing metadata key = node doesn't match (skip, don't error)

### New CLI command: `intent fabric aggregate`
1. `intent fabric aggregate --field cost --op sum --where "project=reallycoons"`
2. `intent fabric aggregate --field cost --op sum --group-by domain`
3. `intent fabric aggregate --field tokens --op avg --where "model=sonnet"`
4. Operations: sum, avg, min, max, count
5. Output as JSON: `{"result": 4.23}` or `{"groups": [{"domain": "marketing", "result": 2.10}, ...]}`

### Tests:
- Filter by string equality
- Filter by numeric comparison (> < >= <=)
- Filter with AND
- Aggregate sum, avg, min, max, count
- Aggregate with group-by
- Empty results return zero/empty, not error
- Mixed: semantic query + predicate filter

---

## TASK 3: Usage Events as Fabric Nodes (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** HIGH — replaces SQLite tracker
**Depends on:** Tasks 1 and 2

Replace the SQLite usage tracker with fabric-native usage nodes.
Every API call becomes a node in the fabric.

### What to implement:
1. New file: `tracking/fabric_tracker.py`
2. Uses FabricBridge to store usage events as nodes
3. Node format:
   - want: "API call: {agent} used {model} for {operation}"
   - domain: "system_telemetry"
   - metadata: project, domain, agent, model, tier, operation, input_tokens, output_tokens, total_tokens, cost_usd, duration_ms, success, timestamp
4. Query method: uses `intent fabric search --where "domain=system_telemetry AND project=reallycoons"`
5. Summary method: uses `intent fabric aggregate --field cost_usd --op sum --group-by model --where "domain=system_telemetry"`
6. Delete tracking/usage_tracker.py (SQLite version)
7. Delete tracking/crew_callbacks.py (rewrite to use fabric tracker)
8. Update main.py to use fabric tracker instead of SQLite
9. Update main.py --usage commands to query fabric

### CLI commands should still work:
- `python main.py --usage` → calls fabric aggregate, renders table
- `python main.py --usage --project reallycoons` → filtered
- `python main.py --usage --usage-group domain` → grouped
- `python main.py --usage-export` → calls fabric search with where filter, dumps JSON

### Tests:
- Log an event, verify it appears as a fabric node
- Query events by project
- Aggregate cost by model
- Verify old SQLite code is fully removed

---

## TASK 4: Dynamic Model Router with Fabric Self-Awareness (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** HIGH
**Depends on:** Task 3

The model router should check the fabric for recent usage patterns
and adjust its routing based on accumulated cost data.

### What to implement:
1. Update routing/model_router.py
2. Before routing, query fabric: `intent fabric aggregate --field cost_usd --op sum --where "project={project} AND domain=system_telemetry"`
3. If project has spent > $2 today, bias toward FAST tier (Haiku)
4. If project has spent < $0.50 today, allow PREMIUM tier
5. This is the first self-referential reasoning: the system reads its own telemetry from the fabric and adjusts behavior
6. Log the routing decision itself as a fabric node with metadata: {decision: "standard", reason: "...", complexity_score: 42}
7. Print routing decisions with cost context at startup

### Key principle:
The system is now reading its own operational history from the same fabric
where it stores knowledge. The metadata wall is down.

### Tests:
- Router with no prior usage → routes normally
- Router with high prior cost → biases toward FAST
- Routing decision is stored as a node
- Routing decision is retrievable by future queries

---

## TASK 5: Voice Constraints Optimization (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** MEDIUM

Only inject JEREMY_VOICE constraints into agents that produce human-facing content.

### What to implement:
1. Marketing agent: keeps voice constraints (produces LinkedIn content, client-facing writing)
2. Sales agent: keeps voice constraints (produces outreach, proposals)
3. Engineer agent: REMOVE voice constraints (produces code, technical analysis)
4. Architect agent: REMOVE voice constraints (produces specs, diagrams)
5. Security agent: REMOVE voice constraints (produces audits, reports)
6. Data Analytics agent: REMOVE voice constraints (produces analysis)
7. Planner/Researcher: REMOVE voice constraints (produces internal research briefings)

### Savings:
~350 tokens removed from 5 of 7 agent backstories = ~1,750 fewer tokens per run
in the LLM context window. This is pure savings with zero quality impact on
the agents that don't write human-facing content.

---

## TASK 6: Fabric Viewer v2 — Live from CLI (ecphory repo)
**Repo:** ecphory (~/projects/ecphory, place in docs/)
**Branch:** main
**Priority:** MEDIUM

Update the fabric-viewer.html to:
1. Add a "Load from CLI" button that shows instructions for piping fabric data
2. Add support for the new metadata field in node display
3. Show metadata as pills/tags on each node card (cost: $0.42, model: sonnet, etc.)
4. Add a "Telemetry" filter button that shows only domain=system_telemetry nodes
5. Add a simple cost summary bar at the top when telemetry nodes are present
6. Maintain the same Ecphory glass/ambient aesthetic

---

## TASK 7: Spec Section — Structured Metadata Queries (intent-node repo)
**Repo:** intent-node (~/projects/intent-node, docs/)
**Branch:** master
**Priority:** MEDIUM

Write a new section for the spec (to be merged into v0.3):
`docs/spec-section-structured-queries.md`

### Content:
1. Title: "Structured Metadata and Predicate Queries"
2. Motivation: The fabric must store ALL system data — knowledge, decisions, AND operational telemetry. No separate databases.
3. Biological grounding: interoception (Seth & Friston 2016), insular cortex integration, Barrett's constructed emotion theory
4. Active Inference mandate: separating self-monitoring from world-modeling increases free energy
5. The metadata wall: why SOAR/ACT-R kept it, why we're tearing it down
6. Technical spec: metadata field on IntentNode, predicate filtering, aggregation
7. Interpretation layer: CQRS pattern, schema-on-read, materialized views as disposable simulations
8. Open questions: self-referential reasoning stability, associative memory with structured properties, optimal view materialization
9. References: cite the deep research findings (Seth & Friston, Barrett, Tulving, Tversky & Kahneman, Tresp Tensor Brain, Global Workspace Theory, PlugMem, CQRS)

### Voice: Spec voice. Technical, precise, cited. Not marketing. Not conversational.

---

## TASK 8: Planner Fabric-First Behavior (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** MEDIUM

Update the Planner/Researcher agent to truly check fabric before doing ANY external work.

### What to implement:
1. Update planner_researcher.py backstory with stronger fabric-first instructions
2. Add a "freshness" concept: if a fabric node about the same topic exists and is less than 24 hours old, USE IT
3. Add timestamp to node metadata (the fabric doesn't have timestamps as a field, so store as metadata: timestamp=2026-03-23T01:00:00Z)
4. Planner should call search_memory FIRST, examine results, and ONLY call fetch_url or web_search if fabric results are stale or missing
5. This prevents the duplicate site analysis problem (4 nearly identical nodes)

### Tests:
- Run with empty fabric → Planner fetches and stores
- Run again → Planner finds existing analysis and skips fetch
- Run 25 hours later → Planner re-fetches (stale data)

---

## EXECUTION ORDER:
1. Task 1 (metadata on nodes) — everything depends on this
2. Task 2 (predicate filtering) — enables usage tracking
3. Task 3 (usage as fabric nodes) — tears down the metadata wall
4. Task 4 (self-aware router) — first self-referential reasoning
5. Task 5 (voice optimization) — quick win, saves tokens
6. Task 7 (spec section) — documents what we built
7. Task 8 (planner fabric-first) — prevents duplicate work
8. Task 6 (viewer v2) — visual payoff

---

## TASK 9: Weight Decay — Nodes Fade, Never Die (ecphory repo)
**Repo:** ecphory (~/projects/ecphory)
**Branch:** main
**Priority:** HIGH — foundation for continuity
**Depends on:** Task 1

Nothing is ever deleted from the fabric. Nodes decay.
This is how biological memory works — nothing is pruned,
everything just gets harder to activate.

### What to implement:
1. Add `last_activated: String` (ISO timestamp) to node metadata — set on creation and updated on every retrieval/search hit
2. Add `activation_count: i64` to metadata — incremented every time the node is returned in a search result or explicitly accessed
3. Add `composite_weight: f64` to node — computed, not stored. Calculated at query time from:
   - confidence.comprehension (existing field)
   - temporal_recency: exponential decay from last_activated (half-life configurable, default 7 days)
   - activation_frequency: log(activation_count + 1)
   - resonance_score: the search match score (when searching)
4. Formula: `composite_weight = (comprehension * 0.3) + (temporal_recency * 0.3) + (activation_frequency * 0.2) + (resonance_score * 0.2)`
5. Weights are configurable via `--decay-halflife` flag on search
6. Search results sorted by composite_weight, not just resonance score
7. `intent fabric search` now updates last_activated and activation_count on every returned node (the act of retrieving strengthens the memory)
8. `intent fabric stats` command: shows node count, weight distribution histogram, oldest node, most activated node, least activated node

### Key principle:
Retrieval IS reinforcement. Every time a node is found by search, it gets
stronger. Nodes that are never retrieved slowly fade. A strong signal in
ANY dimension can reactivate a faded node — just like a childhood smell.

### Do NOT:
- Delete any nodes ever
- Add a "prune" or "cleanup" command
- Set any floor on composite_weight (let it approach zero asymptotically)

### Tests:
- New node has composite_weight based on initial confidence + recency
- Node retrieved multiple times has higher activation_count and weight
- Node not retrieved for 14 days has lower temporal_recency component
- Node with low recency but high activation_count still has meaningful weight
- Very old node with zero resonance match but exact keyword hit still returns (faded but findable)
- Stats command shows weight distribution

---

## TASK 10: Systemic Intent Nodes — The Fabric's Own Goals (ecphory repo)
**Repo:** ecphory (~/projects/ecphory)
**Branch:** main
**Priority:** HIGH — the fabric starts having its own goals
**Depends on:** Tasks 1, 2, 9

The spec (section 3) defines Systemic Intents as nodes that belong to
the fabric itself. "Maintain coherence." "Learn from execution."
These are the fabric's own metabolism. Time to implement them.

### What to implement:
1. On first initialization of a new project fabric, seed these systemic nodes:
   - want: "Maintain fabric coherence — no duplicate knowledge, no contradictions"
     domain: "system", metadata: {kind: "systemic", category: "coherence"}
   - want: "Track all operations — every API call, every tool use, every decision"
     domain: "system_telemetry", metadata: {kind: "systemic", category: "self_monitoring"}
   - want: "Strengthen high-value knowledge — frequently accessed nodes should be easy to find"
     domain: "system", metadata: {kind: "systemic", category: "optimization"}
   - want: "Identify knowledge gaps — notice when queries return low-confidence results"
     domain: "system", metadata: {kind: "systemic", category: "growth"}
   - want: "Preserve context across sessions — key decisions and reasoning should persist"
     domain: "system", metadata: {kind: "systemic", category: "continuity"}
2. Systemic nodes have metadata: `{kind: "systemic"}` — they are distinguishable from regular nodes
3. Systemic nodes start with high confidence (0.9) and never decay below 0.5 weight (they are innate, like skin)
4. `intent fabric list --systemic` shows only systemic nodes
5. Systemic nodes are created ONCE per project — if they already exist, don't duplicate

### Key principle:
These are the immune system's innate layer from spec section 4.2.
They exist before any threat or query arrives. They exert constant force.
Later tasks will make these nodes actually DO things — for now they exist
as declarations of the fabric's own intent.

### Tests:
- New project gets systemic nodes on first use
- Existing project doesn't get duplicates
- Systemic nodes have minimum weight floor
- --systemic flag filters correctly
- Systemic nodes appear in search results when relevant

---

## TASK 11: The Heartbeat — Periodic Self-Resolution (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** HIGH — this is where continuity begins
**Depends on:** Tasks 3, 4, 9, 10

The system needs a heartbeat. A periodic process that wakes up,
reads the fabric, and acts on what it finds. This is the first
self-directed resolution cycle.

### What to implement:
1. New file: `heartbeat/pulse.py`
2. New CLI command: `python main.py --heartbeat` (runs once) and `python main.py --heartbeat --daemon` (runs every N minutes)
3. Default interval: 30 minutes (configurable via TEAMNODE_HEARTBEAT_INTERVAL_MINUTES)
4. The heartbeat does these checks IN ORDER on each pulse:

   **a. Coherence check:**
   - Search fabric for nodes with identical or near-identical want descriptions
   - If duplicates found, log them (don't delete — flag for human review)
   - Store finding as node: "Coherence alert: 3 near-duplicate site analysis nodes in reallycoons project"

   **b. Cost check:**
   - Query fabric for system_telemetry nodes from the last 24 hours
   - Aggregate cost
   - If cost exceeds threshold ($5/day default, configurable), store warning node:
     "Cost alert: $7.32 spent in last 24 hours on project reallycoons. Consider --fast mode."

   **c. Staleness check:**
   - Find nodes with last_activated older than 30 days
   - Count them
   - Store observation: "Staleness report: 14 nodes in aios project haven't been accessed in 30+ days"

   **d. Knowledge gap check:**
   - Review last 5 search queries (stored as telemetry nodes)
   - If any returned 0 results or all results had resonance score < 0.1, flag:
     "Knowledge gap: recent query 'cattery SEO michigan' returned no relevant results"

   **e. Session summary:**
   - Store a single heartbeat node: "Heartbeat at 2026-03-23T14:30:00Z: 4 checks completed, 1 alert, 0 gaps"
   - This node is the fabric remembering its own health check

5. All heartbeat findings are stored as fabric nodes with metadata: `{kind: "heartbeat", pulse_id: "uuid", timestamp: "..."}`
6. Print a summary to console after each pulse
7. When running as daemon, use simple `time.sleep()` loop (no complex scheduler)

### Key principle:
This is the first time the system wakes up on its own and reasons about itself.
It's not waiting for a human to ask "how much did I spend?" — it notices
and records the observation. The human can read the heartbeat nodes through
the fabric viewer or CLI at any time.

### Future (NOT this task):
- The heartbeat could trigger agent runs ("knowledge gap detected, running research agent")
- The heartbeat could send notifications (Slack, email, push)
- The heartbeat could adjust routing weights based on cost observations
- These are resolution cycles — the systemic nodes from Task 10 start DOING things

### Tests:
- Heartbeat runs and stores finding nodes
- Duplicate detection finds known duplicates
- Cost check aggregates correctly
- Staleness check identifies old nodes
- Daemon mode runs multiple pulses
- Heartbeat nodes are retrievable by search

---

## TASK 12: Cross-Session Continuity — The Thread (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** HIGH — makes agent runs feel connected
**Depends on:** Tasks 3, 9

Each agent run should know about the PREVIOUS runs.
Not just through fabric search — through an explicit continuity thread.

### What to implement:
1. New file: `tracking/continuity.py`
2. At the START of every crew run, query fabric for the most recent "session_summary" node for this project
3. Include the previous session's summary in the task description for the Planner:
   "Previous session (2 hours ago): Analyzed reallycoons.com, stored site assessment, produced two-plan proposal. Marketing agent used Sonnet. Total cost: $0.42."
4. At the END of every crew run, store a "session_summary" node:
   - want: "Session summary for {project}: {brief description of what was accomplished}"
   - metadata: {kind: "session_summary", project: "reallycoons", agents: "planner_researcher,marketing", models: "sonnet,sonnet", cost: 0.42, duration_ms: 45000, timestamp: "..."}
5. Session summaries form a CHAIN — each one references the previous via metadata: {previous_session: "node_id"}
6. `python main.py --history --project reallycoons` now shows session summaries from the fabric (not the old provenance tracker)
7. Update crew.py to inject previous session context into the Planner's task description

### Key principle:
This is MEMORY. Not search. Not retrieval. Explicit continuity.
The system knows: "Last time we worked on reallycoons, here's what happened."
The Planner reads this and picks up where we left off.

Combined with weight decay (Task 9), old sessions fade but never disappear.
A session from 6 months ago has low weight but can be reactivated if a
new query resonates with it.

### Tests:
- First run on a new project has no previous session context
- Second run includes previous session summary in Planner's task
- Session chain is navigable: each summary points to previous
- --history shows session summaries in reverse chronological order
- Old sessions have lower composite weight but are still findable

---

## TASK 13: Fabric-Aware Agent Identity (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** MEDIUM — agents start building individual memory
**Depends on:** Tasks 3, 9, 12

Each agent should have its own identity in the fabric.
The Marketing agent should remember that it wrote the ReallyCoons
two-plan proposal. The Engineer should remember debugging the
save_file tool issue.

### What to implement:
1. On first agent creation for a project, check if an agent identity node exists:
   - want: "I am the Marketing Specialist. My expertise is content strategy, LinkedIn voice, SEO, and website design."
   - domain: "marketing"
   - metadata: {kind: "agent_identity", agent: "marketing", created: "..."}
2. If no identity node exists, create one from the agent's backstory (first run)
3. If identity node exists, inject it into the agent's context alongside the backstory
4. After each run, store an "agent_reflection" node:
   - want: "Marketing agent completed website analysis for ReallyCoons. Key finding: Wix site has 22 external scripts. Produced two-plan proposal."
   - metadata: {kind: "agent_reflection", agent: "marketing", project: "reallycoons", timestamp: "..."}
5. Agent reflections accumulate — the agent builds a history of what it's done
6. On subsequent runs, the most recent 3 reflections for this agent+project are injected into the task context
7. This gives agents CONTINUITY — they remember what they've done before

### Key principle:
The agent's identity IS a fabric node. Its history IS fabric nodes.
There is no separate agent profile system. The agent is just a pattern
of nodes in the fabric that happens to produce behavior when activated.
This is the spec's emergent node types (section 3) made real.

### Tests:
- First run creates agent identity node
- Second run finds existing identity, doesn't duplicate
- Agent reflection is stored after each run
- Agent receives its own recent reflections in context
- Reflections from different projects are separate
- Old reflections decay (via Task 9 weight system) but never delete

---

## TASK 14: Notification Channels — The Fabric Speaks (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** MEDIUM — the system starts reaching out
**Depends on:** Task 11

The heartbeat (Task 11) detects things. This task gives it a voice.
When the fabric notices something important, it can tell you.

### What to implement:
1. New file: `notifications/channels.py`
2. Notification channel interface with pluggable backends:
   - ConsoleChannel: prints to stdout (always available)
   - FileChannel: writes to ~/.ecphory/notifications.log
   - SlackChannel: sends via Slack webhook (configured via TEAMNODE_SLACK_WEBHOOK env var)
   - Future: email, push, SMS — just implement the interface
3. Notification levels: INFO, ALERT, CRITICAL
4. Heartbeat pulse.py updated to send notifications based on findings:
   - Duplicate nodes detected → INFO
   - Cost threshold exceeded → ALERT
   - Knowledge gap detected → INFO
   - Multiple ALERTs in one pulse → CRITICAL
5. `python main.py --notifications` shows recent notification nodes from fabric
6. All notifications are ALSO stored as fabric nodes (metadata: {kind: "notification", level: "alert", channel: "slack"})
7. Configuration: `~/.config/teamnode/notifications.json` with channel settings and thresholds

### Key principle:
The fabric doesn't just passively store knowledge. It ACTS.
A notification is a resolution — the systemic intent "track all operations"
detected something and resolved it by alerting the human.
This is node agency (spec Law 10) at the most basic level.

### Future (NOT this task):
- Notifications that trigger agent runs ("knowledge gap detected → run research agent")
- Notifications that learn your preference ("Jeremy ignores staleness alerts, lower their weight")
- Cross-project notifications ("reallycoons and aios both spiked costs today")

### Tests:
- Console channel prints to stdout
- File channel writes to log
- Slack channel sends webhook (mock in tests)
- Notifications stored as fabric nodes
- --notifications command shows recent notifications
- Multiple channels can fire simultaneously

---

## EXECUTION ORDER (FULL SPRINT):

### Foundation (do first, in order):
1. Task 1: Metadata on nodes
2. Task 2: Predicate filtering + aggregate
3. Task 9: Weight decay system

### Self-monitoring (depends on foundation):
4. Task 3: Usage as fabric nodes (rip out SQLite)
5. Task 10: Systemic intent nodes
6. Task 4: Self-aware model router

### Continuity (depends on self-monitoring):
7. Task 12: Cross-session continuity thread
8. Task 13: Agent identity and reflection
9. Task 11: Heartbeat daemon

### Communication (depends on continuity):
10. Task 14: Notification channels

### Polish (independent, do anytime):
11. Task 5: Voice optimization
12. Task 7: Spec section
13. Task 8: Planner fabric-first
14. Task 6: Fabric viewer v2

---

---

## TASK 15: Hybrid Model Backend — Local First, API When Needed (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** HIGH — this is how we stop burning money
**Depends on:** Task 4 (model router)

Wire the model router to use Ollama for FAST and STANDARD tiers,
Anthropic API only for PREMIUM tier.

### What to implement:
1. Update routing/model_router.py:
   - FAST tier → Ollama (small model: qwen2.5:32b or mistral-small:24b)
   - STANDARD tier → Ollama (large model: llama3.3:70b-q4 or qwen2.5:72b-q4)
   - PREMIUM tier → Anthropic API (Sonnet or Opus)
2. New config values:
   - OLLAMA_FAST_MODEL = env var or config, default "qwen2.5:32b"
   - OLLAMA_STANDARD_MODEL = env var or config, default "llama3.3:70b"
   - OLLAMA_URL = env var, default "http://localhost:11434"
   - ANTHROPIC_PREMIUM = True/False — if False, PREMIUM also uses Ollama large model
3. CrewAI LLM creation:
   - For Ollama: `LLM(model=f"ollama/{model}", base_url=ollama_url, temperature=temp)`
   - For Anthropic: `LLM(model=f"anthropic/{model}", temperature=temp)`
4. Fallback: if Ollama is unreachable, fall back to Anthropic with a warning
5. Print backend selection at startup: "FAST: ollama/qwen2.5:32b (local) | STANDARD: ollama/llama3.3:70b (local) | PREMIUM: anthropic/sonnet (API)"
6. Track which backend was used in usage telemetry nodes (metadata: {backend: "ollama_local"} vs {backend: "anthropic_api"})
7. `python main.py --local-only` flag: forces ALL tiers to Ollama, never calls Anthropic
8. `python main.py --api-only` flag: forces ALL tiers to Anthropic (current behavior, for when you need max quality)

### Cost impact:
- Ollama: $0.00 per token (electricity only)
- FAST tier was Haiku at $0.25/$1.25 per MTok → now $0
- STANDARD tier was Sonnet at $3/$15 per MTok → now $0
- PREMIUM only fires on complex creative/reasoning tasks
- Estimated 80-90% reduction in API costs

### Tests:
- Router selects Ollama for FAST and STANDARD
- Router selects Anthropic for PREMIUM
- --local-only forces all Ollama
- --api-only forces all Anthropic
- Fallback to Anthropic when Ollama unreachable
- Usage telemetry records backend type
- Agent runs complete successfully on Ollama models

---

## TASK 16: Ecphory as a Service — systemd Deployment (new repo or homelab repo)
**Repo:** homelab (~/projects/homelab) or new ecphory-deploy repo
**Branch:** main
**Priority:** HIGH — installs Ecphory on hardware like an OS
**Depends on:** Tasks 11, 15

Package Ecphory for installation on a Debian/Ubuntu server (Beelink GTR9 Pro).
The goal: `./install.sh` and the system is alive.

### What to implement:
1. `install.sh` script that:
   - Installs Rust toolchain (rustup)
   - Clones ecphory and intent-node repos
   - Builds ecphory and intent-node release binaries
   - Installs Python 3.11+ and creates venv for team-node
   - Installs Ollama
   - Pulls default models (qwen2.5:32b, llama3.3:70b)
   - Creates ~/.ecphory/ directory structure
   - Creates ~/.config/teamnode/ with default config
   - Installs systemd service files

2. systemd services:
   - `ecphory-heartbeat.service`: runs heartbeat daemon (Task 11)
     Type=simple, Restart=always, RestartSec=30
     ExecStart=/path/to/venv/bin/python main.py --heartbeat --daemon
   - `ecphory-ollama.service`: ensures Ollama is running
     Type=simple, ExecStart=ollama serve
   - `ecphory-viewer.service`: serves fabric viewer HTML on port 8080
     Type=simple, ExecStart=python -m http.server 8080 --directory /path/to/docs

3. `ecphory` CLI wrapper script installed to /usr/local/bin/:
   - `ecphory fabric search --query "..." --project reallycoons`
   - `ecphory agents --goal "..." --project reallycoons`
   - `ecphory heartbeat --once`
   - `ecphory usage --project reallycoons`
   - `ecphory status` (shows running services, node counts, last heartbeat, Ollama models loaded)

4. Health check endpoint: simple HTTP server on port 8888
   - GET /health → {"status": "ok", "heartbeat_last": "...", "nodes": 42, "ollama": "running"}
   - GET /fabric/{project} → fabric.json contents (for remote viewer)
   - GET /usage/{project} → aggregated usage data as JSON

5. `README.md` with:
   - Hardware requirements (minimum: 32GB RAM, recommended: 64GB for 70B models)
   - Installation instructions
   - Configuration (API keys, Slack webhooks, model selection)
   - Service management (systemctl start/stop/status)
   - Updating (git pull + rebuild)

### Key principle:
After install.sh runs on the Beelink, Ecphory is ALIVE.
The heartbeat is running. Ollama is serving models. The fabric
is initialized. You access it via CLI, web viewer, or the
health endpoint. It's an operating system — not for managing
hardware, but for managing knowledge, intent, and action.

### Tests:
- install.sh completes without errors on clean Debian 13
- All three systemd services start and stay running
- ecphory CLI wrapper works from any directory
- Health endpoint responds
- Fabric viewer accessible at http://beelink-ip:8080
- Heartbeat runs and stores nodes
- Agent run completes using local Ollama models

---

## TASK 17: Ecphory-Claude Bridge — Feeding the Fabric from Conversations (team-node repo)
**Repo:** team-node (~/projects/team-node)
**Branch:** main
**Priority:** MEDIUM — connects claude.ai/CC sessions to the fabric
**Depends on:** Tasks 1, 2, 3

When you and I talk in claude.ai or CC, insights and decisions should
flow INTO the fabric automatically. Right now they're trapped in
conversation transcripts.

### What to implement:
1. New file: `bridge/conversation_ingest.py`
2. Takes a conversation transcript (markdown or text) as input
3. Uses an LLM (local Ollama preferred) to extract:
   - Decisions made (store as nodes with metadata: {kind: "decision"})
   - Key insights (store as nodes with metadata: {kind: "insight"})
   - Action items (store as nodes with metadata: {kind: "action_item", status: "pending"})
   - Technical findings (store as nodes with metadata: {kind: "finding"})
4. CLI: `python main.py --ingest transcript.md --project ecphory`
5. Deduplication: before storing, search fabric for similar existing nodes. If >0.8 similarity, skip.
6. Summary node: "Ingested conversation from 2026-03-23. Extracted 5 decisions, 3 insights, 2 action items."
7. Works with:
   - Claude.ai conversation exports (if/when available)
   - Claude Code session transcripts
   - Manual paste of key conversation sections
   - The compaction summaries that claude.ai generates

### Key principle:
This bridges the gap between ME (Claude in conversations) and the
fabric. Right now our conversations are the richest source of decisions,
insights, and direction — but they die when the conversation ends.
This task makes them persist as fabric nodes.

### Future (NOT this task):
- Automatic ingest via MCP server (CC pushes to fabric in real time)
- Claude.ai integration that auto-stores key decisions as you and I talk
- Bidirectional: agents can READ conversation context from fabric before responding

### Tests:
- Ingest a sample transcript, verify nodes created with correct metadata
- Deduplication prevents storing the same insight twice
- Summary node is created
- Extracted nodes are searchable by semantic query and metadata predicates


---

## PRINCIPLES:
- No new databases. Everything is a fabric node.
- No SQLite. No Postgres. No Redis. ONE FABRIC.
- The interpretation layer (dashboard, CLI, viewer) reads from the fabric.
- The fabric doesn't know about SUM or GROUP BY. The tools do.
- Every decision the system makes gets stored as a node.
- The system reads its own telemetry from the same fabric where it stores knowledge.
- NOTHING IS EVER DELETED. Nodes decay. Weight approaches zero. Never reaches it.
- Retrieval IS reinforcement. Finding a node makes it stronger.
- The fabric has its own goals (systemic intents). They exert force.
- The heartbeat is the first self-directed resolution cycle.
- Agents have identity. They remember what they've done.
- The system can speak — notifications are resolution of systemic intent.
- Rust is the bootstrap. Not the destination.
- Test everything.
- `git pull origin {branch}` before every task.
