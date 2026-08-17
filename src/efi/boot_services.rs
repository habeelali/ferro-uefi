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
use super::protocols::{EfiLoadedImageProtocol, LOADED_IMAGE_PROTOCOL_GUID};
use super::types::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

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
pub type LoadImageFn =
    extern "C" fn(u8, EfiHandle, *mut c_void, *mut c_void, usize, *mut EfiHandle) -> EfiStatus;
pub type StartImageFn = extern "C" fn(EfiHandle, *mut usize, *mut *mut u16) -> EfiStatus;
pub type ExitBootServicesFn = extern "C" fn(EfiHandle, usize) -> EfiStatus;
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

    pub load_image: LoadImageFn,
    pub start_image: StartImageFn,
    pub exit: StubFn,
    pub unload_image: StubFn,
    pub exit_boot_services: ExitBootServicesFn,

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

/// Set once ExitBootServices succeeds. Per spec, a real batch of Boot
/// Services functions become invalid to call after that point; the
/// functions below that matter for us check this and return
/// EFI_UNSUPPORTED instead of silently doing boot-time things after
/// the caller has taken over the machine.
static BOOT_SERVICES_EXITED: AtomicBool = AtomicBool::new(false);

fn exited() -> bool {
    BOOT_SERVICES_EXITED.load(Ordering::Relaxed)
}

// LoadImage/StartImage state. EFI_LOADED_IMAGE_PROTOCOL doesn't carry
// an entry-point field (real firmwares track that internally too), so
// ENTRY_POINTS is a parallel table keyed by the same handle index
// protocol_db uses -- both arrays are sized to protocol_db::MAX_HANDLES
// so the indices always line up.
const EMPTY_LOADED_IMAGE: EfiLoadedImageProtocol = EfiLoadedImageProtocol {
    revision: 0x1000,
    parent_handle: core::ptr::null_mut(),
    system_table: core::ptr::null_mut(),
    device_handle: core::ptr::null_mut(),
    file_path: core::ptr::null_mut(),
    reserved: core::ptr::null_mut(),
    load_options_size: 0,
    load_options: core::ptr::null_mut(),
    image_base: core::ptr::null_mut(),
    image_size: 0,
    image_code_type: EFI_LOADER_CODE,
    image_data_type: EFI_LOADER_DATA,
    unload: None,
};
static mut LOADED_IMAGE_PROTOCOLS: [EfiLoadedImageProtocol; protocol_db::MAX_HANDLES] =
    [EMPTY_LOADED_IMAGE; protocol_db::MAX_HANDLES];
static mut ENTRY_POINTS: [u64; protocol_db::MAX_HANDLES] = [0; protocol_db::MAX_HANDLES];

extern "C" fn raise_tpl(new_tpl: usize) -> usize {
    // Bookkeeping only -- doesn't actually mask interrupts by
    // priority yet. Fine while nothing depends on TPL for exclusion.
    CURRENT_TPL.swap(new_tpl, Ordering::Relaxed)
}

extern "C" fn restore_tpl(old_tpl: usize) {
    CURRENT_TPL.store(old_tpl, Ordering::Relaxed);
}

extern "C" fn allocate_pages(_alloc_type: u32, _memory_type: u32, pages: usize, memory: *mut u64) -> EfiStatus {
    if exited() {
        return EFI_UNSUPPORTED;
    }
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
        // Changes exactly when the map does (see memory.rs); this is
        // what ExitBootServices checks against.
        unsafe { *map_key = super::memory::generation() as usize };
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
    if exited() {
        return EFI_UNSUPPORTED;
    }
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

/// Loads a PE32+ AArch64 EFI application already sitting in memory
/// (`source_buffer`/`source_size`) -- there's no device-path support,
/// so `device_path` is accepted but unused; every caller so far loads
/// straight from a buffer FAT32 already read off the SD card.
extern "C" fn load_image(
    _boot_policy: u8,
    parent_image_handle: EfiHandle,
    _device_path: *mut c_void,
    source_buffer: *mut c_void,
    source_size: usize,
    image_handle: *mut EfiHandle,
) -> EfiStatus {
    if exited() {
        return EFI_UNSUPPORTED;
    }
    if source_buffer.is_null() || image_handle.is_null() || source_size == 0 {
        return EFI_INVALID_PARAMETER;
    }
    let data = unsafe { core::slice::from_raw_parts(source_buffer as *const u8, source_size) };

    let loaded = match crate::pe::load(data) {
        Ok(l) => l,
        Err(_) => return EFI_UNSUPPORTED,
    };

    let Some(index) = protocol_db::find_or_create_handle(core::ptr::null_mut()) else {
        return EFI_OUT_OF_RESOURCES;
    };
    let handle = protocol_db::handle_for_index(index);

    let li_ptr = core::ptr::addr_of_mut!(LOADED_IMAGE_PROTOCOLS);
    let ep_ptr = core::ptr::addr_of_mut!(ENTRY_POINTS);
    unsafe {
        (*li_ptr)[index] = EMPTY_LOADED_IMAGE;
        (*li_ptr)[index].parent_handle = parent_image_handle;
        (*li_ptr)[index].system_table = core::ptr::addr_of_mut!(super::system_table::SYSTEM_TABLE);
        (*li_ptr)[index].image_base = loaded.image_base as *mut c_void;
        (*li_ptr)[index].image_size = loaded.image_size;
        (*ep_ptr)[index] = loaded.entry_point;
    }

    let interface = unsafe { core::ptr::addr_of_mut!((*li_ptr)[index]) as *mut c_void };
    if !protocol_db::install(index, LOADED_IMAGE_PROTOCOL_GUID, interface) {
        return EFI_OUT_OF_RESOURCES;
    }

    unsafe { *image_handle = handle };
    EFI_SUCCESS
}

/// Calls straight into the loaded image's entry point with
/// `(ImageHandle, SystemTable*)`, per the EFI calling convention --
/// on AArch64 that's just plain AAPCS64, so this is a direct call
/// through a transmuted function pointer, no ABI shim needed.
extern "C" fn start_image(
    image_handle: EfiHandle,
    _exit_data_size: *mut usize,
    _exit_data: *mut *mut u16,
) -> EfiStatus {
    if exited() {
        return EFI_UNSUPPORTED;
    }
    let Some(index) = protocol_db::index_of(image_handle) else {
        return EFI_INVALID_PARAMETER;
    };
    let entry = unsafe { (*core::ptr::addr_of!(ENTRY_POINTS))[index] };
    if entry == 0 {
        return EFI_INVALID_PARAMETER;
    }

    let entry_fn: extern "C" fn(EfiHandle, *mut super::system_table::SystemTable) -> EfiStatus =
        unsafe { core::mem::transmute(entry as usize) };
    let st = core::ptr::addr_of_mut!(super::system_table::SYSTEM_TABLE);
    entry_fn(image_handle, st)
}

/// The real map-key handshake: the caller must pass the map_key it
/// got from a GetMemoryMap call made after its own last allocation,
/// proving it isn't about to tear things down against a stale map.
/// `_image_handle` isn't checked against anything -- we don't track
/// "the" boot image the way a real firmware with a single boot flow
/// does, since Ferro's own menu can load different images across
/// multiple attempts.
extern "C" fn exit_boot_services(_image_handle: EfiHandle, map_key: usize) -> EfiStatus {
    if exited() {
        return EFI_SUCCESS; // already exited; idempotent per spec intent
    }
    if map_key as u64 != super::memory::generation() {
        return EFI_INVALID_PARAMETER;
    }
    BOOT_SERVICES_EXITED.store(true, Ordering::Relaxed);
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

    load_image,
    start_image,
    exit: stub,
    unload_image: stub,
    exit_boot_services,

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
