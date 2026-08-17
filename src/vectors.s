// AArch64 exception vector table for EL1.
//
// We run at EL1h (SPSel=1, SP_EL1), so in practice only the "current EL,
// SPx" group is expected to fire. All 16 slots are filled anyway so a
// stray IRQ/FIQ/SError, or an unexpected AArch32 trap, produces a
// diagnostic instead of silently running off into whatever garbage sits
// at the vector base (which is exactly the failure mode this replaces --
// see the FP/SIMD trap bug in boot.s history).
//
// Every slot except IRQ-at-SPx is fatal: no register save, no resume,
// just report and halt. IRQ-at-SPx is the one path that must actually
// return, since a timer tick isn't an error -- it gets a real context
// save/restore/eret routine (irq_entry) instead of exception_common.

.section .text.vectors, "ax"

.macro ventry kind
.balign 0x80
    mov     x0, #\kind
    b       exception_common
.endm

.balign 0x800
.global vector_table_el1
vector_table_el1:
    ventry 0    // Synchronous, current EL, SP0
    ventry 1    // IRQ,         current EL, SP0
    ventry 2    // FIQ,         current EL, SP0
    ventry 3    // SError,      current EL, SP0

    ventry 4    // Synchronous, current EL, SPx
.balign 0x80
    b       irq_entry           // IRQ, current EL, SPx -- the live path
    ventry 6    // FIQ,         current EL, SPx
    ventry 7    // SError,      current EL, SPx

    ventry 8    // Synchronous, lower EL, AArch64
    ventry 9    // IRQ,         lower EL, AArch64
    ventry 10   // FIQ,         lower EL, AArch64
    ventry 11   // SError,      lower EL, AArch64

    ventry 12   // Synchronous, lower EL, AArch32
    ventry 13   // IRQ,         lower EL, AArch32
    ventry 14   // FIQ,         lower EL, AArch32
    ventry 15   // SError,      lower EL, AArch32

exception_common:
    mrs     x1, esr_el1
    mrs     x2, elr_el1
    mrs     x3, far_el1
    mrs     x4, spsr_el1
    bl      rust_exception_handler
exception_hang:
    wfe
    b       exception_hang

// Full GP-register + ELR/SPSR save, dispatch to Rust, restore, eret.
// 17 x 16-byte slots: 15 register pairs (x0-x29), one pair for
// (x30, elr_el1), one for spsr_el1 (padded).
.equ CTX_SIZE, 17 * 16

.balign 4
irq_entry:
    sub     sp, sp, #CTX_SIZE
    stp     x0, x1, [sp, #16*0]
    stp     x2, x3, [sp, #16*1]
    stp     x4, x5, [sp, #16*2]
    stp     x6, x7, [sp, #16*3]
    stp     x8, x9, [sp, #16*4]
    stp     x10, x11, [sp, #16*5]
    stp     x12, x13, [sp, #16*6]
    stp     x14, x15, [sp, #16*7]
    stp     x16, x17, [sp, #16*8]
    stp     x18, x19, [sp, #16*9]
    stp     x20, x21, [sp, #16*10]
    stp     x22, x23, [sp, #16*11]
    stp     x24, x25, [sp, #16*12]
    stp     x26, x27, [sp, #16*13]
    stp     x28, x29, [sp, #16*14]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x30, x0, [sp, #16*15]
    str     x1, [sp, #16*16]

    bl      rust_irq_handler

    ldr     x1, [sp, #16*16]
    ldp     x30, x0, [sp, #16*15]
    msr     spsr_el1, x1
    msr     elr_el1, x0
    ldp     x0, x1, [sp, #16*0]
    ldp     x2, x3, [sp, #16*1]
    ldp     x4, x5, [sp, #16*2]
    ldp     x6, x7, [sp, #16*3]
    ldp     x8, x9, [sp, #16*4]
    ldp     x10, x11, [sp, #16*5]
    ldp     x12, x13, [sp, #16*6]
    ldp     x14, x15, [sp, #16*7]
    ldp     x16, x17, [sp, #16*8]
    ldp     x18, x19, [sp, #16*9]
    ldp     x20, x21, [sp, #16*10]
    ldp     x22, x23, [sp, #16*11]
    ldp     x24, x25, [sp, #16*12]
    ldp     x26, x27, [sp, #16*13]
    ldp     x28, x29, [sp, #16*14]
    add     sp, sp, #CTX_SIZE
    eret
