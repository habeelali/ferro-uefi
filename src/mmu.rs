//! Minimal AArch64 stage-1 MMU bring-up. Identity-maps (VA == PA):
//!   - 0x0000_0000-0x3EFF_FFFF: RAM, Normal Write-Back, executable.
//!   - 0x3F00_0000-0x3FFF_FFFF: BCM2837 peripherals, Device-nGnRnE.
//!   - 0x4000_0000-0x7FFF_FFFF: ARM-local (mailboxes, local timer, ...)
//!     and whatever else lives up there, Device-nGnRnE.
//! Anything past 0x8000_0000 is left unmapped and faults -- see
//! exceptions.rs, which is exactly why that one got built first.
//!
//! 4KiB granule, TCR_EL1.T0SZ = 25 (39-bit input address -> walk starts
//! at level 1, so a level-1 entry is a 1GiB block/table and a level-2
//! entry is a 2MiB block). No level-3 (4KiB page) tables anywhere --
//! nothing here needs page granularity yet.

use core::arch::asm;

const BLOCK_2M: u64 = 0x0020_0000;

const PERIPHERAL_BASE: u64 = crate::mmio::PERIPHERAL_BASE as u64;
const ARM_LOCAL_BASE: u64 = 0x4000_0000;

const MT_NORMAL: u64 = 0;
const MT_DEVICE_NGNRNE: u64 = 1;

const DESC_VALID_BLOCK: u64 = 0b01;
const DESC_VALID_TABLE: u64 = 0b11;
const AF: u64 = 1 << 10; // Access Flag -- must be set or first touch faults.
const UXN: u64 = 1 << 54;
const PXN: u64 = 1 << 53;

#[repr(align(4096))]
struct PageTable([u64; 512]);

static mut L1_TABLE: PageTable = PageTable([0; 512]);
static mut L2_TABLE: PageTable = PageTable([0; 512]);

fn block_desc(pa: u64, attr_idx: u64, shareability: u64, executable: bool) -> u64 {
    let mut d = DESC_VALID_BLOCK;
    d |= attr_idx << 2;
    d |= shareability << 8;
    d |= AF;
    d |= pa;
    if !executable {
        d |= UXN | PXN;
    }
    d
}

/// Build the page tables and switch the MMU on. Must run before
/// anything relies on cached memory or on faults past 0x8000_0000
/// being anything other than fatal (they still are -- just now via a
/// translation fault instead of an external abort).
pub unsafe fn init() {
    let peripheral_first_entry = (PERIPHERAL_BASE / BLOCK_2M) as usize;
    let l2 = core::ptr::addr_of_mut!(L2_TABLE.0);
    for i in 0..512usize {
        let pa = (i as u64) * BLOCK_2M;
        let desc = if i < peripheral_first_entry {
            block_desc(pa, MT_NORMAL, 0b11, true) // RAM: Inner Shareable, executable
        } else {
            block_desc(pa, MT_DEVICE_NGNRNE, 0b10, false) // peripherals
        };
        (*l2)[i] = desc;
    }

    let l2_addr = core::ptr::addr_of!(L2_TABLE) as u64;
    let l1 = core::ptr::addr_of_mut!(L1_TABLE.0);
    (*l1)[0] = DESC_VALID_TABLE | l2_addr; // 0x0000_0000-0x3FFF_FFFF
    (*l1)[1] = block_desc(ARM_LOCAL_BASE, MT_DEVICE_NGNRNE, 0b10, false); // 0x4000_0000-0x7FFF_FFFF

    let mair: u64 = (0xFFu64 << (8 * MT_NORMAL)) | (0x00u64 << (8 * MT_DEVICE_NGNRNE));

    let tcr: u64 = 25            // T0SZ: 39-bit input address, walk starts at L1
        | (1 << 8)                 // IRGN0: table walks are Normal WB RA WA
        | (1 << 10)                // ORGN0: ditto, outer
        | (0b11 << 12)              // SH0: Inner Shareable
        | (0b00 << 14)              // TG0: 4KiB granule
        | (1 << 23); // EPD1: no TTBR1 walks, we don't use the upper VA range

    let l1_addr = core::ptr::addr_of!(L1_TABLE) as u64;

    asm!(
        "msr mair_el1, {mair}",
        "msr tcr_el1, {tcr}",
        "msr ttbr0_el1, {ttbr0}",
        "isb",
        mair = in(reg) mair,
        tcr = in(reg) tcr,
        ttbr0 = in(reg) l1_addr,
    );

    let mut sctlr: u64;
    asm!("mrs {0}, sctlr_el1", out(reg) sctlr);
    sctlr |= 1 << 0; // M: MMU enable
    sctlr |= 1 << 2; // C: data cache enable
    sctlr |= 1 << 12; // I: instruction cache enable
    asm!(
        "dsb sy",
        "msr sctlr_el1, {sctlr}",
        "isb",
        sctlr = in(reg) sctlr,
    );
}
