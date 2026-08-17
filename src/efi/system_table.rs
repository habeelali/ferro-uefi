//! EFI_SYSTEM_TABLE. Console fields are still null -- there's no
//! EFI_SIMPLE_TEXT_INPUT/OUTPUT_PROTOCOL yet, so there's nothing real
//! to point them at. BootServices and RuntimeServices both point at
//! real, working tables.

use super::boot_services::BootServices;
use super::crc32::crc32_ieee;
use super::runtime_services::RuntimeServices;
use super::types::*;
use core::ffi::c_void;

#[repr(C)]
pub struct SystemTable {
    pub hdr: EfiTableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: EfiHandle,
    pub con_in: *mut c_void,
    pub console_out_handle: EfiHandle,
    pub con_out: *mut c_void,
    pub standard_error_handle: EfiHandle,
    pub std_err: *mut c_void,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut c_void,
}

unsafe impl Sync for SystemTable {}

static FIRMWARE_VENDOR: [u16; 6] = [
    b'F' as u16,
    b'e' as u16,
    b'r' as u16,
    b'r' as u16,
    b'o' as u16,
    0,
];

pub static mut SYSTEM_TABLE: SystemTable = SystemTable {
    hdr: EfiTableHeader {
        signature: 0, // patched in init()
        revision: (2 << 16) | 100,
        header_size: core::mem::size_of::<SystemTable>() as u32,
        crc32: 0, // patched in init()
        reserved: 0,
    },
    firmware_vendor: core::ptr::null(),
    firmware_revision: 0x0001_0000,
    console_in_handle: core::ptr::null_mut(),
    con_in: core::ptr::null_mut(),
    console_out_handle: core::ptr::null_mut(),
    con_out: core::ptr::null_mut(),
    standard_error_handle: core::ptr::null_mut(),
    std_err: core::ptr::null_mut(),
    runtime_services: core::ptr::null_mut(), // patched in init()
    boot_services: core::ptr::null_mut(), // patched in init()
    number_of_table_entries: 0,
    configuration_table: core::ptr::null_mut(),
};

/// Must run after boot_services::init() and runtime_services::init().
/// Wires in the real BootServices/RuntimeServices pointers and
/// FirmwareVendor, then computes a real CRC32 (with crc32 zeroed
/// during the calculation, per spec).
pub fn init() {
    unsafe {
        let st = core::ptr::addr_of_mut!(SYSTEM_TABLE);
        (*st).firmware_vendor = FIRMWARE_VENDOR.as_ptr();
        (*st).boot_services = core::ptr::addr_of_mut!(super::boot_services::BOOT_SERVICES);
        (*st).runtime_services = core::ptr::addr_of_mut!(super::runtime_services::RUNTIME_SERVICES);
        (*st).hdr.signature = u64::from_le_bytes(*b"IBI SYST");
        (*st).hdr.crc32 = 0;
        let bytes =
            core::slice::from_raw_parts(st as *const u8, core::mem::size_of::<SystemTable>());
        (*st).hdr.crc32 = crc32_ieee(bytes);
    }
}
