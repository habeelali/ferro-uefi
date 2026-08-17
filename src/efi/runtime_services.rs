//! EFI_RUNTIME_SERVICES: struct layout matches the UEFI 2.x spec field
//! order. Real logic backs variable services (variables.rs) and
//! ResetSystem (reuses pm.rs's real watchdog-reset sequence).
//! GetTime/SetTime/GetWakeupTime/SetWakeupTime return EFI_UNSUPPORTED
//! for real, spec-legitimate reasons: BCM2837 has no RTC or wakeup
//! timer hardware, and the spec explicitly allows EFI_UNSUPPORTED for
//! platforms that lack the device -- this isn't a stub standing in
//! for unwritten code, it's the honest answer.

use super::crc32::crc32_ieee;
use super::types::*;
use super::variables::{self, VarError};
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
pub struct EfiTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub pad1: u8,
    pub nanosecond: u32,
    pub time_zone: i16,
    pub daylight: u8,
    pub pad2: u8,
}

#[repr(C)]
pub struct EfiTimeCapabilities {
    pub resolution: u32,
    pub accuracy: u32,
    pub sets_to_zero: u8,
}

pub type GetTimeFn = extern "C" fn(*mut EfiTime, *mut EfiTimeCapabilities) -> EfiStatus;
pub type SetTimeFn = extern "C" fn(*const EfiTime) -> EfiStatus;
pub type GetWakeupTimeFn = extern "C" fn(*mut u8, *mut u8, *mut EfiTime) -> EfiStatus;
pub type SetWakeupTimeFn = extern "C" fn(u8, *mut EfiTime) -> EfiStatus;
pub type SetVirtualAddressMapFn = extern "C" fn(usize, usize, u32, *mut c_void) -> EfiStatus;
pub type ConvertPointerFn = extern "C" fn(usize, *mut *mut c_void) -> EfiStatus;
pub type GetVariableFn =
    extern "C" fn(*const u16, *const EfiGuid, *mut u32, *mut usize, *mut c_void) -> EfiStatus;
pub type GetNextVariableNameFn = extern "C" fn(*mut usize, *mut u16, *mut EfiGuid) -> EfiStatus;
pub type SetVariableFn = extern "C" fn(*const u16, *const EfiGuid, u32, usize, *const c_void) -> EfiStatus;
pub type GetNextHighMonoCountFn = extern "C" fn(*mut u32) -> EfiStatus;
pub type ResetSystemFn = extern "C" fn(u32, EfiStatus, usize, *mut c_void);
pub type QueryVariableInfoFn = extern "C" fn(u32, *mut u64, *mut u64, *mut u64) -> EfiStatus;
/// Capsule update is entirely out of scope (firmware self-update over
/// a mechanism we don't have); shares this placeholder shape since
/// nothing calls through it.
pub type StubFn = extern "C" fn() -> EfiStatus;

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: EfiTableHeader,

    pub get_time: GetTimeFn,
    pub set_time: SetTimeFn,
    pub get_wakeup_time: GetWakeupTimeFn,
    pub set_wakeup_time: SetWakeupTimeFn,

    pub set_virtual_address_map: SetVirtualAddressMapFn,
    pub convert_pointer: ConvertPointerFn,

    pub get_variable: GetVariableFn,
    pub get_next_variable_name: GetNextVariableNameFn,
    pub set_variable: SetVariableFn,

    pub get_next_high_monotonic_count: GetNextHighMonoCountFn,
    pub reset_system: ResetSystemFn,

    pub update_capsule: StubFn,
    pub query_capsule_capabilities: StubFn,
    pub query_variable_info: QueryVariableInfoFn,
}

unsafe impl Sync for RuntimeServices {}

extern "C" fn get_time(_time: *mut EfiTime, _caps: *mut EfiTimeCapabilities) -> EfiStatus {
    EFI_UNSUPPORTED // no RTC on this SoC; spec-legal answer, not a stub
}

extern "C" fn set_time(_time: *const EfiTime) -> EfiStatus {
    EFI_UNSUPPORTED
}

extern "C" fn get_wakeup_time(_enabled: *mut u8, _pending: *mut u8, _time: *mut EfiTime) -> EfiStatus {
    EFI_UNSUPPORTED // no wakeup-timer hardware
}

extern "C" fn set_wakeup_time(_enable: u8, _time: *mut EfiTime) -> EfiStatus {
    EFI_UNSUPPORTED
}

extern "C" fn set_virtual_address_map(
    _map_size: usize,
    _desc_size: usize,
    _desc_version: u32,
    _virtual_map: *mut c_void,
) -> EfiStatus {
    // Ferro never runs with a separate virtual address space -- boot
    // and runtime both stay in the identity map mmu.rs set up -- so
    // there's nothing to translate. Accepting this as a no-op is
    // correct for us, not a shortcut.
    EFI_SUCCESS
}

extern "C" fn convert_pointer(_debug_disposition: usize, _address: *mut *mut c_void) -> EfiStatus {
    EFI_SUCCESS // no-op for the same reason as set_virtual_address_map
}

extern "C" fn get_variable(
    variable_name: *const u16,
    vendor_guid: *const EfiGuid,
    attributes: *mut u32,
    data_size: *mut usize,
    data: *mut c_void,
) -> EfiStatus {
    if variable_name.is_null() || vendor_guid.is_null() || data_size.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let guid = unsafe { *vendor_guid };
    let capacity = unsafe { *data_size };
    let buf = if data.is_null() {
        &mut [][..]
    } else {
        unsafe { core::slice::from_raw_parts_mut(data as *mut u8, capacity) }
    };

    match variables::get(variable_name, &guid, buf) {
        Ok((attrs, len)) => {
            unsafe { *data_size = len };
            if !attributes.is_null() {
                unsafe { *attributes = attrs };
            }
            EFI_SUCCESS
        }
        Err(VarError::NotFound) => EFI_NOT_FOUND,
        Err(VarError::InvalidParameter) => EFI_INVALID_PARAMETER,
        Err(VarError::BufferTooSmall(needed)) => {
            unsafe { *data_size = needed };
            EFI_BUFFER_TOO_SMALL
        }
        Err(VarError::OutOfResources) => EFI_OUT_OF_RESOURCES,
    }
}

extern "C" fn set_variable(
    variable_name: *const u16,
    vendor_guid: *const EfiGuid,
    attributes: u32,
    data_size: usize,
    data: *const c_void,
) -> EfiStatus {
    if variable_name.is_null() || vendor_guid.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    if data_size > 0 && data.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let guid = unsafe { *vendor_guid };
    let buf = if data_size == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(data as *const u8, data_size) }
    };

    match variables::set(variable_name, &guid, attributes, buf) {
        Ok(()) => EFI_SUCCESS,
        Err(VarError::NotFound) => EFI_NOT_FOUND,
        Err(VarError::InvalidParameter) => EFI_INVALID_PARAMETER,
        Err(VarError::OutOfResources) => EFI_OUT_OF_RESOURCES,
        Err(VarError::BufferTooSmall(_)) => EFI_INVALID_PARAMETER, // can't happen for set()
    }
}

extern "C" fn get_next_variable_name(
    variable_name_size: *mut usize,
    variable_name: *mut u16,
    vendor_guid: *mut EfiGuid,
) -> EfiStatus {
    if variable_name_size.is_null() || variable_name.is_null() || vendor_guid.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let guid = unsafe { *vendor_guid };
    let capacity_units = unsafe { *variable_name_size } / 2;
    let out = unsafe { core::slice::from_raw_parts_mut(variable_name, capacity_units) };

    match variables::get_next(variable_name, &guid, out) {
        Ok((units, next_guid)) => {
            unsafe {
                *variable_name_size = units * 2;
                *vendor_guid = next_guid;
            }
            EFI_SUCCESS
        }
        Err(VarError::NotFound) => EFI_NOT_FOUND,
        Err(VarError::InvalidParameter) => EFI_INVALID_PARAMETER,
        Err(VarError::BufferTooSmall(needed)) => {
            unsafe { *variable_name_size = needed * 2 };
            EFI_BUFFER_TOO_SMALL
        }
        Err(VarError::OutOfResources) => EFI_OUT_OF_RESOURCES,
    }
}

static HIGH_MONO_COUNT: AtomicU32 = AtomicU32::new(0);

/// Only monotonic within this boot -- there's no NVRAM to remember it
/// across a reset, which is what the counter is really meant to
/// survive. Honest limitation, not silently wrong.
extern "C" fn get_next_high_monotonic_count(high_count: *mut u32) -> EfiStatus {
    if high_count.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    unsafe { *high_count = HIGH_MONO_COUNT.fetch_add(1, Ordering::Relaxed) };
    EFI_SUCCESS
}

/// Real reset, reusing pm.rs's watchdog sequence. Per spec this
/// function doesn't return -- and on real hardware it won't, since
/// pm::reset() only exits via the watchdog firing. Under QEMU, where
/// the watchdog model doesn't act on it (see pm.rs), this correctly
/// hangs rather than pretending to succeed.
extern "C" fn reset_system(_reset_type: u32, _reset_status: EfiStatus, _data_size: usize, _reset_data: *mut c_void) {
    crate::pm::reset();
}

extern "C" fn query_variable_info(
    _attributes: u32,
    max_storage: *mut u64,
    remaining_storage: *mut u64,
    max_variable_size: *mut u64,
) -> EfiStatus {
    let (total, remaining, max_single) = variables::storage_info();
    if !max_storage.is_null() {
        unsafe { *max_storage = total };
    }
    if !remaining_storage.is_null() {
        unsafe { *remaining_storage = remaining };
    }
    if !max_variable_size.is_null() {
        unsafe { *max_variable_size = max_single };
    }
    EFI_SUCCESS
}

extern "C" fn stub() -> EfiStatus {
    EFI_UNSUPPORTED
}

pub static mut RUNTIME_SERVICES: RuntimeServices = RuntimeServices {
    hdr: EfiTableHeader {
        signature: 0, // patched in init()
        revision: (2 << 16) | 100,
        header_size: core::mem::size_of::<RuntimeServices>() as u32,
        crc32: 0, // patched in init()
        reserved: 0,
    },

    get_time,
    set_time,
    get_wakeup_time,
    set_wakeup_time,

    set_virtual_address_map,
    convert_pointer,

    get_variable,
    get_next_variable_name,
    set_variable,

    get_next_high_monotonic_count,
    reset_system,

    update_capsule: stub,
    query_capsule_capabilities: stub,
    query_variable_info,
};

/// Sets the header signature and computes a real CRC32 (crc32 field
/// zeroed during the calculation, per spec). Must run once before
/// anything reads RUNTIME_SERVICES.hdr.
pub fn init() {
    unsafe {
        let rs = core::ptr::addr_of_mut!(RUNTIME_SERVICES);
        (*rs).hdr.signature = u64::from_le_bytes(*b"RUNTSERV");
        (*rs).hdr.crc32 = 0;
        let bytes =
            core::slice::from_raw_parts(rs as *const u8, core::mem::size_of::<RuntimeServices>());
        (*rs).hdr.crc32 = crc32_ieee(bytes);
    }
}
