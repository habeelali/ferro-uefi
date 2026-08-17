#!/usr/bin/env bash
# Boot Ferro under QEMU's raspi3b machine model.
#
# QEMU's raspi3b does not reproduce the Pi's VideoCore boot chain
# (bootcode.bin/start.elf/armstub8.bin) -- it loads our ELF straight in
# and starts all 4 Cortex-A53 cores at its entry point, same as the real
# armstub8.bin hand-off would. That's exactly the milestone-1 target:
# our AArch64 code as the first (and only) thing that runs after reset.
#
# Usage: run-qemu.sh [debug|release] [--gui]
#   --gui opens a real window showing the framebuffer (splash/boot
#   menu). Keyboard input for the menu still goes through THIS
#   terminal (arrows/j/k/Enter) -- the menu reads UART, not the
#   graphical window's keyboard, since there's no USB HID driver yet.
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="debug"
DISPLAY_MODE="none"
for arg in "$@"; do
    case "$arg" in
        --gui) DISPLAY_MODE="gtk" ;;
        debug|release) PROFILE="$arg" ;;
        *)
            echo "error: unrecognized argument '$arg'" >&2
            echo "usage: $0 [debug|release] [--gui]" >&2
            exit 1
            ;;
    esac
done

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
    -display "$DISPLAY_MODE"
