//! Cross-reboot persistence for the UEFI variable store, backed by a
//! real file (`FERRO.VAR`) in the volume's root directory via
//! `Fat32::write_file`/`read_file` -- an ordinary file any other
//! FAT32 implementation can see and understand, not a private corner
//! of the reserved sectors.

use crate::efi::variables;
use crate::fat32::{Fat32, Fat32Error};
use crate::sd::{Card, SdError};

const VAR_FILE_NAME: [u8; 11] = *b"FERRO   VAR";
const MAX_BYTES: usize = 8192;

// Static, not stack-allocated: this is a big buffer relative to the
// 64 KiB boot stack, and save/load run several call frames deep from
// the menu.
static mut BUF: [u8; MAX_BYTES] = [0; MAX_BYTES];

#[derive(Debug)]
pub enum PersistError {
    TooLarge,
    Sd(#[allow(dead_code)] SdError),
    Fat32(#[allow(dead_code)] Fat32Error),
    NotOurData,
}

impl From<SdError> for PersistError {
    fn from(e: SdError) -> Self {
        PersistError::Sd(e)
    }
}

impl From<Fat32Error> for PersistError {
    fn from(e: Fat32Error) -> Self {
        PersistError::Fat32(e)
    }
}

/// Serializes the current variable store and writes it to FERRO.VAR
/// in the volume's root directory.
pub fn save(card: &Card, fs: &Fat32) -> Result<usize, PersistError> {
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let written = variables::serialize(buf).ok_or(PersistError::TooLarge)?;
    fs.write_file(card, &VAR_FILE_NAME, &buf[..written])?;
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

/// The same cached card/filesystem, for other code (efi::block_io,
/// efi::file_protocol) that needs real disk access on behalf of a
/// loaded EFI application without re-running SD init + FAT32 mount.
pub fn get_context() -> Option<(Card, Fat32)> {
    unsafe { *core::ptr::addr_of!(SD_CONTEXT) }
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

/// Reads FERRO.VAR from the volume's root directory and merges any
/// previously-saved variables into the live store. Returns the number
/// loaded, or NotOurData if the file doesn't exist yet or doesn't
/// start with our magic.
pub fn load(card: &Card, fs: &Fat32) -> Result<usize, PersistError> {
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let n = fs.read_file(card, &VAR_FILE_NAME, buf)?;
    variables::deserialize(&buf[..n]).ok_or(PersistError::NotOurData)
}
