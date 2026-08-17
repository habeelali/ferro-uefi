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

const ITEMS: [MenuItem; 3] = [
    MenuItem {
        label: "BOOT FROM SD",
        description: "MOUNT THE SD CARD AND LIST WHAT'S ON IT",
    },
    MenuItem {
        label: "SYSTEM INFO",
        description: "CPU, TIMER, AND MMU STATE, READ LIVE FROM HARDWARE",
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

    if count > 0 {
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

    fb.flush();
    wait_for_key(uart);
}

fn select(fb: &Framebuffer, uart: &mut Uart, s: &MenuState) {
    match s.selected {
        0 => boot_from_sd(fb, uart),
        1 => show_system_info(fb, uart),
        2 => {
            writeln!(uart, "\n[menu] REBOOT selected").ok();
            show_message(fb, uart, "REBOOTING", &["ASKING THE WATCHDOG FOR A RESET..."]);
            pm::reset();
        }
        _ => unreachable!(),
    }
    draw_menu(fb, s);
}

/// Runs the boot menu forever (selecting Reboot is the only way out,
/// and that doesn't return either).
pub fn run(fb: &Framebuffer, uart: &mut Uart) -> ! {
    let mut state = MenuState { selected: 0 };
    draw_menu(fb, &state);

    let mut esc_state = 0u8; // 0 = idle, 1 = saw ESC, 2 = saw ESC '['
    loop {
        let Some(b) = uart.getc() else { continue };
        match esc_state {
            0 if b == 0x1B => esc_state = 1,
            1 if b == b'[' => esc_state = 2,
            2 => {
                esc_state = 0;
                match b {
                    b'A' => {
                        move_up(&mut state);
                        draw_menu(fb, &state);
                    }
                    b'B' => {
                        move_down(&mut state);
                        draw_menu(fb, &state);
                    }
                    _ => {}
                }
            }
            _ => {
                esc_state = 0;
                match b {
                    b'k' | b'K' => {
                        move_up(&mut state);
                        draw_menu(fb, &state);
                    }
                    b'j' | b'J' => {
                        move_down(&mut state);
                        draw_menu(fb, &state);
                    }
                    b'\r' | b'\n' => select(fb, uart, &state),
                    _ => {}
                }
            }
        }
    }
}
