//! USB HID boot-protocol keyboard support: walks from the root port
//! through a hub (if there is one -- QEMU's raspi3b test setup always
//! puts one there, and real external keyboards are very often on a
//! hub too) to find and enumerate a keyboard, then polls its
//! interrupt endpoint for key reports.

use crate::timer;
use crate::usb::{self, Device, Speed, UsbError};

#[derive(Debug)]
pub enum HidError {
    Usb(#[allow(dead_code)] UsbError),
    NoKeyboardFound,
}

impl From<UsbError> for HidError {
    fn from(e: UsbError) -> Self {
        HidError::Usb(e)
    }
}

pub struct Keyboard {
    dev: Device,
    ep_num: u32,
    ep_mps: usize,
    prev_report: [u8; 8],
}

/// Walks from the root-port device down through a hub (if there is
/// one) to find a HID boot-protocol keyboard, enumerating and
/// configuring it along the way.
pub fn find_keyboard(root_speed: Speed) -> Result<Keyboard, HidError> {
    let root = Device {
        addr: 0,
        max_packet_size: 8,
        low_speed: root_speed == Speed::Low,
    };

    let mut desc = [0u8; 18];
    usb::get_device_descriptor(&root, &mut desc)?;

    if desc[4] == 0x09 {
        // Hub: assign it an address, then walk its ports looking for
        // the first one with something connected.
        let mut hub = Device {
            addr: 0,
            max_packet_size: (desc[7] as u32).max(8),
            low_speed: root.low_speed,
        };
        usb::set_address(&mut hub, 1)?;

        let mut hub_desc = [0u8; 9];
        usb::hub_get_descriptor(&hub, &mut hub_desc)?;
        let num_ports = hub_desc[2];

        for port in 1..=num_ports as u16 {
            if let Ok(Some(speed)) = usb::hub_power_and_reset_port(&hub, port) {
                if let Ok(kbd) = enumerate_as_keyboard(speed, 2) {
                    return Ok(kbd);
                }
            }
        }
        Err(HidError::NoKeyboardFound)
    } else {
        // Directly connected -- try it as the keyboard itself.
        Ok(enumerate_as_keyboard(root_speed, 1)?)
    }
}

fn enumerate_as_keyboard(speed: Speed, addr: u32) -> Result<Keyboard, HidError> {
    let mut dev = Device {
        addr: 0,
        max_packet_size: 8,
        low_speed: speed == Speed::Low,
    };
    let mut desc = [0u8; 18];
    usb::get_device_descriptor(&dev, &mut desc)?;
    usb::set_address(&mut dev, addr)?;
    dev.max_packet_size = (desc[7] as u32).max(8);

    let mut cfg_hdr = [0u8; 9];
    usb::get_configuration_descriptor(&dev, &mut cfg_hdr)?;
    let total_len = (u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]) as usize).clamp(9, 64);
    let config_value = cfg_hdr[5];

    let mut cfg = [0u8; 64];
    let n = usb::get_configuration_descriptor(&dev, &mut cfg[..total_len])?;

    let (ep_num, ep_mps) = find_hid_keyboard_endpoint(&cfg[..n]).ok_or(HidError::NoKeyboardFound)?;

    usb::set_configuration(&dev, config_value)?;
    timer::sleep_ticks(1);

    Ok(Keyboard {
        dev,
        ep_num,
        ep_mps: ep_mps.min(8),
        prev_report: [0; 8],
    })
}

/// Scans a configuration descriptor for a HID boot-protocol keyboard
/// interface (class 3, subclass 1, protocol 1) and returns its
/// interrupt IN endpoint's (number, max packet size).
fn find_hid_keyboard_endpoint(cfg: &[u8]) -> Option<(u32, usize)> {
    let mut i = 0;
    let mut in_keyboard_interface = false;
    while i + 2 <= cfg.len() {
        let len = cfg[i] as usize;
        if len == 0 || i + len > cfg.len() {
            break;
        }
        let desc_type = cfg[i + 1];
        if desc_type == 4 && len >= 9 {
            let class = cfg[i + 5];
            let subclass = cfg[i + 6];
            let protocol = cfg[i + 7];
            in_keyboard_interface = class == 3 && subclass == 1 && protocol == 1;
        } else if desc_type == 5 && len >= 7 && in_keyboard_interface {
            let ep_addr = cfg[i + 2];
            if ep_addr & 0x80 != 0 {
                let mps = u16::from_le_bytes([cfg[i + 4], cfg[i + 5]]) as usize;
                return Some(((ep_addr & 0x0F) as u32, mps));
            }
        }
        i += len;
    }
    None
}

/// Polls the keyboard once (non-blocking: 0 new keys is the normal
/// "nothing pressed" result, not an error). Returns newly-pressed
/// keycodes -- present in this report but not the previous one -- so
/// callers see each keydown exactly once rather than a flood of
/// repeats while a key is held.
pub fn poll_new_keys(kbd: &mut Keyboard) -> Result<[u8; 6], UsbError> {
    let mut report = [0u8; 8];
    let n = usb::interrupt_in(&kbd.dev, kbd.ep_num, &mut report[..kbd.ep_mps])?;
    if n == 0 {
        return Ok([0; 6]);
    }

    let mut new_keys = [0u8; 6];
    let mut ni = 0;
    for &code in &report[2..8] {
        if code != 0 && !kbd.prev_report[2..8].contains(&code) && ni < 6 {
            new_keys[ni] = code;
            ni += 1;
        }
    }
    kbd.prev_report = report;
    Ok(new_keys)
}

/// Translates a USB HID boot-protocol keycode to the byte(s) Ferro's
/// menu (ui.rs) already understands from the UART input path -- j/k
/// or arrow-key ANSI escape sequences, Enter to select. Returns the
/// number of bytes written to `out` (0 for keys with no menu action).
pub fn keycode_to_menu_bytes(code: u8, out: &mut [u8; 3]) -> usize {
    match code {
        0x0D => {
            out[0] = b'j'; // J
            1
        }
        0x0E => {
            out[0] = b'k'; // K
            1
        }
        0x28 => {
            out[0] = b'\r'; // Enter
            1
        }
        0x51 => {
            out[0] = 0x1B;
            out[1] = b'[';
            out[2] = b'B'; // Down arrow
            3
        }
        0x52 => {
            out[0] = 0x1B;
            out[1] = b'[';
            out[2] = b'A'; // Up arrow
            3
        }
        0x50 => {
            out[0] = 0x1B;
            out[1] = b'[';
            out[2] = b'D'; // Left arrow
            3
        }
        0x4F => {
            out[0] = 0x1B;
            out[1] = b'[';
            out[2] = b'C'; // Right arrow
            3
        }
        0x29 => {
            out[0] = 0x08; // Escape -> Back
            1
        }
        0x2A => {
            out[0] = 0x7F; // Backspace -> Back
            1
        }
        _ => 0,
    }
}
