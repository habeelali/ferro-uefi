//! UEFI variable storage: a fixed-capacity, in-RAM store backing
//! GetVariable/SetVariable/GetNextVariableName. No persistence across
//! reboots yet -- there's no flash/NVRAM driver, so this is exactly
//! as durable as the rest of RAM and no more. That's the honest first
//! slice; persistence is a real follow-up once something needs it.

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
