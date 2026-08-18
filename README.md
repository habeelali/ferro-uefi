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

**Milestone 12 (done):** real `ExitBootServices` (`boot_services.rs`),
including the actual map-key handshake, not just a status code.
`GetMemoryMap` now hands back a map key that changes exactly when the
memory map does (a generation counter in `memory.rs`, bumped on every
`AllocatePages`) instead of always returning 0. `ExitBootServices`
checks the caller's key against the current generation and rejects a
stale one with `EFI_INVALID_PARAMETER` -- the real protocol a bootloader
is supposed to follow (fetch the map, note the key, call
ExitBootServices with that exact key, retry from the top if it was
rejected because something else changed the map in between). On
success, a handful of boot-only services (`AllocatePages`/
`AllocatePool`/`LoadImage`/`StartImage`) start correctly returning
`EFI_UNSUPPORTED`, while variable and other runtime services -- which
the spec says must keep working -- are untouched by the flag, and
`GetMemoryMap` itself is deliberately left ungated since the spec
explicitly permits calling it afterward too.

Calling `ExitBootServices` successfully is a one-way trip -- boot
services stay off for the rest of that boot, which would break Ferro's
own menu trying to `LoadImage` a second time in the same session -- so
this was verified with a temporary, isolated test call (wrong key
correctly rejected, real key correctly accepted, `AllocatePages`
correctly gated off afterward, variable services correctly still
working afterward) that was removed once confirmed, the same way
earlier temporary UART tracing was. The real implementation it
verified stays in `boot_services.rs`; only the throwaway test call
was reverted. Full regression (menu, SD/FAT32, PE loader) re-verified
clean afterward on both debug and release builds.

**Milestone 13 (done):** real SD **writes** (`sd.rs`, CMD24) and
genuine cross-reboot persistence for the variable store (`persist.rs`),
without needing FAT32 write support at all. FAT32 volumes always
reserve more sectors before the FAT tables than the four the spec
actually uses (boot sector, FSInfo, and their conventional backups at
sectors 6-7) -- mkfs.fat's default is 32 reserved sectors, leaving a
wide, genuinely unused margin real FAT32 implementations never touch.
`Fat32::private_scratch_region()` claims a conservative slice of that
padding (starting at sector 16, verified against real `mkfs.fat`
output rather than assumed) as private raw storage for a serialized
variable-store blob. The boot menu gained "Save Variables to SD", and
"Boot from SD" now auto-loads whatever was last saved, merging it into
the live store.

Verified about as thoroughly as this project gets: saved variables
(set earlier by the Boot Services smoke test) on one QEMU boot,
**independently** parsed the raw disk image's bytes at the exact
scratch-region offset with a standalone Python script (outside Ferro
entirely) and confirmed the magic, count, name, GUID, attributes, and
value all matched exactly -- proof the real SDHCI write path (not just
"looks plausible") put the right bytes in the right place. Then booted
a **completely fresh QEMU process** against that same disk image and
confirmed "Boot from SD" auto-loaded and correctly reported "1
variable(s) merged" -- genuine state surviving a full, independent
reboot, not just the same run. The no-prior-save case (fresh card,
never written) was also verified to fail gracefully rather than
misbehave. Full regression re-run clean afterward given how much of
`sd.rs`'s shared command-sending code changed to add write support.

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
- `src/ui.rs` -- boot log, splash screen, two-pane boot manager + settings
  screen, driven by UART and/or USB HID.
- `src/settings.rs` -- setup options (verbose boot, accent theme, USB
  enable) backed by real UEFI variables, not UI-only state.
- `src/pm.rs` -- BCM2837 power management (watchdog reset).
- `src/sd.rs` -- SDHCI SD card driver, read+write (see milestone 8's real-vs-emulated caveat).
- `src/fat32.rs` -- read-only FAT32: mount, list root, read file by name.
- `src/persist.rs` -- variable-store save/load via FAT32's unused reserved sectors.
- `src/pe.rs` -- PE32+ (AArch64) loader: parse, load, base-relocate.
- `src/usb.rs` -- DWC2 USB host controller driver (DMA-mode control transfers).
- `src/hid.rs` -- USB hub traversal + HID boot-protocol keyboard enumeration/polling.
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

## Milestone 14 (done): USB (dwc2) host + HID keyboard

The largest single piece of register-level work in the project so far.

- `usb.rs`: a DWC2 (BCM2837's USB 2.0 host controller) driver --
  core reset, forced host mode, root port power/reset with speed
  detection, and control transfers. First attempt used slave/FIFO-mode
  register pushes and got a STALL on every data stage; fetching QEMU's
  actual `hw/usb/hcd-dwc2.c` from its GitHub mirror (network access
  turned out to be available) showed its channel logic only ever reads
  and writes guest memory through the `HCDMA` register -- it doesn't
  implement FIFO push/pop at all. Rewrote around DMA-mode transfers
  (with `cache.rs` clean/invalidate around every transfer buffer, the
  same "not cache-coherent with something else that touches this
  memory" situation as the GPU mailbox) and a `GET_DESCRIPTOR(DEVICE)`
  request came back byte-exact correct: `bLength=18`,
  `bDescriptorType=1`, and a self-consistent 18-byte descriptor --
  which also revealed the root-port device is a hub (`bDeviceClass=9`),
  not a keyboard directly, matching what `info usb` showed all along.
- `hid.rs`: hub traversal (SET_ADDRESS, hub descriptor, per-port
  power+reset+status) to reach a downstream HID boot-protocol
  keyboard, its own enumeration (SET_ADDRESS, configuration descriptor
  parse, SET_CONFIGURATION), and interrupt-endpoint polling.

The interactive menu integration looked broken for a while (a
`sendkey j` sent over QEMU's monitor while the menu was up didn't move
the selection). Root cause turned out to be the *test harness*, not
the driver: verification used a fixed few-second sleep before sending
the key, racing actual (host-load-dependent) QEMU boot time. Once the
test waited for the real "USB HID keyboard connected" UART line before
sending a key, every press landed correctly and 1000s of polls/sec
were measured against `poll_new_keys()`. Screendump-verified the
on-screen highlight moving row-by-row on repeated presses. UART-driven
navigation is unaffected either way, and now both input paths also
drive every sub-screen ("press any key to return" included), not just
the main menu.

## Milestone 15 (done): setup UI overhaul + real settings

The boot menu was functionally fine but visually and functionally
thin: four items, one giant font size, nothing configurable, no live
system data. This pass:

- **Two-pane boot manager**: a menu list on the left, a live "SYSTEM
  STATUS" panel on the right (uptime, CPU/EL, timer frequency, MMU
  state, input device, accent theme, NVRAM usage) that keeps updating
  on an idle redraw timer even when nothing's being pressed, not just
  when the selection moves.
- **Real settings, not UI-only toggles** (`settings.rs`): Verbose Boot
  Log, Accent Theme, and USB HID Keyboard are backed by actual UEFI
  variables through the same `efi::variables` store GetVariable/
  SetVariable use -- changing one in the SETTINGS screen calls
  `variables::set()` directly, and "SAVE VARIABLES TO SD" persists it
  for real, since `persist.rs` serializes every in-use variable
  regardless of which code created it. Round-trip verified: toggle
  Verbose Boot off and Accent Theme to Green, save, then BOOT FROM SD
  (which re-merges persisted variables into the live store and
  re-syncs the settings cache) and confirm the SETTINGS screen still
  shows OFF/GREEN.
  - Verbose Boot Log toggles the boot log's real inter-line delay
    (`LINE_DELAY_TICKS`), not a cosmetic flag.
  - Accent Theme (Amber/Cyan/Green) recolors the whole UI immediately.
  - USB HID Keyboard, when disabled, skips `usb::init()` entirely on
    the next boot -- an actually-testable effect, not a stub.
- **Smaller, denser fonts** for menu/body text (scale 2 instead of 3)
  now that there's real content to lay out instead of four oversized
  labels; the splash screen keeps its large logo treatment since
  that's a one-time title screen, not a working UI.
- **SYSTEM INFO** now renders live register values on the framebuffer
  itself (MIDR_EL1, exception level, CNTFRQ_EL0, USB/NVRAM status)
  instead of a static summary plus "see UART log."
- **Found and fixed a real color bug** while adding the theme system:
  the mailbox framebuffer request was asking VideoCore for RGB pixel
  order (`TAG_SET_PIXEL_ORDER` = 1) but the actual memory layout
  QEMU's raspi3b model exposes for that tag value is BGR -- confirmed
  by sampling screendump pixels against known `0xRRGGBB` constants
  (e.g. `SELECT_BG = 0x2A3A4A` was rendering as `(74, 58, 42)`, the
  exact channel-reversed value). Every named color in the UI --
  including the "amber" accent that had been silently rendering blue
  since milestone 6 -- had been swapped the whole time. Fixed by
  requesting pixel order 0 instead; verified all three accent themes
  now render their actual intended hues.
- Added missing font glyphs (`. , ' ( ) + _ %`) that several existing
  UI strings were already relying on without anyone noticing they
  rendered as blank squares (e.g. "WHAT'S ON IT", "(FULL TEXT OVER
  UART)").

## Milestone 16 (done): automatic NON_VOLATILE variable persistence

`SetVariable` with the `EFI_VARIABLE_NON_VOLATILE` attribute now
actually persists immediately, not only when the user explicitly picks
SAVE VARIABLES TO SD. `persist.rs` caches the `Card`/`Fat32` handle
(both small, `Copy`, register-state-plus-geometry structs -- cheap to
stash, not open file handles) the first time either BOOT FROM SD or
SAVE VARIABLES TO SD successfully mounts a card; from then on, any
NON_VOLATILE `SetVariable` call -- through the real EFI_RUNTIME_SERVICES
table or from the SETTINGS screen -- triggers a background save
through that cached context. If no card has been mounted yet this
session, it's a silent no-op (there's nothing to write through), so
the explicit menu item is still what gets the *first* write on a
session that never otherwise touched the SD card.

Verified end to end: BOOT FROM SD to establish a mount, change the
accent theme in SETTINGS *without* ever pressing SAVE VARIABLES TO
SD, then cold-restart QEMU against the same disk image and BOOT FROM
SD again -- the SETTINGS screen still shows the changed theme.

Still backed by the reserved-sector scratch region, not real files --
see the next milestone.

## Next milestones

1. Real FAT32 write support, so persisted variables live in an actual
   file instead of a private, non-standard corner of the reserved
   sectors -- the scratch-region approach works but isn't something
   another OS's FAT32 driver would ever look at or understand.
2. Real Pi 3 hardware validation -- nothing here has touched physical
   hardware yet, and there are three known gaps waiting there: the
   pm.rs reset sequence (right code, unverified effect), sd.rs's
   controller choice (right controller for QEMU, wrong one for the
   physical SD slot -- would need an sdhost driver), and the
   framebuffer pixel-order fix above (only verified against QEMU's
   model, not a real VideoCore GPU).
