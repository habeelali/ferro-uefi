// AArch64 exception vector table for EL1.
//
// We run at EL1h (SPSel=1, SP_EL1), so in practice only the "Synchronous -
// current EL, SPx" slot is expected to fire until interrupts/EL0 exist.
// All 16 slots are filled anyway so a stray IRQ/FIQ/SError, or an
// unexpected AArch32 trap, produces a diagnostic instead of silently
// running off into whatever garbage sits at the vector base (which is
// exactly the failure mode this replaces -- see the FP/SIMD trap bug in
// boot.s history).
//
// Each vector deliberately does NOT attempt to save/restore full
// register state or resume execution: every path here is fatal. The
// goal is turning a silent hang into a printed reason, not recovery.

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
    ventry 5    // IRQ,         current EL, SPx
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
