//! BCM2836 "local" per-core control block (Broadcom's QA7). This --
//! not an ARM GIC, which BCM2837 does not have -- is how the ARM
//! generic timer, the two per-core mailboxes, and the shared GPU IRQ
//! line get routed to each Cortex-A53 core.

use crate::mmio::{self, LOCAL_BASE};

const CORE0_TIMER_IRQCNTL: usize = LOCAL_BASE + 0x40;
const CORE0_IRQ_SOURCE: usize = LOCAL_BASE + 0x60;

/// Bit shared by CORE0_TIMER_IRQCNTL (enable) and CORE0_IRQ_SOURCE
/// (pending) for the non-secure physical generic timer (CNTP) -- the
/// timer Ferro actually uses (EL1, no secure world in play).
pub const CNTPNSIRQ: u32 = 1 << 1;

/// Route the core-0 CNTP timer interrupt to core 0. Must run before
/// unmasking IRQs at the CPU (PSTATE.I), or the first tick has nowhere
/// to go.
pub unsafe fn enable_core0_timer_irq() {
    mmio::write(CORE0_TIMER_IRQCNTL, CNTPNSIRQ);
}

/// Which local IRQ sources are currently pending for core 0.
pub unsafe fn core0_irq_source() -> u32 {
    mmio::read(CORE0_IRQ_SOURCE)
}
