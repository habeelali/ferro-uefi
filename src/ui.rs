//! Splash screen and interactive boot menu/setup UI.
//!
//! Navigation works over the UART serial console (arrow keys or j/k,
//! Enter to select, Backspace/q to go back) and, once USB is up, a
//! real USB HID keyboard -- both feed the same key vocabulary (see
//! `Key`, `decode_byte`, and hid::keycode_to_menu_bytes).
//!
//! Every screen shares a common header/footer/border "chrome"
//! (`draw_chrome`) and prints its body text one line at a time with a
//! real delay and a live UART mirror (`print_lines`), rather than
//! blitting the whole screen at once -- closer to how real firmware
//! setup screens and POST logs actually read. The boot manager itself
//! is a two-pane layout: a menu list on the left, a live system
//! status panel on the right that keeps updating (uptime, input
//! device, NVRAM usage) even while nothing is being pressed.
//!
//! SETTINGS is backed by real UEFI variables (see settings.rs) --
//! changing a value here calls the same variable store GetVariable/
//! SetVariable read and write, and SAVE VARIABLES TO SD persists it
//! for real.

use crate::efi::variables;
use crate::fat32::Fat32;
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::framebuffer::Framebuffer;
use crate::hid::{self, Keyboard};
use crate::sd::Card;
use crate::settings;
use crate::uart::Uart;
use crate::{pm, timer};
use core::fmt::Write;

const BG: u32 = 0x0012_1620;
const HEADER_BG: u32 = 0x0020_2A38;
const BORDER: u32 = 0x0035_4250;
const FG: u32 = 0x00E0_E6EC;
const DIM: u32 = 0x0070_7880;
const ERROR: u32 = 0x00FF_5040;

fn accent() -> u32 {
    settings::get().theme.accent()
}

fn select_bg() -> u32 {
    settings::get().theme.select_bg()
}

const HEADER_H: u32 = 50;
const FOOTER_H: u32 = 40;
const MARGIN: u32 = 20;
const CONTENT_X: u32 = 40;
const CONTENT_Y: u32 = HEADER_H + 35;

/// Ticks between lines when printing progressively -- only applied
/// when settings::get().verbose_boot is on; off makes the log print
/// at full speed, still line by line rather than all at once.
const LINE_DELAY_TICKS: u64 = 6;

/// Ticks between idle redraws of the boot manager screen, so the
/// status panel's uptime/live fields keep moving even when nothing's
/// being pressed.
const IDLE_REDRAW_TICKS: u64 = 100;

fn text_width(text: &str, scale: u32) -> u32 {
    text.chars().count() as u32 * (GLYPH_WIDTH + 1) * scale
}

fn line_height(scale: u32) -> u32 {
    (GLYPH_HEIGHT + 3) * scale
}

/// A tiny fixed-capacity string buffer so screens can render live
/// numeric values (uptime, byte counts) with `write!` instead of only
/// ever showing static text.
struct LineBuf {
    buf: [u8; 80],
    len: usize,
}

impl LineBuf {
    fn new() -> Self {
        LineBuf { buf: [0; 80], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for LineBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let n = bytes.len().min(self.buf.len().saturating_sub(self.len));
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

/// Shared screen chrome: header bar with the firmware name and a
/// per-screen title, a bordered content panel, and a footer bar with
/// a context hint. Every screen after the splash uses this so the UI
/// reads as one coherent piece of firmware rather than a pile of ad
/// hoc drawing calls.
fn draw_chrome(fb: &Framebuffer, title: &str, hint: &str) {
    fb.clear(BG);

    fb.fill_rect(0, 0, fb.width, HEADER_H, HEADER_BG);
    fb.draw_text(MARGIN, 16, "FERRO UEFI", 3, accent());
    let title_x = fb.width.saturating_sub(text_width(title, 2) + MARGIN);
    fb.draw_text(title_x, 19, title, 2, FG);
    fb.fill_rect(0, HEADER_H, fb.width, 2, BORDER);

    fb.fill_rect(0, fb.height - FOOTER_H, fb.width, FOOTER_H, HEADER_BG);
    fb.fill_rect(0, fb.height - FOOTER_H, fb.width, 2, BORDER);
    fb.draw_text(MARGIN, fb.height - FOOTER_H + 12, hint, 2, DIM);

    let panel_y = HEADER_H + 12;
    let panel_h = fb.height - HEADER_H - FOOTER_H - 24;
    fb.draw_rect_outline(MARGIN, panel_y, fb.width - 2 * MARGIN, panel_h, 2, BORDER);

    fb.flush();
}

/// Draws and UART-mirrors `lines` one at a time. The inter-line delay
/// honors settings::get().verbose_boot -- off means full speed, still
/// one line at a time rather than one blit.
fn print_lines(
    fb: &Framebuffer,
    uart: &mut Uart,
    x: u32,
    y: u32,
    scale: u32,
    color: u32,
    lines: &[&str],
) -> u32 {
    let delay = if settings::get().verbose_boot { LINE_DELAY_TICKS } else { 0 };
    let mut cy = y;
    for line in lines {
        fb.draw_text(x, cy, line, scale, color);
        fb.flush();
        writeln!(uart, "{line}").ok();
        if delay > 0 {
            timer::sleep_ticks(delay);
        }
        cy += line_height(scale);
    }
    cy
}

/// A POST-style text log shown on the framebuffer before the branded
/// splash -- everything up to this point (EL drop, MMU, timer/IRQ,
/// framebuffer itself) only ever reached UART, since there was no
/// display yet to put it on.
pub fn boot_log(fb: &Framebuffer, uart: &mut Uart, lines: &[&str]) {
    draw_chrome(fb, "BOOT LOG", "PLEASE WAIT...");
    print_lines(fb, uart, CONTENT_X, CONTENT_Y, 2, FG, lines);
}

pub fn splash(fb: &Framebuffer) {
    fb.clear(BG);

    let title = "FERRO UEFI";
    let title_scale = 6;
    let tx = (fb.width.saturating_sub(text_width(title, title_scale))) / 2;
    fb.draw_text(tx, fb.height / 2 - 60, title, title_scale, accent());

    let sub = "RASPBERRY PI 3 / BCM2837";
    let sub_scale = 3;
    let sx = (fb.width.saturating_sub(text_width(sub, sub_scale))) / 2;
    fb.draw_text(sx, fb.height / 2 + 20, sub, sub_scale, FG);

    fb.flush();
}

struct MenuItem {
    label: &'static str,
    description: &'static str,
}

const ITEMS: [MenuItem; 5] = [
    MenuItem {
        label: "BOOT FROM SD",
        description: "MOUNT THE SD CARD, LOAD SAVED VARIABLES, LIST WHAT'S ON IT",
    },
    MenuItem {
        label: "SETTINGS",
        description: "VERBOSE BOOT, ACCENT THEME, USB HID -- REAL UEFI VARIABLES",
    },
    MenuItem {
        label: "SYSTEM INFO",
        description: "CPU, TIMER, MMU, USB, AND NVRAM STATE, READ LIVE FROM HARDWARE",
    },
    MenuItem {
        label: "SAVE VARIABLES TO SD",
        description: "WRITE UEFI VARIABLES (INCLUDING SETTINGS) TO THE SD CARD",
    },
    MenuItem {
        label: "REBOOT",
        description: "ISSUE A WATCHDOG RESET",
    },
];

struct MenuState {
    selected: usize,
}

fn move_up(s: &mut MenuState) {
    s.selected = s.selected.checked_sub(1).unwrap_or(ITEMS.len() - 1);
}

fn move_down(s: &mut MenuState) {
    s.selected = (s.selected + 1) % ITEMS.len();
}

const DIVIDER_X: u32 = 400;
const STATUS_X: u32 = DIVIDER_X + 30;

/// Reads the handful of live CPU/system registers the status panel
/// and SYSTEM INFO screen both show.
fn cpu_snapshot() -> (u64, u64, u64) {
    let midr: u64;
    let freq: u64;
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {0}, midr_el1", out(reg) midr);
        core::arch::asm!("mrs {0}, cntfrq_el0", out(reg) freq);
        core::arch::asm!("mrs {0}, CurrentEL", out(reg) el);
    }
    (midr, freq, (el >> 2) & 0x3)
}

fn draw_status_panel(fb: &Framebuffer, keyboard: &Option<Keyboard>) {
    let x = STATUS_X;
    let mut y = CONTENT_Y;
    let s = settings::get();

    fb.draw_text(x, y, "SYSTEM STATUS", 2, accent());
    y += line_height(2) + 6;

    let (_, freq, el) = cpu_snapshot();
    let ticks = timer::ticks();
    let secs = ticks / 100;
    let tenths = (ticks % 100) / 10;

    let mut lb = LineBuf::new();
    write!(lb, "UPTIME   {secs}.{tenths}S").ok();
    fb.draw_text(x, y, lb.as_str(), 2, FG);
    y += line_height(2);

    fb.draw_text(x, y, "BOARD    PI 3 / BCM2837", 2, FG);
    y += line_height(2);

    let mut lb = LineBuf::new();
    write!(lb, "CPU      4X CORTEX-A53 EL{el}").ok();
    fb.draw_text(x, y, lb.as_str(), 2, FG);
    y += line_height(2);

    let mut lb = LineBuf::new();
    write!(lb, "TIMER    {}MHZ", freq / 1_000_000).ok();
    fb.draw_text(x, y, lb.as_str(), 2, FG);
    y += line_height(2);

    fb.draw_text(x, y, "MMU      ENABLED", 2, FG);
    y += line_height(2) + 6;

    let input_line = if keyboard.is_some() {
        "INPUT    USB HID + UART"
    } else if s.usb_enabled {
        "INPUT    UART ONLY (NO USB DEV)"
    } else {
        "INPUT    UART ONLY (USB DISABLED)"
    };
    fb.draw_text(x, y, input_line, 2, keyboard.is_some().then_some(FG).unwrap_or(DIM));
    y += line_height(2);

    let mut lb = LineBuf::new();
    write!(lb, "THEME    {}", s.theme.name()).ok();
    fb.draw_text(x, y, lb.as_str(), 2, FG);
    y += line_height(2) + 6;

    let (max, remaining, _) = variables::storage_info();
    let used = max - remaining;
    let mut lb = LineBuf::new();
    write!(lb, "NVRAM    {used}/{max} BYTES").ok();
    fb.draw_text(x, y, lb.as_str(), 2, FG);
}

fn draw_menu(fb: &Framebuffer, s: &MenuState, keyboard: &Option<Keyboard>) {
    draw_chrome(fb, "BOOT MANAGER", "ARROWS OR J/K: MOVE    ENTER: SELECT");

    let row_h = 32;
    let mut y = CONTENT_Y + 4;
    for (i, item) in ITEMS.iter().enumerate() {
        if i == s.selected {
            fb.fill_rect(CONTENT_X, y - 6, DIVIDER_X - CONTENT_X - 20, row_h, select_bg());
            fb.fill_rect(CONTENT_X, y - 6, 4, row_h, accent());
        }
        let color = if i == s.selected { accent() } else { FG };
        fb.draw_text(CONTENT_X + 20, y, item.label, 2, color);
        y += row_h + 6;
    }

    let desc_y = y + 14;
    let desc_max_w = DIVIDER_X - CONTENT_X - 20;
    // Descriptions are long; wrap at whatever fits so they don't spill
    // across the divider into the status panel.
    let max_chars = (desc_max_w / ((GLYPH_WIDTH + 1) * 2)).max(1) as usize;
    let desc = ITEMS[s.selected].description;
    let mut start = 0;
    let mut dy = desc_y;
    let bytes = desc.as_bytes();
    while start < bytes.len() {
        let end = (start + max_chars).min(bytes.len());
        // Break on the last space within range, if there is one, so
        // words don't get split mid-word.
        let mut cut = end;
        if end < bytes.len() {
            if let Some(pos) = desc[start..end].rfind(' ') {
                cut = start + pos;
            }
        }
        fb.draw_text(CONTENT_X, dy, &desc[start..cut], 2, DIM);
        dy += line_height(2);
        start = if cut == start { end } else { cut + 1 };
    }

    fb.fill_rect(DIVIDER_X, HEADER_H + 12, 2, fb.height - HEADER_H - FOOTER_H - 24, BORDER);
    draw_status_panel(fb, keyboard);

    fb.flush();
}

/// One resolved key press, after any multi-byte escape sequence has
/// been fully parsed -- the vocabulary both the UART path and the USB
/// HID path (via hid::keycode_to_menu_bytes) ultimately produce.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Back,
}

/// Tracks ANSI-escape parsing state across calls, since arrow keys
/// arrive as a 3-byte sequence (ESC '[' 'A'/'B'/'C'/'D') that may be
/// fed in one byte at a time.
struct InputState {
    esc: u8, // 0 = idle, 1 = saw ESC, 2 = saw ESC '['
}

fn decode_byte(b: u8, input: &mut InputState) -> Option<Key> {
    match input.esc {
        0 if b == 0x1B => {
            input.esc = 1;
            None
        }
        1 if b == b'[' => {
            input.esc = 2;
            None
        }
        2 => {
            input.esc = 0;
            match b {
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                _ => None,
            }
        }
        _ => {
            input.esc = 0;
            match b {
                b'k' | b'K' => Some(Key::Up),
                b'j' | b'J' => Some(Key::Down),
                b'h' | b'H' => Some(Key::Left),
                b'l' | b'L' => Some(Key::Right),
                b'\r' | b'\n' => Some(Key::Enter),
                0x08 | 0x7F | b'q' | b'Q' => Some(Key::Back),
                _ => None,
            }
        }
    }
}

/// Non-blocking: resolves at most one key from whichever input source
/// has something waiting, UART checked first.
fn poll_menu_key(uart: &mut Uart, keyboard: &mut Option<Keyboard>, input: &mut InputState) -> Option<Key> {
    if let Some(b) = uart.getc() {
        return decode_byte(b, input);
    }
    if let Some(kbd) = keyboard.as_mut() {
        if let Ok(keys) = hid::poll_new_keys(kbd) {
            for &code in keys.iter() {
                if code == 0 {
                    continue;
                }
                let mut bytes = [0u8; 3];
                let n = hid::keycode_to_menu_bytes(code, &mut bytes);
                for &b in &bytes[..n] {
                    if let Some(k) = decode_byte(b, input) {
                        return Some(k);
                    }
                }
            }
        }
    }
    None
}

/// Non-blocking: true if *any* key arrived on either input source,
/// regardless of what it decodes to -- used for "press any key".
fn poll_any_key(uart: &mut Uart, keyboard: &mut Option<Keyboard>) -> bool {
    if uart.getc().is_some() {
        return true;
    }
    if let Some(kbd) = keyboard.as_mut() {
        if let Ok(keys) = hid::poll_new_keys(kbd) {
            return keys.iter().any(|&c| c != 0);
        }
    }
    false
}

fn wait_for_key(uart: &mut Uart, keyboard: &mut Option<Keyboard>) {
    loop {
        if poll_any_key(uart, keyboard) {
            return;
        }
    }
}

fn show_message(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>, title: &str, lines: &[&str]) {
    draw_chrome(fb, title, "PRESS ANY KEY TO RETURN");
    print_lines(fb, uart, CONTENT_X, CONTENT_Y, 2, FG, lines);
    wait_for_key(uart, keyboard);
}

fn show_error(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>, title: &str, lines: &[&str]) {
    draw_chrome(fb, title, "PRESS ANY KEY TO RETURN");
    print_lines(fb, uart, CONTENT_X, CONTENT_Y, 2, ERROR, lines);
    wait_for_key(uart, keyboard);
}

fn show_system_info(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>) {
    let (midr, freq, el) = cpu_snapshot();
    let s = settings::get();
    let (max, remaining, max_var) = variables::storage_info();

    writeln!(uart, "\n[menu] SYSTEM INFO selected").ok();
    writeln!(uart, "  board : Raspberry Pi 3 / BCM2837").ok();
    writeln!(uart, "  cpu   : 4x Cortex-A53, MIDR_EL1 = 0x{midr:08x}").ok();
    writeln!(uart, "  EL    : {el}").ok();
    writeln!(uart, "  timer : CNTFRQ_EL0 = {freq} Hz, {} ticks since boot", timer::ticks()).ok();
    writeln!(uart, "  mmu   : enabled (identity-mapped RAM + device regions)").ok();
    writeln!(
        uart,
        "  usb   : keyboard={} usb_enabled_setting={}",
        keyboard.is_some(),
        s.usb_enabled
    )
    .ok();
    writeln!(
        uart,
        "  nvram : {}/{} bytes used, {} bytes max per variable",
        max - remaining,
        max,
        max_var
    )
    .ok();

    draw_chrome(fb, "SYSTEM INFO", "PRESS ANY KEY TO RETURN");

    let mut midr_line = LineBuf::new();
    write!(midr_line, "MIDR_EL1: 0X{midr:08X}").ok();
    let mut el_line = LineBuf::new();
    write!(el_line, "EXCEPTION LEVEL: EL{el}").ok();
    let mut freq_line = LineBuf::new();
    write!(freq_line, "GENERIC TIMER: {freq} HZ ({} TICKS SINCE BOOT)", timer::ticks()).ok();
    let mut usb_line = LineBuf::new();
    write!(
        usb_line,
        "USB: {}",
        if keyboard.is_some() {
            "HID KEYBOARD CONNECTED"
        } else if s.usb_enabled {
            "NO HID KEYBOARD FOUND"
        } else {
            "DISABLED IN SETTINGS"
        }
    )
    .ok();
    let mut nvram_line = LineBuf::new();
    write!(nvram_line, "NVRAM: {}/{} BYTES USED (MAX {} PER VAR)", max - remaining, max, max_var).ok();

    print_lines(
        fb,
        uart,
        CONTENT_X,
        CONTENT_Y,
        2,
        FG,
        &[
            "BOARD: RASPBERRY PI 3 / BCM2837",
            "CPU: 4X CORTEX-A53 (ARMV8-A)",
            midr_line.as_str(),
            el_line.as_str(),
            freq_line.as_str(),
            "MMU: ENABLED (IDENTITY-MAPPED)",
            "IRQ SOURCE: BCM2836 LOCAL BLOCK (NOT A GIC)",
            usb_line.as_str(),
            nvram_line.as_str(),
        ],
    );
    wait_for_key(uart, keyboard);
}

/// The SETTINGS screen: a short list of real, UEFI-variable-backed
/// options. Enter/Right changes the selected setting's value; Left
/// cycles it backward where that makes sense (accent theme); Up/Down
/// move the selection; Backspace/q/Esc returns to the boot manager.
fn run_settings(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>) {
    writeln!(uart, "\n[menu] SETTINGS selected").ok();

    const ROWS: usize = 3;
    let mut selected = 0usize;
    let mut input = InputState { esc: 0 };

    let describe = |i: usize| -> &'static str {
        match i {
            0 => "TOGGLES THE DELAY BETWEEN BOOT LOG LINES. OFF PRINTS FULL SPEED.",
            1 => "CHANGES THE MENU'S ACCENT COLOR. APPLIES IMMEDIATELY.",
            _ => "ENABLE/DISABLE USB HOST + HID KEYBOARD. TAKES EFFECT NEXT BOOT.",
        }
    };

    let draw = |fb: &Framebuffer, selected: usize| {
        draw_chrome(
            fb,
            "SETTINGS",
            "ARROWS: MOVE    ENTER/RIGHT: CHANGE    BACKSPACE/Q: BACK",
        );
        let s = settings::get();
        let row_h = 34;
        let mut y = CONTENT_Y + 4;

        let rows: [LineBuf; ROWS] = {
            let mut vb = LineBuf::new();
            write!(vb, "VERBOSE BOOT LOG            {}", if s.verbose_boot { "ON" } else { "OFF" }).ok();
            let mut tb = LineBuf::new();
            write!(tb, "ACCENT THEME                {}", s.theme.name()).ok();
            let mut ub = LineBuf::new();
            write!(ub, "USB HID KEYBOARD            {}", if s.usb_enabled { "ENABLED" } else { "DISABLED" }).ok();
            [vb, tb, ub]
        };

        for (i, line) in rows.iter().enumerate() {
            if i == selected {
                fb.fill_rect(CONTENT_X, y - 6, fb.width - 2 * CONTENT_X - 40, row_h, select_bg());
                fb.fill_rect(CONTENT_X, y - 6, 4, row_h, accent());
            }
            let color = if i == selected { accent() } else { FG };
            fb.draw_text(CONTENT_X + 20, y, line.as_str(), 2, color);
            y += row_h + 8;
        }

        y += 14;
        fb.draw_text(CONTENT_X, y, describe(selected), 2, DIM);

        fb.flush();
    };

    draw(fb, selected);

    loop {
        let Some(key) = poll_menu_key(uart, keyboard, &mut input) else {
            continue;
        };
        match key {
            Key::Up => selected = selected.checked_sub(1).unwrap_or(ROWS - 1),
            Key::Down => selected = (selected + 1) % ROWS,
            Key::Back => return,
            Key::Enter | Key::Right => {
                apply_change(selected, true);
                draw(fb, selected);
                continue;
            }
            Key::Left => {
                apply_change(selected, false);
                draw(fb, selected);
                continue;
            }
        }
        draw(fb, selected);
    }
}

fn apply_change(row: usize, forward: bool) {
    let s = settings::get();
    match row {
        0 => {
            settings::set_verbose_boot(!s.verbose_boot);
        }
        1 => {
            let next = if forward {
                s.theme.next()
            } else {
                // Three themes: going "back" is the same as going
                // forward twice.
                s.theme.next().next()
            };
            settings::set_theme(next);
        }
        _ => {
            settings::set_usb_enabled(!s.usb_enabled);
        }
    }
}

fn boot_from_sd(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>) {
    writeln!(uart, "\n[menu] BOOT FROM SD selected").ok();
    draw_chrome(fb, "BOOT FROM SD", "PLEASE WAIT...");
    let y = print_lines(
        fb,
        uart,
        CONTENT_X,
        CONTENT_Y,
        2,
        FG,
        &["INITIALIZING SD CARD (SDHCI)..."],
    );

    let card = match Card::init() {
        Ok(c) => c,
        Err(e) => {
            writeln!(uart, "  sd init failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                keyboard,
                "BOOT FROM SD",
                &["SD CARD INIT FAILED.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    let y = print_lines(fb, uart, CONTENT_X, y, 2, FG, &["SD CARD READY."]);

    let fs = match Fat32::mount(&card) {
        Ok(fs) => fs,
        Err(e) => {
            writeln!(uart, "  fat32 mount failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                keyboard,
                "BOOT FROM SD",
                &["NO FAT32 VOLUME FOUND.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    let mut y = print_lines(fb, uart, CONTENT_X, y, 2, FG, &["FAT32 VOLUME MOUNTED."]);
    crate::persist::set_context(card, fs);

    match crate::persist::load(&card, &fs) {
        Ok(n) => {
            y = print_lines(
                fb,
                uart,
                CONTENT_X,
                y,
                2,
                FG,
                &["LOADED SAVED VARIABLES FROM SD."],
            );
            writeln!(uart, "  ({n} variable(s) merged into the live store)").ok();
            // Re-sync the cached settings in case a previously-saved
            // SETTINGS value (verbose boot, theme, USB) just got
            // merged into the live variable store.
            settings::init();
        }
        Err(e) => {
            // Nothing saved yet on a fresh card -- not an error worth
            // interrupting the boot flow for.
            writeln!(uart, "  no saved variables loaded: {e:?}").ok();
        }
    }

    let mut names = [[0u8; 11]; 8];
    let mut sizes = [0u32; 8];
    let count = match fs.list_root(&card, &mut names, &mut sizes) {
        Ok(n) => n,
        Err(e) => {
            writeln!(uart, "  list_root failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                keyboard,
                "BOOT FROM SD",
                &["FAILED TO READ ROOT DIRECTORY."],
            );
            return;
        }
    };

    writeln!(uart, "  root directory ({count} entries):").ok();
    y += line_height(2);
    fb.draw_text(CONTENT_X, y, "ROOT DIRECTORY:", 2, accent());
    y += line_height(2);
    for i in 0..count {
        let name = core::str::from_utf8(&names[i]).unwrap_or("????????.???");
        fb.draw_text(CONTENT_X + 20, y, name, 2, FG);
        fb.flush();
        writeln!(uart, "    {name}  {} bytes", sizes[i]).ok();
        if settings::get().verbose_boot {
            timer::sleep_ticks(LINE_DELAY_TICKS);
        }
        y += line_height(2);
    }
    if count == 0 {
        fb.draw_text(CONTENT_X + 20, y, "(EMPTY)", 2, DIM);
        y += line_height(2);
    }

    let efi_index = (0..count).find(|&i| &names[i][8..11] == b"EFI");

    if efi_index.is_none() && count > 0 {
        let mut content = [0u8; 256];
        match fs.read_file(&card, &names[0], &mut content) {
            Ok(n) => {
                let text = core::str::from_utf8(&content[..n]).unwrap_or("<binary>");
                writeln!(uart, "  first file contents ({n} bytes):").ok();
                writeln!(uart, "{text}").ok();
                y += line_height(2);
                fb.draw_text(CONTENT_X, y, "FIRST FILE (FULL TEXT OVER UART):", 2, DIM);
                y += line_height(2);
                fb.draw_text(CONTENT_X + 20, y, text.lines().next().unwrap_or(text), 2, accent());
            }
            Err(e) => {
                writeln!(uart, "  read_file failed: {e:?}").ok();
            }
        }
    }

    if let Some(i) = efi_index {
        let name = core::str::from_utf8(&names[i]).unwrap_or("????????.???");
        y += line_height(2);
        fb.draw_text(CONTENT_X, y, "EFI APPLICATION FOUND:", 2, accent());
        y += line_height(2);
        fb.draw_text(CONTENT_X + 20, y, name, 2, FG);
        fb.flush();
        writeln!(uart, "\n  EFI application found: {name}").ok();
        boot_efi_app(fb, uart, &fs, &card, &names[i], y);
    }

    fb.flush();
    wait_for_key(uart, keyboard);
}

/// Reads a `.EFI` file into a static load buffer and runs it through
/// the real EFI_BOOT_SERVICES table -- LoadImage (PE/COFF parse +
/// relocate), HandleProtocol (fetch EFI_LOADED_IMAGE_PROTOCOL back to
/// independently cross-check what LoadImage did), then StartImage
/// (call the entry point with (ImageHandle, SystemTable*) and get its
/// EFI_STATUS back).
fn boot_efi_app(fb: &Framebuffer, uart: &mut Uart, fs: &Fat32, card: &Card, name: &[u8; 11], mut y: u32) {
    use crate::efi::boot_services::BOOT_SERVICES;
    use crate::efi::protocols::{EfiLoadedImageProtocol, LOADED_IMAGE_PROTOCOL_GUID};
    use crate::efi::system_table::SYSTEM_TABLE;
    use crate::efi::types::{EfiHandle, EFI_SUCCESS};
    use core::ffi::c_void;

    static mut LOAD_BUFFER: [u8; 65536] = [0; 65536];

    let buf = unsafe { &mut *core::ptr::addr_of_mut!(LOAD_BUFFER) };
    let n = match fs.read_file(card, name, buf) {
        Ok(n) => n,
        Err(e) => {
            writeln!(uart, "  read_file failed: {e:?}").ok();
            fb.draw_text(CONTENT_X + 20, y, "FAILED TO READ FILE FROM SD.", 2, ERROR);
            return;
        }
    };
    writeln!(uart, "  read {n} bytes from SD").ok();

    let bs = unsafe { &*core::ptr::addr_of!(BOOT_SERVICES) };

    let mut image_handle: EfiHandle = core::ptr::null_mut();
    let status = (bs.load_image)(
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        buf.as_mut_ptr() as *mut c_void,
        n,
        &mut image_handle,
    );
    writeln!(uart, "  LoadImage -> status=0x{status:x} handle={image_handle:p}").ok();
    y += line_height(2);
    if status != EFI_SUCCESS {
        fb.draw_text(CONTENT_X + 20, y, "LOADIMAGE FAILED (SEE UART).", 2, ERROR);
        return;
    }
    fb.draw_text(CONTENT_X + 20, y, "LOADIMAGE OK -- PE PARSED AND RELOCATED.", 2, FG);
    fb.flush();
    if settings::get().verbose_boot {
        timer::sleep_ticks(LINE_DELAY_TICKS);
    }

    // Cross-check: fetch EFI_LOADED_IMAGE_PROTOCOL back via
    // HandleProtocol, independent of whatever LoadImage told us
    // directly, to prove the protocol database round-trips correctly
    // for a real, freshly-loaded image.
    let mut iface: *mut c_void = core::ptr::null_mut();
    let status = (bs.handle_protocol)(image_handle, &LOADED_IMAGE_PROTOCOL_GUID, &mut iface);
    if status == EFI_SUCCESS && !iface.is_null() {
        let li = unsafe { &*(iface as *const EfiLoadedImageProtocol) };
        writeln!(
            uart,
            "  HandleProtocol(LOADED_IMAGE) -> base={:p} size={}",
            li.image_base, li.image_size
        )
        .ok();
    }

    writeln!(
        uart,
        "  SystemTable @ {:p} (should match S: printed by the app below)",
        core::ptr::addr_of!(SYSTEM_TABLE)
    )
    .ok();

    y += line_height(2);
    fb.draw_text(CONTENT_X + 20, y, "STARTING IMAGE (OUTPUT BELOW IS FROM THE APP ITSELF):", 2, DIM);
    fb.flush();

    writeln!(uart, "  --- entering StartImage ---").ok();
    let mut exit_data_size: usize = 0;
    let status = (bs.start_image)(image_handle, &mut exit_data_size, core::ptr::null_mut());
    writeln!(uart, "\n  --- back from StartImage: status=0x{status:x} ---").ok();
}

fn save_variables_to_sd(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>) {
    writeln!(uart, "\n[menu] SAVE VARIABLES TO SD selected").ok();
    draw_chrome(fb, "SAVE VARIABLES TO SD", "PLEASE WAIT...");
    let y = print_lines(
        fb,
        uart,
        CONTENT_X,
        CONTENT_Y,
        2,
        FG,
        &["INITIALIZING SD CARD (SDHCI)..."],
    );

    let card = match Card::init() {
        Ok(c) => c,
        Err(e) => {
            writeln!(uart, "  sd init failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                keyboard,
                "SAVE VARIABLES TO SD",
                &["SD CARD INIT FAILED.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    let y = print_lines(fb, uart, CONTENT_X, y, 2, FG, &["SD CARD READY."]);

    let fs = match Fat32::mount(&card) {
        Ok(fs) => fs,
        Err(e) => {
            writeln!(uart, "  fat32 mount failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                keyboard,
                "SAVE VARIABLES TO SD",
                &["NO FAT32 VOLUME FOUND.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    let y = print_lines(fb, uart, CONTENT_X, y, 2, FG, &["FAT32 VOLUME MOUNTED."]);
    crate::persist::set_context(card, fs);

    match crate::persist::save(&card, &fs) {
        Ok(bytes) => {
            writeln!(uart, "  wrote {bytes} bytes to FERRO.VAR in the root directory").ok();
            print_lines(
                fb,
                uart,
                CONTENT_X,
                y,
                2,
                FG,
                &["VARIABLES SAVED (INCLUDING SETTINGS).", "THEY'LL RELOAD NEXT TIME YOU BOOT FROM SD."],
            );
        }
        Err(e) => {
            writeln!(uart, "  save failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                keyboard,
                "SAVE VARIABLES TO SD",
                &["FAILED TO SAVE VARIABLES.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    }
    wait_for_key(uart, keyboard);
}

fn select(fb: &Framebuffer, uart: &mut Uart, keyboard: &mut Option<Keyboard>, s: &MenuState) {
    match s.selected {
        0 => boot_from_sd(fb, uart, keyboard),
        1 => run_settings(fb, uart, keyboard),
        2 => show_system_info(fb, uart, keyboard),
        3 => save_variables_to_sd(fb, uart, keyboard),
        4 => {
            writeln!(uart, "\n[menu] REBOOT selected").ok();
            show_message(fb, uart, keyboard, "REBOOTING", &["ASKING THE WATCHDOG FOR A RESET..."]);
            pm::reset();
        }
        _ => unreachable!(),
    }
    draw_menu(fb, s, keyboard);
}

/// Runs the boot menu forever (selecting Reboot is the only way out,
/// and that doesn't return either). Polls both the UART serial
/// console and, if USB is enabled and a HID keyboard was found, that
/// too -- either one drives the same menu through the same key
/// handling. Redraws periodically even without input so the status
/// panel's live fields (uptime) keep moving.
pub fn run(fb: &Framebuffer, uart: &mut Uart) -> ! {
    settings::init();

    let mut keyboard: Option<Keyboard> = if settings::get().usb_enabled {
        match crate::usb::init() {
            Ok(speed) => match hid::find_keyboard(speed) {
                Ok(kbd) => {
                    writeln!(uart, "\n[menu] USB HID keyboard connected").ok();
                    Some(kbd)
                }
                Err(e) => {
                    writeln!(uart, "\n[menu] no USB HID keyboard found ({e:?}) -- UART input only").ok();
                    None
                }
            },
            Err(e) => {
                writeln!(uart, "\n[menu] USB init failed ({e:?}) -- UART input only").ok();
                None
            }
        }
    } else {
        writeln!(uart, "\n[menu] USB disabled in settings -- UART input only").ok();
        None
    };

    let mut state = MenuState { selected: 0 };
    draw_menu(fb, &state, &keyboard);

    let mut input = InputState { esc: 0 };
    let mut last_draw = timer::ticks();
    loop {
        if let Some(key) = poll_menu_key(uart, &mut keyboard, &mut input) {
            match key {
                Key::Up => move_up(&mut state),
                Key::Down => move_down(&mut state),
                Key::Enter => select(fb, uart, &mut keyboard, &state),
                _ => continue,
            }
            draw_menu(fb, &state, &keyboard);
            last_draw = timer::ticks();
            continue;
        }

        if timer::ticks().wrapping_sub(last_draw) >= IDLE_REDRAW_TICKS {
            draw_menu(fb, &state, &keyboard);
            last_draw = timer::ticks();
        }
    }
}
