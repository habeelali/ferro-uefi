//! Ferro — a from-scratch Rust UEFI firmware for the Raspberry Pi 3
//! (BCM2837, AArch64). This is milestone 1: entry from reset, drop to
//! EL1, bring up UART0, prove the boot chain end to end. No UEFI layers
//! yet — those build on top of this once the hardware bring-up is solid.

#![no_std]
#![no_main]

mod cache;
mod efi;
mod exceptions;
mod fat32;
mod font;
mod framebuffer;
mod irq;
mod local_intc;
mod mailbox;
mod mmio;
mod mmu;
mod pm;
mod sd;
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
    efi::init();

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

            efi::boot_services::set_framebuffer_region(fb.ptr as u64, (fb.pitch as u64) * (fb.height as u64));
            boot_services_smoke_test(&mut uart);

            ui::boot_log(
                &fb,
                &mut uart,
                &[
                    "CORE0 -> EL1 (BOOT.S)",
                    "EXCEPTION VECTORS INSTALLED (VECTORS.S)",
                    "MMU ENABLED: RAM=NORMAL WB, MMIO=DEVICE-NGNRNE",
                    "TIMER/IRQ ONLINE (BCM2836 LOCAL BLOCK, NOT A GIC)",
                    "FRAMEBUFFER ALLOCATED VIA VIDEOCORE MAILBOX",
                    "STARTING BOOT MENU...",
                ],
            );
            timer::sleep_ticks(60); // brief hold once the log finishes printing

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

/// Exercises Boot Services entirely through the real EFI_BOOT_SERVICES
/// function-pointer table -- the same table (same ABI) a loaded EFI
/// application will eventually call through, once there's a loader.
fn boot_services_smoke_test(uart: &mut uart::Uart) {
    use efi::types::*;

    let bs = unsafe { &*core::ptr::addr_of!(efi::boot_services::BOOT_SERVICES) };

    let mut page: u64 = 0;
    let status = (bs.allocate_pages)(0, EFI_BOOT_SERVICES_DATA, 2, &mut page);
    writeln!(
        uart,
        "milestone 7: AllocatePages(2) -> status=0x{status:x} base=0x{page:x}"
    )
    .ok();

    let mut pool: *mut core::ffi::c_void = core::ptr::null_mut();
    let status = (bs.allocate_pool)(EFI_BOOT_SERVICES_DATA, 100, &mut pool);
    writeln!(
        uart,
        "milestone 7: AllocatePool(100) -> status=0x{status:x} ptr={pool:p}"
    )
    .ok();

    let test_guid = EfiGuid {
        data1: 0x1234_5678,
        data2: 0xABCD,
        data3: 0xEF01,
        data4: [1, 2, 3, 4, 5, 6, 7, 8],
    };
    let marker: u32 = 0xFEED_FACE;
    let marker_ptr = &marker as *const u32 as *mut core::ffi::c_void;
    let mut handle: EfiHandle = core::ptr::null_mut();
    let status = (bs.install_protocol_interface)(&mut handle, &test_guid, 0, marker_ptr);
    writeln!(
        uart,
        "milestone 7: InstallProtocolInterface -> status=0x{status:x} handle={handle:p}"
    )
    .ok();

    let mut found: *mut core::ffi::c_void = core::ptr::null_mut();
    let status = (bs.locate_protocol)(&test_guid, core::ptr::null_mut(), &mut found);
    writeln!(
        uart,
        "milestone 7: LocateProtocol -> status=0x{status:x} round_trip_ok={}",
        found == marker_ptr
    )
    .ok();

    let mut map = [EfiMemoryDescriptor {
        ty: 0,
        physical_start: 0,
        virtual_start: 0,
        number_of_pages: 0,
        attribute: 0,
    }; 8];
    let mut map_size = core::mem::size_of_val(&map);
    let mut map_key = 0usize;
    let mut desc_size = 0usize;
    let mut desc_version = 0u32;
    let status = (bs.get_memory_map)(
        &mut map_size,
        map.as_mut_ptr(),
        &mut map_key,
        &mut desc_size,
        &mut desc_version,
    );
    let count = map_size / desc_size.max(1);
    writeln!(
        uart,
        "milestone 7: GetMemoryMap -> status=0x{status:x} entries={count}"
    )
    .ok();
    for d in &map[..count] {
        writeln!(
            uart,
            "  [{:#010x}-{:#010x}) type={} pages={}",
            d.physical_start,
            d.physical_start + d.number_of_pages * 4096,
            d.ty,
            d.number_of_pages
        )
        .ok();
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
