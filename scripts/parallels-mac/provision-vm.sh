#!/usr/bin/env bash
#
# Provision a fresh Parallels Desktop VM that boots Ecphory aarch64 with
# an attached persistence disk. Runs on Apple Silicon Mac with Parallels
# Desktop ≥ 19. Idempotent against the same VM name (skips create if
# the VM already exists; re-uses bundles already on disk).
#
# Background: the prior Mac-CC validation cycle (nisaba/projects/ecphory/
# handoffs/handoff-cc-ecphory-os-mac-validation.md) discovered the actual
# Parallels CLI surface that works on PD 26.3.2:
#   - prl_disk_tool create --hdd <path> --size <N>M  (64 MiB minimum)
#   - prl_disk_tool create seeds inner-image GUIDs deterministically;
#     back-to-back creates produce bundles with identical UIDs. Patch
#     the bundle-level <Uid> in the .pvs XML so two bundles can attach
#     to the same VM without conflict.
#   - prl_disk_tool convert --src/--dst flags do NOT exist; only
#     `convert -i --hdd <bundle>` (validation) is supported.
#   - prlctl create + --device-add hdd + --efi-boot on
#   - prlctl capture <vm> --file <path> for headless screenshots
#   - prlctl stop --kill OR --acpi (without flags is interactive;
#     no headless bypass)
#
# Usage:
#   ./provision-vm.sh --boot-img ~/Downloads/ecphory-aarch64.img \
#                     [--vm-name ecphory-nucleation] \
#                     [--storage-size 128] \
#                     [--out-dir ~/ecphory-vms]

set -euo pipefail

# ---- defaults ----
VM_NAME="ecphory-nucleation"
STORAGE_SIZE_MIB=128
OUT_DIR="$HOME/ecphory-vms"
BOOT_IMG=""
MEMSIZE_MIB=1024

# ---- arg parse ----
while [[ $# -gt 0 ]]; do
  case "$1" in
    --boot-img) BOOT_IMG="$2"; shift 2;;
    --vm-name) VM_NAME="$2"; shift 2;;
    --storage-size) STORAGE_SIZE_MIB="$2"; shift 2;;
    --out-dir) OUT_DIR="$2"; shift 2;;
    --memsize) MEMSIZE_MIB="$2"; shift 2;;
    -h|--help)
      sed -n '3,/^$/p' "$0" | sed 's/^# //;s/^#//'
      exit 0;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

if [[ -z "$BOOT_IMG" ]]; then
  echo "ERROR: --boot-img is required" >&2
  exit 2
fi
# Resolve to absolute path (dd doesn't expand ~ inside if=/of=).
BOOT_IMG="$(cd "$(dirname "$BOOT_IMG")" && pwd)/$(basename "$BOOT_IMG")"
if [[ ! -f "$BOOT_IMG" ]]; then
  echo "ERROR: boot image not found: $BOOT_IMG" >&2
  exit 2
fi

# ---- prerequisites ----
for tool in prlctl prl_disk_tool uuidgen plutil; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "ERROR: '$tool' not found in PATH. Install Parallels Desktop ≥ 19." >&2
    exit 3
  fi
done

mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

BOOT_HDD="$OUT_DIR/${VM_NAME}-boot.hdd"
STORAGE_HDD="$OUT_DIR/${VM_NAME}-storage.hdd"

echo "==> Configuration"
echo "    VM name:      $VM_NAME"
echo "    Boot image:   $BOOT_IMG"
echo "    Out dir:      $OUT_DIR"
echo "    Storage size: ${STORAGE_SIZE_MIB} MiB"
echo "    RAM:          ${MEMSIZE_MIB} MiB"
echo "    Boot HDD:     $BOOT_HDD"
echo "    Storage HDD:  $STORAGE_HDD"
echo

# ---- helper: patch the bundle UID so two same-day bundles don't collide ----
patch_bundle_uid() {
  local bundle="$1"
  local pvs
  pvs="$(find "$bundle" -maxdepth 2 -name 'DiskDescriptor.xml' -o -name '*.pvs' 2>/dev/null | head -1)"
  if [[ -z "$pvs" ]]; then
    echo "WARN: no descriptor found in $bundle; skipping UID patch" >&2
    return 0
  fi
  local new_uid="{$(uuidgen | tr 'A-Z' 'a-z')}"
  # Replace the first <Uid>{...}</Uid> occurrence. macOS sed wants the
  # `-i ''` form for in-place no-backup edits.
  sed -i '' -E "s|<Uid>\\{[^}]*\\}</Uid>|<Uid>$new_uid</Uid>|" "$pvs"
  echo "    UID patched in $(basename "$bundle"): $new_uid"
}

# ---- helper: create + dd boot bundle ----
make_boot_bundle() {
  if [[ -d "$BOOT_HDD" ]]; then
    echo "==> Boot bundle exists; refusing to overwrite. rm -rf '$BOOT_HDD' to redo."
    return 0
  fi
  echo "==> Creating boot bundle"
  # 64 MiB is the prl_disk_tool minimum AND happens to match the .img size.
  prl_disk_tool create --hdd "$BOOT_HDD" --size 64M
  local inner
  inner="$(find "$BOOT_HDD" -name '*.hds' | head -1)"
  if [[ -z "$inner" ]]; then
    echo "ERROR: no inner .hds in created bundle" >&2
    exit 4
  fi
  echo "    dd boot image -> $inner"
  dd if="$BOOT_IMG" of="$inner" bs=1m conv=notrunc 2>&1 | tail -3
  patch_bundle_uid "$BOOT_HDD"
  prl_disk_tool convert -i --hdd "$BOOT_HDD" >/dev/null 2>&1 || true
}

# ---- helper: create empty storage bundle ----
make_storage_bundle() {
  if [[ -d "$STORAGE_HDD" ]]; then
    echo "==> Storage bundle exists; reusing (rm -rf '$STORAGE_HDD' to start fresh)."
    return 0
  fi
  echo "==> Creating storage bundle (${STORAGE_SIZE_MIB} MiB)"
  prl_disk_tool create --hdd "$STORAGE_HDD" --size "${STORAGE_SIZE_MIB}M"
  patch_bundle_uid "$STORAGE_HDD"
}

# ---- helper: create + configure VM ----
make_vm() {
  if prlctl list --all --no-header 2>/dev/null | awk '{print $NF}' | grep -Fxq "$VM_NAME"; then
    echo "==> VM '$VM_NAME' already exists; skipping create. Use --vm-name to make a new one."
    return 0
  fi
  echo "==> Creating VM '$VM_NAME'"
  prlctl create "$VM_NAME" --ostype linux --distribution other --no-hdd
  echo "==> Attaching boot HDD"
  prlctl set "$VM_NAME" --device-add hdd --image "$BOOT_HDD" --enable
  echo "==> Attaching storage HDD"
  prlctl set "$VM_NAME" --device-add hdd --image "$STORAGE_HDD" --enable
  echo "==> Setting EFI boot + memory"
  prlctl set "$VM_NAME" --efi-boot on
  prlctl set "$VM_NAME" --memsize "$MEMSIZE_MIB"
}

make_boot_bundle
make_storage_bundle
make_vm

cat <<EOF

==> Provisioning complete.

To start the VM:
    prlctl start $VM_NAME

To capture the framebuffer (headless screenshot, no macOS permissions):
    prlctl capture $VM_NAME --file ~/Desktop/$VM_NAME-\$(date +%s).png

To send commands (e.g. type 'persist'+enter):
    ./typetext.sh $VM_NAME "persist"
    ./typetext.sh $VM_NAME --enter

To stop:
    prlctl stop $VM_NAME --kill   # immediate (use this for the harsh-stop
                                  # lifecycle test that found the Parallels
                                  # flush bug last time)
    prlctl stop $VM_NAME --acpi   # graceful (~5s)

To swap a new kernel image into the existing VM (preserves storage):
    ./swap-kernel.sh --vm-name $VM_NAME \\
                     --boot-img <path-to-new-ecphory-aarch64.img>

Bundles live in:
    $BOOT_HDD
    $STORAGE_HDD
EOF
