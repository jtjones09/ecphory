#!/usr/bin/env bash
#
# Run the full Ecphory lifecycle on a provisioned Parallels VM:
#   start → wait for genesis → persist → screenshot →
#   stop --kill → start → wait for restore → screenshot.
#
# This is the harsh-stop reboot test that originally surfaced the
# Parallels flush-bug back in May 2026 (handoff-cc-ecphory-os-mac-
# validation.md). Re-running it on the new GenerativeModel build is
# the load-bearing claim for "nucleation persistence works on Apple
# silicon."
#
# Usage:
#   ./lifecycle-test.sh <vm-name> [--shot-dir ~/Desktop]

set -euo pipefail

VM="${1:?usage: $0 <vm-name> [--shot-dir <path>]}"
shift || true
SHOT_DIR="$HOME/Desktop"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --shot-dir) SHOT_DIR="$2"; shift 2;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

mkdir -p "$SHOT_DIR"
SHOT_DIR="$(cd "$SHOT_DIR" && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
TYPE="$HERE/typetext.sh"
[[ -x "$TYPE" ]] || chmod +x "$TYPE"

shoot() {
  local label="$1"
  local out="$SHOT_DIR/${VM}-${label}-$(date +%Y%m%d-%H%M%S).png"
  prlctl capture "$VM" --file "$out"
  echo "    shot: $out"
}

echo "==> Boot 1 (fresh genesis)"
prlctl start "$VM"
sleep 12
shoot "boot1-genesis"

echo "==> Type 'persist'+enter, wait for snapshot"
"$TYPE" "$VM" --line "persist"
sleep 4
shoot "boot1-after-persist"

echo "==> Type 'model'+enter (capture the boot=1 state)"
"$TYPE" "$VM" --line "model"
sleep 2
shoot "boot1-model"

echo "==> Harsh stop (--kill — the test that found the flush bug)"
prlctl stop "$VM" --kill
sleep 3

echo "==> Boot 2 (expect restore)"
prlctl start "$VM"
sleep 12
shoot "boot2-restore"

echo "==> Type 'model'+enter (expect boots=2 with continued obs count)"
"$TYPE" "$VM" --line "model"
sleep 2
shoot "boot2-model"

echo "==> Type 'causal'+enter"
"$TYPE" "$VM" --line "causal"
sleep 2
shoot "boot2-causal"

cat <<EOF

==> Lifecycle test complete. Check screenshots in $SHOT_DIR

The load-bearing photographs are:
  - boot1-genesis:       'no prior snapshot (bad magic); fresh genesis'
  - boot1-after-persist: 'persisted N bytes' lines
  - boot2-restore:       'restored N nodes / M edges from disk (lamport L)'
  - boot2-model:         'boots=2', obs=N continuing from before the kill

If boot2-restore shows 'no prior snapshot (checksum mismatch); fresh
genesis' instead of 'restored ...', the Parallels flush regression has
returned. See handoff-cc-ecphory-os-mac-validation.md for the prior
debug session.
EOF
