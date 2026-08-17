//! Framebuffer allocation via the VideoCore property mailbox.

use crate::cache;
use crate::mailbox;

const TAG_SET_PHYS_WH: u32 = 0x0004_8003;
const TAG_SET_VIRT_WH: u32 = 0x0004_8004;
const TAG_SET_DEPTH: u32 = 0x0004_8005;
const TAG_SET_PIXEL_ORDER: u32 = 0x0004_8006;
const TAG_ALLOCATE_BUFFER: u32 = 0x0004_0001;
const TAG_GET_PITCH: u32 = 0x0004_0008;
const TAG_END: u32 = 0;

const PIXEL_ORDER_RGB: u32 = 1;

/// GPU responses carry a bus address aliased into one of its four
/// cache-domain views (see mailbox.rs); the low 30 bits are the real
/// ARM-visible physical address.
const GPU_ALIAS_MASK: u32 = 0x3FFF_FFFF;

#[repr(C, align(16))]
struct MsgBuffer([u32; 36]);

static mut MSG: MsgBuffer = MsgBuffer([0; 36]);

pub struct Framebuffer {
    pub ptr: *mut u8,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
}

impl Framebuffer {
    fn size(&self) -> usize {
        self.pitch as usize * self.height as usize
    }

    /// Writes go through our (cached) identity map of GPU-carved RAM;
    /// `flush()` must run before the GPU display pipeline is expected
    /// to see them.
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y as usize * self.pitch as usize + x as usize * (self.bpp as usize / 8);
        unsafe { core::ptr::write_volatile(self.ptr.add(offset) as *mut u32, color) };
    }

    pub fn fill_rect(&self, x0: u32, y0: u32, w: u32, h: u32, color: u32) {
        let x1 = (x0 + w).min(self.width);
        let y1 = (y0 + h).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                self.put_pixel(x, y, color);
            }
        }
    }

    pub fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// Draws a `thickness`-pixel border around the given rectangle
    /// (the border is inside the rectangle's bounds, not outside).
    pub fn draw_rect_outline(&self, x: u32, y: u32, w: u32, h: u32, thickness: u32, color: u32) {
        self.fill_rect(x, y, w, thickness, color); // top
        self.fill_rect(x, y + h.saturating_sub(thickness), w, thickness, color); // bottom
        self.fill_rect(x, y, thickness, h, color); // left
        self.fill_rect(x + w.saturating_sub(thickness), y, thickness, h, color); // right
    }

    /// Push pending writes out to the point of coherency so the GPU's
    /// display pipeline (which doesn't snoop our D-cache) sees them.
    pub fn flush(&self) {
        cache::clean_and_invalidate_range(self.ptr as usize, self.size());
    }

    /// Draws one glyph at `scale` pixels per font pixel; `(x, y)` is
    /// the glyph's top-left corner.
    pub fn draw_char(&self, x: u32, y: u32, c: char, scale: u32, fg: u32) {
        let rows = crate::font::glyph(c);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..crate::font::GLYPH_WIDTH {
                if (bits >> (crate::font::GLYPH_WIDTH - 1 - col)) & 1 != 0 {
                    self.fill_rect(
                        x + col * scale,
                        y + row as u32 * scale,
                        scale,
                        scale,
                        fg,
                    );
                }
            }
        }
    }

    /// Draws `text` left-to-right starting at `(x, y)`, one glyph cell
    /// (including a 1-column gap) per character.
    pub fn draw_text(&self, x: u32, y: u32, text: &str, scale: u32, fg: u32) {
        let advance = (crate::font::GLYPH_WIDTH + 1) * scale;
        for (i, c) in text.chars().enumerate() {
            self.draw_char(x + i as u32 * advance, y, c, scale, fg);
        }
    }
}

/// Ask the GPU for a `width`x`height` framebuffer at `depth` bits per
/// pixel. Returns None if the GPU rejected the request or handed back
/// a buffer that doesn't look real.
pub fn init(width: u32, height: u32, depth: u32) -> Option<Framebuffer> {
    let b = unsafe { core::ptr::addr_of_mut!(MSG.0) };

    let mut i = 0usize;
    macro_rules! push {
        ($v:expr) => {
            unsafe {
                (*b)[i] = $v;
            }
            i += 1;
        };
    }

    push!(0); // total size, patched below
    push!(0); // request code

    push!(TAG_SET_PHYS_WH);
    push!(8);
    push!(8);
    push!(width);
    push!(height);

    push!(TAG_SET_VIRT_WH);
    push!(8);
    push!(8);
    push!(width);
    push!(height);

    push!(TAG_SET_DEPTH);
    push!(4);
    push!(4);
    push!(depth);

    push!(TAG_SET_PIXEL_ORDER);
    push!(4);
    push!(4);
    push!(PIXEL_ORDER_RGB);

    let alloc_resp = i + 3;
    push!(TAG_ALLOCATE_BUFFER);
    push!(8);
    push!(8);
    push!(4096); // alignment
    push!(0);

    let pitch_resp = i + 3;
    push!(TAG_GET_PITCH);
    push!(4);
    push!(4);
    push!(0);

    push!(TAG_END);

    unsafe {
        (*b)[0] = (i * 4) as u32;
    }

    let msg_slice = unsafe { core::slice::from_raw_parts_mut(b as *mut u32, i) };
    if !mailbox::call(msg_slice, mailbox::CHANNEL_PROPERTY) {
        return None;
    }

    let (base, size, pitch) = unsafe { ((*b)[alloc_resp], (*b)[alloc_resp + 1], (*b)[pitch_resp]) };
    let base = base & GPU_ALIAS_MASK;

    if base == 0 || size == 0 || pitch == 0 {
        return None;
    }

    Some(Framebuffer {
        ptr: base as *mut u8,
        width,
        height,
        pitch,
        bpp: depth,
    })
}
