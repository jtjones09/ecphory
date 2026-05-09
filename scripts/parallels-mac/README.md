# Parallels-on-Mac tooling

Mac-side scripts for provisioning a Parallels Desktop VM that boots
Ecphory aarch64 with a persistence disk, plus the lifecycle test that
re-runs the harsh-stop reboot validation from Session 21.

These pair with images built on Enki:
- `~/projects/ecphory/target/release/build/runner-aarch64-*/out/ecphory-aarch64.img`

Copy the .img to the Mac (Parallels shared folder, AirDrop, scp,
whatever), then run from a Mac terminal.

## Prerequisites

- macOS on Apple Silicon (M1/M2/M3/M4)
- Parallels Desktop ≥ 19 — provides `prlctl`, `prl_disk_tool`
- `uuidgen`, `plutil`, `dd` (system-default)
- **Bash ≥ 4** — `typetext.sh` uses `declare -A` (associative arrays)
  which is bash 4+. macOS ships system bash 3.2; `brew install bash`
  and run the scripts via the homebrew bash. Mac-CC's Session 25c run
  surfaced this. The script's shebang is `#!/usr/bin/env bash` so
  whichever bash is first on PATH wins.

## Quick start

```sh
chmod +x provision-vm.sh swap-kernel.sh typetext.sh lifecycle-test.sh

# 1. Create the VM. Drop the .img onto your Mac first.
./provision-vm.sh --boot-img ~/Downloads/ecphory-aarch64.img

# 2. Start it manually if you want to drive it interactively:
prlctl start ecphory-nucleation

# OR: run the full automated lifecycle test (boot → persist → kill → boot → check restore)
./lifecycle-test.sh ecphory-nucleation
```

## What each script does

### `provision-vm.sh`

Idempotent VM setup. Creates two Parallels `.hdd` bundles (boot + storage)
in `~/ecphory-vms/` by default, writes the boot image into the boot
bundle's inner `.hds`, patches the bundle UID (Parallels generates
deterministic GUIDs which collide when two same-day bundles attach to
one VM), creates the VM with `--efi-boot on`, attaches both HDDs, sets
RAM. Skips any step whose target already exists.

### `swap-kernel.sh`

Drop a new kernel `.img` onto an existing VM's boot disk WITHOUT
disturbing the storage disk. Use this to test multiple kernel versions
against the same persistence state. Stops the VM if running, `dd`'s
the new image over the inner `.hds`, validates the bundle. Storage
state survives.

### `typetext.sh`

Send keyboard input via `prlctl send-key-event`. AT Set 1 scancodes
only (Parallels rejects hex; decimals work). Lowercase ASCII +
digits + a small punctuation set + Enter/Backspace/Esc/Tab. That covers
every Ecphory operator command.

```sh
./typetext.sh ecphory-nucleation "model"           # types "model"
./typetext.sh ecphory-nucleation --enter           # presses Enter
./typetext.sh ecphory-nucleation --line "persist"  # types + Enter
```

### `lifecycle-test.sh`

End-to-end harsh-stop reboot test. Boots, types `persist`, screenshots,
runs `prlctl stop --kill`, boots again, screenshots, types `model` and
`causal`. Lands six photos in `~/Desktop` (or `--shot-dir`) labeled by
phase. Reproduces the test that originally caught the Parallels flush
regression in Session 21.

The load-bearing screenshot is `boot2-restore-*.png` — should show
`restored N nodes / M edges from disk (lamport L)`. If it shows
`no prior snapshot (checksum mismatch); fresh genesis`, the Parallels
flush regression has returned and we have a real bug.

## Lessons from the prior validation cycle (preserved here so the next CC doesn't re-derive)

These came from Mac-CC's first-boot session on M2 Max in May 2026,
recorded verbatim in `nisaba/projects/ecphory/handoffs/handoff-cc-
ecphory-os-mac-validation.md`. They're load-bearing for these scripts:

1. **`prl_disk_tool create` minimum size is 64 MiB.** `--size 16Mb`
   returns `PRL_ERR_DISK_CREATE_IMAGE_ERROR`. The boot bundle is sized
   exactly to the .img (64 MiB). The storage bundle defaults to
   128 MiB so it's larger than the boot disk and the picker picks it.

2. **`prl_disk_tool create` seeds inner-image GUIDs deterministically.**
   Back-to-back creates produce bundles with identical UIDs. If both
   attach to the same VM, the VM refuses. `provision-vm.sh` patches
   the bundle-level `<Uid>` in the .pvs descriptor before attaching.

3. **`prl_disk_tool convert --src/--dst` does NOT exist on PD 26.3.2.**
   Only `convert -i --hdd <bundle>` (validation) is supported.

4. **`prlctl capture <vm> --file <path>` is the headless screenshot
   path.** Doesn't need macOS Screen-Recording permission. We use it
   throughout `lifecycle-test.sh`.

5. **`prlctl send-key-event --scancode <decimal>` only — hex rejected.**
   Always send press+release pairs (otherwise guest autorepeat fires).
   `typetext.sh` does both.

6. **`prlctl stop` without `--kill` opens an interactive confirmation
   that has no headless bypass.** Use `--acpi` for graceful (~5s) or
   `--kill` for immediate. Both produce identical correct results
   post-flush-fix.

7. **`dd` does NOT expand `~` in `if=`/`of=` argument values** (the `=`
   breaks tilde expansion). `provision-vm.sh` resolves to absolute
   paths before invoking dd.

## Troubleshooting

**"Boot disk image won't attach"** — usually the GUID-collision case.
Delete both bundles (`rm -rf ~/ecphory-vms/ecphory-nucleation-*.hdd`)
and re-run `provision-vm.sh`. The script's UID patch should prevent
this, but if you've manually copied bundles around, the patch may
have been bypassed.

**"Kernel boots but `disks` shows 0 BlockIO devices"** — the storage
HDD didn't attach. `prlctl list-info <vm>` confirms attached HDDs.

**"`restored N nodes / M edges`" never appears after kill+boot** —
the Parallels flush regression. Check that the kernel image is
actually the post-flush-fix build (`Op::FlushStorage` should land in
`kernel_uefi_common::handle_flush_storage`). Easy way to verify:
`shasum -a 256 ~/Downloads/ecphory-aarch64.img` and confirm against
the SHA Enki published with the build.

**"VM won't boot — black screen"** — Parallels might be picking the
wrong default boot order. `prlctl list-info <vm>` shows EFI/BIOS
boot device order. Re-run `prlctl set <vm> --efi-boot on` and try
again.

## Where to put screenshots after a session

Photos from Mac sessions go to `nisaba/projects/ecphory/handoffs/
ecphory-os-mac-validation-evidence/screenshots/<session-tag>/` (next to
the original Mac-validation evidence). Mirror the lifecycle test's
output filenames so the timeline is grep-able.
