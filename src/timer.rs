//! ARM generic timer (CNTP: EL1 non-secure physical timer) as Ferro's
//! tick source. Delivery is via the BCM2836 local block, not a GIC --
//! see local_intc.rs.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);

/// Arm the timer for roughly `hz` ticks/second (CNTFRQ_EL0 may not
/// divide evenly; close enough for a UI tick). Interrupts still need
/// local_intc::enable_core0_timer_irq() and PSTATE.I cleared before
/// any of this actually fires.
pub fn init(hz: u64) {
    let freq: u64;
    unsafe { asm!("mrs {0}, cntfrq_el0", out(reg) freq) };
    let interval = freq / hz;
    TICK_INTERVAL.store(interval, Ordering::Relaxed);
    unsafe {
        asm!("msr cntp_tval_el0, {0}", in(reg) interval);
        asm!("msr cntp_ctl_el0, {0}", in(reg) 1u64); // ENABLE=1, IMASK=0
    }
}

/// Called from the IRQ path when the timer fired. Rearms for the next
/// tick (which is also what clears the timer's own pending condition)
/// and bumps the counter.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    let interval = TICK_INTERVAL.load(Ordering::Relaxed);
    unsafe { asm!("msr cntp_tval_el0, {0}", in(reg) interval) };
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Busy-wait using the tick counter. Fine for splash/menu timing; not
/// meant for anything latency-sensitive.
pub fn sleep_ticks(n: u64) {
    let target = ticks() + n;
    while ticks() < target {
        unsafe { asm!("wfe") };
    }
}

/// Rounds a microsecond duration up to whole ticks, for callers (like
/// EFI_BOOT_SERVICES.Stall) that only know time in microseconds.
pub fn micros_to_ticks(us: u64) -> u64 {
    let freq: u64;
    unsafe { asm!("mrs {0}, cntfrq_el0", out(reg) freq) };
    let interval = TICK_INTERVAL.load(Ordering::Relaxed).max(1);
    let numerator = us.saturating_mul(freq);
    let denominator = 1_000_000 * interval;
    (numerator + denominator - 1) / denominator
}
