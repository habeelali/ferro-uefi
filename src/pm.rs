//! BCM2837 power management: just enough to trigger a real watchdog
//! reset (there's no other software-visible reset path on this SoC).

use crate::mmio::{self, PERIPHERAL_BASE};

const PM_BASE: usize = PERIPHERAL_BASE + 0x0010_0000;
const PM_RSTC: usize = PM_BASE + 0x1C;
const PM_WDOG: usize = PM_BASE + 0x24;

const PM_PASSWORD: u32 = 0x5A00_0000;
const PM_RSTC_WRCFG_FULL_RESET: u32 = 0x20;
const PM_RSTC_WRCFG_MASK: u32 = 0x30;

/// Arms the watchdog for the shortest possible timeout and requests a
/// full reset; the SoC restarts once the watchdog expires. Never
/// returns on real hardware. Under QEMU's raspi3b this SoC block isn't
/// modeled, so it's a no-op there -- see the caller for how that's
/// handled.
pub fn reset() -> ! {
    unsafe {
        mmio::write(PM_WDOG, PM_PASSWORD | 1);
        let rstc = mmio::read(PM_RSTC);
        mmio::write(
            PM_RSTC,
            PM_PASSWORD | (rstc & !PM_RSTC_WRCFG_MASK) | PM_RSTC_WRCFG_FULL_RESET,
        );
    }
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
