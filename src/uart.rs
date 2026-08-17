//! PL011 UART0 driver for BCM2837, plus the GPIO alt-function setup it
//! needs (pins 14/15 -> TXD0/RXD0). This is the firmware's only output
//! device at this stage of bring-up.

use crate::mmio::{self, GPIO_BASE, UART0_BASE};

const GPFSEL1: usize = GPIO_BASE + 0x04;
const GPPUD: usize = GPIO_BASE + 0x94;
const GPPUDCLK0: usize = GPIO_BASE + 0x98;

const UART_DR: usize = UART0_BASE + 0x00;
const UART_FR: usize = UART0_BASE + 0x18;
const UART_IBRD: usize = UART0_BASE + 0x24;
const UART_FBRD: usize = UART0_BASE + 0x28;
const UART_LCRH: usize = UART0_BASE + 0x2C;
const UART_CR: usize = UART0_BASE + 0x30;
const UART_ICR: usize = UART0_BASE + 0x44;

const FR_TXFF: u32 = 1 << 5;
const FR_RXFE: u32 = 1 << 4;

pub struct Uart;

impl Uart {
    /// Configure GPIO 14/15 as UART0 TXD0/RXD0 and bring up PL011 at
    /// 115200 8N1, assuming the standard 48 MHz UART clock.
    pub fn init() -> Self {
        unsafe {
            // GPFSEL1: FSEL14 (bits 12-14) and FSEL15 (bits 15-17) = ALT0 (100).
            let mut sel = mmio::read(GPFSEL1);
            sel &= !((0b111 << 12) | (0b111 << 15));
            sel |= (0b100 << 12) | (0b100 << 15);
            mmio::write(GPFSEL1, sel);

            // Disable pull-up/down on pins 14/15 (BCM2837 legacy sequence).
            mmio::write(GPPUD, 0);
            for _ in 0..150 {
                core::arch::asm!("nop");
            }
            mmio::write(GPPUDCLK0, (1 << 14) | (1 << 15));
            for _ in 0..150 {
                core::arch::asm!("nop");
            }
            mmio::write(GPPUDCLK0, 0);

            mmio::write(UART_CR, 0); // disable UART
            mmio::write(UART_ICR, 0x7FF); // clear pending interrupts
            mmio::write(UART_IBRD, 26); // 48MHz / (16 * 115200) = 26.04
            mmio::write(UART_FBRD, 3); // fractional part ~= 3/64
            mmio::write(UART_LCRH, (1 << 4) | (0b11 << 5)); // FIFOs enabled, 8N1
            mmio::write(UART_CR, (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN, TXE, RXE
        }
        Uart
    }

    pub fn putc(&mut self, c: u8) {
        unsafe {
            while mmio::read(UART_FR) & FR_TXFF != 0 {}
            mmio::write(UART_DR, c as u32);
        }
    }

    /// Non-blocking receive: `None` if nothing's waiting.
    pub fn getc(&mut self) -> Option<u8> {
        unsafe {
            if mmio::read(UART_FR) & FR_RXFE != 0 {
                None
            } else {
                Some(mmio::read(UART_DR) as u8)
            }
        }
    }

    pub fn puts(&mut self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.putc(b'\r');
            }
            self.putc(byte);
        }
    }
}

impl core::fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.puts(s);
        Ok(())
    }
}
