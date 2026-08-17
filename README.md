# Ferro

A from-scratch Rust UEFI firmware for the **Raspberry Pi 3 / BCM2837**
(AArch64, 4x Cortex-A53). Targets QEMU's `raspi3b` machine for
development and real Pi 3 hardware as the deployment target.

Ferro does not replace the Pi's VideoCore boot ROM or GPU firmware
(`bootcode.bin`/`start.elf`) -- those are immutable / closed on real
hardware. What Ferro replaces is the ARM-side stub that GPU firmware
hands off to: on real hardware this is `armstub8.bin`; under QEMU it's
whatever `-kernel` points at. Either way, Ferro's `_start` is the first
instruction executed on the ARM core.

## Status

**Milestone 1 (done):** reset -> park secondary cores -> drop
EL3/EL2 -> EL1 -> zero `.bss` -> UART0 online -> Rust banner printed.

**Milestone 2 (done):** EL1 exception vector table installed before any
Rust code runs. Every fault (data/instruction abort, undefined
instruction, alignment fault, ...) now prints its kind, decoded EC,
ESR/ELR/FAR/SPSR, and halts -- instead of silently hanging. Verified by
deliberately reading an unmapped address and confirming the report.

**Milestone 3 (done):** Stage-1 MMU enabled. Identity-mapped (4KiB
granule, level-1 as top level): RAM as Normal Write-Back/executable up
to the peripheral base, BCM2837 peripherals and the 0x4000_0000
ARM-local region as Device-nGnRnE/non-executable. Data and instruction
caches on. Verified two ways: normal boot still reaches the UART banner
with caches + translation live, and a deliberate read past the mapped
range (0x9000_0000) now reports a genuine "Translation fault, level 1"
(DFSC 0x05) through the same exception path -- unmapped memory now
faults *because of the page tables*, not because nothing answered the
bus.

**Milestone 4 (done):** ARM generic timer (CNTP) + interrupts. BCM2837
has no GIC -- interrupts route through the SoC's per-core "local"
control block instead (Broadcom calls it QA7); see `local_intc.rs`.
`vectors.s` gained a real IRQ path (full register save/restore, `eret`
to resume) alongside the existing fatal-only paths. Verified: armed a
100 Hz tick and slept on it, and got exactly 100 ticks in ~1s --
interrupts are actually firing and resuming correctly, not just
counting in a busy loop.

**Milestone 5 (done):** Framebuffer via the VideoCore property mailbox
(`mailbox.rs`, `framebuffer.rs`). Getting a buffer from the GPU means
crossing a cache/address boundary the CPU's MMU doesn't cover on its
own: the message buffer and framebuffer both need explicit clean+
invalidate (`cache.rs`) since the GPU doesn't snoop our D-cache, and
addresses exchanged with the GPU need its bus-address aliasing (request
via the `0xC000_0000` uncached alias, strip the alias bits back off
addresses it returns). Verified by capturing a real QEMU screendump and
checking exact pixel values at and outside a filled rectangle's
boundary -- not just that the mailbox call returned success.

**Milestone 6 (done):** Splash screen and interactive boot menu
(`font.rs`, `ui.rs`). No USB HID driver exists yet, so navigation is
over the UART serial console (arrow keys or j/k, Enter to select)
while the same UI renders to the framebuffer simultaneously. Items:
Boot from SD (reports not-yet-implemented -- true, that's milestone 8),
System Info (reads MIDR_EL1/CNTFRQ_EL0/CurrentEL for real and prints
them), Reboot (issues the real BCM283x watchdog-reset sequence via
`pm.rs`). Verified by driving QEMU's serial console with actual
keystrokes end-to-end -- not just rendering a static screen -- and
confirmed System Info's output is genuine hardware state (MIDR_EL1
decoded to a real Cortex-A53 ID, CNTFRQ_EL0 read 62.5MHz matching the
Pi 3's known generic timer frequency). One honest gap: Reboot sends the
correct real-hardware reset sequence, but QEMU's `bcm2835-powermgt`
model doesn't act on it, so nothing actually restarts under QEMU --
untested whether it resets real hardware.

No UEFI services yet; that's the next layer to build.

```
$ ./scripts/run-qemu.sh
Ferro UEFI
Raspberry Pi 3 / BCM2837, AArch64
milestone 1: core0 -> EL1 -> UART online
```

## Layout

- `src/boot.s` -- reset entry, core parking, EL3->EL2->EL1 drop, FP/SIMD
  enable, VBAR_EL1 install, stack + `.bss` setup, hand-off to Rust.
- `src/vectors.s` -- EL1 exception vector table; each vector forwards
  (kind, ESR, ELR, FAR, SPSR) to `rust_exception_handler`.
- `src/exceptions.rs` -- decodes and prints fatal exceptions, then halts.
- `src/mmu.rs` -- stage-1 page tables (identity map) and MMU enable.
- `src/local_intc.rs` -- BCM2836 local (QA7) per-core interrupt routing.
- `src/timer.rs` -- ARM generic timer (CNTP), tick counter.
- `src/irq.rs` -- IRQ dispatch (timer only, so far).
- `src/cache.rs` -- D-cache clean+invalidate for GPU-shared buffers.
- `src/mailbox.rs` -- VideoCore property-tag mailbox protocol.
- `src/framebuffer.rs` -- framebuffer allocation + pixel/rect/text drawing.
- `src/font.rs` -- 5x7 bitmap font (just the glyphs the UI uses).
- `src/ui.rs` -- splash screen + boot menu (UART-driven navigation).
- `src/pm.rs` -- BCM2837 power management (watchdog reset).
- `src/main.rs` -- `ferro_main`, panic handler.
- `src/mmio.rs` -- BCM2837 peripheral base addresses, volatile MMIO helpers.
- `src/uart.rs` -- PL011 UART0 driver (+ the GPIO alt-function setup it needs).
- `linker.ld` -- links at `0x80000`, the Pi's default AArch64 kernel load
  address (also what QEMU's `raspi3b -kernel` loader uses).
- `scripts/run-qemu.sh` -- build-and-boot under QEMU.

## Building

```
cargo build            # debug
cargo build --release  # release
```

Target is `aarch64-unknown-none` (set via `.cargo/config.toml`); no
`build-std` needed since that target ships precompiled `core`.

## Running under QEMU

```
./scripts/run-qemu.sh          # debug
./scripts/run-qemu.sh release  # release
```

Note: if you pipe QEMU's `-serial stdio` output through something like
`head`, small one-shot output can appear to vanish -- that's a pipe
buffering artifact in the consuming command, not a firmware bug. Piping
straight to a terminal, or using `-serial file:out.log`, doesn't have
this problem.

## Real hardware

Not yet exercised on physical Pi 3, but the intended path is:

```
SD card /
├── bootcode.bin      (stock Pi firmware)
├── start.elf         (stock Pi firmware)
├── config.txt        (enable_uart=1, arm_64bit=1, armstub=armstub8.bin)
└── armstub8.bin       <- Ferro's binary, renamed
```

`objcopy -O binary` the ELF to a flat binary and place it as
`armstub8.bin`. This hasn't been validated on hardware yet -- do that
before trusting it beyond QEMU.

## Known gotcha worth remembering

LLVM's AArch64 codegen uses NEON register-pair instructions (`stp
q8, q9, ...`) for ordinary stack spills in Rust code, even code that
never touches floating point. Without a vector table, an FP/SIMD trap
(EC 0x07) with no handler retriggers itself forever -- total silence,
no crash message, nothing. `boot.s` clears `cptr_el3`/`cptr_el2` and
sets `cpacr_el1.FPEN` before ever calling into Rust specifically to
avoid this. If a future change moves Rust execution earlier in the EL
chain (e.g. skips through EL2 without landing in `el1_entry`), make
sure this still happens before the first `bl` into Rust.

## Next milestones

1. UEFI Boot Services core (memory map, protocol database).
2. SD/MMC + FAT32, so "Boot from SD" in the menu can do something real.
3. UEFI Runtime Services + variable storage.
4. USB (dwc2) + HID, so the menu can be driven from a real keyboard.
