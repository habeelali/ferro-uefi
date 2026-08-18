//! Cross-reboot persistence for the UEFI variable store, backed by raw
//! SD card sectors -- not a real file, since there's no FAT32 write
//! support. It lives in the reserved area's normally-unused padding
//! sectors (see Fat32::private_scratch_region), sectors real FAT32
//! implementations never touch, so this doesn't risk the filesystem.

use crate::efi::variables;
use crate::fat32::Fat32;
use crate::sd::{Card, SdError};

const SECTOR_SIZE: usize = 512;
const MAX_SECTORS: usize = 16; // matches the realistic scratch-region size (see fat32.rs)

// Static, not stack-allocated: this is a big buffer (8 KiB) relative
// to the 64 KiB boot stack, and save/load run several call frames deep
// from the menu.
static mut BUF: [u8; MAX_SECTORS * SECTOR_SIZE] = [0; MAX_SECTORS * SECTOR_SIZE];

#[derive(Debug)]
pub enum PersistError {
    NoScratchRegion,
    TooLarge,
    Sd(#[allow(dead_code)] SdError),
    NotOurData,
}

impl From<SdError> for PersistError {
    fn from(e: SdError) -> Self {
        PersistError::Sd(e)
    }
}

/// Serializes the current variable store and writes it into the
/// volume's private scratch region.
pub fn save(card: &Card, fs: &Fat32) -> Result<usize, PersistError> {
    let (start_lba, sector_count) = fs.private_scratch_region().ok_or(PersistError::NoScratchRegion)?;

    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let capacity = (sector_count as usize * SECTOR_SIZE).min(buf.len());
    let written = variables::serialize(&mut buf[..capacity]).ok_or(PersistError::TooLarge)?;

    let sectors_needed = (written + SECTOR_SIZE - 1) / SECTOR_SIZE;
    for i in 0..sectors_needed {
        let mut sector = [0u8; SECTOR_SIZE];
        let off = i * SECTOR_SIZE;
        let len = (written - off).min(SECTOR_SIZE);
        sector[..len].copy_from_slice(&buf[off..off + len]);
        card.write_block(start_lba + i as u32, &sector)?;
    }
    Ok(written)
}

/// A card/filesystem the menu already successfully mounted this
/// session, cached so `SetVariable(..., NON_VOLATILE, ...)` can
/// auto-persist without re-running SD init + FAT32 mount on every
/// single call. Set by `set_context` once BOOT FROM SD or SAVE
/// VARIABLES TO SD has mounted a card; `Card`/`Fat32` are small,
/// `Copy` handles (register state + geometry), not open file handles,
/// so caching them is cheap and safe.
static mut SD_CONTEXT: Option<(Card, Fat32)> = None;

pub fn set_context(card: Card, fs: Fat32) {
    unsafe { *core::ptr::addr_of_mut!(SD_CONTEXT) = Some((card, fs)) };
}

/// Best-effort auto-save of the live variable store, used right after
/// a NON_VOLATILE SetVariable call. Does nothing (silently) if no SD
/// context has been established yet this session -- this is a
/// convenience for "I already mounted a card, keep it in sync", not a
/// replacement for the explicit SAVE VARIABLES TO SD menu item, which
/// remains the only way to get a first write on a session that never
/// otherwise touched the SD card.
pub fn autosave() -> Option<Result<usize, PersistError>> {
    let ctx = unsafe { *core::ptr::addr_of!(SD_CONTEXT) };
    ctx.map(|(card, fs)| save(&card, &fs))
}

/// Reads the volume's private scratch region and merges any
/// previously-saved variables into the live store. Returns the number
/// loaded, or NotOurData if the region doesn't start with our magic
/// (nothing saved yet, or it's someone else's data).
pub fn load(card: &Card, fs: &Fat32) -> Result<usize, PersistError> {
    let (start_lba, sector_count) = fs.private_scratch_region().ok_or(PersistError::NoScratchRegion)?;

    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let capacity_sectors = sector_count.min((buf.len() / SECTOR_SIZE) as u32);
    for i in 0..capacity_sectors {
        let mut sector = [0u8; SECTOR_SIZE];
        card.read_block(start_lba + i, &mut sector)?;
        let off = i as usize * SECTOR_SIZE;
        buf[off..off + SECTOR_SIZE].copy_from_slice(&sector);
    }

    variables::deserialize(&buf[..(capacity_sectors as usize * SECTOR_SIZE)]).ok_or(PersistError::NotOurData)
}
