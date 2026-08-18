//! EFI_EVENT support: CreateEvent/SetTimer/WaitForEvent/SignalEvent/
//! CheckEvent/CloseEvent. Ferro is single-threaded and fully polled
//! (no real interrupt-driven scheduler), so "waiting" is a busy loop
//! that re-evaluates each event's condition -- honest given the
//! architecture, and it's exactly what a real EFI app calling
//! WaitForEvent on ConIn's WaitForKey event needs to actually work.

use super::types::{EfiEvent, EfiStatus, EFI_INVALID_PARAMETER, EFI_NOT_READY, EFI_SUCCESS};
use core::ffi::c_void;

const MAX_EVENTS: usize = 16;

// EFI_EVENT_TYPE bits (spec 2.10 section on CreateEvent).
pub const EVT_TIMER: u32 = 0x8000_0000;
#[allow(dead_code)] // documents the bit; nothing in Ferro's own code checks it yet
pub const EVT_NOTIFY_SIGNAL: u32 = 0x0000_0200;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Manually signaled via SignalEvent; state tracked in `signaled`.
    Generic,
    /// Deadline-based; condition is re-derived from the current tick
    /// count rather than a stored flag.
    Timer,
    /// Condition is "is a key available right now" -- re-derived from
    /// console.rs's input sources every check, matching WaitForKey's
    /// real-firmware semantics.
    ConsoleIn,
}

type NotifyFn = extern "C" fn(EfiEvent, *mut c_void);

#[derive(Clone, Copy)]
struct EventSlot {
    in_use: bool,
    kind: Kind,
    signaled: bool,
    notify_fn: Option<NotifyFn>,
    notify_ctx: *mut c_void,
    timer_deadline: u64, // ticks
    timer_period: u64,   // ticks; 0 = one-shot / cancelled
}

const EMPTY: EventSlot = EventSlot {
    in_use: false,
    kind: Kind::Generic,
    signaled: false,
    notify_fn: None,
    notify_ctx: core::ptr::null_mut(),
    timer_deadline: 0,
    timer_period: 0,
};

static mut EVENTS: [EventSlot; MAX_EVENTS] = [EMPTY; MAX_EVENTS];

fn index_to_event(i: usize) -> EfiEvent {
    (i + 1) as EfiEvent
}

fn event_to_index(e: EfiEvent) -> Option<usize> {
    let raw = e as usize;
    if raw == 0 || raw > MAX_EVENTS {
        None
    } else {
        Some(raw - 1)
    }
}

/// Creates an event of `kind`, not tied to any particular EFI_EVENT_TYPE
/// bit pattern -- used internally (e.g. console.rs's WaitForKey event)
/// as well as by the real CreateEvent Boot Services call.
pub fn create(kind: Kind, notify_fn: Option<NotifyFn>, notify_ctx: *mut c_void) -> Option<EfiEvent> {
    let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
    for (i, slot) in events.iter_mut().enumerate() {
        if !slot.in_use {
            *slot = EMPTY;
            slot.in_use = true;
            slot.kind = kind;
            slot.notify_fn = notify_fn;
            slot.notify_ctx = notify_ctx;
            return Some(index_to_event(i));
        }
    }
    None
}

pub fn close(event: EfiEvent) -> EfiStatus {
    let Some(i) = event_to_index(event) else {
        return EFI_INVALID_PARAMETER;
    };
    let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
    if !events[i].in_use {
        return EFI_INVALID_PARAMETER;
    }
    events[i] = EMPTY;
    EFI_SUCCESS
}

/// EFI_BOOT_SERVICES.SetTimer's TimerType values.
pub const TIMER_CANCEL: u32 = 0;
pub const TIMER_PERIODIC: u32 = 1;
#[allow(dead_code)] // documents the spec value; set_timer() treats anything != CANCEL/PERIODIC as one-shot
pub const TIMER_RELATIVE: u32 = 2;

/// `trigger_time_100ns` is 100ns units per spec; converted to ticks
/// via the same clock the rest of Ferro's timing uses.
pub fn set_timer(event: EfiEvent, timer_type: u32, trigger_time_100ns: u64) -> EfiStatus {
    let Some(i) = event_to_index(event) else {
        return EFI_INVALID_PARAMETER;
    };
    let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
    if !events[i].in_use {
        return EFI_INVALID_PARAMETER;
    }
    if timer_type == TIMER_CANCEL {
        events[i].timer_period = 0;
        events[i].timer_deadline = 0;
        return EFI_SUCCESS;
    }
    let micros = trigger_time_100ns / 10;
    let ticks = crate::timer::micros_to_ticks(micros);
    let now = crate::timer::ticks();
    events[i].timer_deadline = now + ticks;
    events[i].timer_period = if timer_type == TIMER_PERIODIC { ticks.max(1) } else { 0 };
    EFI_SUCCESS
}

pub fn signal(event: EfiEvent) -> EfiStatus {
    let Some(i) = event_to_index(event) else {
        return EFI_INVALID_PARAMETER;
    };
    let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
    if !events[i].in_use {
        return EFI_INVALID_PARAMETER;
    }
    events[i].signaled = true;
    if let Some(f) = events[i].notify_fn {
        f(event, events[i].notify_ctx);
    }
    EFI_SUCCESS
}

/// True if `event`'s condition currently holds, re-deriving it fresh
/// for Timer/ConsoleIn kinds rather than trusting a stale flag.
fn condition_met(i: usize) -> bool {
    let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
    match events[i].kind {
        Kind::Generic => events[i].signaled,
        Kind::Timer => {
            if events[i].timer_deadline == 0 && events[i].timer_period == 0 {
                return false; // cancelled / never armed
            }
            let now = crate::timer::ticks();
            if now >= events[i].timer_deadline {
                if events[i].timer_period > 0 {
                    events[i].timer_deadline = now + events[i].timer_period;
                } else {
                    events[i].timer_deadline = 0;
                }
                if let Some(f) = events[i].notify_fn {
                    f(index_to_event(i), events[i].notify_ctx);
                }
                true
            } else {
                false
            }
        }
        Kind::ConsoleIn => super::console::key_available(),
    }
}

pub fn check(event: EfiEvent) -> EfiStatus {
    let Some(i) = event_to_index(event) else {
        return EFI_INVALID_PARAMETER;
    };
    if !unsafe { (*core::ptr::addr_of!(EVENTS))[i].in_use } {
        return EFI_INVALID_PARAMETER;
    }
    if condition_met(i) {
        let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
        if events[i].kind == Kind::Generic {
            events[i].signaled = false; // CheckEvent clears on read, per spec
        }
        EFI_SUCCESS
    } else {
        EFI_NOT_READY
    }
}

/// Busy-waits until one of `events` is signaled, honest to Ferro's
/// fully-polled architecture -- returns its index into `events`.
pub fn wait(events_list: &[EfiEvent]) -> Result<usize, EfiStatus> {
    if events_list.is_empty() {
        return Err(EFI_INVALID_PARAMETER);
    }
    for &e in events_list {
        if event_to_index(e).is_none() {
            return Err(EFI_INVALID_PARAMETER);
        }
    }
    loop {
        for (idx, &e) in events_list.iter().enumerate() {
            let i = event_to_index(e).unwrap();
            if condition_met(i) {
                let events = unsafe { &mut *core::ptr::addr_of_mut!(EVENTS) };
                if events[i].kind == Kind::Generic {
                    events[i].signaled = false;
                }
                return Ok(idx);
            }
        }
        unsafe { core::arch::asm!("wfe") };
    }
}
