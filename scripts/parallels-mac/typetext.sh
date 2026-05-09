#!/usr/bin/env bash
#
# Send a string of text (or a single special key) to a Parallels VM via
# `prlctl send-key-event`. AT Set 1 scancodes only — Parallels rejects
# hex; decimals work.
#
# Usage:
#   ./typetext.sh <vm> "hello world"        # types literal text
#   ./typetext.sh <vm> --enter              # press+release Enter
#   ./typetext.sh <vm> --backspace          # press+release Backspace
#   ./typetext.sh <vm> --esc                # press+release Escape
#   ./typetext.sh <vm> --line "persist"     # types text then presses Enter
#
# Limitation: lowercase ASCII letters, digits, and a small punctuation
# set only. The Tesseract operator surface only needs that vocabulary
# (status, persist, model, causal, disks, etc.) — extending the table
# is straightforward when a new command needs different chars.

set -euo pipefail

VM="${1:-}"
shift || true
[[ -z "$VM" ]] && { echo "Usage: $0 <vm> [--line] '<text>' | --enter | --backspace | --esc" >&2; exit 2; }

# AT Set 1 make codes (decimal). Right column = release = make + 0x80.
declare -A SC=(
  [a]=30 [b]=48 [c]=46 [d]=32 [e]=18 [f]=33 [g]=34 [h]=35 [i]=23 [j]=36
  [k]=37 [l]=38 [m]=50 [n]=49 [o]=24 [p]=25 [q]=16 [r]=19 [s]=31 [t]=20
  [u]=22 [v]=47 [w]=17 [x]=45 [y]=21 [z]=44
  [0]=11 [1]=2  [2]=3  [3]=4  [4]=5  [5]=6  [6]=7  [7]=8  [8]=9  [9]=10
  [' ']=57 ['-']=12 ['=']=13 ['[']=26 [']']=27 [';']=39 [\']=40
  [',']=51 ['.']=52 ['/']=53 ['`']=41 ['\']=43
)

press_release() {
  local code="$1"
  prlctl send-key-event "$VM" --scancode "$code" --event press
  prlctl send-key-event "$VM" --scancode "$code" --event release
}

send_special() {
  case "$1" in
    --enter|--ret|--return) press_release 28;;
    --backspace|--bs)       press_release 14;;
    --esc|--escape)         press_release 1;;
    --tab)                  press_release 15;;
    *) echo "ERROR: unknown special key $1" >&2; exit 3;;
  esac
}

send_text() {
  local text="$1"
  local i ch code
  for ((i=0; i<${#text}; i++)); do
    ch="${text:$i:1}"
    # Lowercase only — uppercase would need a shift-modifier sequence
    # (press LSHIFT scancode 42, press char, release char, release LSHIFT)
    # which is doable but unused so far.
    ch="$(echo -n "$ch" | tr 'A-Z' 'a-z')"
    code="${SC[$ch]:-}"
    if [[ -z "$code" ]]; then
      echo "WARN: no scancode for '$ch'; skipping" >&2
      continue
    fi
    press_release "$code"
    # Tiny inter-key gap so the guest's input ring drains.
    sleep 0.04
  done
}

# ---- dispatch ----
if [[ "$#" -eq 0 ]]; then
  echo "ERROR: nothing to send" >&2
  exit 2
fi

case "$1" in
  --line)
    shift
    send_text "${1:?--line needs a string}"
    press_release 28  # Enter
    ;;
  --enter|--ret|--return|--backspace|--bs|--esc|--escape|--tab)
    send_special "$1";;
  *)
    send_text "$1";;
esac
