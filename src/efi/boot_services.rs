//! EFI_BOOT_SERVICES: struct layout matches the UEFI 2.x spec field
//! order exactly (that's what makes it callable by real EFI apps
//! later), but only a working subset is implemented for real --
//! Memory Services, protocol install/locate/handle, Stall, CopyMem/
//! SetMem, CalculateCrc32, and TPL bookkeeping (which doesn't actually
//! affect interrupt masking yet). Everything else is a stub returning
//! EFI_UNSUPPORTED; the struct fields exist for ABI completeness and
//! get wired up to real logic as the code that needs them lands.

use super::crc32::crc32_ieee;
use super::events::{self, Kind};
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
pub type ReinstallProtocolInterfaceFn =
    extern "C" fn(EfiHandle, *const EfiGuid, *mut c_void, *mut c_void) -> EfiStatus;
pub type UninstallProtocolInterfaceFn = extern "C" fn(EfiHandle, *const EfiGuid, *mut c_void) -> EfiStatus;
pub type HandleProtocolFn = extern "C" fn(EfiHandle, *const EfiGuid, *mut *mut c_void) -> EfiStatus;
pub type LocateHandleFn = extern "C" fn(u32, *const EfiGuid, *mut c_void, *mut usize, *mut EfiHandle) -> EfiStatus;
pub type LocateHandleBufferFn =
    extern "C" fn(u32, *const EfiGuid, *mut c_void, *mut usize, *mut *mut EfiHandle) -> EfiStatus;
pub type LocateProtocolFn = extern "C" fn(*const EfiGuid, *mut c_void, *mut *mut c_void) -> EfiStatus;
pub type InstallConfigurationTableFn = extern "C" fn(*const EfiGuid, *mut c_void) -> EfiStatus;
pub type LoadImageFn =
    extern "C" fn(u8, EfiHandle, *mut c_void, *mut c_void, usize, *mut EfiHandle) -> EfiStatus;
pub type StartImageFn = extern "C" fn(EfiHandle, *mut usize, *mut *mut u16) -> EfiStatus;
pub type UnloadImageFn = extern "C" fn(EfiHandle) -> EfiStatus;
pub type ExitBootServicesFn = extern "C" fn(EfiHandle, usize) -> EfiStatus;
pub type GetNextMonotonicCountFn = extern "C" fn(*mut u64) -> EfiStatus;
pub type StallFn = extern "C" fn(usize) -> EfiStatus;
pub type SetWatchdogTimerFn = extern "C" fn(usize, u64, usize, *mut u16) -> EfiStatus;
pub type CopyMemFn = extern "C" fn(*mut c_void, *const c_void, usize);
pub type SetMemFn = extern "C" fn(*mut c_void, usize, u8);
pub type CalculateCrc32Fn = extern "C" fn(*const c_void, usize, *mut u32) -> EfiStatus;

pub type EventNotifyFn = extern "C" fn(EfiEvent, *mut c_void);
pub type CreateEventFn = extern "C" fn(u32, usize, Option<EventNotifyFn>, *mut c_void, *mut EfiEvent) -> EfiStatus;
pub type CreateEventExFn =
    extern "C" fn(u32, usize, Option<EventNotifyFn>, *const c_void, *const EfiGuid, *mut EfiEvent) -> EfiStatus;
pub type SetTimerFn = extern "C" fn(EfiEvent, u32, u64) -> EfiStatus;
pub type WaitForEventFn = extern "C" fn(usize, *mut EfiEvent, *mut usize) -> EfiStatus;
pub type SignalEventFn = extern "C" fn(EfiEvent) -> EfiStatus;
pub type CloseEventFn = extern "C" fn(EfiEvent) -> EfiStatus;
pub type CheckEventFn = extern "C" fn(EfiEvent) -> EfiStatus;

pub type OpenProtocolFn =
    extern "C" fn(EfiHandle, *const EfiGuid, *mut *mut c_void, EfiHandle, EfiHandle, u32) -> EfiStatus;
pub type CloseProtocolFn = extern "C" fn(EfiHandle, *const EfiGuid, EfiHandle, EfiHandle) -> EfiStatus;
pub type ProtocolsPerHandleFn = extern "C" fn(EfiHandle, *mut *mut *const EfiGuid, *mut usize) -> EfiStatus;

/// A handful of Boot Services functions are either genuinely
/// impossible to implement in safe Rust (InstallMultipleProtocol-
/// Interfaces is a real C varargs function -- Rust can call those but
/// can't *define* one) or need a driver-connection model Ferro
/// doesn't have (ConnectController and friends, since there's no
/// generic driver-binding-protocol framework here, just hand-written
/// device drivers). Those stay honest EFI_UNSUPPORTED stubs sharing
/// this placeholder shape.
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

    pub create_event: CreateEventFn,
    pub set_timer: SetTimerFn,
    pub wait_for_event: WaitForEventFn,
    pub signal_event: SignalEventFn,
    pub close_event: CloseEventFn,
    pub check_event: CheckEventFn,

    pub install_protocol_interface: InstallProtocolInterfaceFn,
    pub reinstall_protocol_interface: ReinstallProtocolInterfaceFn,
    pub uninstall_protocol_interface: UninstallProtocolInterfaceFn,
    pub handle_protocol: HandleProtocolFn,
    pub reserved: *mut c_void,
    pub register_protocol_notify: StubFn,
    pub locate_handle: LocateHandleFn,
    pub locate_device_path: StubFn,
    pub install_configuration_table: InstallConfigurationTableFn,

    pub load_image: LoadImageFn,
    pub start_image: StartImageFn,
    pub exit: StubFn,
    pub unload_image: UnloadImageFn,
    pub exit_boot_services: ExitBootServicesFn,

    pub get_next_monotonic_count: GetNextMonotonicCountFn,
    pub stall: StallFn,
    pub set_watchdog_timer: SetWatchdogTimerFn,

    pub connect_controller: StubFn,
    pub disconnect_controller: StubFn,

    pub open_protocol: OpenProtocolFn,
    pub close_protocol: CloseProtocolFn,
    pub open_protocol_information: StubFn,

    pub protocols_per_handle: ProtocolsPerHandleFn,
    pub locate_handle_buffer: LocateHandleBufferFn,
    pub locate_protocol: LocateProtocolFn,
    pub install_multiple_protocol_interfaces: StubFn,
    pub uninstall_multiple_protocol_interfaces: StubFn,

    pub calculate_crc32: CalculateCrc32Fn,

    pub copy_mem: CopyMemFn,
    pub set_mem: SetMemFn,
    pub create_event_ex: CreateEventExFn,
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

extern "C" fn reinstall_protocol_interface(
    handle: EfiHandle,
    protocol: *const EfiGuid,
    _old_interface: *mut c_void,
    new_interface: *mut c_void,
) -> EfiStatus {
    if protocol.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(index) = protocol_db::index_of(handle) else {
        return EFI_INVALID_PARAMETER;
    };
    let guid = unsafe { *protocol };
    protocol_db::uninstall(index, &guid);
    if !protocol_db::install(index, guid, new_interface) {
        return EFI_OUT_OF_RESOURCES;
    }
    EFI_SUCCESS
}

extern "C" fn uninstall_protocol_interface(handle: EfiHandle, protocol: *const EfiGuid, _interface: *mut c_void) -> EfiStatus {
    if protocol.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(index) = protocol_db::index_of(handle) else {
        return EFI_INVALID_PARAMETER;
    };
    let guid = unsafe { *protocol };
    if protocol_db::uninstall(index, &guid) {
        EFI_SUCCESS
    } else {
        EFI_NOT_FOUND
    }
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

/// LocateHandle search types (EFI_LOCATE_SEARCH_TYPE). Only
/// ByProtocol is implemented for real -- AllHandles and
/// ByRegisterNotify would need either a bigger enumeration (all,
/// unfiltered) or the register_protocol_notify machinery this
/// firmware doesn't have.
const ALL_HANDLES: u32 = 0;
const BY_PROTOCOL: u32 = 2;

extern "C" fn locate_handle(
    search_type: u32,
    protocol: *const EfiGuid,
    _search_key: *mut c_void,
    buffer_size: *mut usize,
    buffer: *mut EfiHandle,
) -> EfiStatus {
    if buffer_size.is_null() || search_type != BY_PROTOCOL || protocol.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let guid = unsafe { *protocol };
    let mut scratch = [core::ptr::null_mut(); protocol_db::MAX_HANDLES];
    let count = protocol_db::handles_with(&guid, &mut scratch);
    if count == 0 {
        return EFI_NOT_FOUND;
    }
    let needed = count * core::mem::size_of::<EfiHandle>();
    let capacity = unsafe { *buffer_size };
    unsafe { *buffer_size = needed };
    if capacity < needed {
        return EFI_BUFFER_TOO_SMALL;
    }
    if !buffer.is_null() {
        unsafe {
            for (i, &h) in scratch[..count].iter().enumerate() {
                *buffer.add(i) = h;
            }
        }
    }
    EFI_SUCCESS
}

extern "C" fn locate_handle_buffer(
    search_type: u32,
    protocol: *const EfiGuid,
    search_key: *mut c_void,
    num_handles: *mut usize,
    buffer: *mut *mut EfiHandle,
) -> EfiStatus {
    if num_handles.is_null() || buffer.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let mut scratch = [core::ptr::null_mut(); protocol_db::MAX_HANDLES];
    let count = if search_type == ALL_HANDLES {
        // No "list every handle" primitive in protocol_db (it only
        // indexes by protocol GUID) -- approximate by taking the
        // union of handles carrying any protocol we've ever
        // installed, which in practice is every live handle, since
        // find_or_create_handle always immediately installs at least
        // one protocol on a fresh handle.
        protocol_db::handles_with(&LOADED_IMAGE_PROTOCOL_GUID, &mut scratch)
            + protocol_db::handles_with(&super::console::SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID, &mut scratch[..])
    } else {
        if protocol.is_null() {
            return EFI_INVALID_PARAMETER;
        }
        let guid = unsafe { *protocol };
        protocol_db::handles_with(&guid, &mut scratch)
    };
    let _ = search_key;
    if count == 0 {
        return EFI_NOT_FOUND;
    }
    let pages = ((count * core::mem::size_of::<EfiHandle>()) as u64 + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE;
    let Some(base) = super::memory::allocate_pages(pages.max(1)) else {
        return EFI_OUT_OF_RESOURCES;
    };
    unsafe {
        let out = base as *mut EfiHandle;
        for (i, &h) in scratch[..count].iter().enumerate() {
            *out.add(i) = h;
        }
        *buffer = out;
    }
    unsafe { *num_handles = count };
    EFI_SUCCESS
}

extern "C" fn protocols_per_handle(
    handle: EfiHandle,
    protocol_buffer: *mut *mut *const EfiGuid,
    protocol_buffer_count: *mut usize,
) -> EfiStatus {
    if protocol_buffer.is_null() || protocol_buffer_count.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let mut scratch = [EfiGuid { data1: 0, data2: 0, data3: 0, data4: [0; 8] }; 8];
    let Some(count) = protocol_db::protocols_on_handle(handle, &mut scratch) else {
        return EFI_INVALID_PARAMETER;
    };
    if count == 0 {
        unsafe {
            *protocol_buffer = core::ptr::null_mut();
            *protocol_buffer_count = 0;
        }
        return EFI_SUCCESS;
    }
    // Caller owns and must FreePool this -- an array of GUID
    // *pointers*, per spec, not the GUIDs themselves; store the GUIDs
    // right after the pointer array in the same allocation and point
    // into them.
    let ptr_bytes = count * core::mem::size_of::<*const EfiGuid>();
    let guid_bytes = count * core::mem::size_of::<EfiGuid>();
    let pages = ((ptr_bytes + guid_bytes) as u64 + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE;
    let Some(base) = super::memory::allocate_pages(pages.max(1)) else {
        return EFI_OUT_OF_RESOURCES;
    };
    unsafe {
        let guids_out = (base as *mut u8).add(ptr_bytes) as *mut EfiGuid;
        let ptrs_out = base as *mut *const EfiGuid;
        for i in 0..count {
            *guids_out.add(i) = scratch[i];
            *ptrs_out.add(i) = guids_out.add(i);
        }
        *protocol_buffer = ptrs_out;
        *protocol_buffer_count = count;
    }
    EFI_SUCCESS
}

extern "C" fn open_protocol(
    handle: EfiHandle,
    protocol: *const EfiGuid,
    interface: *mut *mut c_void,
    _agent_handle: EfiHandle,
    _controller_handle: EfiHandle,
    _attributes: u32,
) -> EfiStatus {
    // No open-reference-counting/usage-agent bookkeeping -- delegates
    // straight to HandleProtocol, which covers the overwhelmingly
    // common BY_HANDLE_PROTOCOL usage pattern real apps rely on.
    handle_protocol(handle, protocol, interface)
}

extern "C" fn close_protocol(_handle: EfiHandle, _protocol: *const EfiGuid, _agent_handle: EfiHandle, _controller_handle: EfiHandle) -> EfiStatus {
    EFI_SUCCESS // nothing to release: open_protocol never took a reference
}

const MAX_CONFIG_TABLE_ENTRIES: usize = 8;
static mut CONFIG_TABLE: [EfiConfigurationTableEntry; MAX_CONFIG_TABLE_ENTRIES] = [EfiConfigurationTableEntry {
    vendor_guid: EfiGuid { data1: 0, data2: 0, data3: 0, data4: [0; 8] },
    vendor_table: core::ptr::null_mut(),
}; MAX_CONFIG_TABLE_ENTRIES];
static CONFIG_TABLE_COUNT: AtomicUsize = AtomicUsize::new(0);

extern "C" fn install_configuration_table(guid: *const EfiGuid, table: *mut c_void) -> EfiStatus {
    if guid.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let g = unsafe { *guid };
    let entries = unsafe { &mut *core::ptr::addr_of_mut!(CONFIG_TABLE) };
    let count = CONFIG_TABLE_COUNT.load(Ordering::Relaxed);

    if let Some(existing) = entries[..count].iter_mut().find(|e| e.vendor_guid == g) {
        if table.is_null() {
            // Remove: shift the tail down over it.
            let idx = (existing as *mut EfiConfigurationTableEntry as usize
                - entries.as_ptr() as usize)
                / core::mem::size_of::<EfiConfigurationTableEntry>();
            entries.copy_within(idx + 1..count, idx);
            CONFIG_TABLE_COUNT.store(count - 1, Ordering::Relaxed);
        } else {
            existing.vendor_table = table;
        }
    } else {
        if table.is_null() {
            return EFI_NOT_FOUND;
        }
        if count >= MAX_CONFIG_TABLE_ENTRIES {
            return EFI_OUT_OF_RESOURCES;
        }
        entries[count] = EfiConfigurationTableEntry { vendor_guid: g, vendor_table: table };
        CONFIG_TABLE_COUNT.store(count + 1, Ordering::Relaxed);
    }

    let st = unsafe { &mut *core::ptr::addr_of_mut!(super::system_table::SYSTEM_TABLE) };
    st.configuration_table = entries.as_mut_ptr() as *mut c_void;
    st.number_of_table_entries = CONFIG_TABLE_COUNT.load(Ordering::Relaxed);
    EFI_SUCCESS
}

static MONOTONIC_COUNT: AtomicU64 = AtomicU64::new(0);

extern "C" fn get_next_monotonic_count(count: *mut u64) -> EfiStatus {
    if count.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    unsafe { *count = MONOTONIC_COUNT.fetch_add(1, Ordering::Relaxed) };
    EFI_SUCCESS
}

extern "C" fn set_watchdog_timer(timeout: usize, _watchdog_code: u64, _data_size: usize, _watchdog_data: *mut u16) -> EfiStatus {
    crate::pm::set_watchdog(timeout as u64);
    EFI_SUCCESS
}

extern "C" fn unload_image(image_handle: EfiHandle) -> EfiStatus {
    let Some(index) = protocol_db::index_of(image_handle) else {
        return EFI_INVALID_PARAMETER;
    };
    let li = unsafe { &(*core::ptr::addr_of!(LOADED_IMAGE_PROTOCOLS))[index] };
    match li.unload {
        Some(f) => f(image_handle),
        None => EFI_UNSUPPORTED,
    }
}

extern "C" fn create_event(
    event_type: u32,
    _notify_tpl: usize,
    notify_function: Option<EventNotifyFn>,
    notify_context: *mut c_void,
    event: *mut EfiEvent,
) -> EfiStatus {
    if event.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let kind = if event_type & events::EVT_TIMER != 0 { Kind::Timer } else { Kind::Generic };
    match events::create(kind, notify_function, notify_context) {
        Some(e) => {
            unsafe { *event = e };
            EFI_SUCCESS
        }
        None => EFI_OUT_OF_RESOURCES,
    }
}

extern "C" fn create_event_ex(
    event_type: u32,
    notify_tpl: usize,
    notify_function: Option<EventNotifyFn>,
    notify_context: *const c_void,
    _event_group: *const EfiGuid,
    event: *mut EfiEvent,
) -> EfiStatus {
    // Event groups aren't tracked (no other code signals a group of
    // events together yet) -- functionally the same as CreateEvent.
    create_event(event_type, notify_tpl, notify_function, notify_context as *mut c_void, event)
}

extern "C" fn set_timer(event: EfiEvent, timer_type: u32, trigger_time: u64) -> EfiStatus {
    events::set_timer(event, timer_type, trigger_time)
}

extern "C" fn signal_event(event: EfiEvent) -> EfiStatus {
    events::signal(event)
}

extern "C" fn close_event(event: EfiEvent) -> EfiStatus {
    events::close(event)
}

extern "C" fn check_event(event: EfiEvent) -> EfiStatus {
    events::check(event)
}

extern "C" fn wait_for_event(number_of_events: usize, event: *mut EfiEvent, index: *mut usize) -> EfiStatus {
    if event.is_null() || index.is_null() || number_of_events == 0 {
        return EFI_INVALID_PARAMETER;
    }
    let mut scratch = [core::ptr::null_mut(); 8];
    if number_of_events > scratch.len() {
        return EFI_INVALID_PARAMETER;
    }
    for i in 0..number_of_events {
        scratch[i] = unsafe { *event.add(i) };
    }
    match events::wait(&scratch[..number_of_events]) {
        Ok(i) => {
            unsafe { *index = i };
            EFI_SUCCESS
        }
        Err(e) => e,
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

    create_event,
    set_timer,
    wait_for_event,
    signal_event,
    close_event,
    check_event,

    install_protocol_interface,
    reinstall_protocol_interface,
    uninstall_protocol_interface,
    handle_protocol,
    reserved: core::ptr::null_mut(),
    register_protocol_notify: stub,
    locate_handle,
    locate_device_path: stub,
    install_configuration_table,

    load_image,
    start_image,
    exit: stub,
    unload_image,
    exit_boot_services,

    get_next_monotonic_count,
    stall,
    set_watchdog_timer,

    connect_controller: stub,
    disconnect_controller: stub,

    open_protocol,
    close_protocol,
    open_protocol_information: stub,

    protocols_per_handle,
    locate_handle_buffer,
    locate_protocol,
    install_multiple_protocol_interfaces: stub,
    uninstall_multiple_protocol_interfaces: stub,

    calculate_crc32,

    copy_mem,
    set_mem,
    create_event_ex,
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
