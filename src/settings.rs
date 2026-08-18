//! Firmware setup settings -- backed by real UEFI variables (via
//! efi::variables, the same store GetVariable/SetVariable read and
//! write), not a UI-only toggle. Changing a setting in the menu calls
//! the same variable store an EFI application could read with
//! GetVariable, and "SAVE VARIABLES TO SD" persists it for real,
//! since persist.rs serializes every in-use variable regardless of
//! which code put it there.

use crate::efi::types::EfiGuid;
use crate::efi::variables;

/// Vendor GUID for Ferro's own setup variables, distinct from any
/// spec-defined or application namespace.
const FERRO_SETTINGS_GUID: EfiGuid = EfiGuid {
    data1: 0x4645_5252,
    data2: 0x4F00,
    data3: 0x0001,
    data4: [b'f', b'e', b'r', b'r', b'o', b'-', b's', b'u'],
};

const ATTR: u32 = 0x7; // NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Amber,
    Cyan,
    Green,
}

impl Theme {
    pub fn accent(self) -> u32 {
        match self {
            Theme::Amber => 0x00FF_A030,
            Theme::Cyan => 0x0030_C8FF,
            Theme::Green => 0x0050_E060,
        }
    }

    pub fn select_bg(self) -> u32 {
        match self {
            Theme::Amber => 0x002A_3A4A,
            Theme::Cyan => 0x0016_3A44,
            Theme::Green => 0x0016_3A22,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Amber => "AMBER",
            Theme::Cyan => "CYAN",
            Theme::Green => "GREEN",
        }
    }

    pub fn next(self) -> Theme {
        match self {
            Theme::Amber => Theme::Cyan,
            Theme::Cyan => Theme::Green,
            Theme::Green => Theme::Amber,
        }
    }

    fn from_byte(b: u8) -> Theme {
        match b {
            1 => Theme::Cyan,
            2 => Theme::Green,
            _ => Theme::Amber,
        }
    }

    fn to_byte(self) -> u8 {
        match self {
            Theme::Amber => 0,
            Theme::Cyan => 1,
            Theme::Green => 2,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub verbose_boot: bool,
    pub theme: Theme,
    pub usb_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            verbose_boot: true,
            theme: Theme::Amber,
            usb_enabled: true,
        }
    }
}

static mut SETTINGS: Settings = Settings {
    verbose_boot: true,
    theme: Theme::Amber,
    usb_enabled: true,
};

fn name_units(s: &str, buf: &mut [u16; 20]) -> *const u16 {
    let mut i = 0;
    for c in s.chars() {
        buf[i] = c as u16;
        i += 1;
    }
    buf[i] = 0;
    buf.as_ptr()
}

fn get_bool(name: &str, default: bool) -> bool {
    let mut nbuf = [0u16; 20];
    let ptr = name_units(name, &mut nbuf);
    let mut data = [0u8; 1];
    match variables::get(ptr, &FERRO_SETTINGS_GUID, &mut data) {
        Ok((_, 1)) => data[0] != 0,
        _ => default,
    }
}

fn set_bool(name: &str, value: bool) {
    let mut nbuf = [0u16; 20];
    let ptr = name_units(name, &mut nbuf);
    let data = [value as u8];
    variables::set(ptr, &FERRO_SETTINGS_GUID, ATTR, &data).ok();
}

fn get_byte(name: &str, default: u8) -> u8 {
    let mut nbuf = [0u16; 20];
    let ptr = name_units(name, &mut nbuf);
    let mut data = [0u8; 1];
    match variables::get(ptr, &FERRO_SETTINGS_GUID, &mut data) {
        Ok((_, 1)) => data[0],
        _ => default,
    }
}

fn set_byte(name: &str, value: u8) {
    let mut nbuf = [0u16; 20];
    let ptr = name_units(name, &mut nbuf);
    let data = [value];
    variables::set(ptr, &FERRO_SETTINGS_GUID, ATTR, &data).ok();
}

/// Loads settings from the live variable store (already populated by
/// persist::load if a saved copy was found on SD) into the in-memory
/// cache the UI reads every frame. Falls back to defaults for any
/// setting that's never been set.
pub fn init() {
    let s = Settings {
        verbose_boot: get_bool("VerboseBoot", true),
        theme: Theme::from_byte(get_byte("AccentTheme", 0)),
        usb_enabled: get_bool("UsbHidEnabled", true),
    };
    unsafe { *core::ptr::addr_of_mut!(SETTINGS) = s };
}

pub fn get() -> Settings {
    unsafe { *core::ptr::addr_of!(SETTINGS) }
}

pub fn set_verbose_boot(v: bool) {
    set_bool("VerboseBoot", v);
    unsafe { (*core::ptr::addr_of_mut!(SETTINGS)).verbose_boot = v };
}

pub fn set_theme(t: Theme) {
    set_byte("AccentTheme", t.to_byte());
    unsafe { (*core::ptr::addr_of_mut!(SETTINGS)).theme = t };
}

pub fn set_usb_enabled(v: bool) {
    set_bool("UsbHidEnabled", v);
    unsafe { (*core::ptr::addr_of_mut!(SETTINGS)).usb_enabled = v };
}
