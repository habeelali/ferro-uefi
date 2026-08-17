//! Cache maintenance for memory shared with the VideoCore GPU. The
//! mailbox property protocol and the framebuffer it hands back are
//! written/read by the GPU directly -- it has no visibility into our
//! D-cache (which mmu.rs turned on), so buffers crossing that boundary
//! need explicit clean+invalidate.

use core::arch::asm;

/// D-cache line size in bytes, read from CTR_EL0 rather than assumed,
/// since it's an implementation-defined field.
fn line_size() -> usize {
    let ctr: u64;
    unsafe { asm!("mrs {0}, ctr_el0", out(reg) ctr) };
    4 << ((ctr >> 16) & 0xf) // DminLine, in words
}

/// Clean and invalidate every line covering `[addr, addr+len)`: pushes
/// our writes out to the point of coherency and drops any stale copy,
/// so the GPU sees what we wrote and our next read sees what it wrote.
pub fn clean_and_invalidate_range(addr: usize, len: usize) {
    let line = line_size();
    let start = addr & !(line - 1);
    let end = (addr + len + line - 1) & !(line - 1);
    let mut a = start;
    while a < end {
        unsafe { asm!("dc civac, {0}", in(reg) a) };
        a += line;
    }
    unsafe { asm!("dsb sy") };
}
