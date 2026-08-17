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

1. MMU + page tables (identity map RAM + device regions) -- now that
   faults are diagnosable, this is the next thing likely to produce them.
2. Timer / mailbox / GIC bring-up.
3. UEFI Boot Services core (memory map, protocol database).
4. SD/MMC + FAT32, so we can load an actual OS.
5. UEFI Runtime Services + variable storage.
6. Setup UI, boot manager.
