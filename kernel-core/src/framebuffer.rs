//! Framebuffer text writer with sub-region (pane) support and 24-bit
//! colour. The fabric speaks to the human operator through electrons
//! hitting a display: this module owns that surface.
//!
//! Substrate-agnostic: knows nothing about UEFI GOP, bootloader_api, or
//! whatever else handed us the framebuffer. Each arch entry point fills
//! `FbInfo` + a `&'static mut [u8]` slice and the fabric draws.

use core::fmt;
use noto_sans_mono_bitmap::{
    FontWeight, RasterHeight, RasterizedChar, get_raster, get_raster_width,
};

#[derive(Clone, Copy, Debug)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    U8,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct FbInfo {
    pub width: usize,
    pub height: usize,
    /// Pixels per row (may exceed `width` if firmware pads scanlines).
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: PixelFormat,
}

pub const CHAR_HEIGHT: usize = RasterHeight::Size16.val();
pub const CHAR_WIDTH: usize = get_raster_width(FontWeight::Regular, RasterHeight::Size16);
pub const LINE_SPACING: usize = 2;
pub const LETTER_SPACING: usize = 0;

#[derive(Clone, Copy)]
pub struct Color(pub u8, pub u8, pub u8);

pub const WHITE: Color = Color(220, 220, 220);
pub const DIM: Color = Color(140, 140, 140);
pub const YELLOW: Color = Color(240, 200, 60);
pub const CYAN: Color = Color(60, 200, 240);
pub const GREEN: Color = Color(80, 220, 100);
pub const RED: Color = Color(240, 80, 80);
pub const BLUE: Color = Color(100, 150, 240);

pub struct FrameBufferWriter {
    buffer: &'static mut [u8],
    info: FbInfo,
}

impl FrameBufferWriter {
    pub fn new(buffer: &'static mut [u8], info: FbInfo) -> Self {
        let mut s = Self { buffer, info };
        s.fill_screen(Color(0, 0, 0));
        s
    }

    pub fn info(&self) -> FbInfo {
        self.info
    }

    pub fn fill_screen(&mut self, color: Color) {
        for px in self.buffer.iter_mut() {
            *px = 0;
        }
        // Then plot color into every pixel.
        let w = self.info.width;
        let h = self.info.height;
        for y in 0..h {
            for x in 0..w {
                self.put_pixel(x, y, color, 255);
            }
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        for dy in 0..h {
            for dx in 0..w {
                self.put_pixel(x + dx, y + dy, color, 255);
            }
        }
    }

    pub fn draw_hline(&mut self, x: usize, y: usize, w: usize, color: Color) {
        for dx in 0..w {
            self.put_pixel(x + dx, y, color, 255);
        }
    }

    pub fn draw_vline(&mut self, x: usize, y: usize, h: usize, color: Color) {
        for dy in 0..h {
            self.put_pixel(x, y + dy, color, 255);
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: Color, intensity: u8) {
        if x >= self.info.width || y >= self.info.height {
            return;
        }
        let offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        let scale = |c: u8| ((c as u16 * intensity as u16) / 255) as u8;
        let r = scale(color.0);
        let g = scale(color.1);
        let b = scale(color.2);
        let pixel: [u8; 4] = match self.info.pixel_format {
            PixelFormat::Rgb => [r, g, b, 0],
            PixelFormat::Bgr => [b, g, r, 0],
            PixelFormat::U8 => {
                // grayscale
                let y = ((r as u16 + g as u16 + b as u16) / 3) as u8;
                [y, 0, 0, 0]
            }
            _ => [r, g, b, 0],
        };
        let bpp = self.info.bytes_per_pixel.min(4);
        let end = offset + bpp;
        if end <= self.buffer.len() {
            self.buffer[offset..end].copy_from_slice(&pixel[..bpp]);
        }
    }

    pub fn draw_glyph_at(&mut self, x: usize, y: usize, c: char, color: Color) {
        let glyph = get_raster(c, FontWeight::Regular, RasterHeight::Size16)
            .unwrap_or_else(|| {
                get_raster('?', FontWeight::Regular, RasterHeight::Size16)
                    .expect("'?' must exist")
            });
        self.draw_glyph(x, y, &glyph, color);
    }

    fn draw_glyph(&mut self, x: usize, y: usize, glyph: &RasterizedChar, color: Color) {
        for (gy, row) in glyph.raster().iter().enumerate() {
            for (gx, &intensity) in row.iter().enumerate() {
                if intensity > 0 {
                    self.put_pixel(x + gx, y + gy, color, intensity);
                }
            }
        }
    }

    pub fn draw_text(
        &mut self,
        mut x: usize,
        y: usize,
        s: &str,
        color: Color,
    ) -> usize {
        for c in s.chars() {
            self.draw_glyph_at(x, y, c, color);
            x += CHAR_WIDTH + LETTER_SPACING;
        }
        x
    }

    /// Iterate `s` as wrapped lines for a region of `width` pixels.
    pub fn wrap_lines<'a>(
        s: &'a str,
        width: usize,
    ) -> impl Iterator<Item = &'a str> + 'a {
        WrapIter {
            s,
            width,
            cursor: 0,
        }
    }
}

struct WrapIter<'a> {
    s: &'a str,
    width: usize,
    cursor: usize,
}

impl<'a> Iterator for WrapIter<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        if self.cursor >= self.s.len() {
            return None;
        }
        let remaining = &self.s[self.cursor..];
        // first explicit newline?
        let nl = remaining.find('\n').unwrap_or(remaining.len());
        let chunk = &remaining[..nl];
        let max_chars = self.width / (CHAR_WIDTH + LETTER_SPACING).max(1);
        let take = chunk
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(chunk.len());
        let line = &chunk[..take];
        self.cursor += take;
        if self.cursor < self.s.len()
            && self.s.as_bytes().get(self.cursor) == Some(&b'\n')
        {
            self.cursor += 1;
        }
        Some(line)
    }
}

/// A scroll-buffered pane that draws itself within a sub-rectangle of
/// the framebuffer. Used by the Tesseract for fabric-state and log panes.
impl fmt::Write for FrameBufferWriter {
    /// Top-of-screen sequential writer — only used for boot logs.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Boot log path: write at line 0 cursor advancing down.
        // We use a dedicated cursor in `BOOT_CURSOR`.
        let mut cur = BOOT_CURSOR.lock();
        for c in s.chars() {
            match c {
                '\n' => {
                    cur.x = BORDER;
                    cur.y += CHAR_HEIGHT + LINE_SPACING;
                }
                '\r' => {
                    cur.x = BORDER;
                }
                c => {
                    if cur.x + CHAR_WIDTH >= self.info.width {
                        cur.x = BORDER;
                        cur.y += CHAR_HEIGHT + LINE_SPACING;
                    }
                    if cur.y + CHAR_HEIGHT + BORDER >= self.info.height {
                        cur.y = BORDER;
                        // wrap to top — boot log should be short enough not to.
                    }
                    self.draw_glyph_at(cur.x, cur.y, c, WHITE);
                    cur.x += CHAR_WIDTH + LETTER_SPACING;
                }
            }
        }
        Ok(())
    }
}

const BORDER: usize = 8;

struct BootCursor {
    x: usize,
    y: usize,
}

static BOOT_CURSOR: spin::Mutex<BootCursor> = spin::Mutex::new(BootCursor { x: 8, y: 8 });

unsafe impl Send for FrameBufferWriter {}
