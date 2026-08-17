// Ferro UEFI — AArch64 boot entry for Raspberry Pi 3 / BCM2837.
//
// Entered at 0x80000 by the VideoCore firmware (real hardware, via
// armstub8.bin) or directly by QEMU's raspi3b -kernel loader. All four
// Cortex-A53 cores start executing here simultaneously, so cores 1-3 are
// parked immediately. Core 0 may land in EL3, EL2, or EL1 depending on
// the loader; this code drops cleanly to EL1 in each case.

.section .text.boot, "ax"
.global _start

_start:
    // Park every core except core 0 (MPIDR_EL1 affinity bits 0-7).
    mrs     x0, mpidr_el1
    and     x0, x0, #0xff
    cbz     x0, primary_core
park_loop:
    wfe
    b       park_loop

primary_core:
    // Determine current exception level.
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #3
    b.eq    from_el3
    cmp     x0, #2
    b.eq    from_el2
    b       el1_entry

from_el3:
    // Don't trap FP/SIMD to EL3 -- LLVM's AArch64 codegen uses NEON
    // register pairs (stp q.., ..) for ordinary stack spills, so this
    // must be clear well before any Rust code runs.
    msr     cptr_el3, xzr
    // Route EL2 in AArch64, no traps, non-secure world for the rest of
    // the boot chain.
    mov     x0, #(1 << 10 | 1 << 0)    // RW=1 (EL2 is AArch64), NS=1
    msr     scr_el3, x0
    mov     x0, #0x3c9                  // EL2h, DAIF masked
    msr     spsr_el3, x0
    adr     x0, from_el2_target
    msr     elr_el3, x0
    eret

from_el2_target:
    b       from_el2

from_el2:
    // Don't trap FP/SIMD to EL2, for the same reason as cptr_el3 above.
    msr     cptr_el2, xzr
    // Drop EL1 execution state to AArch64 and hand off.
    mov     x0, #(1 << 31)               // HCR_EL2.RW = 1
    msr     hcr_el2, x0
    mov     x0, #0x3c5                   // EL1h, DAIF masked
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0
    eret

el1_entry:
    // Enable FP/SIMD access at EL1/EL0 (CPACR_EL1.FPEN = 0b11) -- same
    // reason as the cptr_el3/cptr_el2 writes above.
    mov     x0, #(0b11 << 20)
    msr     cpacr_el1, x0
    isb

    // Install the exception vector table before any Rust code runs, so
    // a fault anywhere below here is reported instead of silently
    // hanging (see vectors.s).
    ldr     x0, =vector_table_el1
    msr     vbar_el1, x0
    isb

    // Stack for core 0.
    ldr     x0, =__stack_top
    mov     sp, x0

    // Zero .bss.
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
bss_zero_loop:
    cmp     x1, x2
    b.hs    bss_zero_done
    str     xzr, [x1], #8
    b       bss_zero_loop
bss_zero_done:

    bl      ferro_main
hang:
    wfe
    b       hang

.size _start, . - _start
.type _start, function
