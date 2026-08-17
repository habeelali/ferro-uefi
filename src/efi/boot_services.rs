//! EFI_BOOT_SERVICES: struct layout matches the UEFI 2.x spec field
//! order exactly (that's what makes it callable by real EFI apps
//! later), but only a working subset is implemented for real --
//! Memory Services, protocol install/locate/handle, Stall, CopyMem/
//! SetMem, CalculateCrc32, and TPL bookkeeping (which doesn't actually
//! affect interrupt masking yet). Everything else is a stub returning
//! EFI_UNSUPPORTED; the struct fields exist for ABI completeness and
//! get wired up to real logic as the code that needs them lands.

use super::crc32::crc32_ieee;
use super::protocol_db;
use super::types::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub type RaiseTplFn = extern "C" fn(usize) -> usize;
pub type RestoreTplFn = extern "C" fn(usize);
pub type AllocatePagesFn = extern "C" fn(u32, u32, usize, *mut u64) -> EfiStatus;
pub type FreePagesFn = extern "C" fn(u64, usize) -> EfiStatus;
pub type GetMemoryMapFn =
    extern "C" fn(*mut usize, *mut EfiMemoryDescriptor, *mut usize, *mut usize, *mut u32) -> EfiStatus;
pub type AllocatePoolFn = extern "C" fn(u32, usize, *mut *mut c_void) -> EfiStatus;
pub type FreePoolFn = extern "C" fn(*mut c_void) -> EfiStatus;
pub type InstallProtocolInterfaceFn =
    extern "C" fn(*mut EfiHandle, *const EfiGuid, u32, *mut c_void) -> EfiStatus;
pub type HandleProtocolFn = extern "C" fn(EfiHandle, *const EfiGuid, *mut *mut c_void) -> EfiStatus;
pub type LocateProtocolFn = extern "C" fn(*const EfiGuid, *mut c_void, *mut *mut c_void) -> EfiStatus;
pub type StallFn = extern "C" fn(usize) -> EfiStatus;
pub type CopyMemFn = extern "C" fn(*mut c_void, *const c_void, usize);
pub type SetMemFn = extern "C" fn(*mut c_void, usize, u8);
pub type CalculateCrc32Fn = extern "C" fn(*const c_void, usize, *mut u32) -> EfiStatus;
/// Real UEFI stub signatures vary a lot; since nothing calls through
/// these yet (no PE/COFF loader exists to run a real EFI app), they
/// share this placeholder shape rather than each getting the exact
/// spec signature. Tighten as real callers show up.
pub type StubFn = extern "C" fn() -> EfiStatus;

#[repr(C)]
pub struct BootServices {
    pub hdr: EfiTableHeader,

    pub raise_tpl: RaiseTplFn,
    pub restore_tpl: RestoreTplFn,

    pub allocate_pages: AllocatePagesFn,
    pub free_pages: FreePagesFn,
    pub get_memory_map: GetMemoryMapFn,
    pub allocate_pool: AllocatePoolFn,
    pub free_pool: FreePoolFn,

    pub create_event: StubFn,
    pub set_timer: StubFn,
    pub wait_for_event: StubFn,
    pub signal_event: StubFn,
    pub close_event: StubFn,
    pub check_event: StubFn,

    pub install_protocol_interface: InstallProtocolInterfaceFn,
    pub reinstall_protocol_interface: StubFn,
    pub uninstall_protocol_interface: StubFn,
    pub handle_protocol: HandleProtocolFn,
    pub reserved: *mut c_void,
    pub register_protocol_notify: StubFn,
    pub locate_handle: StubFn,
    pub locate_device_path: StubFn,
    pub install_configuration_table: StubFn,

    pub load_image: StubFn,
    pub start_image: StubFn,
    pub exit: StubFn,
    pub unload_image: StubFn,
    pub exit_boot_services: StubFn,

    pub get_next_monotonic_count: StubFn,
    pub stall: StallFn,
    pub set_watchdog_timer: StubFn,

    pub connect_controller: StubFn,
    pub disconnect_controller: StubFn,

    pub open_protocol: StubFn,
    pub close_protocol: StubFn,
    pub open_protocol_information: StubFn,

    pub protocols_per_handle: StubFn,
    pub locate_handle_buffer: StubFn,
    pub locate_protocol: LocateProtocolFn,
    pub install_multiple_protocol_interfaces: StubFn,
    pub uninstall_multiple_protocol_interfaces: StubFn,

    pub calculate_crc32: CalculateCrc32Fn,

    pub copy_mem: CopyMemFn,
    pub set_mem: SetMemFn,
    pub create_event_ex: StubFn,
}

unsafe impl Sync for BootServices {}

// Framebuffer region, set once (see set_framebuffer_region) so
// get_memory_map can report it accurately instead of guessing.
static FB_BASE: AtomicU64 = AtomicU64::new(0);
static FB_SIZE: AtomicU64 = AtomicU64::new(0);

pub fn set_framebuffer_region(base: u64, size: u64) {
    FB_BASE.store(base, Ordering::Relaxed);
    FB_SIZE.store(size, Ordering::Relaxed);
}

static CURRENT_TPL: AtomicUsize = AtomicUsize::new(4); // TPL_APPLICATION

extern "C" fn raise_tpl(new_tpl: usize) -> usize {
    // Bookkeeping only -- doesn't actually mask interrupts by
    // priority yet. Fine while nothing depends on TPL for exclusion.
    CURRENT_TPL.swap(new_tpl, Ordering::Relaxed)
}

extern "C" fn restore_tpl(old_tpl: usize) {
    CURRENT_TPL.store(old_tpl, Ordering::Relaxed);
}

extern "C" fn allocate_pages(_alloc_type: u32, _memory_type: u32, pages: usize, memory: *mut u64) -> EfiStatus {
    if memory.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    match super::memory::allocate_pages(pages as u64) {
        Some(base) => {
            unsafe { *memory = base };
            EFI_SUCCESS
        }
        None => EFI_OUT_OF_RESOURCES,
    }
}

extern "C" fn free_pages(_memory: u64, _pages: usize) -> EfiStatus {
    EFI_SUCCESS // accepted, not reclaimed -- see memory.rs
}

extern "C" fn get_memory_map(
    memory_map_size: *mut usize,
    memory_map: *mut EfiMemoryDescriptor,
    map_key: *mut usize,
    descriptor_size: *mut usize,
    descriptor_version: *mut u32,
) -> EfiStatus {
    if memory_map_size.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let fb_base = FB_BASE.load(Ordering::Relaxed);
    let fb_size = FB_SIZE.load(Ordering::Relaxed);

    let mut scratch = [EfiMemoryDescriptor {
        ty: 0,
        physical_start: 0,
        virtual_start: 0,
        number_of_pages: 0,
        attribute: 0,
    }; 8];
    let Some(n) = super::memory::get_memory_map(fb_base, fb_size, &mut scratch) else {
        return EFI_BUFFER_TOO_SMALL;
    };

    let desc_size = core::mem::size_of::<EfiMemoryDescriptor>();
    let capacity = unsafe { *memory_map_size } / desc_size;
    unsafe { *memory_map_size = n * desc_size };
    if capacity < n {
        return EFI_BUFFER_TOO_SMALL;
    }
    if !memory_map.is_null() {
        unsafe {
            for (i, d) in scratch[..n].iter().enumerate() {
                *memory_map.add(i) = *d;
            }
        }
    }
    if !map_key.is_null() {
        unsafe { *map_key = 0 }; // no reclamation yet, so no key to invalidate against
    }
    if !descriptor_size.is_null() {
        unsafe { *descriptor_size = desc_size };
    }
    if !descriptor_version.is_null() {
        unsafe { *descriptor_version = EFI_MEMORY_DESCRIPTOR_VERSION };
    }
    EFI_SUCCESS
}

extern "C" fn allocate_pool(_pool_type: u32, size: usize, buffer: *mut *mut c_void) -> EfiStatus {
    if buffer.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    // Whole-page granularity -- simple and correct, wasteful for
    // small requests. Worth a real sub-page allocator once something
    // makes enough small allocations to care.
    let pages = ((size as u64) + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE;
    match super::memory::allocate_pages(pages.max(1)) {
        Some(base) => {
            unsafe { *buffer = base as *mut c_void };
            EFI_SUCCESS
        }
        None => EFI_OUT_OF_RESOURCES,
    }
}

extern "C" fn free_pool(_buffer: *mut c_void) -> EfiStatus {
    EFI_SUCCESS // accepted, not reclaimed -- see memory.rs
}

extern "C" fn install_protocol_interface(
    handle: *mut EfiHandle,
    protocol: *const EfiGuid,
    _interface_type: u32,
    interface: *mut c_void,
) -> EfiStatus {
    if handle.is_null() || protocol.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let existing = unsafe { *handle };
    let Some(index) = protocol_db::find_or_create_handle(existing) else {
        return EFI_OUT_OF_RESOURCES;
    };
    let guid = unsafe { *protocol };
    if !protocol_db::install(index, guid, interface) {
        return EFI_OUT_OF_RESOURCES;
    }
    unsafe { *handle = protocol_db::handle_for_index(index) };
    EFI_SUCCESS
}

extern "C" fn handle_protocol(handle: EfiHandle, protocol: *const EfiGuid, interface: *mut *mut c_void) -> EfiStatus {
    if protocol.is_null() || interface.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let guid = unsafe { *protocol };
    match protocol_db::handle_protocol(handle, &guid) {
        Some(iface) => {
            unsafe { *interface = iface };
            EFI_SUCCESS
        }
        None => EFI_NOT_FOUND,
    }
}

extern "C" fn locate_protocol(
    protocol: *const EfiGuid,
    _registration: *mut c_void,
    interface: *mut *mut c_void,
) -> EfiStatus {
    if protocol.is_null() || interface.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let guid = unsafe { *protocol };
    match protocol_db::locate(&guid) {
        Some(iface) => {
            unsafe { *interface = iface };
            EFI_SUCCESS
        }
        None => EFI_NOT_FOUND,
    }
}

extern "C" fn stall(microseconds: usize) -> EfiStatus {
    crate::timer::sleep_ticks(crate::timer::micros_to_ticks(microseconds as u64));
    EFI_SUCCESS
}

extern "C" fn copy_mem(destination: *mut c_void, source: *const c_void, length: usize) {
    unsafe { core::ptr::copy(source as *const u8, destination as *mut u8, length) };
}

extern "C" fn set_mem(buffer: *mut c_void, size: usize, value: u8) {
    unsafe { core::ptr::write_bytes(buffer as *mut u8, value, size) };
}

extern "C" fn calculate_crc32(data: *const c_void, data_size: usize, crc32: *mut u32) -> EfiStatus {
    if data.is_null() || crc32.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data as *const u8, data_size) };
    unsafe { *crc32 = crc32_ieee(bytes) };
    EFI_SUCCESS
}

extern "C" fn stub() -> EfiStatus {
    EFI_UNSUPPORTED
}

pub static mut BOOT_SERVICES: BootServices = BootServices {
    hdr: EfiTableHeader {
        signature: 0, // patched in init(), see below
        revision: (2 << 16) | 100,
        header_size: core::mem::size_of::<BootServices>() as u32,
        crc32: 0, // patched in init()
        reserved: 0,
    },

    raise_tpl,
    restore_tpl,

    allocate_pages,
    free_pages,
    get_memory_map,
    allocate_pool,
    free_pool,

    create_event: stub,
    set_timer: stub,
    wait_for_event: stub,
    signal_event: stub,
    close_event: stub,
    check_event: stub,

    install_protocol_interface,
    reinstall_protocol_interface: stub,
    uninstall_protocol_interface: stub,
    handle_protocol,
    reserved: core::ptr::null_mut(),
    register_protocol_notify: stub,
    locate_handle: stub,
    locate_device_path: stub,
    install_configuration_table: stub,

    load_image: stub,
    start_image: stub,
    exit: stub,
    unload_image: stub,
    exit_boot_services: stub,

    get_next_monotonic_count: stub,
    stall,
    set_watchdog_timer: stub,

    connect_controller: stub,
    disconnect_controller: stub,

    open_protocol: stub,
    close_protocol: stub,
    open_protocol_information: stub,

    protocols_per_handle: stub,
    locate_handle_buffer: stub,
    locate_protocol,
    install_multiple_protocol_interfaces: stub,
    uninstall_multiple_protocol_interfaces: stub,

    calculate_crc32,

    copy_mem,
    set_mem,
    create_event_ex: stub,
};

/// Sets the header signature and computes a real CRC32 over the table
/// (with crc32 zeroed during the calculation, per spec). Must run
/// once before anything reads BOOT_SERVICES.hdr.
pub fn init() {
    unsafe {
        let bs = core::ptr::addr_of_mut!(BOOT_SERVICES);
        (*bs).hdr.signature = u64::from_le_bytes(*b"BOOTSERV");
        (*bs).hdr.crc32 = 0;
        let bytes =
            core::slice::from_raw_parts(bs as *const u8, core::mem::size_of::<BootServices>());
        (*bs).hdr.crc32 = crc32_ieee(bytes);
    }
}
