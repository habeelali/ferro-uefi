//! Physical page allocator and EFI memory map builder.
//!
//! The allocator is bump-pointer only: AllocatePages hands out pages
//! monotonically starting just past Ferro's own image; FreePages is
//! accepted (so callers following the real API don't break) but
//! doesn't reclaim. That's the honest first slice -- real reclamation
//! is worth adding once something actually allocates enough to need
//! pages back.
//!
//! It also doesn't yet defend against walking into the GPU-carved
//! framebuffer region: at current allocation volumes (a handful of
//! pages) the bump pointer is nowhere near it, but this stops being
//! true once real workloads allocate hundreds of MB. GetMemoryMap
//! below reports the framebuffer region correctly either way.

use super::types::*;
use core::sync::atomic::{AtomicU64, Ordering};

extern "C" {
    static __end: u8;
}

/// Where RAM stops and BCM2837 MMIO starts -- see mmu.rs, which maps
/// exactly this boundary.
const RAM_LIMIT: u64 = crate::mmio::PERIPHERAL_BASE as u64;

static NEXT_FREE_PAGE: AtomicU64 = AtomicU64::new(0);

fn firmware_end() -> u64 {
    core::ptr::addr_of!(__end) as u64
}

fn page_align_up(addr: u64) -> u64 {
    (addr + EFI_PAGE_SIZE - 1) & !(EFI_PAGE_SIZE - 1)
}

/// Must run once, before the first allocate_pages call.
pub fn init() {
    NEXT_FREE_PAGE.store(page_align_up(firmware_end()), Ordering::Relaxed);
}

pub fn allocate_pages(count: u64) -> Option<u64> {
    loop {
        let base = NEXT_FREE_PAGE.load(Ordering::Relaxed);
        let new = base + count * EFI_PAGE_SIZE;
        if new > RAM_LIMIT {
            return None;
        }
        if NEXT_FREE_PAGE
            .compare_exchange(base, new, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return Some(base);
        }
    }
}

fn bump_mark() -> u64 {
    NEXT_FREE_PAGE.load(Ordering::Relaxed)
}

/// Fills `out` with the current memory map and returns the number of
/// descriptors written, or None if `out` is too small (spec's
/// EFI_BUFFER_TOO_SMALL case). `fb_base`/`fb_size` come from whatever
/// the VideoCore mailbox actually handed back, so the framebuffer
/// region is reported for real, not assumed.
pub fn get_memory_map(fb_base: u64, fb_size: u64, out: &mut [EfiMemoryDescriptor]) -> Option<usize> {
    let firmware_start = 0x8_0000u64; // BASE_ADDRESS in linker.ld
    let firmware_end_aligned = page_align_up(firmware_end());
    let bump = bump_mark();
    let fb_end_aligned = page_align_up(fb_base + fb_size);
    let peripheral_base = crate::mmio::PERIPHERAL_BASE as u64;
    let local_base = crate::mmio::LOCAL_BASE as u64;
    let local_end = local_base + 0x4000_0000; // matches the mmu.rs L1[1] block

    let mut regions: [(u64, u64, u32); 8] = [(0, 0, 0); 8];
    let mut n = 0usize;
    let mut push = |start: u64, end: u64, ty: u32| {
        if end > start {
            regions[n] = (start, end, ty);
            n += 1;
        }
    };

    push(0, firmware_start, EFI_CONVENTIONAL_MEMORY);
    push(firmware_start, firmware_end_aligned, EFI_BOOT_SERVICES_CODE);
    push(firmware_end_aligned, bump, EFI_BOOT_SERVICES_DATA);
    push(bump, fb_base, EFI_CONVENTIONAL_MEMORY);
    push(fb_base, fb_end_aligned, EFI_MEMORY_MAPPED_IO);
    push(fb_end_aligned, peripheral_base, EFI_CONVENTIONAL_MEMORY);
    push(peripheral_base, local_base, EFI_MEMORY_MAPPED_IO);
    push(local_base, local_end, EFI_MEMORY_MAPPED_IO);

    if out.len() < n {
        return None;
    }
    for (i, (start, end, ty)) in regions[..n].iter().enumerate() {
        out[i] = EfiMemoryDescriptor {
            ty: *ty,
            physical_start: *start,
            virtual_start: *start, // identity-mapped
            number_of_pages: (end - start) / EFI_PAGE_SIZE,
            attribute: 0,
        };
    }
    Some(n)
}
