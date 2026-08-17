//! SD card driver for the BCM2837's SDHCI (Arasan-compatible) host
//! controller, using the standard SD Host Controller Simplified
//! Specification register interface -- not Broadcom-proprietary.
//!
//! Real-hardware caveat: on physical Pi 3 boards, this SDHCI
//! controller's pins are wired only to the onboard WiFi chip: the
//! external microSD card slot is wired to the separate, proprietary
//! `sdhost` controller instead (this is Raspberry Pi's own default
//! device-tree wiring for the Pi 3). QEMU's `raspi3b` machine, however,
//! only attaches a `-drive if=sd` image to this SDHCI controller --
//! its `bcm2835-sdhost` model exists but is left with no card attached
//! and no straightforward way to attach one from the command line.
//! This driver is therefore fully real and fully verified end-to-end
//! in QEMU, but **will not read the boot SD card on physical Pi 3
//! hardware** without a further sdhost-specific driver -- an honest
//! gap, not a hidden one, tracked the same way as pm.rs's reset gap.

use crate::mmio::{self, PERIPHERAL_BASE};
use crate::timer;

const SD_BASE: usize = PERIPHERAL_BASE + 0x0030_0000;

#[allow(dead_code)] // documents the register map; no DMA support, PIO only
const REG_ARG2_OR_SDMA: usize = SD_BASE + 0x00;
const REG_BLOCKSIZE_COUNT: usize = SD_BASE + 0x04;
const REG_ARG1: usize = SD_BASE + 0x08;
const REG_TRANSFER_CMD: usize = SD_BASE + 0x0C;
const REG_RESP0: usize = SD_BASE + 0x10;
const REG_RESP1: usize = SD_BASE + 0x14;
const REG_RESP2: usize = SD_BASE + 0x18;
const REG_RESP3: usize = SD_BASE + 0x1C;
const REG_BUFFER: usize = SD_BASE + 0x20;
const REG_PRESENT_STATE: usize = SD_BASE + 0x24;
const REG_CLOCK_TIMEOUT_RESET: usize = SD_BASE + 0x2C;
const REG_INT_STATUS: usize = SD_BASE + 0x30;
const REG_INT_STATUS_ENABLE: usize = SD_BASE + 0x34;
const REG_CAPABILITIES: usize = SD_BASE + 0x40;

const INT_CMD_COMPLETE: u32 = 1 << 0;
const INT_TRANSFER_COMPLETE: u32 = 1 << 1;
const INT_BUFFER_WRITE_READY: u32 = 1 << 4;
const INT_BUFFER_READ_READY: u32 = 1 << 5;
const INT_ERROR_MASK: u32 = 0xFFFF_0000;

const PRESENT_CMD_INHIBIT: u32 = 1 << 0;
const PRESENT_DAT_INHIBIT: u32 = 1 << 1;

#[derive(Debug)]
pub enum SdError {
    Timeout,
    Error(#[allow(dead_code)] u32), // raw error interrupt status bits, read via Debug logging
    NoCard,
}

enum RespType {
    None,
    R2,
    R1, // covers R1/R1b/R3/R6/R7 -- same 48-bit-response wire format
}

#[derive(Clone, Copy, PartialEq)]
enum DataDir {
    None,
    Read,
    Write,
}

fn wait_while(timeout_ticks: u64, mut cond: impl FnMut() -> bool) -> Result<(), SdError> {
    let deadline = timer::ticks() + timeout_ticks;
    while cond() {
        if timer::ticks() > deadline {
            return Err(SdError::Timeout);
        }
    }
    Ok(())
}

fn reset_all() -> Result<(), SdError> {
    unsafe { mmio::write(REG_CLOCK_TIMEOUT_RESET, 1 << 24) }; // RSTA
    wait_while(100, || unsafe { mmio::read(REG_CLOCK_TIMEOUT_RESET) } & (1 << 24) != 0)
}

/// Base clock frequency (Hz) from Capabilities[13:8] (MHz) -- read at
/// runtime rather than assumed, since it's whatever the host
/// controller actually reports.
fn base_clock_hz() -> u64 {
    let caps = unsafe { mmio::read(REG_CAPABILITIES) };
    let mhz = (caps >> 8) & 0x3F;
    mhz as u64 * 1_000_000
}

/// Sets the SD clock to at most `target_hz`, rounding the divisor up.
fn set_clock(target_hz: u64) -> Result<(), SdError> {
    unsafe { mmio::write(REG_CLOCK_TIMEOUT_RESET, 0) }; // clock off while reprogramming

    let base = base_clock_hz().max(1);
    let mut divisor: u32 = 1;
    while base / (2 * divisor as u64) > target_hz {
        divisor += 1;
    }

    let clock_ctrl = (divisor << 8) | (1 << 0); // frequency select | internal clock enable
    unsafe { mmio::write(REG_CLOCK_TIMEOUT_RESET, clock_ctrl) };
    wait_while(1000, || unsafe { mmio::read(REG_CLOCK_TIMEOUT_RESET) } & (1 << 1) == 0)?; // internal clock stable

    let with_sd_clock = clock_ctrl | (1 << 2); // SD clock enable
    unsafe { mmio::write(REG_CLOCK_TIMEOUT_RESET, with_sd_clock) };
    Ok(())
}

fn send_command(index: u8, arg: u32, resp: RespType, data: DataDir) -> Result<[u32; 4], SdError> {
    wait_while(200, || unsafe {
        mmio::read(REG_PRESENT_STATE) & (PRESENT_CMD_INHIBIT | PRESENT_DAT_INHIBIT) != 0
    })?;

    unsafe { mmio::write(REG_INT_STATUS, 0xFFFF_FFFF) }; // clear stale status (write-1-to-clear)
    unsafe { mmio::write(REG_ARG1, arg) };

    let resp_bits: u32 = match resp {
        RespType::None => 0b00,
        RespType::R2 => 0b01,
        RespType::R1 => 0b10,
    };
    // Command register occupies word bits [31:16]; within it, Command
    // Index is bits [13:8] and Data Present Select is bit 5 -- both
    // need the extra +16 to land in the right half of this 32-bit
    // register. (Transfer Mode, bits [15:0], is set separately below;
    // getting these halves crossed was the actual first bug here --
    // it left Data Present Select unset, so the controller never
    // expected a data phase and Buffer Read Ready never fired.)
    let has_data = data != DataDir::None;
    let command = ((index as u32) << 24) | (resp_bits << 16) | (if has_data { 1 << 21 } else { 0 });
    let transfer_mode: u32 = match data {
        DataDir::None => 0,
        DataDir::Read => (1 << 1) | (1 << 4), // Block Count Enable | Direction: card to host
        DataDir::Write => 1 << 1,             // Block Count Enable; Direction bit stays 0 = host to card
    };
    unsafe { mmio::write(REG_TRANSFER_CMD, command | transfer_mode) };

    wait_while(200, || unsafe {
        let status = mmio::read(REG_INT_STATUS);
        status & (INT_CMD_COMPLETE | INT_ERROR_MASK) == 0
    })?;

    let status = unsafe { mmio::read(REG_INT_STATUS) };
    if status & INT_ERROR_MASK != 0 {
        unsafe { mmio::write(REG_INT_STATUS, 0xFFFF_FFFF) };
        return Err(SdError::Error(status));
    }
    unsafe { mmio::write(REG_INT_STATUS, INT_CMD_COMPLETE) };

    Ok(unsafe {
        [
            mmio::read(REG_RESP0),
            mmio::read(REG_RESP1),
            mmio::read(REG_RESP2),
            mmio::read(REG_RESP3),
        ]
    })
}

pub struct Card {
    #[allow(dead_code)] // kept for future re-select/status use, not needed by read_block
    rca: u32,
    /// SDHC/SDXC address the card by 512-byte block number; older
    /// SDSC cards address by raw byte offset. Determined from OCR.CCS.
    block_addressed: bool,
}

impl Card {
    /// Runs the SD initialization sequence (CMD0/CMD8/ACMD41/CMD2/
    /// CMD3/CMD7) and leaves the card selected and ready for block
    /// reads. Assumes an SDHC/SDXC-class card (SD spec v2+), which is
    /// what QEMU's sd-card model presents and what essentially every
    /// real card sold today is.
    pub fn init() -> Result<Card, SdError> {
        reset_all()?;
        unsafe { mmio::write(REG_INT_STATUS_ENABLE, 0xFFFF_FFFF) };
        set_clock(400_000)?; // identification-speed clock first, per spec

        send_command(0, 0, RespType::None, DataDir::None)?; // CMD0: GO_IDLE_STATE

        // CMD8: SEND_IF_COND -- voltage 2.7-3.6V, check pattern 0xAA.
        let r7 = send_command(8, 0x1AA, RespType::R1, DataDir::None)?;
        if r7[0] & 0xFF != 0xAA {
            return Err(SdError::NoCard);
        }

        // ACMD41 (via CMD55) until the card reports ready (OCR busy
        // bit, bit31, set to 1 = not busy = ready).
        let deadline = timer::ticks() + 500; // ~5s at 100Hz
        let ocr = loop {
            send_command(55, 0, RespType::R1, DataDir::None)?; // APP_CMD
            let r3 = send_command(41, 0x40FF_8000, RespType::R1, DataDir::None)?; // HCS + voltage window
            if r3[0] & (1 << 31) != 0 {
                break r3[0];
            }
            if timer::ticks() > deadline {
                return Err(SdError::Timeout);
            }
        };
        let block_addressed = ocr & (1 << 30) != 0; // CCS

        send_command(2, 0, RespType::R2, DataDir::None)?; // CMD2: ALL_SEND_CID
        let r6 = send_command(3, 0, RespType::R1, DataDir::None)?; // CMD3: SEND_RELATIVE_ADDR
        let rca = r6[0] & 0xFFFF_0000;

        set_clock(25_000_000)?; // up to default-speed clock now identification is done

        send_command(7, rca, RespType::R1, DataDir::None)?; // CMD7: SELECT_CARD

        Ok(Card { rca, block_addressed })
    }

    /// Reads one 512-byte block. `lba` is a block number regardless
    /// of the card's actual addressing mode -- the byte-offset
    /// conversion for SDSC cards happens internally.
    pub fn read_block(&self, lba: u32, out: &mut [u8; 512]) -> Result<(), SdError> {
        let addr = if self.block_addressed { lba } else { lba * 512 };

        unsafe { mmio::write(REG_BLOCKSIZE_COUNT, 512 | (1 << 16)) }; // 512 bytes, 1 block
        send_command(17, addr, RespType::R1, DataDir::Read)?; // CMD17: READ_SINGLE_BLOCK

        wait_while(200, || unsafe {
            let status = mmio::read(REG_INT_STATUS);
            status & (INT_BUFFER_READ_READY | INT_ERROR_MASK) == 0
        })?;
        let status = unsafe { mmio::read(REG_INT_STATUS) };
        if status & INT_ERROR_MASK != 0 {
            unsafe { mmio::write(REG_INT_STATUS, 0xFFFF_FFFF) };
            return Err(SdError::Error(status));
        }
        unsafe { mmio::write(REG_INT_STATUS, INT_BUFFER_READ_READY) };

        for chunk in out.chunks_exact_mut(4) {
            let word = unsafe { mmio::read(REG_BUFFER) };
            chunk.copy_from_slice(&word.to_le_bytes());
        }

        wait_while(200, || unsafe {
            mmio::read(REG_INT_STATUS) & (INT_TRANSFER_COMPLETE | INT_ERROR_MASK) == 0
        })?;
        unsafe { mmio::write(REG_INT_STATUS, INT_TRANSFER_COMPLETE) };

        Ok(())
    }

    /// Writes one 512-byte block. Same addressing rules as
    /// read_block: `lba` is always a block number, converted to a
    /// byte offset internally for SDSC cards.
    pub fn write_block(&self, lba: u32, data: &[u8; 512]) -> Result<(), SdError> {
        let addr = if self.block_addressed { lba } else { lba * 512 };

        unsafe { mmio::write(REG_BLOCKSIZE_COUNT, 512 | (1 << 16)) }; // 512 bytes, 1 block
        send_command(24, addr, RespType::R1, DataDir::Write)?; // CMD24: WRITE_BLOCK

        wait_while(200, || unsafe {
            let status = mmio::read(REG_INT_STATUS);
            status & (INT_BUFFER_WRITE_READY | INT_ERROR_MASK) == 0
        })?;
        let status = unsafe { mmio::read(REG_INT_STATUS) };
        if status & INT_ERROR_MASK != 0 {
            unsafe { mmio::write(REG_INT_STATUS, 0xFFFF_FFFF) };
            return Err(SdError::Error(status));
        }
        unsafe { mmio::write(REG_INT_STATUS, INT_BUFFER_WRITE_READY) };

        for chunk in data.chunks_exact(4) {
            let word = u32::from_le_bytes(chunk.try_into().unwrap());
            unsafe { mmio::write(REG_BUFFER, word) };
        }

        wait_while(200, || unsafe {
            mmio::read(REG_INT_STATUS) & (INT_TRANSFER_COMPLETE | INT_ERROR_MASK) == 0
        })?;
        unsafe { mmio::write(REG_INT_STATUS, INT_TRANSFER_COMPLETE) };

        Ok(())
    }
}

