#!/usr/bin/env bash
# Boot Ferro under QEMU's raspi3b machine model.
#
# QEMU's raspi3b does not reproduce the Pi's VideoCore boot chain
# (bootcode.bin/start.elf/armstub8.bin) -- it loads our ELF straight in
# and starts all 4 Cortex-A53 cores at its entry point, same as the real
# armstub8.bin hand-off would. That's exactly the milestone-1 target:
# our AArch64 code as the first (and only) thing that runs after reset.
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="${1:-debug}"
BIN="target/aarch64-unknown-none/${PROFILE}/ferro"

if [[ ! -f "$BIN" ]]; then
    echo "error: $BIN not found -- build first, e.g.:" >&2
    echo "  cargo build                 # debug" >&2
    echo "  cargo build --release        # release" >&2
    exit 1
fi

exec qemu-system-aarch64 \
    -M raspi3b \
    -kernel "$BIN" \
    -serial stdio \
    -display none
