//! IRQ dispatch. Only the ARM generic timer is wired up so far; this
//! grows as more local_intc sources (mailboxes, GPU IRQ) come online.

use crate::{local_intc, timer};

#[no_mangle]
extern "C" fn rust_irq_handler() {
    let source = unsafe { local_intc::core0_irq_source() };
    if source & local_intc::CNTPNSIRQ != 0 {
        timer::on_tick();
    }
}
