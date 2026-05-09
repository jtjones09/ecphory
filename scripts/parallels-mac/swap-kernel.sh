#!/usr/bin/env bash
#
# Replace the kernel image inside an existing Parallels VM's boot bundle
# without disturbing the storage bundle. Used to test multiple kernel
# versions against the same persistence state — drops a new
# ecphory-aarch64.img onto the boot disk, leaves storage alone, restart.
#
# Usage:
#   ./swap-kernel.sh --vm-name ecphory-nucleation \
#                    --boot-img ~/Downloads/ecphory-aarch64.img \
#                    [--out-dir ~/ecphory-vms]

set -euo pipefail

VM_NAME=""
BOOT_IMG=""
OUT_DIR="$HOME/ecphory-vms"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --vm-name) VM_NAME="$2"; shift 2;;
    --boot-img) BOOT_IMG="$2"; shift 2;;
    --out-dir) OUT_DIR="$2"; shift 2;;
    -h|--help)
      sed -n '3,/^$/p' "$0" | sed 's/^# //;s/^#//'
      exit 0;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

[[ -z "$VM_NAME" ]] && { echo "ERROR: --vm-name required" >&2; exit 2; }
[[ -z "$BOOT_IMG" ]] && { echo "ERROR: --boot-img required" >&2; exit 2; }

BOOT_IMG="$(cd "$(dirname "$BOOT_IMG")" && pwd)/$(basename "$BOOT_IMG")"
[[ -f "$BOOT_IMG" ]] || { echo "ERROR: boot image not found: $BOOT_IMG" >&2; exit 2; }

OUT_DIR="$(cd "$OUT_DIR" 2>/dev/null && pwd)" || { echo "ERROR: out dir missing: $OUT_DIR" >&2; exit 2; }
BOOT_HDD="$OUT_DIR/${VM_NAME}-boot.hdd"
[[ -d "$BOOT_HDD" ]] || { echo "ERROR: boot bundle missing: $BOOT_HDD" >&2; exit 2; }

# If the VM is running, stop it first — Parallels will refuse to swap a
# disk under a live VM.
if prlctl list --no-header 2>/dev/null | awk '{print $NF}' | grep -Fxq "$VM_NAME"; then
  echo "==> Stopping running VM '$VM_NAME' (--kill, no host data at risk)"
  prlctl stop "$VM_NAME" --kill 2>/dev/null || true
  sleep 1
fi

INNER="$(find "$BOOT_HDD" -name '*.hds' | head -1)"
[[ -z "$INNER" ]] && { echo "ERROR: no .hds inside $BOOT_HDD" >&2; exit 4; }

BEFORE_SHA="$(shasum -a 256 "$INNER" | cut -c1-16)"
echo "==> Old boot disk: $INNER"
echo "    sha256(first 16): $BEFORE_SHA"

echo "==> Writing new boot image"
dd if="$BOOT_IMG" of="$INNER" bs=1m conv=notrunc 2>&1 | tail -3

AFTER_SHA="$(shasum -a 256 "$INNER" | cut -c1-16)"
echo "    sha256(first 16): $AFTER_SHA"
[[ "$BEFORE_SHA" != "$AFTER_SHA" ]] && echo "    image changed OK"

# Optional: re-validate the bundle's plain-format invariant.
prl_disk_tool convert -i --hdd "$BOOT_HDD" >/dev/null 2>&1 || true

cat <<EOF

==> Kernel swapped. Storage bundle untouched — persistence state from the
    prior kernel is intact.

To boot:
    prlctl start $VM_NAME
EOF
