//! Ferro — a from-scratch Rust UEFI firmware for the Raspberry Pi 3
//! (BCM2837, AArch64). This is milestone 1: entry from reset, drop to
//! EL1, bring up UART0, prove the boot chain end to end. No UEFI layers
//! yet — those build on top of this once the hardware bring-up is solid.

#![no_std]
#![no_main]

mod exceptions;
mod mmio;
mod uart;

use core::fmt::Write;
use core::panic::PanicInfo;

core::arch::global_asm!(include_str!("boot.s"));
core::arch::global_asm!(include_str!("vectors.s"));

#[no_mangle]
pub extern "C" fn ferro_main() -> ! {
    let mut uart = uart::Uart::init();

    writeln!(uart, "\nFerro UEFI").ok();
    writeln!(uart, "Raspberry Pi 3 / BCM2837, AArch64").ok();
    writeln!(uart, "milestone 1: core0 -> EL1 -> UART online").ok();

    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Best-effort: re-init UART (cheap, idempotent enough at this stage)
    // so a panic before ferro_main's uart is reachable still gets seen.
    let mut uart = uart::Uart::init();
    let _ = writeln!(uart, "\nPANIC: {info}");
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
