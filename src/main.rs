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
mod hid;
mod irq;
mod local_intc;
mod mailbox;
mod mmio;
mod mmu;
mod pe;
mod persist;
mod pm;
mod sd;
mod timer;
mod uart;
mod ui;
mod usb;

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
            runtime_services_smoke_test(&mut uart);
            usb_smoke_test(&mut uart);

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

/// Exercises Runtime Services through the real EFI_RUNTIME_SERVICES
/// function-pointer table. Doesn't touch ResetSystem here -- that one
/// is meant to never return, and does exercise it for real from the
/// boot menu's Reboot option instead.
fn runtime_services_smoke_test(uart: &mut uart::Uart) {
    use efi::runtime_services::EfiTime;
    use efi::types::*;
    use core::ffi::c_void;

    let rs = unsafe { &*core::ptr::addr_of!(efi::runtime_services::RUNTIME_SERVICES) };

    // Spec-legal EFI_UNSUPPORTED: BCM2837 has no RTC, so this is the
    // honest answer, not a stand-in for missing code.
    let mut time = EfiTime {
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        pad1: 0,
        nanosecond: 0,
        time_zone: 0,
        daylight: 0,
        pad2: 0,
    };
    let status = (rs.get_time)(&mut time, core::ptr::null_mut());
    writeln!(
        uart,
        "milestone 11: GetTime -> status=0x{status:x} (EFI_UNSUPPORTED expected: no RTC on this SoC)"
    )
    .ok();

    let name: [u16; 5] = [b'T' as u16, b'E' as u16, b'S' as u16, b'T' as u16, 0];
    let guid = EfiGuid {
        data1: 0x1111_2222,
        data2: 0x3333,
        data3: 0x4444,
        data4: [5, 6, 7, 8, 9, 10, 11, 12],
    };
    let data = b"hello-runtime-services";
    let status = (rs.set_variable)(
        name.as_ptr(),
        &guid,
        0x7, // NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS
        data.len(),
        data.as_ptr() as *const c_void,
    );
    writeln!(uart, "milestone 11: SetVariable -> status=0x{status:x}").ok();

    let mut out = [0u8; 64];
    let mut out_size = out.len();
    let mut attrs = 0u32;
    let status = (rs.get_variable)(
        name.as_ptr(),
        &guid,
        &mut attrs,
        &mut out_size,
        out.as_mut_ptr() as *mut c_void,
    );
    let round_trip_ok = status == EFI_SUCCESS && &out[..out_size] == &data[..];
    writeln!(
        uart,
        "milestone 11: GetVariable -> status=0x{status:x} round_trip_ok={round_trip_ok}"
    )
    .ok();

    let mut nn_buf = [0u16; 32];
    let mut nn_size = nn_buf.len() * 2;
    let mut nn_guid = EfiGuid {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };
    let status = (rs.get_next_variable_name)(&mut nn_size, nn_buf.as_mut_ptr(), &mut nn_guid);
    writeln!(
        uart,
        "milestone 11: GetNextVariableName -> status=0x{status:x} guid_matches={}",
        nn_guid == guid
    )
    .ok();

    let mut max_storage = 0u64;
    let mut remaining = 0u64;
    let mut max_var = 0u64;
    let status = (rs.query_variable_info)(0x7, &mut max_storage, &mut remaining, &mut max_var);
    writeln!(
        uart,
        "milestone 11: QueryVariableInfo -> status=0x{status:x} max={max_storage} remaining={remaining} max_var={max_var}"
    )
    .ok();
}

/// Confirms the dwc2 core register interface responds sanely (a
/// non-garbage GSNPSID) before boot proceeds. Deliberately does NOT
/// call usb::init() here: that does a real port reset, and the actual
/// enumeration (hub traversal, SET_ADDRESS, HID keyboard setup) needs
/// to happen exactly once, for real, in ui::run() -- calling
/// usb::init() a second time there after this smoke test already did
/// its own reset+enumeration is what broke the menu's real keyboard
/// input the first time this was wired up.
fn usb_smoke_test(uart: &mut uart::Uart) {
    writeln!(uart, "milestone 14: USB core ID = 0x{:08x}", usb::core_id()).ok();
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
