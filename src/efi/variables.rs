//! UEFI variable storage: a fixed-capacity, in-RAM store backing
//! GetVariable/SetVariable/GetNextVariableName. Cross-reboot
//! persistence isn't automatic -- there's no flash/NVRAM driver -- but
//! `serialize`/`deserialize` here plus `persist.rs`'s SD-card backing
//! give it real, if manually-triggered, durability. See persist.rs
//! for where the bytes actually live.

use super::types::EfiGuid;

pub const MAX_VARIABLES: usize = 32;
pub const MAX_NAME_UNITS: usize = 32; // CHAR16 units, not counting the null terminator
pub const MAX_DATA_LEN: usize = 512;

#[derive(Clone, Copy)]
struct Variable {
    in_use: bool,
    name: [u16; MAX_NAME_UNITS],
    name_len: usize,
    guid: EfiGuid,
    attributes: u32,
    data: [u8; MAX_DATA_LEN],
    data_len: usize,
}

const EMPTY_VAR: Variable = Variable {
    in_use: false,
    name: [0; MAX_NAME_UNITS],
    name_len: 0,
    guid: EfiGuid {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    },
    attributes: 0,
    data: [0; MAX_DATA_LEN],
    data_len: 0,
};

static mut VARS: [Variable; MAX_VARIABLES] = [EMPTY_VAR; MAX_VARIABLES];

/// Reads a NUL-terminated CHAR16 string from `ptr`, up to `max` units
/// (not counting the terminator). Returns None if there's no
/// terminator within that bound (name too long for our store).
unsafe fn read_name(ptr: *const u16, max: usize) -> Option<([u16; MAX_NAME_UNITS], usize)> {
    let mut buf = [0u16; MAX_NAME_UNITS];
    for i in 0..=max {
        let c = *ptr.add(i);
        if c == 0 {
            return Some((buf, i));
        }
        if i == max {
            break;
        }
        buf[i] = c;
    }
    None
}

fn names_eq(a: &[u16], a_len: usize, b: &[u16], b_len: usize) -> bool {
    a_len == b_len && a[..a_len] == b[..b_len]
}

pub enum VarError {
    NotFound,
    InvalidParameter,
    BufferTooSmall(usize),
    OutOfResources,
}

/// Copies a found variable's data/attributes out. `data_buf` may be
/// smaller than the stored value -- callers get BufferTooSmall(needed)
/// and can retry with a bigger buffer, per spec.
pub fn get(
    name_ptr: *const u16,
    guid: &EfiGuid,
    data_buf: &mut [u8],
) -> Result<(u32, usize), VarError> {
    let (name, name_len) = unsafe { read_name(name_ptr, MAX_NAME_UNITS) }.ok_or(VarError::InvalidParameter)?;

    let vars = core::ptr::addr_of!(VARS);
    unsafe {
        for v in (*vars).iter() {
            if v.in_use && v.guid == *guid && names_eq(&v.name, v.name_len, &name, name_len) {
                if data_buf.len() < v.data_len {
                    return Err(VarError::BufferTooSmall(v.data_len));
                }
                data_buf[..v.data_len].copy_from_slice(&v.data[..v.data_len]);
                return Ok((v.attributes, v.data_len));
            }
        }
    }
    Err(VarError::NotFound)
}

/// Sets (or, with `data.is_empty()`, deletes) a variable.
pub fn set(name_ptr: *const u16, guid: &EfiGuid, attributes: u32, data: &[u8]) -> Result<(), VarError> {
    let (name, name_len) = unsafe { read_name(name_ptr, MAX_NAME_UNITS) }.ok_or(VarError::InvalidParameter)?;
    if data.len() > MAX_DATA_LEN {
        return Err(VarError::OutOfResources);
    }

    let vars = core::ptr::addr_of_mut!(VARS);
    unsafe {
        let existing = (*vars)
            .iter_mut()
            .find(|v| v.in_use && v.guid == *guid && names_eq(&v.name, v.name_len, &name, name_len));

        if data.is_empty() {
            if let Some(v) = existing {
                *v = EMPTY_VAR;
                return Ok(());
            }
            return Err(VarError::NotFound);
        }

        if let Some(v) = existing {
            v.attributes = attributes;
            v.data[..data.len()].copy_from_slice(data);
            v.data_len = data.len();
            return Ok(());
        }

        let Some(slot) = (*vars).iter_mut().find(|v| !v.in_use) else {
            return Err(VarError::OutOfResources);
        };
        slot.in_use = true;
        slot.name = name;
        slot.name_len = name_len;
        slot.guid = *guid;
        slot.attributes = attributes;
        slot.data[..data.len()].copy_from_slice(data);
        slot.data_len = data.len();
    }
    Ok(())
}

/// Enumerates variables: given the previous (name, guid) pair (an
/// empty name starts the enumeration), finds and returns the next
/// one in store order. Matches EFI_GET_NEXT_VARIABLE_NAME's contract.
pub fn get_next(
    prev_name_ptr: *const u16,
    prev_guid: &EfiGuid,
    out_name: &mut [u16],
) -> Result<(usize, EfiGuid), VarError> {
    let (prev_name, prev_len) =
        unsafe { read_name(prev_name_ptr, MAX_NAME_UNITS) }.ok_or(VarError::InvalidParameter)?;

    let vars = core::ptr::addr_of!(VARS);
    unsafe {
        let start_index = if prev_len == 0 {
            0
        } else {
            let pos = (*vars)
                .iter()
                .position(|v| v.in_use && v.guid == *prev_guid && names_eq(&v.name, v.name_len, &prev_name, prev_len))
                .ok_or(VarError::InvalidParameter)?;
            pos + 1
        };

        let all = core::slice::from_raw_parts(vars as *const Variable, MAX_VARIABLES);
        for v in &all[start_index..] {
            if v.in_use {
                if out_name.len() < v.name_len + 1 {
                    return Err(VarError::BufferTooSmall(v.name_len + 1));
                }
                out_name[..v.name_len].copy_from_slice(&v.name[..v.name_len]);
                out_name[v.name_len] = 0;
                return Ok((v.name_len + 1, v.guid));
            }
        }
    }
    Err(VarError::NotFound)
}

/// (max storage bytes, remaining storage bytes, max single-variable bytes)
pub fn storage_info() -> (u64, u64, u64) {
    let vars = core::ptr::addr_of!(VARS);
    let used: usize = unsafe {
        (*vars)
            .iter()
            .filter(|v| v.in_use)
            .map(|v| v.data_len)
            .sum()
    };
    let total = (MAX_VARIABLES * MAX_DATA_LEN) as u64;
    (total, total - used as u64, MAX_DATA_LEN as u64)
}

const MAGIC: &[u8; 4] = b"FVR1";

/// Packs every in-use variable into `buf` (magic, count, then each
/// variable's name/guid/attributes/data). Returns the byte count
/// written, or None if `buf` is too small for the current contents.
pub fn serialize(buf: &mut [u8]) -> Option<usize> {
    if buf.len() < 8 {
        return None;
    }
    buf[0..4].copy_from_slice(MAGIC);
    let mut pos = 8usize; // count patched in at the end

    let vars = core::ptr::addr_of!(VARS);
    let mut count = 0u32;
    unsafe {
        for v in (*vars).iter() {
            if !v.in_use {
                continue;
            }
            let entry_len = 2 + v.name_len * 2 + 16 + 4 + 4 + v.data_len;
            if pos + entry_len > buf.len() {
                return None;
            }
            buf[pos..pos + 2].copy_from_slice(&(v.name_len as u16).to_le_bytes());
            pos += 2;
            for &unit in &v.name[..v.name_len] {
                buf[pos..pos + 2].copy_from_slice(&unit.to_le_bytes());
                pos += 2;
            }
            buf[pos..pos + 4].copy_from_slice(&v.guid.data1.to_le_bytes());
            pos += 4;
            buf[pos..pos + 2].copy_from_slice(&v.guid.data2.to_le_bytes());
            pos += 2;
            buf[pos..pos + 2].copy_from_slice(&v.guid.data3.to_le_bytes());
            pos += 2;
            buf[pos..pos + 8].copy_from_slice(&v.guid.data4);
            pos += 8;
            buf[pos..pos + 4].copy_from_slice(&v.attributes.to_le_bytes());
            pos += 4;
            buf[pos..pos + 4].copy_from_slice(&(v.data_len as u32).to_le_bytes());
            pos += 4;
            buf[pos..pos + v.data_len].copy_from_slice(&v.data[..v.data_len]);
            pos += v.data_len;
            count += 1;
        }
    }

    buf[4..8].copy_from_slice(&count.to_le_bytes());
    Some(pos)
}

/// Loads variables packed by `serialize` and merges them into the
/// live store (same dedup/overwrite rules as `set`). Returns the
/// number of variables loaded, or None if `buf` doesn't start with
/// our magic (not our data, or genuinely empty/unwritten storage).
pub fn deserialize(buf: &[u8]) -> Option<usize> {
    if buf.len() < 8 || &buf[0..4] != MAGIC {
        return None;
    }
    let count = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    let mut pos = 8usize;
    let mut loaded = 0usize;

    for _ in 0..count {
        if pos + 2 > buf.len() {
            break;
        }
        let name_len = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if name_len > MAX_NAME_UNITS || pos + name_len * 2 > buf.len() {
            break;
        }
        let mut name = [0u16; MAX_NAME_UNITS];
        for i in 0..name_len {
            name[i] = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap());
            pos += 2;
        }
        if pos + 16 + 4 + 4 > buf.len() {
            break;
        }
        let guid = EfiGuid {
            data1: u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()),
            data2: u16::from_le_bytes(buf[pos + 4..pos + 6].try_into().unwrap()),
            data3: u16::from_le_bytes(buf[pos + 6..pos + 8].try_into().unwrap()),
            data4: buf[pos + 8..pos + 16].try_into().unwrap(),
        };
        pos += 16;
        let attributes = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let data_len = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if data_len > MAX_DATA_LEN || pos + data_len > buf.len() {
            break;
        }
        let data = &buf[pos..pos + data_len];
        pos += data_len;

        // Build a NUL-terminated name buffer to feed set()'s pointer-based API.
        let mut name_nt = [0u16; MAX_NAME_UNITS + 1];
        name_nt[..name_len].copy_from_slice(&name[..name_len]);
        if set(name_nt.as_ptr(), &guid, attributes, data).is_ok() {
            loaded += 1;
        }
    }
    Some(loaded)
}
