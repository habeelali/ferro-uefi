//! Core UEFI 2.x types -- just the subset boot_services and
//! system_table need. Field layouts match the spec exactly where it
//! matters for ABI: this table gets called through function pointers
//! by code we don't control once real EFI applications exist.

use core::ffi::c_void;

pub type EfiStatus = usize;

const ERROR_BIT: usize = 1 << (usize::BITS - 1);
pub const EFI_SUCCESS: EfiStatus = 0;
pub const EFI_INVALID_PARAMETER: EfiStatus = ERROR_BIT | 2;
pub const EFI_UNSUPPORTED: EfiStatus = ERROR_BIT | 3;
pub const EFI_BUFFER_TOO_SMALL: EfiStatus = ERROR_BIT | 5;
pub const EFI_NOT_READY: EfiStatus = ERROR_BIT | 6;
pub const EFI_DEVICE_ERROR: EfiStatus = ERROR_BIT | 7;
pub const EFI_WRITE_PROTECTED: EfiStatus = ERROR_BIT | 8;
pub const EFI_OUT_OF_RESOURCES: EfiStatus = ERROR_BIT | 9;
pub const EFI_NOT_FOUND: EfiStatus = ERROR_BIT | 14;

pub type EfiHandle = *mut c_void;
/// Opaque event token -- like EfiHandle, callers never dereference it
/// themselves; it's just a key into events.rs's table.
pub type EfiEvent = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EfiGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

// EFI_MEMORY_TYPE values (spec order). Several are unused until real
// EFI application loading exists, but the constant set is kept
// complete rather than trimmed to just what's referenced today.
#[allow(dead_code)]
pub const EFI_RESERVED_MEMORY_TYPE: u32 = 0;
#[allow(dead_code)]
pub const EFI_LOADER_CODE: u32 = 1;
#[allow(dead_code)]
pub const EFI_LOADER_DATA: u32 = 2;
pub const EFI_BOOT_SERVICES_CODE: u32 = 3;
pub const EFI_BOOT_SERVICES_DATA: u32 = 4;
#[allow(dead_code)]
pub const EFI_RUNTIME_SERVICES_CODE: u32 = 5;
#[allow(dead_code)]
pub const EFI_RUNTIME_SERVICES_DATA: u32 = 6;
pub const EFI_CONVENTIONAL_MEMORY: u32 = 7;
pub const EFI_MEMORY_MAPPED_IO: u32 = 11;

pub const EFI_PAGE_SIZE: u64 = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EfiMemoryDescriptor {
    pub ty: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

pub const EFI_MEMORY_DESCRIPTOR_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EfiConfigurationTableEntry {
    pub vendor_guid: EfiGuid,
    pub vendor_table: *mut c_void,
}

#[repr(C)]
pub struct EfiTableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}
