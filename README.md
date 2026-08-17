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
Boot from SD (now real -- see milestone 8), System Info (reads
MIDR_EL1/CNTFRQ_EL0/CurrentEL for real and prints
them), Reboot (issues the real BCM283x watchdog-reset sequence via
`pm.rs`). Verified by driving QEMU's serial console with actual
keystrokes end-to-end -- not just rendering a static screen -- and
confirmed System Info's output is genuine hardware state (MIDR_EL1
decoded to a real Cortex-A53 ID, CNTFRQ_EL0 read 62.5MHz matching the
Pi 3's known generic timer frequency). One honest gap: Reboot sends the
correct real-hardware reset sequence, but QEMU's `bcm2835-powermgt`
model doesn't act on it, so nothing actually restarts under QEMU --
untested whether it resets real hardware.

**Milestone 7 (done):** UEFI Boot Services core (`src/efi/`) -- the
first genuinely UEFI-spec layer, not just bare-metal bring-up. A
physical page allocator (bump-pointer; `FreePages`/`FreePool` are
accepted but don't reclaim yet -- honest first slice) backs a real
`EFI_MEMORY_DESCRIPTOR` memory map built from actual boot state
(linker symbols for the firmware's own footprint, the real framebuffer
address/size from the mailbox, the real MMU region boundaries), plus a
fixed-capacity protocol database. `EFI_BOOT_SERVICES` and
`EFI_SYSTEM_TABLE` are laid out field-for-field per the UEFI 2.x spec
-- correct ABI matters since real EFI applications will call through
these by fixed offset -- with a working subset wired to real logic
(memory services, protocol install/locate/handle, Stall, CopyMem/
SetMem, a real CRC32 used for both `CalculateCrc32` and the tables'
own header checksums) and the rest honestly stubbed at
`EFI_UNSUPPORTED` pending the code that needs them. Verified by calling
through the actual function-pointer table -- not the Rust functions
directly -- the same way a loaded EFI application eventually will:
`AllocatePages`/`AllocatePool` return real sequential addresses,
`InstallProtocolInterface`/`LocateProtocol` round-trip a test GUID
correctly, and `GetMemoryMap`'s 8 returned descriptors match reality
exactly (firmware image bounds, the live allocator bump pointer, the
real framebuffer region, and the peripheral/ARM-local MMIO ranges all
line up with independently-known values).

**Milestone 8 (done):** SD card + FAT32 (`sd.rs`, `fat32.rs`) --
"Boot from SD" in the menu now really talks to hardware. One
significant real-vs-emulated mismatch found and documented up front:
Pi 3 has *two* SD-capable controllers, and which one is which differs
between real hardware and QEMU. On a physical Pi 3, the external
microSD slot is wired to the proprietary `sdhost` controller while the
standard SDHCI (Arasan) controller only reaches the onboard WiFi chip
-- confirmed via Raspberry Pi's own device tree. QEMU's `raspi3b`,
however, only attaches a `-drive if=sd` image to SDHCI; `sdhost`'s bus
in the emulated machine is left with no card and no straightforward
way to attach one. Rather than build something unverifiable, Ferro
targets SDHCI: fully real, fully tested end-to-end, but **won't read
the boot SD card on physical Pi 3 hardware** without a further
sdhost-specific driver -- tracked the same way as the pm.rs reset gap,
not hidden.

Found one real bug the hard way: the SDHCI Command register (word bits
[31:16]) and Transfer Mode register (word bits [15:0]) share one
32-bit register at offset 0x0C, and Data Present Select is a *Command*
register bit -- an early version set it at word bit 5 instead of word
bit 21, landing it in a reserved Transfer Mode bit instead. Command
completion still fired (the controller didn't know a data phase was
coming), but Buffer Read Ready never did, so every block read hung.
Diagnosed by adding temporary UART tracing around each wait, narrowing
it to exactly that transition, then removed once fixed.

Verified with a real MBR-partitioned FAT32 image built via `parted` +
`mkfs.fat --offset` + `mtools` (no loopback mount needed, no root):
booted QEMU with `-drive if=sd,file=...`, selected Boot from SD over
the serial console, and got back an exact root-directory listing (two
files, correct sizes) plus a byte-for-byte-correct multi-line file
read through the full stack -- MBR parse, BPB parse, FAT cluster-chain
walk, data read -- not just a mount success code. A no-card run was
verified too: a real, decoded hardware error (Command Timeout) and a
clean return to the menu, not a hang.

**Milestone 9 (done):** UI overhaul (`ui.rs`). Every screen after the
splash now shares real chrome -- a header bar with the firmware name
and a per-screen title, a bordered content panel, a footer bar with a
context-specific hint -- instead of being drawn ad hoc. The boot menu
gained a proper highlight bar (filled row + left accent stripe, not
just colored text and a `>` cursor) and a live description line for
whichever item is selected. Body text on every screen -- the boot log,
system info, Boot from SD's directory listing and file preview, error
screens -- now prints one line at a time with a real delay
(`print_lines`), each line also mirrored to UART as it appears, rather
than the previous instant full-screen blit. Verified with QEMU
screendumps: all seven chrome/state colors (background, header/footer,
border, selection highlight, dim, accent, foreground) present exactly
where the layout code puts them, and the existing keystroke-driven
interaction tests (System Info, Boot from SD with a real FAT32 image)
re-verified against the new code with identical, correct output.

**Milestone 10 (done):** PE/COFF loader (`pe.rs`) + real `LoadImage`/
`StartImage` (`boot_services.rs`) + `EFI_LOADED_IMAGE_PROTOCOL`
(`efi/protocols.rs`). "Boot from SD" now looks for a `.EFI` file on
the volume and actually loads and runs it: parses the DOS/COFF/
Optional headers, copies each section to its RVA in a freshly
allocated image (via the real `AllocatePages`), and -- since our
allocator never lands an image at its preferred `ImageBase` -- applies
`IMAGE_REL_BASED_DIR64` base relocations before calling the entry
point with `(ImageHandle, SystemTable*)`, exactly the EFI calling
convention (which on AArch64 is just plain AAPCS64, no ABI shim
needed).

No `aarch64-unknown-uefi` Rust target was available offline to build a
real reference `.efi`, so the test binary was built by hand instead,
in two independently-checkable pieces: the actual AArch64 machine code
was assembled through the same `global_asm!` pipeline the firmware
itself already uses (so it's real, verifier-checked code, not
hand-encoded bytes), while the PE32+ container around it was
constructed field-by-field in a small script and independently
confirmed well-formed by the system `file` command before Ferro ever
saw it. The test payload embeds one self-referential pointer, patched
in the file to hold the value it *would* have had if linked at its
declared `ImageBase` -- making relocation not just exercised but
required for the test to print the right answer.

Found two real bugs this way, on the first run:
- An off-by-24 offset error in the Optional Header parsing
  (`NumberOfRvaAndSizes` and the data directory array both read from
  the wrong offset) that made the relocation directory look empty, so
  relocations were silently skipped -- caught because the printed
  "relocated" pointer exactly matched the *pre*-relocation value
  instead of the expected post-relocation address.
- A calling-convention bug in the *test binary itself* (not Ferro):
  nested `bl` calls in the hand-written entry stub clobbered the
  return address in `x30` without saving it first, so the final `ret`
  jumped back into the middle of the stub instead of back into Ferro
  -- an infinite loop, caught immediately from the flood of blank
  lines it produced.

After both fixes: `LoadImage` returns a real handle, `HandleProtocol`
fetches `EFI_LOADED_IMAGE_PROTOCOL` back and its `ImageBase`
independently matches what `LoadImage` reported, and the test app
(called through the real, relocated entry point) printed its own
`ImageHandle` and `SystemTable` pointer arguments back over UART --
both exactly matching Ferro's own values -- along with the relocated
self-pointer, which exactly matched `load_base + RVA`, not the
pre-relocation value. `StartImage` then correctly received
`EFI_SUCCESS` back through a real `ret`, and the boot menu resumed
normally afterward. Verified on both debug and release builds.

**Milestone 11 (done):** UEFI Runtime Services (`efi/runtime_services.rs`,
`efi/variables.rs`) -- `EFI_RUNTIME_SERVICES` laid out field-for-field
per spec, wired into `EFI_SYSTEM_TABLE.RuntimeServices`. Real logic
backs the parts that can be real on this hardware: `GetVariable`/
`SetVariable`/`GetNextVariableName`/`QueryVariableInfo` are a genuine
fixed-capacity (32 variables, 512 bytes each) in-RAM store -- no
persistence across reboots yet, since there's no flash/NVRAM driver,
an honest first slice rather than a hidden gap. `ResetSystem` reuses
`pm.rs`'s real watchdog sequence instead of being stubbed. `GetTime`/
`SetTime`/`GetWakeupTime`/`SetWakeupTime` return `EFI_UNSUPPORTED` --
not a stand-in for unwritten code, but the spec-legal, honest answer
for a board with no RTC or wakeup-timer hardware.
`SetVirtualAddressMap`/`ConvertPointer` are correct no-ops for Ferro
specifically, since boot and runtime both stay in the identity map
`mmu.rs` set up -- there's no virtual/physical split to translate.
Verified by calling through the real function-pointer table:
`SetVariable`/`GetVariable` round-trip real data correctly,
`GetNextVariableName` enumerates back the exact GUID that was set,
and `QueryVariableInfo` reports numbers that check out exactly against
the store's real fixed capacity and what's actually used
(`remaining = 16384 - 22`, the exact byte length of the test variable).

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
- `src/ui.rs` -- boot log, splash screen, boot menu (UART-driven navigation).
- `src/pm.rs` -- BCM2837 power management (watchdog reset).
- `src/sd.rs` -- SDHCI SD card driver (see milestone 8's real-vs-emulated caveat).
- `src/fat32.rs` -- read-only FAT32: mount, list root, read file by name.
- `src/pe.rs` -- PE32+ (AArch64) loader: parse, load, base-relocate.
- `src/efi/` -- UEFI Boot Services core:
  - `types.rs` -- EFI_STATUS, EFI_GUID, EFI_MEMORY_DESCRIPTOR, etc.
  - `memory.rs` -- physical page allocator + memory map builder.
  - `protocol_db.rs` -- fixed-capacity handle/protocol database.
  - `protocols.rs` -- protocol structs installed on handles (EFI_LOADED_IMAGE_PROTOCOL so far).
  - `boot_services.rs` -- EFI_BOOT_SERVICES: real subset + spec-shaped stubs.
  - `runtime_services.rs` -- EFI_RUNTIME_SERVICES: variables + real ResetSystem.
  - `variables.rs` -- fixed-capacity in-RAM UEFI variable store.
  - `system_table.rs` -- EFI_SYSTEM_TABLE.
  - `crc32.rs` -- CRC-32 shared by CalculateCrc32 and table header checksums.
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

To try "Boot from SD" with a real FAT32 image (no loopback mount, no
root needed):

```
dd if=/dev/zero of=sd.img bs=1M count=64
parted -s sd.img mklabel msdos
parted -s sd.img mkpart primary fat32 1MiB 100%
mkfs.fat -F 32 --offset=2048 sd.img
mcopy -i sd.img@@1M somefile.txt ::SOMEFILE.TXT
qemu-system-aarch64 -M raspi3b -kernel target/aarch64-unknown-none/debug/ferro \
    -drive if=sd,file=sd.img,format=raw -serial stdio -display none
```

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

1. USB (dwc2) + HID, so the menu can be driven from a real keyboard.
2. Persist the variable store across reboots (needs a flash/NVRAM
   driver, or at minimum writing it to the SD card) -- currently
   real but RAM-only.
3. A menu item that actually boots what's loaded (call ExitBootServices
   and hand off) instead of returning to the menu after StartImage.
4. Real Pi 3 hardware validation -- nothing here has touched physical
   hardware yet, and there are two known gaps waiting there: the
   pm.rs reset sequence (right code, unverified effect) and sd.rs's
   controller choice (right controller for QEMU, wrong one for the
   physical SD slot -- would need an sdhost driver).
