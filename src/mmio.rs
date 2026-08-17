//! Peripheral base addresses for BCM2837 (Raspberry Pi 3), low-peripheral
//! mode. QEMU's `raspi3b` machine models peripherals at these same
//! addresses.

pub const PERIPHERAL_BASE: usize = 0x3F00_0000;
pub const GPIO_BASE: usize = PERIPHERAL_BASE + 0x0020_0000;
pub const UART0_BASE: usize = PERIPHERAL_BASE + 0x0020_1000;
pub const MBOX_BASE: usize = PERIPHERAL_BASE + 0x0000_B880;

/// BCM2836 "local" per-core control block (Broadcom calls it QA7). Not
/// a GIC -- BCM2837 doesn't have one. This is how the ARM generic
/// timer, mailboxes, and the GPU IRQ line get routed to each core.
pub const LOCAL_BASE: usize = 0x4000_0000;

#[inline(always)]
pub unsafe fn read(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

#[inline(always)]
pub unsafe fn write(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}
