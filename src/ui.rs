//! Splash screen and interactive boot menu.
//!
//! There's no USB HID driver yet, so navigation happens over the UART
//! serial console (arrow keys or j/k, Enter to select) while the menu
//! renders to the framebuffer at the same time -- a real dual-output
//! setup UI, just keyboard-less until USB exists.
//!
//! Every screen shares a common header/footer/border "chrome"
//! (`draw_chrome`) and prints its body text one line at a time with a
//! real delay and a live UART mirror (`print_lines`), rather than
//! blitting the whole screen at once -- closer to how real firmware
//! setup screens and POST logs actually read.

use crate::fat32::Fat32;
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::framebuffer::Framebuffer;
use crate::sd::Card;
use crate::uart::Uart;
use crate::{pm, timer};
use core::fmt::Write;

const BG: u32 = 0x0012_1620;
const HEADER_BG: u32 = 0x0020_2A38;
const SELECT_BG: u32 = 0x002A_3A4A;
const BORDER: u32 = 0x0035_4250;
const FG: u32 = 0x00E0_E6EC;
const ACCENT: u32 = 0x00FF_A030;
const DIM: u32 = 0x0070_7880;
const ERROR: u32 = 0x00FF_5040;

const HEADER_H: u32 = 50;
const FOOTER_H: u32 = 40;
const MARGIN: u32 = 20;
const CONTENT_X: u32 = 40;
const CONTENT_Y: u32 = HEADER_H + 35;

/// Ticks between lines when printing progressively -- short enough not
/// to feel sluggish, long enough to actually read as sequential rather
/// than instant.
const LINE_DELAY_TICKS: u64 = 6;

fn text_width(text: &str, scale: u32) -> u32 {
    text.chars().count() as u32 * (GLYPH_WIDTH + 1) * scale
}

fn line_height(scale: u32) -> u32 {
    (GLYPH_HEIGHT + 3) * scale
}

/// Shared screen chrome: header bar with the firmware name and a
/// per-screen title, a bordered content panel, and a footer bar with
/// a context hint. Every screen after the splash uses this so the UI
/// reads as one coherent piece of firmware rather than a pile of ad
/// hoc drawing calls.
fn draw_chrome(fb: &Framebuffer, title: &str, hint: &str) {
    fb.clear(BG);

    fb.fill_rect(0, 0, fb.width, HEADER_H, HEADER_BG);
    fb.draw_text(MARGIN, 16, "FERRO UEFI", 3, ACCENT);
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

/// Draws and UART-mirrors `lines` one at a time with a real delay
/// between them, returning the y coordinate just past the last line.
fn print_lines(
    fb: &Framebuffer,
    uart: &mut Uart,
    x: u32,
    y: u32,
    scale: u32,
    color: u32,
    lines: &[&str],
) -> u32 {
    let mut cy = y;
    for line in lines {
        fb.draw_text(x, cy, line, scale, color);
        fb.flush();
        writeln!(uart, "{line}").ok();
        timer::sleep_ticks(LINE_DELAY_TICKS);
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
    fb.draw_text(tx, fb.height / 2 - 60, title, title_scale, ACCENT);

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

const ITEMS: [MenuItem; 4] = [
    MenuItem {
        label: "BOOT FROM SD",
        description: "MOUNT THE SD CARD, LOAD SAVED VARIABLES, LIST WHAT'S ON IT",
    },
    MenuItem {
        label: "SYSTEM INFO",
        description: "CPU, TIMER, AND MMU STATE, READ LIVE FROM HARDWARE",
    },
    MenuItem {
        label: "SAVE VARIABLES TO SD",
        description: "WRITE UEFI VARIABLES TO THE SD CARD'S RESERVED SECTORS",
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

fn draw_menu(fb: &Framebuffer, s: &MenuState) {
    draw_chrome(fb, "BOOT MANAGER", "ARROWS OR J/K: MOVE    ENTER: SELECT");

    let row_h = 46;
    let mut y = CONTENT_Y + 10;
    for (i, item) in ITEMS.iter().enumerate() {
        if i == s.selected {
            fb.fill_rect(CONTENT_X, y - 8, fb.width - 2 * CONTENT_X, row_h, SELECT_BG);
            fb.fill_rect(CONTENT_X, y - 8, 4, row_h, ACCENT);
        }
        let color = if i == s.selected { ACCENT } else { FG };
        fb.draw_text(CONTENT_X + 30, y, item.label, 3, color);
        y += row_h + 8;
    }

    let desc_y = fb.height - FOOTER_H - 34;
    fb.draw_text(CONTENT_X, desc_y, ITEMS[s.selected].description, 2, DIM);

    fb.flush();
}

fn wait_for_key(uart: &mut Uart) {
    loop {
        if uart.getc().is_some() {
            return;
        }
    }
}

fn show_message(fb: &Framebuffer, uart: &mut Uart, title: &str, lines: &[&str]) {
    draw_chrome(fb, title, "PRESS ANY KEY TO RETURN");
    print_lines(fb, uart, CONTENT_X, CONTENT_Y, 2, FG, lines);
    wait_for_key(uart);
}

fn show_error(fb: &Framebuffer, uart: &mut Uart, title: &str, lines: &[&str]) {
    draw_chrome(fb, title, "PRESS ANY KEY TO RETURN");
    print_lines(fb, uart, CONTENT_X, CONTENT_Y, 2, ERROR, lines);
    wait_for_key(uart);
}

fn show_system_info(fb: &Framebuffer, uart: &mut Uart) {
    let midr: u64;
    let freq: u64;
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {0}, midr_el1", out(reg) midr);
        core::arch::asm!("mrs {0}, cntfrq_el0", out(reg) freq);
        core::arch::asm!("mrs {0}, CurrentEL", out(reg) el);
    }
    let el = (el >> 2) & 0x3;

    writeln!(uart, "\n[menu] SYSTEM INFO selected").ok();
    writeln!(uart, "  board : Raspberry Pi 3 / BCM2837").ok();
    writeln!(uart, "  cpu   : 4x Cortex-A53, MIDR_EL1 = 0x{midr:08x}").ok();
    writeln!(uart, "  EL    : {el}").ok();
    writeln!(
        uart,
        "  timer : CNTFRQ_EL0 = {freq} Hz, {} ticks since boot",
        timer::ticks()
    )
    .ok();
    writeln!(uart, "  mmu   : enabled (identity-mapped RAM + device regions)").ok();

    draw_chrome(fb, "SYSTEM INFO", "PRESS ANY KEY TO RETURN");
    print_lines(
        fb,
        uart,
        CONTENT_X,
        CONTENT_Y,
        2,
        FG,
        &[
            "BOARD: RASPBERRY PI 3 / BCM2837",
            "CPU: 4X CORTEX-A53",
            "MMU: ENABLED (IDENTITY-MAPPED)",
            "TIMER: BCM2836 LOCAL BLOCK (NOT A GIC)",
            "",
            "FULL REGISTER DUMP SENT OVER UART",
        ],
    );
    wait_for_key(uart);
}

fn boot_from_sd(fb: &Framebuffer, uart: &mut Uart) {
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
                "BOOT FROM SD",
                &["NO FAT32 VOLUME FOUND.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    let mut y = print_lines(fb, uart, CONTENT_X, y, 2, FG, &["FAT32 VOLUME MOUNTED."]);

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
                "BOOT FROM SD",
                &["FAILED TO READ ROOT DIRECTORY."],
            );
            return;
        }
    };

    writeln!(uart, "  root directory ({count} entries):").ok();
    y += line_height(2);
    fb.draw_text(CONTENT_X, y, "ROOT DIRECTORY:", 2, ACCENT);
    y += line_height(2);
    for i in 0..count {
        let name = core::str::from_utf8(&names[i]).unwrap_or("????????.???");
        fb.draw_text(CONTENT_X + 20, y, name, 2, FG);
        fb.flush();
        writeln!(uart, "    {name}  {} bytes", sizes[i]).ok();
        timer::sleep_ticks(LINE_DELAY_TICKS);
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
                fb.draw_text(CONTENT_X + 20, y, text.lines().next().unwrap_or(text), 2, ACCENT);
            }
            Err(e) => {
                writeln!(uart, "  read_file failed: {e:?}").ok();
            }
        }
    }

    if let Some(i) = efi_index {
        let name = core::str::from_utf8(&names[i]).unwrap_or("????????.???");
        y += line_height(2);
        fb.draw_text(CONTENT_X, y, "EFI APPLICATION FOUND:", 2, ACCENT);
        y += line_height(2);
        fb.draw_text(CONTENT_X + 20, y, name, 2, FG);
        fb.flush();
        writeln!(uart, "\n  EFI application found: {name}").ok();
        boot_efi_app(fb, uart, &fs, &card, &names[i], y);
    }

    fb.flush();
    wait_for_key(uart);
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
    timer::sleep_ticks(LINE_DELAY_TICKS);

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

fn save_variables_to_sd(fb: &Framebuffer, uart: &mut Uart) {
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
                "SAVE VARIABLES TO SD",
                &["NO FAT32 VOLUME FOUND.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    let y = print_lines(fb, uart, CONTENT_X, y, 2, FG, &["FAT32 VOLUME MOUNTED."]);

    match crate::persist::save(&card, &fs) {
        Ok(bytes) => {
            writeln!(uart, "  wrote {bytes} bytes to the reserved-sector scratch region").ok();
            print_lines(
                fb,
                uart,
                CONTENT_X,
                y,
                2,
                FG,
                &["VARIABLES SAVED.", "THEY'LL RELOAD NEXT TIME YOU BOOT FROM SD."],
            );
        }
        Err(e) => {
            writeln!(uart, "  save failed: {e:?}").ok();
            show_error(
                fb,
                uart,
                "SAVE VARIABLES TO SD",
                &["FAILED TO SAVE VARIABLES.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    }
    wait_for_key(uart);
}

fn select(fb: &Framebuffer, uart: &mut Uart, s: &MenuState) {
    match s.selected {
        0 => boot_from_sd(fb, uart),
        1 => show_system_info(fb, uart),
        2 => save_variables_to_sd(fb, uart),
        3 => {
            writeln!(uart, "\n[menu] REBOOT selected").ok();
            show_message(fb, uart, "REBOOTING", &["ASKING THE WATCHDOG FOR A RESET..."]);
            pm::reset();
        }
        _ => unreachable!(),
    }
    draw_menu(fb, s);
}

/// Tracks ANSI-escape parsing state across calls, since arrow keys
/// arrive as a 3-byte sequence (ESC '[' 'A'/'B') that may be fed in
/// one byte at a time -- from real UART bytes, or from USB HID
/// keycodes translated into the same sequence (see hid.rs).
struct InputState {
    esc: u8, // 0 = idle, 1 = saw ESC, 2 = saw ESC '['
}

/// Feeds one input byte through the menu's key handling -- shared by
/// both the UART path and the USB HID path (translated keycodes are
/// fed through this same byte vocabulary, see hid::keycode_to_menu_bytes),
/// so there's exactly one place that knows what a keypress means.
fn handle_byte(b: u8, input: &mut InputState, fb: &Framebuffer, uart: &mut Uart, state: &mut MenuState) {
    match input.esc {
        0 if b == 0x1B => input.esc = 1,
        1 if b == b'[' => input.esc = 2,
        2 => {
            input.esc = 0;
            match b {
                b'A' => {
                    move_up(state);
                    draw_menu(fb, state);
                }
                b'B' => {
                    move_down(state);
                    draw_menu(fb, state);
                }
                _ => {}
            }
        }
        _ => {
            input.esc = 0;
            match b {
                b'k' | b'K' => {
                    move_up(state);
                    draw_menu(fb, state);
                }
                b'j' | b'J' => {
                    move_down(state);
                    draw_menu(fb, state);
                }
                b'\r' | b'\n' => select(fb, uart, state),
                _ => {}
            }
        }
    }
}

/// Runs the boot menu forever (selecting Reboot is the only way out,
/// and that doesn't return either). Polls both the UART serial
/// console and, if a USB HID keyboard was found, that too -- either
/// one drives the same menu through the same key handling.
pub fn run(fb: &Framebuffer, uart: &mut Uart) -> ! {
    let mut state = MenuState { selected: 0 };
    draw_menu(fb, &state);

    let mut keyboard = match crate::usb::init() {
        Ok(speed) => match crate::hid::find_keyboard(speed) {
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
    };

    let mut input = InputState { esc: 0 };
    loop {
        if let Some(b) = uart.getc() {
            handle_byte(b, &mut input, fb, uart, &mut state);
            continue;
        }

        if let Some(kbd) = keyboard.as_mut() {
            if let Ok(keys) = crate::hid::poll_new_keys(kbd) {
                for &code in keys.iter() {
                    if code == 0 {
                        continue;
                    }
                    let mut bytes = [0u8; 3];
                    let n = crate::hid::keycode_to_menu_bytes(code, &mut bytes);
                    for &b in &bytes[..n] {
                        handle_byte(b, &mut input, fb, uart, &mut state);
                    }
                }
            }
        }
    }
}
