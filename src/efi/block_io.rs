//! EFI_BLOCK_IO_PROTOCOL, backed by sd.rs. Installed on the same
//! handle as EFI_SIMPLE_FILE_SYSTEM_PROTOCOL (see file_protocol.rs)
//! so a loaded EFI application can find both "the disk" and "the
//! filesystem on it" the way real firmware presents a block device.

use super::types::{EfiGuid, EfiStatus, EFI_DEVICE_ERROR, EFI_INVALID_PARAMETER, EFI_SUCCESS};
use core::ffi::c_void;

pub const BLOCK_IO_PROTOCOL_GUID: EfiGuid = EfiGuid {
    data1: 0x964E_5B21,
    data2: 0x6459,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

const SECTOR_SIZE: u32 = 512;

#[repr(C)]
pub struct BlockIoMedia {
    pub media_id: u32,
    pub removable_media: u8,
    pub media_present: u8,
    pub logical_partition: u8,
    pub read_only: u8,
    pub write_caching: u8,
    pub block_size: u32,
    pub io_align: u32,
    pub last_block: u64,
}

pub type BlockResetFn = extern "C" fn(*mut BlockIoProtocol, u8) -> EfiStatus;
pub type BlockReadFn = extern "C" fn(*mut BlockIoProtocol, u32, u64, usize, *mut c_void) -> EfiStatus;
pub type BlockWriteFn = extern "C" fn(*mut BlockIoProtocol, u32, u64, usize, *const c_void) -> EfiStatus;
pub type BlockFlushFn = extern "C" fn(*mut BlockIoProtocol) -> EfiStatus;

#[repr(C)]
pub struct BlockIoProtocol {
    pub revision: u64,
    pub media: *mut BlockIoMedia,
    pub reset: BlockResetFn,
    pub read_blocks: BlockReadFn,
    pub write_blocks: BlockWriteFn,
    pub flush_blocks: BlockFlushFn,
}

unsafe impl Sync for BlockIoProtocol {}
unsafe impl Sync for BlockIoMedia {}

const EFI_BLOCK_IO_PROTOCOL_REVISION: u64 = 0x0001_0000;

static mut MEDIA: BlockIoMedia = BlockIoMedia {
    media_id: 1,
    removable_media: 1,
    media_present: 0, // set true in install() once a card is confirmed mounted
    logical_partition: 0,
    read_only: 0,
    write_caching: 0,
    block_size: SECTOR_SIZE,
    io_align: 4,
    // sd.rs doesn't parse the card's CSD register for real capacity
    // yet, so this can't be an honest exact count -- deliberately
    // large rather than 0, so well-behaved callers reading LBAs that
    // are actually in range (our own FAT32 partition, well within any
    // real SD card) don't get incorrectly rejected as "past the end
    // of the disk".
    last_block: 0x0FFF_FFFF,
};

static mut PROTO: BlockIoProtocol = BlockIoProtocol {
    revision: EFI_BLOCK_IO_PROTOCOL_REVISION,
    media: core::ptr::null_mut(),
    reset: block_reset,
    read_blocks: block_read,
    write_blocks: block_write,
    flush_blocks: block_flush,
};

extern "C" fn block_reset(_this: *mut BlockIoProtocol, _extended: u8) -> EfiStatus {
    EFI_SUCCESS
}

extern "C" fn block_read(_this: *mut BlockIoProtocol, _media_id: u32, lba: u64, buffer_size: usize, buffer: *mut c_void) -> EfiStatus {
    if buffer.is_null() || buffer_size % SECTOR_SIZE as usize != 0 {
        return EFI_INVALID_PARAMETER;
    }
    let Some((card, _)) = crate::persist::get_context() else {
        return EFI_DEVICE_ERROR;
    };
    let count = buffer_size / SECTOR_SIZE as usize;
    let out = unsafe { core::slice::from_raw_parts_mut(buffer as *mut u8, buffer_size) };
    for i in 0..count {
        let mut sector = [0u8; SECTOR_SIZE as usize];
        if card.read_block(lba as u32 + i as u32, &mut sector).is_err() {
            return EFI_DEVICE_ERROR;
        }
        out[i * SECTOR_SIZE as usize..(i + 1) * SECTOR_SIZE as usize].copy_from_slice(&sector);
    }
    EFI_SUCCESS
}

extern "C" fn block_write(_this: *mut BlockIoProtocol, _media_id: u32, lba: u64, buffer_size: usize, buffer: *const c_void) -> EfiStatus {
    if buffer.is_null() || buffer_size % SECTOR_SIZE as usize != 0 {
        return EFI_INVALID_PARAMETER;
    }
    let Some((card, _)) = crate::persist::get_context() else {
        return EFI_DEVICE_ERROR;
    };
    let count = buffer_size / SECTOR_SIZE as usize;
    let data = unsafe { core::slice::from_raw_parts(buffer as *const u8, buffer_size) };
    for i in 0..count {
        let mut sector = [0u8; SECTOR_SIZE as usize];
        sector.copy_from_slice(&data[i * SECTOR_SIZE as usize..(i + 1) * SECTOR_SIZE as usize]);
        if card.write_block(lba as u32 + i as u32, &sector).is_err() {
            return EFI_DEVICE_ERROR;
        }
    }
    EFI_SUCCESS
}

extern "C" fn block_flush(_this: *mut BlockIoProtocol) -> EfiStatus {
    EFI_SUCCESS // sd.rs's writes are synchronous already
}

/// Installs EFI_BLOCK_IO_PROTOCOL on `index` (a protocol_db handle),
/// using the SD card's real capacity if it's known (falls back to a
/// conservative placeholder LastBlock otherwise, since sd.rs doesn't
/// currently expose CSD-derived capacity).
pub fn install(index: usize) -> bool {
    unsafe {
        MEDIA.media_present = crate::persist::get_context().is_some() as u8;
        PROTO.media = core::ptr::addr_of_mut!(MEDIA);
    }
    super::protocol_db::install(index, BLOCK_IO_PROTOCOL_GUID, core::ptr::addr_of_mut!(PROTO) as *mut c_void)
}
