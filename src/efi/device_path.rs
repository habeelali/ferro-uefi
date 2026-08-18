//! EFI_DEVICE_PATH_PROTOCOL: minimal but real node structures (a
//! Vendor-defined hardware node identifying "the SD card" for the
//! block/filesystem handle, and a Media File Path node for
//! LoadedImage->FilePath) instead of the null pointers the loaded-
//! image protocol carried before. No general device-path parsing or
//! multi-instance paths -- just enough that code which walks a device
//! path looking for its End node doesn't dereference NULL.

use super::types::EfiGuid;
use core::ffi::c_void;

pub const DEVICE_PATH_PROTOCOL_GUID: EfiGuid = EfiGuid {
    data1: 0x0957_6E91,
    data2: 0x6D3F,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

const TYPE_HARDWARE: u8 = 0x01;
const SUBTYPE_VENDOR: u8 = 0x04;
const TYPE_MEDIA: u8 = 0x04;
const SUBTYPE_FILE_PATH: u8 = 0x04;
const TYPE_END: u8 = 0x7F;
const SUBTYPE_END_ENTIRE: u8 = 0xFF;

fn write_header(buf: &mut [u8], offset: usize, node_type: u8, sub_type: u8, length: u16) -> usize {
    buf[offset] = node_type;
    buf[offset + 1] = sub_type;
    buf[offset + 2..offset + 4].copy_from_slice(&length.to_le_bytes());
    offset + 4
}

fn write_end(buf: &mut [u8], offset: usize) -> usize {
    write_header(buf, offset, TYPE_END, SUBTYPE_END_ENTIRE, 4)
}

// Vendor(FERRO_SD_GUID) + End -- identifies the block/filesystem
// handle's "device" without pretending to know real MBR/partition
// addressing a HardDrive media node would need.
const FERRO_SD_VENDOR_GUID: EfiGuid = EfiGuid {
    data1: 0x4645_5252,
    data2: 0x4F00,
    data3: 0x0002,
    data4: *b"ferro-sd",
};

const SD_PATH_LEN: usize = 4 + 16 + 4; // vendor header+guid, then end node
static mut SD_DEVICE_PATH: [u8; SD_PATH_LEN] = [0; SD_PATH_LEN];
static mut SD_DEVICE_PATH_BUILT: bool = false;

/// A stable device path identifying "the SD card" -- installed on the
/// block I/O and simple file system handle so protocols that expect a
/// real (non-null) EFI_DEVICE_PATH_PROTOCOL on that handle find one.
pub fn sd_device_path() -> *mut c_void {
    unsafe {
        if !SD_DEVICE_PATH_BUILT {
            let buf = &mut *core::ptr::addr_of_mut!(SD_DEVICE_PATH);
            let mut off = write_header(buf, 0, TYPE_HARDWARE, SUBTYPE_VENDOR, 20);
            buf[off..off + 4].copy_from_slice(&FERRO_SD_VENDOR_GUID.data1.to_le_bytes());
            off += 4;
            buf[off..off + 2].copy_from_slice(&FERRO_SD_VENDOR_GUID.data2.to_le_bytes());
            off += 2;
            buf[off..off + 2].copy_from_slice(&FERRO_SD_VENDOR_GUID.data3.to_le_bytes());
            off += 2;
            buf[off..off + 8].copy_from_slice(&FERRO_SD_VENDOR_GUID.data4);
            off += 8;
            write_end(buf, off);
            SD_DEVICE_PATH_BUILT = true;
        }
        core::ptr::addr_of_mut!(SD_DEVICE_PATH) as *mut c_void
    }
}

const MAX_FILE_PATH_BYTES: usize = 4 + 2 * 13 + 2 + 4; // FilePath header + up to "\NAME.EXT\0" + End
static mut FILE_PATH_BUF: [u8; MAX_FILE_PATH_BYTES] = [0; MAX_FILE_PATH_BYTES];

/// Builds a Media/FilePath("\<NAME>.<EXT>") + End device path for a
/// just-loaded 8.3-named file, into a static scratch buffer (one live
/// loaded image's worth at a time, matching how LoadImage itself only
/// ever tracks one in-flight load) -- used for LoadedImage->FilePath
/// so an app asking "what file am I" gets a real answer.
pub fn build_file_path(name_8_3: &[u8; 11]) -> *mut c_void {
    let mut name_units = [0u16; 13]; // "\NAME.EXT\0" worst case
    let mut n = 0usize;
    name_units[n] = b'\\' as u16;
    n += 1;
    for &b in &name_8_3[0..8] {
        if b != b' ' {
            name_units[n] = b as u16;
            n += 1;
        }
    }
    let ext = &name_8_3[8..11];
    if ext != b"   " {
        name_units[n] = b'.' as u16;
        n += 1;
        for &b in ext {
            if b != b' ' {
                name_units[n] = b as u16;
                n += 1;
            }
        }
    }
    name_units[n] = 0;
    n += 1;

    let node_len = 4 + n * 2;
    unsafe {
        let buf = &mut *core::ptr::addr_of_mut!(FILE_PATH_BUF);
        let mut off = write_header(buf, 0, TYPE_MEDIA, SUBTYPE_FILE_PATH, node_len as u16);
        for &unit in &name_units[..n] {
            buf[off..off + 2].copy_from_slice(&unit.to_le_bytes());
            off += 2;
        }
        write_end(buf, off);
        core::ptr::addr_of_mut!(FILE_PATH_BUF) as *mut c_void
    }
}
