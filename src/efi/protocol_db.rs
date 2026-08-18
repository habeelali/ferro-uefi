//! Fixed-capacity handle/protocol database. No heap exists yet, so
//! this is plain static arrays rather than the Vec-of-Vecs a real
//! implementation would use -- MAX_HANDLES/MAX_PROTOCOLS_PER_HANDLE
//! are the honest capacity limit that comes with that.

use super::types::EfiGuid;
use core::ffi::c_void;

pub const MAX_HANDLES: usize = 32;
const MAX_PROTOCOLS_PER_HANDLE: usize = 8;

#[derive(Clone, Copy)]
struct ProtocolEntry {
    guid: EfiGuid,
    interface: *mut c_void,
}

#[derive(Clone, Copy)]
struct HandleEntry {
    in_use: bool,
    protocols: [Option<ProtocolEntry>; MAX_PROTOCOLS_PER_HANDLE],
}

const EMPTY_HANDLE: HandleEntry = HandleEntry {
    in_use: false,
    protocols: [None; MAX_PROTOCOLS_PER_HANDLE],
};

static mut HANDLES: [HandleEntry; MAX_HANDLES] = [EMPTY_HANDLE; MAX_HANDLES];

/// Handles are just table indices disguised as pointers (offset by 1
/// so index 0 doesn't collide with NULL) -- valid per spec, since
/// EFI_HANDLE is an opaque token callers are never meant to deref.
fn handle_to_index(handle: *mut c_void) -> Option<usize> {
    let raw = handle as usize;
    if raw == 0 {
        None
    } else {
        Some(raw - 1)
    }
}

fn index_to_handle(index: usize) -> *mut c_void {
    (index + 1) as *mut c_void
}

/// Table index backing `handle`, if it's a live (in-use) handle.
/// Public so callers that need to attach out-of-band state to a
/// handle (LoadImage's entry-point table, for instance) can key it by
/// the same index this database uses internally.
pub fn index_of(handle: *mut c_void) -> Option<usize> {
    let i = handle_to_index(handle)?;
    let handles = core::ptr::addr_of!(HANDLES);
    unsafe { (*handles)[i].in_use.then_some(i) }
}

/// Finds an existing handle's slot, or allocates a fresh one. Returns
/// None if the table is full.
pub fn find_or_create_handle(existing: *mut c_void) -> Option<usize> {
    let handles = core::ptr::addr_of_mut!(HANDLES);
    if let Some(i) = handle_to_index(existing) {
        return unsafe { (*handles)[i].in_use.then_some(i) };
    }
    unsafe {
        for i in 0..MAX_HANDLES {
            if !(*handles)[i].in_use {
                (*handles)[i] = EMPTY_HANDLE;
                (*handles)[i].in_use = true;
                return Some(i);
            }
        }
    }
    None
}

pub fn handle_for_index(index: usize) -> *mut c_void {
    index_to_handle(index)
}

/// Installs `interface` under `guid` on the handle at `index`. Returns
/// false if that handle's protocol slots are all full.
pub fn install(index: usize, guid: EfiGuid, interface: *mut c_void) -> bool {
    let handles = core::ptr::addr_of_mut!(HANDLES);
    unsafe {
        for slot in (*handles)[index].protocols.iter_mut() {
            if slot.is_none() {
                *slot = Some(ProtocolEntry { guid, interface });
                return true;
            }
        }
    }
    false
}

/// First handle (in table order) exposing `guid`, and its interface
/// pointer.
pub fn locate(guid: &EfiGuid) -> Option<*mut c_void> {
    let handles = core::ptr::addr_of!(HANDLES);
    unsafe {
        for h in (*handles).iter() {
            if !h.in_use {
                continue;
            }
            for slot in h.protocols.iter().flatten() {
                if slot.guid == *guid {
                    return Some(slot.interface);
                }
            }
        }
    }
    None
}

/// `guid`'s interface pointer on a specific handle.
pub fn handle_protocol(handle: *mut c_void, guid: &EfiGuid) -> Option<*mut c_void> {
    let index = handle_to_index(handle)?;
    let handles = core::ptr::addr_of!(HANDLES);
    unsafe {
        if !(*handles)[index].in_use {
            return None;
        }
        for slot in (*handles)[index].protocols.iter().flatten() {
            if slot.guid == *guid {
                return Some(slot.interface);
            }
        }
    }
    None
}

/// Removes `guid` from `index`'s handle, for ReinstallProtocolInterface
/// (uninstall-then-install) and UninstallProtocolInterface. Returns
/// false if that handle didn't have `guid` installed.
pub fn uninstall(index: usize, guid: &EfiGuid) -> bool {
    let handles = core::ptr::addr_of_mut!(HANDLES);
    unsafe {
        for slot in (*handles)[index].protocols.iter_mut() {
            if matches!(slot, Some(p) if p.guid == *guid) {
                *slot = None;
                return true;
            }
        }
    }
    false
}

/// Every live handle (in table order) that has `guid` installed,
/// written into `out` -- returns the count found, capped at `out`'s
/// length (real callers size `out` from a first LocateHandle call
/// that reports the needed count via EFI_BUFFER_TOO_SMALL).
pub fn handles_with(guid: &EfiGuid, out: &mut [*mut c_void]) -> usize {
    let handles = core::ptr::addr_of!(HANDLES);
    let mut count = 0;
    unsafe {
        for (i, h) in (*handles).iter().enumerate() {
            if !h.in_use {
                continue;
            }
            if h.protocols.iter().flatten().any(|p| p.guid == *guid) {
                if count < out.len() {
                    out[count] = index_to_handle(i);
                }
                count += 1;
            }
        }
    }
    count
}

/// Every protocol GUID installed on `handle`, written into `out` --
/// returns the count found (capped at `out`'s length), or None if
/// `handle` isn't a live handle at all.
pub fn protocols_on_handle(handle: *mut c_void, out: &mut [EfiGuid]) -> Option<usize> {
    let index = handle_to_index(handle)?;
    let handles = core::ptr::addr_of!(HANDLES);
    unsafe {
        if !(*handles)[index].in_use {
            return None;
        }
        let mut count = 0;
        for p in (*handles)[index].protocols.iter().flatten() {
            if count < out.len() {
                out[count] = p.guid;
            }
            count += 1;
        }
        Some(count)
    }
}
