//! Splash screen and interactive boot menu.
//!
//! There's no USB HID driver yet, so navigation happens over the UART
//! serial console (arrow keys or j/k, Enter to select) while the menu
//! renders to the framebuffer at the same time -- a real dual-output
//! setup UI, just keyboard-less until USB exists.

use crate::fat32::Fat32;
use crate::font::GLYPH_WIDTH;
use crate::framebuffer::Framebuffer;
use crate::sd::Card;
use crate::uart::Uart;
use crate::{pm, timer};
use core::fmt::Write;

const BG: u32 = 0x0018_2028;
const FG: u32 = 0x00E0_E6EC;
const ACCENT: u32 = 0x00FF_A030;
const DIM: u32 = 0x0070_7880;

fn text_width(text: &str, scale: u32) -> u32 {
    text.chars().count() as u32 * (GLYPH_WIDTH + 1) * scale
}

/// A POST-style text log shown on the framebuffer before the branded
/// splash -- everything up to this point (EL drop, MMU, timer/IRQ,
/// framebuffer itself) only ever reached UART, since there was no
/// display yet to put it on.
pub fn boot_log(fb: &Framebuffer, lines: &[&str]) {
    fb.clear(BG);
    fb.draw_text(20, 20, "FERRO UEFI - BOOT LOG", 3, ACCENT);
    let mut y = 70;
    for line in lines {
        fb.draw_text(20, y, line, 2, FG);
        y += 26;
    }
    fb.flush();
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

const ITEMS: [&str; 3] = ["BOOT FROM SD", "SYSTEM INFO", "REBOOT"];

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
    fb.clear(BG);
    fb.draw_text(40, 40, "FERRO UEFI - BOOT MENU", 4, ACCENT);

    let mut y = 160;
    for (i, item) in ITEMS.iter().enumerate() {
        let color = if i == s.selected { ACCENT } else { FG };
        if i == s.selected {
            fb.draw_text(40, y, ">", 3, ACCENT);
        }
        fb.draw_text(90, y, item, 3, color);
        y += 50;
    }

    fb.draw_text(
        40,
        fb.height - 60,
        "J/K OR ARROWS: MOVE   ENTER: SELECT",
        2,
        DIM,
    );
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
    fb.clear(BG);
    fb.draw_text(40, 40, title, 4, ACCENT);
    let mut y = 130;
    for line in lines {
        fb.draw_text(40, y, line, 2, FG);
        writeln!(uart, "  {line}").ok();
        y += 30;
    }
    fb.draw_text(40, fb.height - 60, "PRESS ANY KEY TO RETURN", 2, DIM);
    fb.flush();
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

    show_message(
        fb,
        uart,
        "SYSTEM INFO",
        &[
            "BOARD: RASPBERRY PI 3 / BCM2837",
            "CPU: 4X CORTEX-A53",
            "MMU: ENABLED",
            "FULL DETAIL SENT OVER UART",
        ],
    );
}

fn boot_from_sd(fb: &Framebuffer, uart: &mut Uart) {
    writeln!(uart, "\n[menu] BOOT FROM SD selected").ok();
    fb.clear(BG);
    fb.draw_text(40, 40, "BOOT FROM SD", 4, ACCENT);
    fb.draw_text(40, 110, "INITIALIZING SD CARD (SDHCI)...", 2, FG);
    fb.flush();

    let card = match Card::init() {
        Ok(c) => c,
        Err(e) => {
            writeln!(uart, "  sd init failed: {e:?}").ok();
            show_message(
                fb,
                uart,
                "BOOT FROM SD",
                &["SD CARD INIT FAILED.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    writeln!(uart, "  sd card ready").ok();

    let fs = match Fat32::mount(&card) {
        Ok(fs) => fs,
        Err(e) => {
            writeln!(uart, "  fat32 mount failed: {e:?}").ok();
            show_message(
                fb,
                uart,
                "BOOT FROM SD",
                &["NO FAT32 VOLUME FOUND.", "SEE UART LOG FOR DETAILS."],
            );
            return;
        }
    };
    writeln!(uart, "  fat32 mounted").ok();

    let mut names = [[0u8; 11]; 8];
    let mut sizes = [0u32; 8];
    let count = match fs.list_root(&card, &mut names, &mut sizes) {
        Ok(n) => n,
        Err(e) => {
            writeln!(uart, "  list_root failed: {e:?}").ok();
            show_message(
                fb,
                uart,
                "BOOT FROM SD",
                &["FAILED TO READ ROOT DIRECTORY."],
            );
            return;
        }
    };

    writeln!(uart, "  root directory ({count} entries):").ok();
    fb.clear(BG);
    fb.draw_text(40, 40, "BOOT FROM SD - ROOT DIRECTORY", 3, ACCENT);
    let mut y = 110;
    for i in 0..count {
        let name = core::str::from_utf8(&names[i]).unwrap_or("????????.???");
        writeln!(uart, "    {name}  {} bytes", sizes[i]).ok();
        fb.draw_text(40, y, name, 2, FG);
        y += 26;
    }
    if count == 0 {
        fb.draw_text(40, y, "(EMPTY)", 2, DIM);
    }

    // Read the first entry's contents for real, as proof the data path
    // (not just directory metadata) works end to end.
    if count > 0 {
        let mut content = [0u8; 256];
        match fs.read_file(&card, &names[0], &mut content) {
            Ok(n) => {
                let text = core::str::from_utf8(&content[..n]).unwrap_or("<binary>");
                writeln!(uart, "  first file contents ({n} bytes):").ok();
                writeln!(uart, "{text}").ok();
                y += 40;
                fb.draw_text(40, y, "FIRST FILE (SEE UART FOR FULL CONTENTS):", 2, DIM);
                y += 26;
                fb.draw_text(40, y, text.lines().next().unwrap_or(text), 2, ACCENT);
            }
            Err(e) => {
                writeln!(uart, "  read_file failed: {e:?}").ok();
            }
        }
    }

    fb.draw_text(40, fb.height - 60, "PRESS ANY KEY TO RETURN", 2, DIM);
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
