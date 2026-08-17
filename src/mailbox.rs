//! VideoCore mailbox, property-tag channel (channel 8): the mechanism
//! for asking the GPU firmware to do things only it can -- allocate a
//! framebuffer, report clock rates, and so on. The GPU has its own bus
//! address space and isn't coherent with our D-cache, hence the
//! address aliasing and cache maintenance here.

use crate::cache;
use crate::mmio::{self, MBOX_BASE};

const MBOX_READ: usize = MBOX_BASE + 0x00;
const MBOX_STATUS: usize = MBOX_BASE + 0x18;
const MBOX_WRITE: usize = MBOX_BASE + 0x20;

const MBOX_FULL: u32 = 1 << 31;
const MBOX_EMPTY: u32 = 1 << 30;

pub const CHANNEL_PROPERTY: u32 = 8;

/// GPU-side alias that bypasses the VideoCore's L1/L2 cache -- the
/// standard "give the GPU an address it won't cache" convention every
/// Pi mailbox caller uses.
const GPU_UNCACHED_ALIAS: u32 = 0xC000_0000;

/// Send `msg` (a property-tag buffer, 16-byte aligned, `msg[0]` = size
/// in bytes) over `channel` and wait for the reply in place. Returns
/// true if the GPU reported success (`msg[1] == 0x8000_0000`).
pub fn call(msg: &mut [u32], channel: u32) -> bool {
    let addr = msg.as_ptr() as usize;
    let len = msg.len() * 4;
    debug_assert_eq!(addr & 0xF, 0, "mailbox buffer must be 16-byte aligned");

    cache::clean_and_invalidate_range(addr, len);

    let bus_addr = (addr as u32 & !0xF) | GPU_UNCACHED_ALIAS;
    unsafe {
        while mmio::read(MBOX_STATUS) & MBOX_FULL != 0 {}
        mmio::write(MBOX_WRITE, bus_addr | (channel & 0xF));

        loop {
            while mmio::read(MBOX_STATUS) & MBOX_EMPTY != 0 {}
            if mmio::read(MBOX_READ) == (bus_addr | (channel & 0xF)) {
                break;
            }
        }
    }

    cache::clean_and_invalidate_range(addr, len);

    msg[1] == 0x8000_0000
}
