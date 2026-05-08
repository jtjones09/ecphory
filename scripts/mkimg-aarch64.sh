#!/usr/bin/env bash
# Wrap an aarch64 UEFI .efi binary in a bootable disk image with GPT + ESP.
#
# Usage: mkimg-aarch64.sh <kernel.efi> <output.img>
set -euo pipefail

KERNEL_EFI="${1:?usage: $0 <kernel.efi> <output.img>}"
OUT_IMG="${2:?usage: $0 <kernel.efi> <output.img>}"

if [[ ! -f "$KERNEL_EFI" ]]; then
    echo "kernel .efi not found: $KERNEL_EFI" >&2
    exit 1
fi

# 64 MiB image — plenty of headroom for the ~140 KiB .efi.
IMG_SIZE_MIB=64
ESP_SIZE_MIB=62

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# 1. Create the FAT32 ESP partition contents in a loose file.
ESP_IMG="$WORK/esp.img"
truncate -s "${ESP_SIZE_MIB}M" "$ESP_IMG"
mformat -i "$ESP_IMG" -h 32 -t 32 -n 64 -c 1 ::
mmd -i "$ESP_IMG" ::/EFI
mmd -i "$ESP_IMG" ::/EFI/BOOT
mcopy -i "$ESP_IMG" "$KERNEL_EFI" ::/EFI/BOOT/BOOTAA64.EFI

# 2. Create the outer GPT image.
truncate -s "${IMG_SIZE_MIB}M" "$OUT_IMG"
parted -s "$OUT_IMG" mklabel gpt
parted -s "$OUT_IMG" mkpart ESP fat32 1MiB 100%
parted -s "$OUT_IMG" set 1 esp on

# 3. Splat the ESP contents into partition 1.
PART_OFFSET_BYTES=$((1024 * 1024)) # 1 MiB partition start
dd if="$ESP_IMG" of="$OUT_IMG" bs=1M seek=1 count="$ESP_SIZE_MIB" conv=notrunc status=none

echo "wrote $OUT_IMG ($(du -h "$OUT_IMG" | cut -f1))"
