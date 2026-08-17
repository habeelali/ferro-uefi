//! Fatal exception reporting. Every vector in `vectors.s` lands here with
//! (kind, ESR_EL1, ELR_EL1, FAR_EL1, SPSR_EL1) and never returns: the
//! point is to turn a silent hang into a printed reason.

use crate::uart;
use core::fmt::Write;

const KIND_NAMES: [&str; 16] = [
    "Synchronous (current EL, SP0)",
    "IRQ (current EL, SP0)",
    "FIQ (current EL, SP0)",
    "SError (current EL, SP0)",
    "Synchronous (current EL, SPx)",
    "IRQ (current EL, SPx)",
    "FIQ (current EL, SPx)",
    "SError (current EL, SPx)",
    "Synchronous (lower EL, AArch64)",
    "IRQ (lower EL, AArch64)",
    "FIQ (lower EL, AArch64)",
    "SError (lower EL, AArch64)",
    "Synchronous (lower EL, AArch32)",
    "IRQ (lower EL, AArch32)",
    "FIQ (lower EL, AArch32)",
    "SError (lower EL, AArch32)",
];

/// A selection of ESR_EL1.EC values worth naming; anything else just
/// prints as a raw hex EC.
fn ec_name(ec: u64) -> Option<&'static str> {
    Some(match ec {
        0x00 => "Unknown reason",
        0x07 => "SVE/Advanced SIMD/FP access trapped",
        0x0E => "Illegal Execution state",
        0x15 => "SVC instruction (AArch64)",
        0x20 => "Instruction Abort, lower EL",
        0x21 => "Instruction Abort, same EL",
        0x22 => "PC alignment fault",
        0x24 => "Data Abort, lower EL",
        0x25 => "Data Abort, same EL",
        0x26 => "SP alignment fault",
        0x2C => "Trapped FP exception (AArch64)",
        0x3C => "BRK instruction (AArch64)",
        _ => return None,
    })
}

#[no_mangle]
extern "C" fn rust_exception_handler(kind: u64, esr: u64, elr: u64, far: u64, spsr: u64) -> ! {
    let mut u = uart::Uart::init();

    let kind_name = KIND_NAMES
        .get(kind as usize)
        .copied()
        .unwrap_or("unknown vector");
    let ec = (esr >> 26) & 0x3f;

    writeln!(u, "\n!! Ferro: unhandled exception !!").ok();
    writeln!(u, "  kind : {kind_name}").ok();
    match ec_name(ec) {
        Some(name) => writeln!(u, "  EC   : 0x{ec:02x} ({name})").ok(),
        None => writeln!(u, "  EC   : 0x{ec:02x}").ok(),
    };
    writeln!(u, "  ESR  : 0x{esr:08x}").ok();
    writeln!(u, "  ELR  : 0x{elr:016x}").ok();
    writeln!(u, "  FAR  : 0x{far:016x}").ok();
    writeln!(u, "  SPSR : 0x{spsr:08x}").ok();
    writeln!(u, "halting.").ok();

    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
