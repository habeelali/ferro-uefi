//! EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL / EFI_SIMPLE_TEXT_INPUT_PROTOCOL --
//! the two protocols System Table's ConOut/ConIn point at. Output
//! renders to the framebuffer as a real scrolling text console (see
//! Framebuffer::scroll_up) and mirrors to UART, matching the rest of
//! Ferro's dual-output style; input reads from UART and, if present,
//! the USB HID keyboard -- both already used by the boot menu, wired
//! in here via raw pointers set once by ui.rs (see `init`) since
//! these are called through plain `extern "C"` function pointers with
//! no way to capture a Rust closure/reference.

use super::events::{self, Kind};
use super::types::{EfiEvent, EfiGuid, EfiStatus, EFI_INVALID_PARAMETER, EFI_NOT_READY, EFI_SUCCESS, EFI_UNSUPPORTED};
use crate::framebuffer::Framebuffer;
use crate::hid::{self, Keyboard};
use crate::uart::Uart;
use core::ffi::c_void;

pub const SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID: EfiGuid = EfiGuid {
    data1: 0x3874_77C2,
    data2: 0x69C7,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

pub const SIMPLE_TEXT_INPUT_PROTOCOL_GUID: EfiGuid = EfiGuid {
    data1: 0x3874_77C1,
    data2: 0x69C7,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

// -- output --

pub type TextResetFn = extern "C" fn(*mut SimpleTextOutputProtocol, u8) -> EfiStatus;
pub type TextOutputStringFn = extern "C" fn(*mut SimpleTextOutputProtocol, *const u16) -> EfiStatus;
pub type TextTestStringFn = extern "C" fn(*mut SimpleTextOutputProtocol, *const u16) -> EfiStatus;
pub type TextQueryModeFn = extern "C" fn(*mut SimpleTextOutputProtocol, usize, *mut usize, *mut usize) -> EfiStatus;
pub type TextSetModeFn = extern "C" fn(*mut SimpleTextOutputProtocol, usize) -> EfiStatus;
pub type TextSetAttributeFn = extern "C" fn(*mut SimpleTextOutputProtocol, usize) -> EfiStatus;
pub type TextClearScreenFn = extern "C" fn(*mut SimpleTextOutputProtocol) -> EfiStatus;
pub type TextSetCursorPositionFn = extern "C" fn(*mut SimpleTextOutputProtocol, usize, usize) -> EfiStatus;
pub type TextEnableCursorFn = extern "C" fn(*mut SimpleTextOutputProtocol, u8) -> EfiStatus;

#[repr(C)]
pub struct SimpleTextOutputMode {
    pub max_mode: i32,
    pub mode: i32,
    pub attribute: i32,
    pub cursor_column: i32,
    pub cursor_row: i32,
    pub cursor_visible: u8, // EFI BOOLEAN is UINT8, kept spec-exact
}

#[repr(C)]
pub struct SimpleTextOutputProtocol {
    pub reset: TextResetFn,
    pub output_string: TextOutputStringFn,
    pub test_string: TextTestStringFn,
    pub query_mode: TextQueryModeFn,
    pub set_mode: TextSetModeFn,
    pub set_attribute: TextSetAttributeFn,
    pub clear_screen: TextClearScreenFn,
    pub set_cursor_position: TextSetCursorPositionFn,
    pub enable_cursor: TextEnableCursorFn,
    pub mode: *mut SimpleTextOutputMode,
}

unsafe impl Sync for SimpleTextOutputProtocol {}
unsafe impl Sync for SimpleTextOutputMode {}

// -- input --

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

pub type InputResetFn = extern "C" fn(*mut SimpleTextInputProtocol, u8) -> EfiStatus;
pub type ReadKeyStrokeFn = extern "C" fn(*mut SimpleTextInputProtocol, *mut InputKey) -> EfiStatus;

#[repr(C)]
pub struct SimpleTextInputProtocol {
    pub reset: InputResetFn,
    pub read_key_stroke: ReadKeyStrokeFn,
    pub wait_for_key: EfiEvent,
}

unsafe impl Sync for SimpleTextInputProtocol {}

// EFI scan codes (SCAN_NULL..SCAN_ESC), spec table 111.
const SCAN_UP: u16 = 0x01;
const SCAN_DOWN: u16 = 0x02;
const SCAN_RIGHT: u16 = 0x03;
const SCAN_LEFT: u16 = 0x04;
const SCAN_ESC: u16 = 0x17;

const GLYPH_SCALE: u32 = 2;
const CELL_W: u32 = (crate::font::GLYPH_WIDTH + 1) * GLYPH_SCALE;
const CELL_H: u32 = (crate::font::GLYPH_HEIGHT + 3) * GLYPH_SCALE;

// EFI's 16-entry text palette, approximated in RGB -- the spec
// defines the color *names*, not exact hardware RGB values.
const PALETTE: [u32; 16] = [
    0x0000_0000, 0x0000_00C8, 0x0000_A000, 0x0000_A0A0, 0x00A0_0000, 0x00A0_00A0, 0x00A0_5000, 0x00A0_A0A0,
    0x0050_5050, 0x0050_50FF, 0x0050_FF50, 0x0050_FFFF, 0x00FF_5050, 0x00FF_50FF, 0x00FF_FF50, 0x00FF_FFFF,
];

static mut FB_PTR: *const Framebuffer = core::ptr::null();
static mut UART_PTR: *mut Uart = core::ptr::null_mut();
static mut KEYBOARD_PTR: *mut Keyboard = core::ptr::null_mut();

static mut CURSOR_COL: i32 = 0;
static mut CURSOR_ROW: i32 = 0;
static mut ATTRIBUTE: usize = 0x07; // light gray on black, the common EFI default

static mut OUT_MODE: SimpleTextOutputMode = SimpleTextOutputMode {
    max_mode: 1,
    mode: 0,
    attribute: 0x07,
    cursor_column: 0,
    cursor_row: 0,
    cursor_visible: 1,
};

static mut WAIT_FOR_KEY_EVENT: EfiEvent = core::ptr::null_mut();

static mut OUT_PROTO: SimpleTextOutputProtocol = SimpleTextOutputProtocol {
    reset: text_reset,
    output_string: output_string,
    test_string: test_string,
    query_mode: query_mode,
    set_mode: set_mode,
    set_attribute: set_attribute,
    clear_screen: clear_screen,
    set_cursor_position: set_cursor_position,
    enable_cursor: enable_cursor,
    mode: core::ptr::null_mut(), // patched in init()
};

static mut IN_PROTO: SimpleTextInputProtocol = SimpleTextInputProtocol {
    reset: input_reset,
    read_key_stroke,
    wait_for_key: core::ptr::null_mut(), // patched in init()
};

fn grid_size() -> (u32, u32) {
    let fb = unsafe { &*FB_PTR };
    (fb.width / CELL_W, fb.height / CELL_H)
}

fn fg_bg() -> (u32, u32) {
    let attr = unsafe { ATTRIBUTE };
    let fg = PALETTE[(attr & 0x0F).min(15)];
    let bg = PALETTE[((attr >> 4) & 0x07).min(15)];
    (fg, bg)
}

fn newline() {
    let (_, rows) = grid_size();
    unsafe {
        CURSOR_COL = 0;
        CURSOR_ROW += 1;
        if CURSOR_ROW as u32 >= rows {
            let fb = &*FB_PTR;
            let (_, bg) = fg_bg();
            fb.scroll_up(CELL_H, bg);
            CURSOR_ROW = rows as i32 - 1;
        }
    }
}

fn put_char(c: char) {
    let fb = unsafe { &*FB_PTR };
    let (fg, _) = fg_bg();
    let (cols, _) = grid_size();
    unsafe {
        if CURSOR_COL as u32 >= cols {
            newline();
        }
        let x = CURSOR_COL as u32 * CELL_W;
        let y = CURSOR_ROW as u32 * CELL_H;
        fb.draw_char(x, y, c, GLYPH_SCALE, fg);
        CURSOR_COL += 1;
    }
}

extern "C" fn text_reset(this: *mut SimpleTextOutputProtocol, _extended: u8) -> EfiStatus {
    clear_screen(this)
}

extern "C" fn output_string(_this: *mut SimpleTextOutputProtocol, string: *const u16) -> EfiStatus {
    if string.is_null() || unsafe { FB_PTR.is_null() } {
        return EFI_INVALID_PARAMETER;
    }
    let mut i = 0isize;
    loop {
        let c = unsafe { *string.offset(i) };
        if c == 0 {
            break;
        }
        i += 1;
        match c {
            0x0D => {
                unsafe { CURSOR_COL = 0 };
                if let Some(u) = unsafe { UART_PTR.as_mut() } {
                    u.putc(b'\r');
                }
            }
            0x0A => {
                newline();
                if let Some(u) = unsafe { UART_PTR.as_mut() } {
                    u.putc(b'\n');
                }
            }
            0x08 => unsafe {
                if CURSOR_COL > 0 {
                    CURSOR_COL -= 1;
                }
            },
            0x20..=0x7E => {
                put_char(c as u8 as char);
                if let Some(u) = unsafe { UART_PTR.as_mut() } {
                    u.putc(c as u8);
                }
            }
            _ => {} // outside our font's coverage -- dropped, not garbled
        }
    }
    sync_mode();
    unsafe { (*FB_PTR).flush() };
    EFI_SUCCESS
}

extern "C" fn test_string(_this: *mut SimpleTextOutputProtocol, string: *const u16) -> EfiStatus {
    if string.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    let mut i = 0isize;
    loop {
        let c = unsafe { *string.offset(i) };
        if c == 0 {
            break;
        }
        i += 1;
        if !matches!(c, 0x0D | 0x0A | 0x08 | 0x20..=0x7E) {
            return EFI_UNSUPPORTED;
        }
    }
    EFI_SUCCESS
}

extern "C" fn query_mode(_this: *mut SimpleTextOutputProtocol, mode: usize, cols: *mut usize, rows: *mut usize) -> EfiStatus {
    if mode != 0 {
        return EFI_UNSUPPORTED; // only mode 0 exists
    }
    let (c, r) = grid_size();
    if !cols.is_null() {
        unsafe { *cols = c as usize };
    }
    if !rows.is_null() {
        unsafe { *rows = r as usize };
    }
    EFI_SUCCESS
}

extern "C" fn set_mode(_this: *mut SimpleTextOutputProtocol, mode: usize) -> EfiStatus {
    if mode == 0 {
        EFI_SUCCESS
    } else {
        EFI_UNSUPPORTED
    }
}

extern "C" fn set_attribute(_this: *mut SimpleTextOutputProtocol, attribute: usize) -> EfiStatus {
    unsafe { ATTRIBUTE = attribute };
    sync_mode();
    EFI_SUCCESS
}

extern "C" fn clear_screen(_this: *mut SimpleTextOutputProtocol) -> EfiStatus {
    if unsafe { FB_PTR.is_null() } {
        return EFI_UNSUPPORTED;
    }
    let (_, bg) = fg_bg();
    unsafe {
        (*FB_PTR).clear(bg);
        (*FB_PTR).flush();
        CURSOR_COL = 0;
        CURSOR_ROW = 0;
    }
    sync_mode();
    EFI_SUCCESS
}

extern "C" fn set_cursor_position(_this: *mut SimpleTextOutputProtocol, column: usize, row: usize) -> EfiStatus {
    let (cols, rows) = grid_size();
    if column as u32 >= cols || row as u32 >= rows {
        return EFI_INVALID_PARAMETER;
    }
    unsafe {
        CURSOR_COL = column as i32;
        CURSOR_ROW = row as i32;
    }
    sync_mode();
    EFI_SUCCESS
}

extern "C" fn enable_cursor(_this: *mut SimpleTextOutputProtocol, visible: u8) -> EfiStatus {
    unsafe { OUT_MODE.cursor_visible = visible };
    EFI_SUCCESS
}

fn sync_mode() {
    unsafe {
        OUT_MODE.attribute = ATTRIBUTE as i32;
        OUT_MODE.cursor_column = CURSOR_COL;
        OUT_MODE.cursor_row = CURSOR_ROW;
    }
}

extern "C" fn input_reset(_this: *mut SimpleTextInputProtocol, _extended: u8) -> EfiStatus {
    EFI_SUCCESS
}

struct AnsiState {
    esc: u8,
}
static mut ANSI: AnsiState = AnsiState { esc: 0 };

/// Resolves one UART byte into a key, tracking multi-byte ANSI arrow
/// sequences (ESC '[' 'A'..'D') the same way the boot menu does.
/// Returns None both for "nothing yet" and "mid-sequence" -- callers
/// just try again on the next poll, same as ui.rs's InputState.
fn uart_byte_to_key(b: u8) -> Option<InputKey> {
    let st = unsafe { &mut *core::ptr::addr_of_mut!(ANSI) };
    match st.esc {
        0 if b == 0x1B => {
            st.esc = 1;
            None
        }
        1 if b == b'[' => {
            st.esc = 2;
            None
        }
        2 => {
            st.esc = 0;
            let scan = match b {
                b'A' => SCAN_UP,
                b'B' => SCAN_DOWN,
                b'C' => SCAN_RIGHT,
                b'D' => SCAN_LEFT,
                _ => return None,
            };
            Some(InputKey { scan_code: scan, unicode_char: 0 })
        }
        1 => {
            st.esc = 0;
            Some(InputKey { scan_code: SCAN_ESC, unicode_char: 0 })
        }
        _ => {
            st.esc = 0;
            match b {
                0x0D | 0x0A => Some(InputKey { scan_code: 0, unicode_char: 0x0D }),
                0x08 | 0x7F => Some(InputKey { scan_code: 0, unicode_char: 0x08 }),
                0x09 => Some(InputKey { scan_code: 0, unicode_char: 0x09 }),
                0x20..=0x7E => Some(InputKey { scan_code: 0, unicode_char: b as u16 }),
                _ => None,
            }
        }
    }
}

/// Resolves one USB HID boot-protocol keycode into a key. Letters and
/// digits use the USB HID usage table's direct arithmetic mapping
/// (usage 0x04 = 'a' .. 0x1D = 'z', 0x1E = '1' .. 0x27 = '0'); arrows
/// and control keys map to their EFI scan codes / control chars.
fn hid_code_to_key(code: u8) -> Option<InputKey> {
    match code {
        0x04..=0x1D => Some(InputKey { scan_code: 0, unicode_char: (b'a' + (code - 0x04)) as u16 }),
        0x1E..=0x26 => Some(InputKey { scan_code: 0, unicode_char: (b'1' + (code - 0x1E)) as u16 }),
        0x27 => Some(InputKey { scan_code: 0, unicode_char: b'0' as u16 }),
        0x28 => Some(InputKey { scan_code: 0, unicode_char: 0x0D }), // Enter
        0x29 => Some(InputKey { scan_code: SCAN_ESC, unicode_char: 0 }),
        0x2A => Some(InputKey { scan_code: 0, unicode_char: 0x08 }), // Backspace
        0x2B => Some(InputKey { scan_code: 0, unicode_char: 0x09 }), // Tab
        0x2C => Some(InputKey { scan_code: 0, unicode_char: b' ' as u16 }),
        0x4F => Some(InputKey { scan_code: SCAN_RIGHT, unicode_char: 0 }),
        0x50 => Some(InputKey { scan_code: SCAN_LEFT, unicode_char: 0 }),
        0x51 => Some(InputKey { scan_code: SCAN_DOWN, unicode_char: 0 }),
        0x52 => Some(InputKey { scan_code: SCAN_UP, unicode_char: 0 }),
        _ => None,
    }
}

fn poll_key_raw() -> Option<InputKey> {
    if let Some(u) = unsafe { UART_PTR.as_mut() } {
        if let Some(b) = u.getc() {
            // A resolved key might come back None if this byte only
            // advanced a pending ANSI sequence -- that's fine, the
            // sequence's state persists for the next poll.
            if let Some(k) = uart_byte_to_key(b) {
                return Some(k);
            }
        }
    }
    if let Some(kbd) = unsafe { KEYBOARD_PTR.as_mut() } {
        if let Ok(keys) = hid::poll_new_keys(kbd) {
            for &code in keys.iter() {
                if code == 0 {
                    continue;
                }
                if let Some(k) = hid_code_to_key(code) {
                    return Some(k);
                }
            }
        }
    }
    None
}

/// Used by events.rs's ConsoleIn event kind -- true if ReadKeyStroke
/// would succeed right now.
pub fn key_available() -> bool {
    // Peeking without consuming would need its own buffering; instead
    // this mirrors ReadKeyStroke's own logic closely enough for the
    // common WaitForEvent-then-ReadKeyStroke pattern real apps use --
    // if a byte/keycode is sitting in the UART FIFO or the last USB
    // poll saw a new key, report ready. Not perfectly side-effect-free
    // (a UART byte gets consumed into the ANSI state machine either
    // way), but that's the same trade-off the boot menu already makes.
    poll_key_raw().is_some()
}

extern "C" fn read_key_stroke(_this: *mut SimpleTextInputProtocol, key: *mut InputKey) -> EfiStatus {
    if key.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    match poll_key_raw() {
        Some(k) => {
            unsafe { *key = k };
            EFI_SUCCESS
        }
        None => EFI_NOT_READY,
    }
}

/// Wires the console to the same framebuffer, UART, and (if present)
/// USB keyboard the boot menu already owns, and resets cursor/
/// attribute state. Must be called before handing control to a loaded
/// EFI image -- raw pointers rather than borrows because these get
/// called through plain `extern "C"` function pointers with no way to
/// carry a lifetime, but they're all backed by ui::run()'s locals,
/// which live for the rest of the firmware's execution (run() never
/// returns).
pub fn init(fb: *const Framebuffer, uart: *mut Uart, keyboard: *mut Keyboard) {
    unsafe {
        FB_PTR = fb;
        UART_PTR = uart;
        KEYBOARD_PTR = keyboard;
        CURSOR_COL = 0;
        CURSOR_ROW = 0;
        ATTRIBUTE = 0x07;
        ANSI.esc = 0;

        OUT_PROTO.mode = core::ptr::addr_of_mut!(OUT_MODE);

        if WAIT_FOR_KEY_EVENT.is_null() {
            WAIT_FOR_KEY_EVENT = events::create(Kind::ConsoleIn, None, core::ptr::null_mut())
                .unwrap_or(core::ptr::null_mut());
        }
        IN_PROTO.wait_for_key = WAIT_FOR_KEY_EVENT;
    }

    let st = unsafe { &mut *core::ptr::addr_of_mut!(super::system_table::SYSTEM_TABLE) };
    st.con_out = core::ptr::addr_of_mut!(OUT_PROTO) as *mut c_void;
    st.con_in = core::ptr::addr_of_mut!(IN_PROTO) as *mut c_void;

    // Give ConOut/ConIn real (if minimal) handles of their own, with
    // the corresponding protocol installed -- some apps HandleProtocol
    // these instead of dereferencing SystemTable->ConOut directly.
    // Only ever allocated once (init() re-runs on every boot_efi_app
    // call, once per loaded image, but the console handles are the
    // same firmware-owned objects every time).
    if st.console_out_handle.is_null() {
        if let Some(i) = super::protocol_db::find_or_create_handle(core::ptr::null_mut()) {
            super::protocol_db::install(i, SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID, core::ptr::addr_of_mut!(OUT_PROTO) as *mut c_void);
            st.console_out_handle = super::protocol_db::handle_for_index(i);
        }
    }
    if st.console_in_handle.is_null() {
        if let Some(i) = super::protocol_db::find_or_create_handle(core::ptr::null_mut()) {
            super::protocol_db::install(i, SIMPLE_TEXT_INPUT_PROTOCOL_GUID, core::ptr::addr_of_mut!(IN_PROTO) as *mut c_void);
            st.console_in_handle = super::protocol_db::handle_for_index(i);
        }
    }
    st.standard_error_handle = st.console_out_handle;
    st.std_err = st.con_out;
}
