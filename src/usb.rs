//! USB host driver for the BCM2837's DesignWare Hi-Speed USB 2.0
//! On-The-Go (dwc2) controller. DMA mode, polled -- no interrupts,
//! matching the rest of Ferro's driver style. (QEMU's `dwc2` model
//! only implements DMA-mode transfers -- its channel logic reads/
//! writes guest memory directly via the HCDMA register and doesn't
//! look at the FIFO push/pop registers at all, confirmed against
//! QEMU's own hw/usb/hcd-dwc2.c -- so slave/FIFO mode was never going
//! to work against it regardless of how correct the FIFO code was.)
//!
//! First slice: core reset, forcing host mode, root port power-on and
//! reset, and one control transfer (GET_DESCRIPTOR) to prove a real
//! device on the other end responds. Not yet: multi-channel
//! scheduling, bulk/interrupt transfers, or a HID class driver -- see
//! the README for what's still ahead.

use crate::cache;
use crate::mmio::{self, PERIPHERAL_BASE};
use crate::timer;

const USB_BASE: usize = PERIPHERAL_BASE + 0x0098_0000;

#[allow(dead_code)] // documents the register map; OTG-specific, not needed in plain host mode
const GOTGCTL: usize = USB_BASE + 0x000;
const GAHBCFG: usize = USB_BASE + 0x008;
const GUSBCFG: usize = USB_BASE + 0x00C;
const GRSTCTL: usize = USB_BASE + 0x010;
#[allow(dead_code)] // documents the register map; unused now that data moves via DMA, not the RxFIFO
const GINTSTS: usize = USB_BASE + 0x014;
#[allow(dead_code)]
const GRXSTSP: usize = USB_BASE + 0x020;
#[allow(dead_code)] // documents the register map; core reset default FIFO sizing is fine so far
const GRXFSIZ: usize = USB_BASE + 0x024;
#[allow(dead_code)]
const GNPTXFSIZ: usize = USB_BASE + 0x028;
const GSNPSID: usize = USB_BASE + 0x040;

const HCFG: usize = USB_BASE + 0x400;
const HPRT: usize = USB_BASE + 0x440;
const HCCHAR0: usize = USB_BASE + 0x500;
const HCINT0: usize = USB_BASE + 0x508;
#[allow(dead_code)] // documents the register map; unused since we poll HCINT0 directly, no IRQs
const HCINTMSK0: usize = USB_BASE + 0x50C;
const HCTSIZ0: usize = USB_BASE + 0x510;
const HCDMA0: usize = USB_BASE + 0x514;

const GRSTCTL_CSFTRST: u32 = 1 << 0;
const GRSTCTL_TXFFLSH: u32 = 1 << 5;
const GRSTCTL_RXFFLSH: u32 = 1 << 4;
const GRSTCTL_AHBIDLE: u32 = 1 << 31;

const GAHBCFG_DMA_EN: u32 = 1 << 5;

const GUSBCFG_FORCE_HOST: u32 = 1 << 29;
const GUSBCFG_FORCE_DEVICE: u32 = 1 << 30;

const HPRT_CONN_STS: u32 = 1 << 0;
const HPRT_CONN_DET: u32 = 1 << 1;
#[allow(dead_code)] // documents the register map; not yet checked, only cleared as a W1C bit
const HPRT_ENA: u32 = 1 << 2;
const HPRT_ENA_CHNG: u32 = 1 << 3;
const HPRT_RST: u32 = 1 << 8;
const HPRT_PWR: u32 = 1 << 12;
const HPRT_SPD_SHIFT: u32 = 17;
const HPRT_SPD_MASK: u32 = 0b11 << HPRT_SPD_SHIFT;

/// Speed values as reported in HPRT.PrtSpd / used in HCCHAR.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Speed {
    High,
    Full,
    Low,
}

#[derive(Debug)]
pub enum UsbError {
    Timeout,
    NoDevice,
    TransferError(#[allow(dead_code)] u32),
    Stall,
}

fn wait_while(timeout_ticks: u64, mut cond: impl FnMut() -> bool) -> Result<(), UsbError> {
    let deadline = timer::ticks() + timeout_ticks;
    while cond() {
        if timer::ticks() > deadline {
            return Err(UsbError::Timeout);
        }
    }
    Ok(())
}

/// Resets the dwc2 core, forces it into host mode, and enables DMA
/// (this device's channel logic only implements DMA-mode transfers --
/// see the module doc comment). Must run before anything else touches
/// the controller.
fn reset_core() -> Result<(), UsbError> {
    wait_while(200, || unsafe { mmio::read(GRSTCTL) } & GRSTCTL_AHBIDLE == 0)?;

    unsafe { mmio::write(GRSTCTL, GRSTCTL_CSFTRST) };
    wait_while(200, || unsafe { mmio::read(GRSTCTL) } & GRSTCTL_CSFTRST != 0)?;
    wait_while(200, || unsafe { mmio::read(GRSTCTL) } & GRSTCTL_AHBIDLE == 0)?;
    timer::sleep_ticks(5); // let the PHY settle, matches common driver practice

    let usbcfg = unsafe { mmio::read(GUSBCFG) };
    let usbcfg = (usbcfg | GUSBCFG_FORCE_HOST) & !GUSBCFG_FORCE_DEVICE;
    unsafe { mmio::write(GUSBCFG, usbcfg) };
    timer::sleep_ticks(3); // mode switch takes a moment to take effect

    let ahbcfg = unsafe { mmio::read(GAHBCFG) };
    unsafe { mmio::write(GAHBCFG, ahbcfg | GAHBCFG_DMA_EN) };

    Ok(())
}

/// Powers the root port on. Doesn't error if nothing's plugged in --
/// callers check via `port_connected()`.
fn power_on_port() {
    let mut hprt = unsafe { mmio::read(HPRT) };
    // HPRT has several write-1-to-clear bits (ConnDetect, EnaChng,
    // ...) mixed in with the fields we want to set; mask those off
    // before OR-ing in PrtPwr so we don't accidentally clear pending
    // status by writing 1 to them as a side effect of a read-modify-write.
    hprt &= !(HPRT_CONN_DET | HPRT_ENA_CHNG | (1 << 5) | (1 << 4));
    hprt |= HPRT_PWR;
    unsafe { mmio::write(HPRT, hprt) };
}

fn port_connected() -> bool {
    unsafe { mmio::read(HPRT) & HPRT_CONN_STS != 0 }
}

/// Issues a root port reset (required before a newly-connected device
/// will respond to anything) and reports the speed it came back at.
fn reset_port() -> Result<Speed, UsbError> {
    let mut hprt = unsafe { mmio::read(HPRT) };
    hprt &= !(HPRT_CONN_DET | HPRT_ENA_CHNG | (1 << 5) | (1 << 4));
    hprt |= HPRT_RST;
    unsafe { mmio::write(HPRT, hprt) };
    timer::sleep_ticks(6); // >= 50ms USB reset pulse (100Hz tick, 6 ticks ~= 60ms)

    let mut hprt = unsafe { mmio::read(HPRT) };
    hprt &= !(HPRT_RST | HPRT_CONN_DET | HPRT_ENA_CHNG | (1 << 5) | (1 << 4));
    unsafe { mmio::write(HPRT, hprt) };
    timer::sleep_ticks(2); // recovery time before the port is usable

    if !port_connected() {
        return Err(UsbError::NoDevice);
    }

    let speed_bits = (unsafe { mmio::read(HPRT) } & HPRT_SPD_MASK) >> HPRT_SPD_SHIFT;
    Ok(match speed_bits {
        0 => Speed::High,
        1 => Speed::Full,
        _ => Speed::Low,
    })
}

/// Brings the controller all the way up: core reset, host mode, DMA
/// enable, port power, port reset. Returns the connected device's
/// speed, or UsbError::NoDevice if nothing's plugged into the root port.
pub fn init() -> Result<Speed, UsbError> {
    reset_core()?;

    // HCFG.FSLSPclkSel: 01 = PHY clock is 48MHz (matches the internal
    // full/low-speed-capable PHY on this SoC).
    let hcfg = unsafe { mmio::read(HCFG) };
    unsafe { mmio::write(HCFG, (hcfg & !0b11) | 0b01) };

    // Flush both FIFOs (bit4 RxFFlsh, bit5 TxFFlsh w/ TxFNum=0x10 = all).
    // Not load-bearing in DMA mode, but harmless and keeps the core in
    // a clean state.
    unsafe { mmio::write(GRSTCTL, GRSTCTL_RXFFLSH | GRSTCTL_TXFFLSH | (0x10 << 6)) };
    wait_while(200, || unsafe {
        mmio::read(GRSTCTL) & (GRSTCTL_RXFFLSH | GRSTCTL_TXFFLSH) != 0
    })?;

    power_on_port();
    timer::sleep_ticks(10); // let a real device's power stabilize (VBUS settle)

    if !port_connected() {
        return Err(UsbError::NoDevice);
    }

    reset_port()
}

/// Diagnostic-only: the core's identification register, so a caller
/// can confirm we're actually talking to a dwc2 core and not reading
/// back all-zero/all-one garbage from a misconfigured address.
pub fn core_id() -> u32 {
    unsafe { mmio::read(GSNPSID) }
}

const HCINT_CHHLTD: u32 = 1 << 1;
const HCINT_STALL: u32 = 1 << 3;
const HCINT_NAK: u32 = 1 << 4;
const HCINT_XACTERR: u32 = 1 << 7;
const HCINT_ERROR_MASK: u32 = HCINT_STALL | HCINT_XACTERR | (1 << 2) | (1 << 10);

const EP_TYPE_CONTROL: u32 = 0b00;
const EP_TYPE_INTERRUPT: u32 = 0b11;

#[allow(dead_code)] // DATA0/DATA1 toggle pair; kept for future multi-packet interrupt polling
const PID_DATA0: u32 = 0b00;
const PID_DATA1: u32 = 0b10;
const PID_SETUP: u32 = 0b11;

/// One USB device's worth of addressing context: its assigned
/// address (0 until SET_ADDRESS), the max packet size of the endpoint
/// currently being talked to, and its speed. Only channel 0 is used --
/// one transaction in flight at a time, which is all a single-threaded
/// polling driver needs.
pub struct Device {
    pub addr: u32,
    pub max_packet_size: u32,
    pub low_speed: bool,
}

fn hcchar(ep_dir_in: bool, ep_num: u32, ep_type: u32, dev_addr: u32, mps: u32, low_speed: bool) -> u32 {
    (mps & 0x7FF)
        | ((ep_num & 0xF) << 11)
        | ((ep_dir_in as u32) << 15)
        | ((low_speed as u32) << 17)
        | (ep_type << 18)
        | (1 << 20) // MCnt = 1
        | ((dev_addr & 0x7F) << 22)
}

/// Runs one DMA-mode transaction on channel 0 and waits for it to
/// complete. `buf` is the data to send (OUT/SETUP) or the destination
/// to receive into (IN) -- either way, its physical address (identity-
/// mapped, so just its pointer) goes straight into HCDMA, since this
/// controller only implements DMA-mode channel transfers.
///
/// The buffer is CPU-cacheable RAM but gets written/read by something
/// that isn't our CPU (same situation as the GPU mailbox in
/// mailbox.rs), so it's cleaned/invalidated around the DMA exactly
/// like that code does.
#[allow(clippy::too_many_arguments)]
fn transact(
    dev: &Device,
    ep_num: u32,
    ep_type: u32,
    ep_dir_in: bool,
    pid: u32,
    buf: &mut [u8],
) -> Result<usize, UsbError> {
    let xfer_size = buf.len();
    let pkt_cnt = if xfer_size == 0 {
        1
    } else {
        (xfer_size + dev.max_packet_size as usize - 1) / dev.max_packet_size as usize
    };

    let addr = buf.as_mut_ptr() as usize;
    if !ep_dir_in {
        // OUT/SETUP: make sure what we wrote is actually in RAM, not
        // just sitting in our D-cache, before the "DMA engine" reads it.
        cache::clean_and_invalidate_range(addr, xfer_size.max(1));
    }

    unsafe {
        mmio::write(HCDMA0, addr as u32);
        mmio::write(
            HCTSIZ0,
            (xfer_size as u32 & 0x7FFFF) | ((pkt_cnt as u32 & 0x3FF) << 19) | (pid << 29),
        );
        mmio::write(HCINT0, 0xFFFF_FFFF); // clear stale status
        mmio::write(
            HCCHAR0,
            hcchar(ep_dir_in, ep_num, ep_type, dev.addr, dev.max_packet_size, dev.low_speed) | (1 << 31), // ChEna
        );
    }

    wait_while(300, || unsafe { mmio::read(HCINT0) & (HCINT_CHHLTD | HCINT_ERROR_MASK) == 0 })?;

    let hcint = unsafe { mmio::read(HCINT0) };
    unsafe { mmio::write(HCINT0, 0xFFFF_FFFF) };

    if hcint & HCINT_STALL != 0 {
        return Err(UsbError::Stall);
    }
    if hcint & (HCINT_XACTERR | (1 << 2) | (1 << 10)) != 0 {
        return Err(UsbError::TransferError(hcint));
    }
    if hcint & HCINT_NAK != 0 {
        return Err(UsbError::TransferError(hcint));
    }

    // Remaining XferSize tells us how many bytes weren't transferred;
    // the rest of the original request size is what actually moved.
    let remaining = (unsafe { mmio::read(HCTSIZ0) } & 0x7FFFF) as usize;
    let actual = xfer_size.saturating_sub(remaining);

    if ep_dir_in && actual > 0 {
        // The "DMA engine" wrote straight to RAM; drop any stale
        // cached copy so we read what actually arrived.
        cache::clean_and_invalidate_range(addr, actual);
    }

    Ok(actual)
}

/// One standard/class control transfer (SETUP, optional data stage,
/// status stage) against `dev`'s control endpoint (0). `buf` is
/// filled with response data for an IN request, or holds the data to
/// send for an OUT request; empty for a no-data request. Returns the
/// byte count actually transferred during the data stage.
fn control_transfer(
    dev: &Device,
    bm_request_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    buf: &mut [u8],
) -> Result<usize, UsbError> {
    let dir_in = bm_request_type & 0x80 != 0;
    let mut setup: [u8; 8] = [
        bm_request_type,
        b_request,
        (w_value & 0xFF) as u8,
        (w_value >> 8) as u8,
        (w_index & 0xFF) as u8,
        (w_index >> 8) as u8,
        (buf.len() as u16 & 0xFF) as u8,
        ((buf.len() as u16) >> 8) as u8,
    ];

    transact(dev, 0, EP_TYPE_CONTROL, false, PID_SETUP, &mut setup)?;

    let n = if buf.is_empty() {
        0
    } else if dir_in {
        transact(dev, 0, EP_TYPE_CONTROL, true, PID_DATA1, buf)?
    } else {
        transact(dev, 0, EP_TYPE_CONTROL, false, PID_DATA1, buf)?
    };

    // Status stage is the opposite direction of the data stage; with
    // no data stage at all (buf empty), status is always IN.
    let status_dir_in = buf.is_empty() || !dir_in;
    transact(dev, 0, EP_TYPE_CONTROL, status_dir_in, PID_DATA1, &mut [])?;

    Ok(n)
}

const REQ_GET_STATUS: u8 = 0x00;
const REQ_CLEAR_FEATURE: u8 = 0x01;
const REQ_SET_FEATURE: u8 = 0x03;
const REQ_SET_ADDRESS: u8 = 0x05;
const REQ_GET_DESCRIPTOR: u8 = 0x06;
const REQ_SET_CONFIGURATION: u8 = 0x09;

const DESC_TYPE_DEVICE: u16 = 1;
const DESC_TYPE_CONFIGURATION: u16 = 2;
const DESC_TYPE_HUB: u16 = 0x29;

/// GET_DESCRIPTOR(DEVICE). Spec allows a short read on the very first
/// request (`out.len() < 18`), before the host knows the real
/// bMaxPacketSize0.
pub fn get_device_descriptor(dev: &Device, out: &mut [u8]) -> Result<usize, UsbError> {
    control_transfer(dev, 0x80, REQ_GET_DESCRIPTOR, DESC_TYPE_DEVICE << 8, 0, out)
}

/// GET_DESCRIPTOR(CONFIGURATION). Callers typically fetch just the
/// 9-byte header first to learn wTotalLength, then re-fetch that many
/// bytes to get the interface/endpoint descriptors that follow it.
pub fn get_configuration_descriptor(dev: &Device, out: &mut [u8]) -> Result<usize, UsbError> {
    control_transfer(dev, 0x80, REQ_GET_DESCRIPTOR, DESC_TYPE_CONFIGURATION << 8, 0, out)
}

/// SET_ADDRESS. Per spec the device needs a couple milliseconds to
/// actually apply it; `dev.addr` is only updated after that settle,
/// so a caller must use the returned/updated `dev` for anything after.
pub fn set_address(dev: &mut Device, new_addr: u32) -> Result<(), UsbError> {
    control_transfer(dev, 0x00, REQ_SET_ADDRESS, new_addr as u16, 0, &mut [])?;
    timer::sleep_ticks(1);
    dev.addr = new_addr;
    Ok(())
}

pub fn set_configuration(dev: &Device, config: u8) -> Result<(), UsbError> {
    control_transfer(dev, 0x00, REQ_SET_CONFIGURATION, config as u16, 0, &mut [])?;
    Ok(())
}

/// Class request GET_DESCRIPTOR(HUB) -- device recipient, class type,
/// device-to-host (bmRequestType 0xA0).
pub fn hub_get_descriptor(dev: &Device, out: &mut [u8]) -> Result<usize, UsbError> {
    control_transfer(dev, 0xA0, REQ_GET_DESCRIPTOR, DESC_TYPE_HUB << 8, 0, out)
}

pub fn hub_set_port_feature(dev: &Device, port: u16, feature: u16) -> Result<(), UsbError> {
    control_transfer(dev, 0x23, REQ_SET_FEATURE, feature, port, &mut [])?;
    Ok(())
}

pub fn hub_clear_port_feature(dev: &Device, port: u16, feature: u16) -> Result<(), UsbError> {
    control_transfer(dev, 0x23, REQ_CLEAR_FEATURE, feature, port, &mut [])?;
    Ok(())
}

/// GET_PORT_STATUS -- 4 bytes: wPortStatus, wPortChange (both LE u16).
pub fn hub_get_port_status(dev: &Device, port: u16) -> Result<[u8; 4], UsbError> {
    let mut buf = [0u8; 4];
    control_transfer(dev, 0xA3, REQ_GET_STATUS, 0, port, &mut buf)?;
    Ok(buf)
}

const PORT_FEATURE_CONNECTION: u16 = 0;
const PORT_FEATURE_RESET: u16 = 4;
const PORT_FEATURE_POWER: u16 = 8;
const PORT_FEATURE_C_CONNECTION: u16 = 16;
const PORT_FEATURE_C_RESET: u16 = 20;

/// Powers a hub's downstream port and, if something's connected,
/// resets it and reports the speed it came back at -- the hub-relayed
/// equivalent of `reset_port()` for the root port.
pub fn hub_power_and_reset_port(dev: &Device, port: u16) -> Result<Option<Speed>, UsbError> {
    hub_set_port_feature(dev, port, PORT_FEATURE_POWER)?;
    timer::sleep_ticks(10); // downstream device power-on settle

    let status = hub_get_port_status(dev, port)?;
    let port_status = u16::from_le_bytes([status[0], status[1]]);
    if port_status & (1 << PORT_FEATURE_CONNECTION) == 0 {
        return Ok(None);
    }

    hub_set_port_feature(dev, port, PORT_FEATURE_RESET)?;
    timer::sleep_ticks(6); // >= 50ms reset pulse
    let _ = hub_clear_port_feature(dev, port, PORT_FEATURE_C_RESET);
    let _ = hub_clear_port_feature(dev, port, PORT_FEATURE_C_CONNECTION);
    timer::sleep_ticks(2);

    let status = hub_get_port_status(dev, port)?;
    let port_status = u16::from_le_bytes([status[0], status[1]]);
    // Standard hub port status bits: 9 = low-speed, 10 = high-speed;
    // neither set means full-speed.
    let speed = if port_status & (1 << 9) != 0 {
        Speed::Low
    } else if port_status & (1 << 10) != 0 {
        Speed::High
    } else {
        Speed::Full
    };
    Ok(Some(speed))
}

/// One interrupt IN transaction against `ep_num` (just the number;
/// direction is always IN for this call, matching how HID keyboards
/// use their single interrupt endpoint). Returns the byte count
/// actually received -- 0 is a normal "nothing new" result (NAK),
/// not an error, since interrupt endpoints are polled.
pub fn interrupt_in(dev: &Device, ep_num: u32, buf: &mut [u8]) -> Result<usize, UsbError> {
    match transact(dev, ep_num, EP_TYPE_INTERRUPT, true, PID_DATA1, buf) {
        Ok(n) => Ok(n),
        Err(UsbError::TransferError(hcint)) if hcint & HCINT_NAK != 0 => Ok(0),
        Err(e) => Err(e),
    }
}
