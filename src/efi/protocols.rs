//! Protocol structs installed on handles. Just EFI_LOADED_IMAGE_PROTOCOL
//! for now, since that's what LoadImage needs to hand back -- more
//! arrive as the code that needs them does.

use super::types::{EfiGuid, EfiHandle, EfiStatus};
use core::ffi::c_void;

pub const LOADED_IMAGE_PROTOCOL_GUID: EfiGuid = EfiGuid {
    data1: 0x5B1B_31A1,
    data2: 0x9562,
    data3: 0x11D2,
    data4: [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

pub type EfiImageUnload = extern "C" fn(EfiHandle) -> EfiStatus;

#[repr(C)]
pub struct EfiLoadedImageProtocol {
    pub revision: u32,
    pub parent_handle: EfiHandle,
    pub system_table: *mut super::system_table::SystemTable,
    pub device_handle: EfiHandle,
    pub file_path: *mut c_void, // EFI_DEVICE_PATH_PROTOCOL* -- not implemented
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: u32,
    pub image_data_type: u32,
    pub unload: Option<EfiImageUnload>,
}

unsafe impl Sync for EfiLoadedImageProtocol {}
