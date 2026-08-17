//! Minimal PE32+ (AArch64) loader: parses the DOS/COFF/Optional
//! headers, copies each section to its RVA in a freshly allocated
//! image, and applies IMAGE_REL_BASED_DIR64 base relocations if the
//! image didn't land at its preferred ImageBase (which it essentially
//! never will here, since our allocator is a simple bump allocator
//! with no say over the address). No imports, no TLS, no exceptions
//! directory -- just what a self-contained EFI application needs.

use crate::efi::memory;

const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_DIR64: u16 = 10;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;
const PE32_PLUS_MAGIC: u16 = 0x020B;
const DATA_DIR_BASE_RELOCATION: usize = 5;

#[derive(Debug)]
pub enum PeError {
    Truncated,
    NotDos,
    NotPe,
    WrongMachine,
    NotPe32Plus,
    AllocationFailed,
    UnsupportedRelocation(#[allow(dead_code)] u16), // read via Debug logging
}

pub struct LoadedPe {
    pub image_base: u64,
    pub image_size: u64,
    pub entry_point: u64,
}

fn ru16(d: &[u8], off: usize) -> Result<u16, PeError> {
    d.get(off..off + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .ok_or(PeError::Truncated)
}

fn ru32(d: &[u8], off: usize) -> Result<u32, PeError> {
    d.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(PeError::Truncated)
}

fn ru64(d: &[u8], off: usize) -> Result<u64, PeError> {
    d.get(off..off + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or(PeError::Truncated)
}

/// Loads a PE32+ AArch64 EFI application from `data` (the whole file,
/// already in memory) into a freshly allocated image and returns
/// where it landed and where to call into it.
pub fn load(data: &[u8]) -> Result<LoadedPe, PeError> {
    if data.len() < 0x40 || &data[0..2] != b"MZ" {
        return Err(PeError::NotDos);
    }
    let pe = ru32(data, 0x3C)? as usize;
    if data.get(pe..pe + 4) != Some(&b"PE\0\0"[..]) {
        return Err(PeError::NotPe);
    }

    let machine = ru16(data, pe + 4)?;
    if machine != IMAGE_FILE_MACHINE_ARM64 {
        return Err(PeError::WrongMachine);
    }
    let number_of_sections = ru16(data, pe + 6)? as usize;
    let size_of_optional_header = ru16(data, pe + 20)? as usize;

    let opt = pe + 24;
    let magic = ru16(data, opt)?;
    if magic != PE32_PLUS_MAGIC {
        return Err(PeError::NotPe32Plus);
    }
    let address_of_entry_point = ru32(data, opt + 16)? as u64;
    let image_base = ru64(data, opt + 24)?;
    let size_of_image = ru32(data, opt + 56)? as u64;
    // Standard fields (24 bytes) + Windows-specific fields (88 bytes)
    // = 112 bytes before the data directory array; NumberOfRvaAndSizes
    // is the last Windows-specific field, at relative offset 108.
    let number_of_rva_and_sizes = ru32(data, opt + 108)? as usize;

    let base_reloc_dir = if number_of_rva_and_sizes > DATA_DIR_BASE_RELOCATION {
        let dir_off = opt + 112 + DATA_DIR_BASE_RELOCATION * 8;
        let rva = ru32(data, dir_off)?;
        let size = ru32(data, dir_off + 4)?;
        (rva, size)
    } else {
        (0, 0)
    };

    let pages = (size_of_image + 0xFFF) / 0x1000;
    let load_base = memory::allocate_pages(pages).ok_or(PeError::AllocationFailed)?;

    // Zero the image first (covers section padding / BSS-like gaps
    // between sections, which the file doesn't carry data for).
    unsafe { core::ptr::write_bytes(load_base as *mut u8, 0, (pages * 0x1000) as usize) };

    let sections_off = opt + size_of_optional_header;
    for i in 0..number_of_sections {
        let s = sections_off + i * 40;
        let virtual_size = ru32(data, s + 8)?;
        let virtual_address = ru32(data, s + 12)? as u64;
        let size_of_raw_data = ru32(data, s + 16)?;
        let pointer_to_raw_data = ru32(data, s + 20)? as usize;

        let copy_len = size_of_raw_data.min(virtual_size) as usize;
        let src = data
            .get(pointer_to_raw_data..pointer_to_raw_data + copy_len)
            .ok_or(PeError::Truncated)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                (load_base + virtual_address) as *mut u8,
                copy_len,
            );
        }
    }

    let delta = load_base.wrapping_sub(image_base);
    if delta != 0 && base_reloc_dir.1 > 0 {
        apply_relocations(base_reloc_dir.0, base_reloc_dir.1, load_base, delta)?;
    }

    Ok(LoadedPe {
        image_base: load_base,
        image_size: pages * 0x1000,
        entry_point: load_base + address_of_entry_point,
    })
}

fn apply_relocations(
    dir_rva: u32,
    dir_size: u32,
    load_base: u64,
    delta: u64,
) -> Result<(), PeError> {
    // The relocation directory's bytes are identical in the file and
    // in memory (it's plain section data we already copied), so we
    // can walk it straight out of the loaded image.
    let mut block_addr = load_base + dir_rva as u64;
    let dir_end = block_addr + dir_size as u64;

    while block_addr < dir_end {
        let page_rva = unsafe { core::ptr::read_unaligned(block_addr as *const u32) };
        let block_size = unsafe { core::ptr::read_unaligned((block_addr + 4) as *const u32) };
        if block_size < 8 {
            break;
        }
        let entry_count = (block_size as usize - 8) / 2;
        for i in 0..entry_count {
            let entry_addr = block_addr + 8 + (i * 2) as u64;
            let entry = unsafe { core::ptr::read_unaligned(entry_addr as *const u16) };
            let reloc_type = entry >> 12;
            let offset = (entry & 0x0FFF) as u64;
            match reloc_type {
                IMAGE_REL_BASED_ABSOLUTE => {} // padding entry, no-op by definition
                IMAGE_REL_BASED_DIR64 => {
                    let target = load_base + page_rva as u64 + offset;
                    unsafe {
                        let value = core::ptr::read_unaligned(target as *const u64);
                        core::ptr::write_unaligned(target as *mut u64, value.wrapping_add(delta));
                    }
                }
                other => return Err(PeError::UnsupportedRelocation(other)),
            }
        }
        block_addr += block_size as u64;
    }

    Ok(())
}
