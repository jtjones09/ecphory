# TASKS.md — Ecphory Sprint: Unified Fabric + Self-Monitoring
# For Claude Code execution on intent-node, ecphory, and team-node repos
# Priority: TOP → BOTTOM. Each task is self-contained.
# Expected execution: CC on Opus, one task at a time, git pull before each.
# CANONICAL VERSION — this is the source of truth. Other repos have pointers.

See the downloaded SPRINT-TASKS.md for the full 726-line task list.
This file was too large for the GitHub API single-file push.

Push the full version from your Mac:
  cp ~/Downloads/SPRINT-TASKS.md ~/Documents/GitHub/ecphory/SPRINT-TASKS.md
  cd ~/Documents/GitHub/ecphory
  git add SPRINT-TASKS.md
  git commit -m "Add canonical sprint tasks — 17 tasks for unified fabric"
  git push origin main

Or from the VM:
  scp the file to ~/projects/ecphory/SPRINT-TASKS.md
  cd ~/projects/ecphory
  git add SPRINT-TASKS.md && git commit -m "Add sprint tasks" && git push

## Quick Reference — 17 tasks, 4 phases:

### Foundation (Tasks 1, 2, 9) — ecphory repo, Rust:
1. Structured metadata on IntentNode
2. Predicate filtering + aggregate CLI
9. Weight decay — nodes fade, never die

### Self-monitoring (Tasks 3, 10, 4) — team-node + ecphory:
3. Usage events as fabric nodes (rip out SQLite)
10. Systemic intent nodes (fabric's own goals)
4. Self-aware model router reads own telemetry

### Continuity (Tasks 12, 13, 11) — team-node:
12. Cross-session continuity thread
13. Agent identity and reflection
11. Heartbeat daemon (self-directed resolution)

### Communication (Task 14) — team-node:
14. Notification channels (fabric speaks)

### Infrastructure (Tasks 15, 16, 17):
15. Hybrid model backend (Ollama local + API premium)
16. systemd deployment on hardware
17. Conversation-to-fabric bridge

### Polish (Tasks 5, 6, 7, 8):
5. Voice constraints optimization
6. Fabric viewer v2
7. Spec section on structured queries
8. Planner fabric-first behavior
