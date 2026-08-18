//! EFI_SIMPLE_FILE_SYSTEM_PROTOCOL + EFI_FILE_PROTOCOL, backed by
//! fat32.rs. Root-directory files only (Ferro's FAT32 driver has no
//! subdirectory support), and each open file's contents are cached
//! whole in a fixed-size in-memory slot rather than streamed --
//! fat32::read_file only ever reads a whole file from its start, so
//! real seek+partial-read semantics are implemented here in memory,
//! with the real disk write happening on Close/Flush. That bounds
//! file size to MAX_FILE_BYTES, an honest, documented limitation
//! rather than a silent one.

use super::types::{
    EfiGuid, EfiStatus, EFI_BUFFER_TOO_SMALL, EFI_INVALID_PARAMETER, EFI_NOT_FOUND, EFI_OUT_OF_RESOURCES,
    EFI_SUCCESS, EFI_UNSUPPORTED, EFI_WRITE_PROTECTED,
};
use crate::fat32::Fat32;
use crate::sd::Card;
use core::ffi::c_void;

pub const SIMPLE_FILE_SYSTEM_PROTOCOL_GUID: EfiGuid = EfiGuid {
    data1: 0x964E_5B22,
    data2: 0x6459,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

pub const FILE_INFO_GUID: EfiGuid = EfiGuid {
    data1: 0x0957_6E92,
    data2: 0x6D3F,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

#[allow(dead_code)] // documents the spec bit; Open() treats absence of WRITE as read-only, doesn't check this one explicitly
pub const EFI_FILE_MODE_READ: u64 = 0x0000_0000_0000_0001;
pub const EFI_FILE_MODE_WRITE: u64 = 0x0000_0000_0000_0002;
pub const EFI_FILE_MODE_CREATE: u64 = 0x8000_0000_0000_0000;
pub const EFI_FILE_DIRECTORY: u64 = 0x0000_0000_0000_0010;
const EFI_WARN_DELETE_FAILURE: EfiStatus = 9; // no ERROR_BIT -- a warning, not an error

type OpenFn = extern "C" fn(*mut FileProtocol, *mut *mut FileProtocol, *const u16, u64, u64) -> EfiStatus;
type CloseFn = extern "C" fn(*mut FileProtocol) -> EfiStatus;
type DeleteFn = extern "C" fn(*mut FileProtocol) -> EfiStatus;
type ReadFn = extern "C" fn(*mut FileProtocol, *mut usize, *mut c_void) -> EfiStatus;
type WriteFn = extern "C" fn(*mut FileProtocol, *mut usize, *const c_void) -> EfiStatus;
type GetPositionFn = extern "C" fn(*mut FileProtocol, *mut u64) -> EfiStatus;
type SetPositionFn = extern "C" fn(*mut FileProtocol, u64) -> EfiStatus;
type GetInfoFn = extern "C" fn(*mut FileProtocol, *const EfiGuid, *mut usize, *mut c_void) -> EfiStatus;
type SetInfoFn = extern "C" fn(*mut FileProtocol, *const EfiGuid, usize, *const c_void) -> EfiStatus;
type FlushFn = extern "C" fn(*mut FileProtocol) -> EfiStatus;

#[repr(C)]
pub struct FileProtocol {
    pub revision: u64,
    pub open: OpenFn,
    pub close: CloseFn,
    pub delete: DeleteFn,
    pub read: ReadFn,
    pub write: WriteFn,
    pub get_position: GetPositionFn,
    pub set_position: SetPositionFn,
    pub get_info: GetInfoFn,
    pub set_info: SetInfoFn,
    pub flush: FlushFn,
}
unsafe impl Sync for FileProtocol {}

type OpenVolumeFn = extern "C" fn(*mut SimpleFileSystemProtocol, *mut *mut FileProtocol) -> EfiStatus;

#[repr(C)]
pub struct SimpleFileSystemProtocol {
    pub revision: u64,
    pub open_volume: OpenVolumeFn,
}
unsafe impl Sync for SimpleFileSystemProtocol {}

const EFI_FILE_PROTOCOL_REVISION: u64 = 0x0001_0000;
const MAX_OPEN_FILES: usize = 4;
const MAX_FILE_BYTES: usize = 65536;
const MAX_DIR_ENTRIES: usize = 16;

/// Per-open-file state. `FileProtocol` itself carries no data (it's
/// just function pointers per the ABI), so each open handle gets one
/// of these, addressed the same way protocol_db addresses handles --
/// a small table index, not a real pointer.
struct OpenFile {
    in_use: bool,
    /// `Some(cluster)` if this handle is a directory listing that
    /// cluster (root or any subdirectory reached via `Open`); `None`
    /// for a plain file.
    dir_cluster: Option<u32>,
    name_8_3: [u8; 11],
    data: [u8; MAX_FILE_BYTES],
    len: usize,
    pos: usize,
    dirty: bool,
    for_write: bool,
    // Directory enumeration state (directory handles only, i.e.
    // dir_cluster.is_some()): the listing is snapshotted at Open time,
    // and Read() walks it one entry per call, matching
    // EFI_FILE_PROTOCOL.Read's directory semantics (each call returns
    // one EFI_FILE_INFO, empty read at end of directory).
    dir_names: [[u8; 11]; MAX_DIR_ENTRIES],
    dir_sizes: [u32; MAX_DIR_ENTRIES],
    dir_count: usize,
    dir_index: usize,
}

const EMPTY_FILE: OpenFile = OpenFile {
    in_use: false,
    dir_cluster: None,
    name_8_3: [b' '; 11],
    data: [0; MAX_FILE_BYTES],
    len: 0,
    pos: 0,
    dirty: false,
    for_write: false,
    dir_names: [[b' '; 11]; MAX_DIR_ENTRIES],
    dir_sizes: [0; MAX_DIR_ENTRIES],
    dir_count: 0,
    dir_index: 0,
};

static mut FILES: [OpenFile; MAX_OPEN_FILES] = [EMPTY_FILE; MAX_OPEN_FILES];
// One FileProtocol vtable instance per slot (all identical function
// pointers; only which slot a given *mut FileProtocol resolves to
// differs), so a caller's `This` pointer can be mapped back to its
// OpenFile by pointer identity.
static mut PROTOS: [FileProtocol; MAX_OPEN_FILES] = [FILE_PROTO_TEMPLATE; MAX_OPEN_FILES];

const FILE_PROTO_TEMPLATE: FileProtocol = FileProtocol {
    revision: EFI_FILE_PROTOCOL_REVISION,
    open: file_open,
    close: file_close,
    delete: file_delete,
    read: file_read,
    write: file_write,
    get_position: file_get_position,
    set_position: file_set_position,
    get_info: file_get_info,
    set_info: file_set_info,
    flush: file_flush,
};

fn this_to_index(this: *mut FileProtocol) -> Option<usize> {
    let base = core::ptr::addr_of!(PROTOS) as *mut FileProtocol;
    let offset = (this as usize).checked_sub(base as usize)?;
    let i = offset / core::mem::size_of::<FileProtocol>();
    if i < MAX_OPEN_FILES && unsafe { (*core::ptr::addr_of!(FILES))[i].in_use } {
        Some(i)
    } else {
        None
    }
}

fn alloc_slot() -> Option<usize> {
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
    for (i, f) in files.iter_mut().enumerate() {
        if !f.in_use {
            *f = EMPTY_FILE;
            f.in_use = true;
            unsafe { (*core::ptr::addr_of_mut!(PROTOS))[i] = FILE_PROTO_TEMPLATE };
            return Some(i);
        }
    }
    None
}

fn slot_handle(i: usize) -> *mut FileProtocol {
    unsafe { core::ptr::addr_of_mut!(PROTOS[i]) }
}

/// Converts a CHAR16 path into raw ASCII bytes (non-ASCII chars
/// become `?`), preserving `\` separators -- unlike an 8.3-name
/// converter, this doesn't reject multi-component paths, since
/// `fs.resolve_from` is what walks those.
fn char16_path_to_ascii(path: *const u16, out: &mut [u8; 128]) -> Option<usize> {
    let mut n = 0;
    let mut i = 0isize;
    loop {
        let c = unsafe { *path.offset(i) };
        if c == 0 {
            break;
        }
        i += 1;
        if n >= out.len() {
            return None;
        }
        out[n] = if c < 128 { c as u8 } else { b'?' };
        n += 1;
    }
    Some(n)
}

fn is_root_path(path: *const u16) -> bool {
    let c0 = unsafe { *path };
    if c0 == 0 {
        return true;
    }
    if c0 == b'\\' as u16 {
        let c1 = unsafe { *path.offset(1) };
        return c1 == 0;
    }
    c0 == b'.' as u16 && unsafe { *path.offset(1) } == 0
}

fn open_dir(card: &Card, fs: &Fat32, dir_cluster: u32) -> Option<usize> {
    let i = alloc_slot()?;
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
    files[i].dir_cluster = Some(dir_cluster);
    let mut names = [[0u8; 11]; MAX_DIR_ENTRIES];
    let mut sizes = [0u32; MAX_DIR_ENTRIES];
    let count = fs.list_dir(card, dir_cluster, &mut names, &mut sizes).unwrap_or(0);
    files[i].dir_names = names;
    files[i].dir_sizes = sizes;
    files[i].dir_count = count;
    files[i].dir_index = 0;
    Some(i)
}

extern "C" fn open_volume(_this: *mut SimpleFileSystemProtocol, root: *mut *mut FileProtocol) -> EfiStatus {
    if root.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some((card, fs)) = crate::persist::get_context() else {
        return EFI_NOT_FOUND;
    };
    let Some(i) = open_dir(&card, &fs, fs.root_cluster()) else {
        return EFI_OUT_OF_RESOURCES;
    };
    unsafe { *root = slot_handle(i) };
    EFI_SUCCESS
}

/// Static, not stack-allocated: MAX_FILE_BYTES is 64KiB, and this
/// runs deep in a call chain (loaded EFI app -> this function
/// pointer) against Ferro's own 64KiB boot stack -- a stack-local
/// buffer this size here silently overflows it (a real bug this
/// implementation had and fixed, see the README).
static mut OPEN_SCRATCH: [u8; MAX_FILE_BYTES] = [0; MAX_FILE_BYTES];

extern "C" fn file_open(
    this: *mut FileProtocol,
    new_handle: *mut *mut FileProtocol,
    file_name: *const u16,
    open_mode: u64,
    _attributes: u64,
) -> EfiStatus {
    if new_handle.is_null() || file_name.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(parent) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    let Some(parent_dir_cluster) = (unsafe { (*core::ptr::addr_of!(FILES))[parent].dir_cluster }) else {
        return EFI_UNSUPPORTED; // opening "relative to" a plain file makes no sense
    };
    let Some((card, fs)) = crate::persist::get_context() else {
        return EFI_NOT_FOUND;
    };

    if is_root_path(file_name) {
        let Some(i) = open_dir(&card, &fs, parent_dir_cluster) else {
            return EFI_OUT_OF_RESOURCES;
        };
        unsafe { *new_handle = slot_handle(i) };
        return EFI_SUCCESS;
    }

    let mut ascii = [0u8; 128];
    let Some(n) = char16_path_to_ascii(file_name, &mut ascii) else {
        return EFI_NOT_FOUND;
    };
    let raw = &ascii[..n];
    // A leading `\` means "relative to the volume root" regardless of
    // `this`, per spec; otherwise it's relative to `this`.
    let (start_cluster, path) = if raw.first() == Some(&b'\\') {
        (fs.root_cluster(), &raw[1..])
    } else {
        (parent_dir_cluster, raw)
    };
    let single_component = !path.contains(&b'\\');

    // The original root-file create/overwrite path (unchanged
    // behavior, including CREATE support) -- kept exactly for
    // single-component names directly under the root, which is all
    // FERRO.VAR persistence and every existing caller ever needed.
    if single_component && start_cluster == fs.root_cluster() {
        let Some(name) = to_8_3_or_none(path) else {
            return EFI_NOT_FOUND;
        };
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(OPEN_SCRATCH) };
        let len = match fs.read_file(&card, &name, buf) {
            Ok(n) => n,
            Err(_) if open_mode & EFI_FILE_MODE_CREATE != 0 => {
                if fs.write_file(&card, &name, &[]).is_err() {
                    return EFI_UNSUPPORTED;
                }
                0
            }
            Err(_) => return EFI_NOT_FOUND,
        };

        let Some(i) = alloc_slot() else {
            return EFI_OUT_OF_RESOURCES;
        };
        let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
        files[i].name_8_3 = name;
        files[i].data[..len].copy_from_slice(&buf[..len]);
        files[i].len = len;
        files[i].pos = 0;
        files[i].for_write = open_mode & EFI_FILE_MODE_WRITE != 0;
        files[i].dirty = false;

        unsafe { *new_handle = slot_handle(i) };
        return EFI_SUCCESS;
    }

    // Any other path (a subdirectory, or a multi-component path even
    // under the root) -- real traversal via resolve_from, read-only:
    // write_file has no subdirectory support, so honestly reject
    // CREATE/WRITE here instead of silently writing to the wrong
    // place.
    if open_mode & (EFI_FILE_MODE_WRITE | EFI_FILE_MODE_CREATE) != 0 {
        return super::types::EFI_WRITE_PROTECTED;
    }
    let Ok((first_cluster, size, is_dir)) = fs.resolve_from(&card, start_cluster, path) else {
        return EFI_NOT_FOUND;
    };
    if is_dir {
        let Some(i) = open_dir(&card, &fs, if first_cluster == 0 { fs.root_cluster() } else { first_cluster }) else {
            return EFI_OUT_OF_RESOURCES;
        };
        unsafe { *new_handle = slot_handle(i) };
        return EFI_SUCCESS;
    }

    let buf = unsafe { &mut *core::ptr::addr_of_mut!(OPEN_SCRATCH) };
    let len = match fs.read_from(&card, first_cluster, size, buf) {
        Ok(n) => n,
        Err(_) => return EFI_NOT_FOUND,
    };
    let Some(i) = alloc_slot() else {
        return EFI_OUT_OF_RESOURCES;
    };
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
    // Only used to label GetInfo's FileName / a Close-time write-back
    // that never triggers here (for_write stays false) -- doesn't
    // need to be a real 8.3 name for a nested file, just something to
    // show.
    files[i].name_8_3 = path.rsplit(|&b| b == b'\\').next().and_then(to_8_3_or_none).unwrap_or([b' '; 11]);
    files[i].data[..len].copy_from_slice(&buf[..len]);
    files[i].len = len;
    files[i].pos = 0;
    files[i].for_write = false;
    files[i].dirty = false;

    unsafe { *new_handle = slot_handle(i) };
    EFI_SUCCESS
}

fn to_8_3_or_none(component: &[u8]) -> Option<[u8; 11]> {
    let mut name = [b' '; 11];
    let (base, ext) = match component.iter().position(|&b| b == b'.') {
        Some(dot) => (&component[..dot], &component[dot + 1..]),
        None => (component, &[][..]),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }
    for (i, &b) in base.iter().enumerate() {
        name[i] = b.to_ascii_uppercase();
    }
    for (i, &b) in ext.iter().enumerate() {
        name[8 + i] = b.to_ascii_uppercase();
    }
    Some(name)
}

fn flush_if_dirty(i: usize) {
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
    if files[i].for_write && files[i].dirty {
        if let Some((card, fs)) = crate::persist::get_context() {
            let name = files[i].name_8_3;
            let len = files[i].len;
            let data_ptr = files[i].data.as_ptr();
            let data = unsafe { core::slice::from_raw_parts(data_ptr, len) };
            let _ = fs.write_file(&card, &name, data);
        }
        files[i].dirty = false;
    }
}

extern "C" fn file_close(this: *mut FileProtocol) -> EfiStatus {
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    flush_if_dirty(i);
    unsafe { (*core::ptr::addr_of_mut!(FILES))[i].in_use = false };
    EFI_SUCCESS
}

extern "C" fn file_delete(this: *mut FileProtocol) -> EfiStatus {
    // fat32.rs has no delete support -- close the handle (spec-legal
    // fallback) and say so honestly via the warning code, not a lie.
    let _ = file_close(this);
    EFI_WARN_DELETE_FAILURE
}

extern "C" fn file_read(this: *mut FileProtocol, buffer_size: *mut usize, buffer: *mut c_void) -> EfiStatus {
    if buffer_size.is_null() || buffer.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };

    if files[i].dir_cluster.is_some() {
        if files[i].dir_index >= files[i].dir_count {
            unsafe { *buffer_size = 0 };
            return EFI_SUCCESS; // end of directory
        }
        let idx = files[i].dir_index;
        let name = files[i].dir_names[idx];
        let size = files[i].dir_sizes[idx];
        let needed = write_file_info(buffer, unsafe { *buffer_size }, &name, size as u64, true);
        match needed {
            Ok(written) => {
                unsafe { *buffer_size = written };
                files[i].dir_index += 1;
                EFI_SUCCESS
            }
            Err(needed) => {
                unsafe { *buffer_size = needed };
                EFI_BUFFER_TOO_SMALL
            }
        }
    } else {
        let remaining = files[i].len.saturating_sub(files[i].pos);
        let n = remaining.min(unsafe { *buffer_size });
        if n > 0 {
            let out = unsafe { core::slice::from_raw_parts_mut(buffer as *mut u8, n) };
            out.copy_from_slice(&files[i].data[files[i].pos..files[i].pos + n]);
            files[i].pos += n;
        }
        unsafe { *buffer_size = n };
        EFI_SUCCESS
    }
}

extern "C" fn file_write(this: *mut FileProtocol, buffer_size: *mut usize, buffer: *const c_void) -> EfiStatus {
    if buffer_size.is_null() || buffer.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
    if files[i].dir_cluster.is_some() {
        return EFI_UNSUPPORTED;
    }
    if !files[i].for_write {
        return EFI_WRITE_PROTECTED;
    }
    let n = unsafe { *buffer_size };
    if files[i].pos + n > MAX_FILE_BYTES {
        return EFI_OUT_OF_RESOURCES;
    }
    let src = unsafe { core::slice::from_raw_parts(buffer as *const u8, n) };
    files[i].data[files[i].pos..files[i].pos + n].copy_from_slice(src);
    files[i].pos += n;
    files[i].len = files[i].len.max(files[i].pos);
    files[i].dirty = true;
    EFI_SUCCESS
}

extern "C" fn file_get_position(this: *mut FileProtocol, position: *mut u64) -> EfiStatus {
    if position.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    unsafe { *position = (*core::ptr::addr_of!(FILES))[i].pos as u64 };
    EFI_SUCCESS
}

extern "C" fn file_set_position(this: *mut FileProtocol, position: u64) -> EfiStatus {
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    let files = unsafe { &mut *core::ptr::addr_of_mut!(FILES) };
    if files[i].dir_cluster.is_some() {
        if position == 0 {
            files[i].dir_index = 0;
            return EFI_SUCCESS;
        }
        return EFI_UNSUPPORTED;
    }
    if position == u64::MAX {
        files[i].pos = files[i].len; // spec: seek to end-of-file
    } else if (position as usize) > MAX_FILE_BYTES {
        return EFI_INVALID_PARAMETER;
    } else {
        files[i].pos = position as usize;
    }
    EFI_SUCCESS
}

/// Formats an EFI_FILE_INFO for `name`/`size` into `buffer` if it
/// fits within `capacity`; returns the byte count written, or the
/// byte count actually needed (Err) if it doesn't.
fn write_file_info(buffer: *mut c_void, capacity: usize, name_8_3: &[u8; 11], size: u64, is_dir: bool) -> Result<usize, usize> {
    let mut name_units = [0u16; 13];
    let mut n = 0usize;
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

    // EFI_FILE_INFO: Size,FileSize,PhysicalSize (u64 x3) + 3x EFI_TIME
    // (16 bytes each, all-zero: no RTC on this platform) + Attribute
    // (u64) + FileName[] (CHAR16, n units).
    let header_len = 8 * 3 + 16 * 3 + 8;
    let total = header_len + n * 2;
    if capacity < total {
        return Err(total);
    }

    let out = unsafe { core::slice::from_raw_parts_mut(buffer as *mut u8, total) };
    out[0..8].copy_from_slice(&(total as u64).to_le_bytes());
    out[8..16].copy_from_slice(&size.to_le_bytes());
    out[16..24].copy_from_slice(&size.to_le_bytes());
    out[24..24 + 48].fill(0); // three all-zero EFI_TIME structs
    let attr: u64 = if is_dir { EFI_FILE_DIRECTORY } else { 0 };
    out[72..80].copy_from_slice(&attr.to_le_bytes());
    for (i, &unit) in name_units[..n].iter().enumerate() {
        out[80 + i * 2..80 + i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
    Ok(total)
}

extern "C" fn file_get_info(
    this: *mut FileProtocol,
    information_type: *const EfiGuid,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> EfiStatus {
    if information_type.is_null() || buffer_size.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    if unsafe { *information_type } != FILE_INFO_GUID {
        return EFI_UNSUPPORTED;
    }
    let files = unsafe { &*core::ptr::addr_of!(FILES) };
    let (name, size, is_dir) = (files[i].name_8_3, files[i].len as u64, files[i].dir_cluster.is_some());
    match write_file_info(buffer, unsafe { *buffer_size }, &name, size, is_dir) {
        Ok(written) => {
            unsafe { *buffer_size = written };
            EFI_SUCCESS
        }
        Err(needed) => {
            unsafe { *buffer_size = needed };
            EFI_BUFFER_TOO_SMALL
        }
    }
}

extern "C" fn file_set_info(
    _this: *mut FileProtocol,
    _information_type: *const EfiGuid,
    _buffer_size: usize,
    _buffer: *const c_void,
) -> EfiStatus {
    EFI_UNSUPPORTED // metadata (timestamps, attributes) is read-only here
}

extern "C" fn file_flush(this: *mut FileProtocol) -> EfiStatus {
    let Some(i) = this_to_index(this) else {
        return EFI_INVALID_PARAMETER;
    };
    flush_if_dirty(i);
    EFI_SUCCESS
}

static mut SFS_PROTO: SimpleFileSystemProtocol = SimpleFileSystemProtocol {
    revision: 0x0001_0000,
    open_volume,
};

/// Installs EFI_SIMPLE_FILE_SYSTEM_PROTOCOL on `index`.
pub fn install(index: usize) -> bool {
    super::protocol_db::install(
        index,
        SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
        core::ptr::addr_of_mut!(SFS_PROTO) as *mut c_void,
    )
}
