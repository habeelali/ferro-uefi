//! Ferro — a from-scratch Rust UEFI firmware for the Raspberry Pi 3
//! (BCM2837, AArch64). This is milestone 1: entry from reset, drop to
//! EL1, bring up UART0, prove the boot chain end to end. No UEFI layers
//! yet — those build on top of this once the hardware bring-up is solid.

#![no_std]
#![no_main]

mod cache;
mod exceptions;
mod font;
mod framebuffer;
mod irq;
mod local_intc;
mod mailbox;
mod mmio;
mod mmu;
mod pm;
mod timer;
mod uart;
mod ui;

use core::fmt::Write;
use core::panic::PanicInfo;

core::arch::global_asm!(include_str!("boot.s"));
core::arch::global_asm!(include_str!("vectors.s"));

#[no_mangle]
pub extern "C" fn ferro_main() -> ! {
    unsafe { mmu::init() };

    let mut uart = uart::Uart::init();

    writeln!(uart, "\nFerro UEFI").ok();
    writeln!(uart, "Raspberry Pi 3 / BCM2837, AArch64").ok();
    writeln!(uart, "milestone 1: core0 -> EL1 -> UART online").ok();
    writeln!(uart, "milestone 3: MMU enabled, RAM + device regions identity-mapped").ok();

    timer::init(100); // 100 Hz tick
    unsafe {
        local_intc::enable_core0_timer_irq();
        core::arch::asm!("msr daifclr, #2"); // unmask IRQ
    }
    timer::sleep_ticks(100); // ~1s, proves IRQs are actually being delivered
    writeln!(
        uart,
        "milestone 4: timer/IRQ online via BCM2836 local block ({} ticks in ~1s)",
        timer::ticks()
    )
    .ok();

    match framebuffer::init(800, 600, 32) {
        Some(fb) => {
            writeln!(
                uart,
                "milestone 5: framebuffer {}x{} pitch={} @ {:p}",
                fb.width, fb.height, fb.pitch, fb.ptr
            )
            .ok();

            ui::splash(&fb);
            timer::sleep_ticks(150); // ~1.5s
            writeln!(uart, "milestone 6: entering boot menu (serial console input)").ok();
            ui::run(&fb, &mut uart);
        }
        None => {
            writeln!(uart, "milestone 5: framebuffer allocation FAILED").ok();
            loop {
                unsafe { core::arch::asm!("wfe") };
            }
        }
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
