use alloc::vec;
use alloc::vec::Vec;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput};

use crate::discover::OsKind;
use crate::font_data::{
    GlyphMetric, FONT_LG_DATA, FONT_LG_GLYPHS, FONT_LG_LINE_HEIGHT, FONT_MONO_DATA,
    FONT_MONO_GLYPHS, FONT_MONO_LINE_HEIGHT, FONT_UI_DATA, FONT_UI_GLYPHS, FONT_UI_LINE_HEIGHT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    #[inline(always)]
    pub const fn to_blt(self) -> BltPixel {
        BltPixel::new(self.r, self.g, self.b)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FontKind {
    Large,
    Ui,
    Mono,
}

impl FontKind {
    pub fn line_height(self) -> usize {
        match self {
            FontKind::Large => FONT_LG_LINE_HEIGHT,
            FontKind::Ui => FONT_UI_LINE_HEIGHT,
            FontKind::Mono => FONT_MONO_LINE_HEIGHT,
        }
    }

    fn metrics_and_data(self) -> (&'static [GlyphMetric; 95], &'static [u8]) {
        match self {
            FontKind::Large => (&FONT_LG_GLYPHS, &FONT_LG_DATA),
            FontKind::Ui => (&FONT_UI_GLYPHS, &FONT_UI_DATA),
            FontKind::Mono => (&FONT_MONO_GLYPHS, &FONT_MONO_DATA),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && py >= self.y as i32
            && px < (self.x + self.w) as i32
            && py < (self.y + self.h) as i32
    }
}

#[inline(always)]
fn blend_pixel(dst: BltPixel, src: Color, alpha: u8) -> BltPixel {
    if alpha == 0 {
        return dst;
    }
    if alpha == 255 {
        return src.to_blt();
    }
    let a = alpha as u32;
    let inv = 255 - a;
    let r = (src.r as u32 * a + dst.red as u32 * inv + 127) / 255;
    let g = (src.g as u32 * a + dst.green as u32 * inv + 127) / 255;
    let b = (src.b as u32 * a + dst.blue as u32 * inv + 127) / 255;
    BltPixel::new(r as u8, g as u8, b as u8)
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub buf: Vec<BltPixel>,
    gop_handle: uefi::Handle,
    cursor_scratch: Vec<BltPixel>,
    last_cursor: Option<(i32, i32)>,
}

const CURSOR_W: usize = 24;
const CURSOR_H: usize = 30;

// Clean crisp arrow cursor bitmap (1 = white fill, 2 = dark outline, 0 = transparent)
const CURSOR_SHAPE: [&[u8]; 15] = [
    b"2           ",
    b"22          ",
    b"212         ",
    b"2112        ",
    b"21112       ",
    b"211112      ",
    b"2111112     ",
    b"21111112    ",
    b"211111112   ",
    b"2111111112  ",
    b"21111222222 ",
    b"2112112     ",
    b"212 2112    ",
    b"22   2112   ",
    b"      222   ",
];

impl Canvas {
    pub fn init() -> Option<Self> {
        let gop_handle = boot::get_handle_for_protocol::<GraphicsOutput>().ok()?;
        let (w, h) = {
            let gop = unsafe {
                boot::open_protocol::<GraphicsOutput>(
                    OpenProtocolParams {
                        handle: gop_handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    OpenProtocolAttributes::GetProtocol,
                )
            }
            .ok()?;
            gop.current_mode_info().resolution()
        };
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return None;
        }

        let buf = vec![BltPixel::new(11, 15, 23); w * h];
        let cursor_scratch = vec![BltPixel::new(0, 0, 0); CURSOR_W * CURSOR_H];
        Some(Self {
            w,
            h,
            buf,
            gop_handle,
            cursor_scratch,
            last_cursor: None,
        })
    }

    pub fn clear(&mut self, color: Color) {
        self.buf.fill(color.to_blt());
    }

    #[inline(always)]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < self.w && y < self.h {
            self.buf[y * self.w + x] = color.to_blt();
        }
    }

    #[inline(always)]
    pub fn blend_px(&mut self, x: usize, y: usize, color: Color, alpha: u8) {
        if x < self.w && y < self.h {
            let idx = y * self.w + x;
            self.buf[idx] = blend_pixel(self.buf[idx], color, alpha);
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        let x1 = x.saturating_add(w).min(self.w);
        let y1 = y.saturating_add(h).min(self.h);
        if x >= x1 || y >= y1 {
            return;
        }
        let px = color.to_blt();
        for yy in y..y1 {
            let row = yy * self.w;
            self.buf[row + x..row + x1].fill(px);
        }
    }

    /// Filled rounded rectangle with 2x2 subpixel anti-aliased corners.
    pub fn fill_rounded_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        radius: usize,
        color: Color,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let r = radius.min(w / 2).min(h / 2);
        if r == 0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }

        // Fast interior spans
        self.fill_rect(x + r, y, w - 2 * r, h, color);
        self.fill_rect(x, y + r, r, h - 2 * r, color);
        self.fill_rect(x + w - r, y + r, r, h - 2 * r, color);

        // Anti-aliased 4 corners (integer 4x4 supersampling)
        let r4 = (r as i64) * 4;
        let r4_sq = r4 * r4;
        let corners = [
            (x, y, r as i64, r as i64),
            (x + w - r, y, 0, r as i64),
            (x, y + h - r, r as i64, 0),
            (x + w - r, y + h - r, 0, 0),
        ];

        for (bx, by, cx, cy) in corners {
            for dy in 0..r {
                for dx in 0..r {
                    let mut inside = 0u32;
                    for sy in 0..4i64 {
                        let py = (dy as i64) * 4 + sy - cy * 4 + (if cy == 0 { 1 } else { -1 });
                        for sx in 0..4i64 {
                            let px =
                                (dx as i64) * 4 + sx - cx * 4 + (if cx == 0 { 1 } else { -1 });
                            if px * px + py * py <= r4_sq {
                                inside += 1;
                            }
                        }
                    }
                    if inside > 0 {
                        let alpha = ((inside * 255) / 16) as u8;
                        self.blend_px(bx + dx, by + dy, color, alpha);
                    }
                }
            }
        }
    }

    /// Outlined rounded rectangle border (useful for selected card glow).
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_rounded_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        radius: usize,
        thickness: usize,
        color: Color,
    ) {
        if w <= thickness * 2 || h <= thickness * 2 {
            return;
        }
        let r = radius.min(w / 2).min(h / 2);
        let t = thickness;

        // Straight edges
        self.fill_rect(x + r, y, w - 2 * r, t, color);
        self.fill_rect(x + r, y + h - t, w - 2 * r, t, color);
        self.fill_rect(x, y + r, t, h - 2 * r, color);
        self.fill_rect(x + w - t, y + r, t, h - 2 * r, color);

        // Corner arcs
        let ro = (r as i64) * 2;
        let ri = (r.saturating_sub(t) as i64) * 2;
        let ro_sq = ro * ro;
        let ri_sq = ri * ri;
        let corners = [
            (x, y, r as i64, r as i64),
            (x + w - r, y, 0, r as i64),
            (x, y + h - r, r as i64, 0),
            (x + w - r, y + h - r, 0, 0),
        ];

        for (bx, by, cx, cy) in corners {
            for dy in 0..r {
                for dx in 0..r {
                    let mut hits = 0u32;
                    for sy in 0..2i64 {
                        let py = (dy as i64) * 2 + sy - cy * 2;
                        for sx in 0..2i64 {
                            let px = (dx as i64) * 2 + sx - cx * 2;
                            let d2 = px * px + py * py;
                            if d2 <= ro_sq && d2 >= ri_sq {
                                hits += 1;
                            }
                        }
                    }
                    if hits > 0 {
                        self.blend_px(bx + dx, by + dy, color, ((hits * 255) / 4) as u8);
                    }
                }
            }
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color) {
        if radius <= 0 {
            return;
        }
        let r2_sq = (radius as i64 * 2) * (radius as i64 * 2);
        for dy in -radius..=radius {
            let py = cy + dy;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for dx in -radius..=radius {
                let px = cx + dx;
                if px < 0 || px >= self.w as i32 {
                    continue;
                }
                let mut hits = 0u32;
                for sy in 0..2i64 {
                    let fy = (dy as i64) * 2 + sy;
                    for sx in 0..2i64 {
                        let fx = (dx as i64) * 2 + sx;
                        if fx * fx + fy * fy <= r2_sq {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.blend_px(px as usize, py as usize, color, ((hits * 255) / 4) as u8);
                }
            }
        }
    }

    pub fn measure_text(&self, font: FontKind, text: &str) -> usize {
        let (glyphs, _) = font.metrics_and_data();
        let mut width = 0usize;
        for ch in text.chars() {
            let idx = if (32..=126).contains(&(ch as u32)) {
                (ch as usize) - 32
            } else {
                ('?' as usize) - 32
            };
            width += glyphs[idx].advance as usize;
        }
        width
    }

    pub fn draw_text(&mut self, font: FontKind, x: usize, y: usize, text: &str, color: Color) {
        let (glyphs, data) = font.metrics_and_data();
        let mut pen_x = x as i32;
        let base_y = y as i32;

        for ch in text.chars() {
            let idx = if (32..=126).contains(&(ch as u32)) {
                (ch as usize) - 32
            } else {
                ('?' as usize) - 32
            };
            let g = glyphs[idx];
            let gw = g.w as usize;
            let gh = g.h as usize;

            if gw > 0 && gh > 0 {
                let gx0 = pen_x + g.x_off as i32;
                let gy0 = base_y + g.y_off as i32;
                let base_off = g.offset as usize;
                let mut pix_i = 0usize;

                for dy in 0..gh {
                    let py = gy0 + dy as i32;
                    for dx in 0..gw {
                        let byte = data[base_off + (pix_i >> 1)];
                        let nib = if (pix_i & 1) == 0 {
                            byte >> 4
                        } else {
                            byte & 0x0F
                        };
                        pix_i += 1;
                        if nib > 0 {
                            let px = gx0 + dx as i32;
                            if px >= 0 && py >= 0 && (px as usize) < self.w && (py as usize) < self.h
                            {
                                let alpha = nib * 17;
                                self.blend_px(px as usize, py as usize, color, alpha);
                            }
                        }
                    }
                }
            }
            pen_x += g.advance as i32;
        }
    }

    pub fn draw_text_ellipsis(
        &mut self,
        font: FontKind,
        x: usize,
        y: usize,
        max_w: usize,
        text: &str,
        color: Color,
    ) {
        if self.measure_text(font, text) <= max_w {
            self.draw_text(font, x, y, text, color);
            return;
        }
        let dots_w = self.measure_text(font, "...");
        let budget = max_w.saturating_sub(dots_w);
        let (glyphs, _) = font.metrics_and_data();
        let mut cur_w = 0usize;
        let mut end_byte = 0usize;
        for (byte_idx, ch) in text.char_indices() {
            let idx = if (32..=126).contains(&(ch as u32)) {
                (ch as usize) - 32
            } else {
                ('?' as usize) - 32
            };
            let adv = glyphs[idx].advance as usize;
            if cur_w + adv > budget {
                break;
            }
            cur_w += adv;
            end_byte = byte_idx + ch.len_utf8();
        }
        self.draw_text(font, x, y, &text[..end_byte], color);
        self.draw_text(font, x + cur_w, y, "...", color);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_wrapped_text(
        &mut self,
        font: FontKind,
        x: usize,
        y: usize,
        max_w: usize,
        max_lines: usize,
        text: &str,
        color: Color,
    ) -> usize {
        let lh = font.line_height() + 4;
        let mut cy = y;
        let mut lines_drawn = 0usize;
        let mut cur_line = alloc::string::String::new();

        for word in text.split_whitespace() {
            let candidate_len = if cur_line.is_empty() {
                self.measure_text(font, word)
            } else {
                self.measure_text(font, &cur_line)
                    + self.measure_text(font, " ")
                    + self.measure_text(font, word)
            };

            if candidate_len <= max_w {
                if !cur_line.is_empty() {
                    cur_line.push(' ');
                }
                cur_line.push_str(word);
            } else {
                if !cur_line.is_empty() {
                    if lines_drawn + 1 >= max_lines {
                        cur_line.push_str("...");
                        self.draw_text_ellipsis(font, x, cy, max_w, &cur_line, color);
                        return cy + lh;
                    }
                    self.draw_text(font, x, cy, &cur_line, color);
                    cy += lh;
                    lines_drawn += 1;
                    cur_line.clear();
                }
                if self.measure_text(font, word) > max_w {
                    if lines_drawn + 1 >= max_lines {
                        self.draw_text_ellipsis(font, x, cy, max_w, word, color);
                        return cy + lh;
                    }
                    self.draw_text_ellipsis(font, x, cy, max_w, word, color);
                    cy += lh;
                    lines_drawn += 1;
                } else {
                    cur_line.push_str(word);
                }
            }
        }

        if !cur_line.is_empty() && lines_drawn < max_lines {
            self.draw_text_ellipsis(font, x, cy, max_w, &cur_line, color);
            cy += lh;
        }
        cy
    }

    /// Custom sleek geometric logo for gaokunBoot (top-left header).
    pub fn draw_brand_logo(&mut self, x: usize, y: usize, size: usize) {
        let bg = Color::rgb(18, 36, 62);
        let border = Color::rgb(56, 189, 248);
        let hi = Color::rgb(125, 211, 252);
        let accent = Color::rgb(52, 211, 153);

        self.fill_rounded_rect(x, y, size, size, size / 4, bg);
        self.stroke_rounded_rect(x, y, size, size, size / 4, 2, border);

        // Stylized upward dual-chevron boot emblem + base bar
        let cx = x + size / 2;
        let stroke = (size / 11).max(4);
        let span = size * 24 / 100;

        // Upper bright chevron
        let top_y = y + size * 22 / 100;
        for i in 0..span {
            self.fill_rect(cx.saturating_sub(i + stroke / 2), top_y + i, stroke, stroke, hi);
            self.fill_rect(cx + i - stroke / 2, top_y + i, stroke, stroke, hi);
        }

        // Lower emerald chevron
        let mid_y = y + size * 44 / 100;
        for i in 0..(span * 80 / 100) {
            self.fill_rect(cx.saturating_sub(i + stroke / 2), mid_y + i, stroke, stroke, accent);
            self.fill_rect(cx + i - stroke / 2, mid_y + i, stroke, stroke, accent);
        }

        // Crisp bottom pill bar
        let bar_w = size * 46 / 100;
        self.fill_rounded_rect(
            cx - bar_w / 2,
            y + size * 74 / 100,
            bar_w,
            stroke + 1,
            3,
            border,
        );
    }

    /// Crisp vector OS badge icon (Windows 4-pane, Android robot dome, Linux terminal, Tools gear, Generic).
    pub fn draw_os_icon(&mut self, x: usize, y: usize, size: usize, kind: OsKind) {
        let (bg, fg) = match kind {
            OsKind::Windows => (Color::rgb(18, 48, 78), Color::rgb(56, 189, 248)),
            OsKind::Android => (Color::rgb(16, 56, 42), Color::rgb(52, 211, 153)),
            OsKind::Linux => (Color::rgb(64, 44, 16), Color::rgb(251, 191, 36)),
            OsKind::Tools => (Color::rgb(46, 32, 74), Color::rgb(167, 139, 250)),
            OsKind::Generic => (Color::rgb(34, 43, 58), Color::rgb(148, 163, 184)),
        };

        self.fill_rounded_rect(x, y, size, size, size / 4, bg);
        self.stroke_rounded_rect(x, y, size, size, size / 4, 2, fg);

        match kind {
            OsKind::Windows => {
                // Iconic 4-pane grid
                let pad = size * 24 / 100;
                let gap = (size / 14).max(4);
                let cell = (size - pad * 2 - gap) / 2;
                let x0 = x + pad;
                let y0 = y + pad;
                self.fill_rounded_rect(x0, y0, cell, cell, 3, fg);
                self.fill_rounded_rect(x0 + cell + gap, y0, cell, cell, 3, fg);
                self.fill_rounded_rect(x0, y0 + cell + gap, cell, cell, 3, fg);
                self.fill_rounded_rect(x0 + cell + gap, y0 + cell + gap, cell, cell, 3, fg);
            }
            OsKind::Android => {
                // Clean Android head dome + eyes + antennae
                let cx = (x + size / 2) as i32;
                let cy = (y + size * 62 / 100) as i32;
                let r = (size * 28 / 100) as i32;
                // Upper semicircle
                for dy in -r..=0 {
                    for dx in -r..=r {
                        if dx * dx + dy * dy <= r * r {
                            self.set_pixel((cx + dx) as usize, (cy + dy) as usize, fg);
                        }
                    }
                }
                // Slight bottom bar
                self.fill_rect(
                    (cx - r) as usize,
                    cy as usize,
                    (r * 2) as usize,
                    (size / 14).max(3),
                    fg,
                );
                // Two eyes
                let eye_r = (size / 22).max(2) as i32;
                let eye_dx = r * 44 / 100;
                let eye_y = cy - r * 38 / 100;
                self.fill_circle(cx - eye_dx, eye_y, eye_r, bg);
                self.fill_circle(cx + eye_dx, eye_y, eye_r, bg);
                // Antennae
                let ant_len = (r * 45 / 100).max(5);
                for i in 0..ant_len {
                    let ax1 = cx - r * 42 / 100 - i / 2;
                    let ax2 = cx + r * 42 / 100 + i / 2;
                    let ay = cy - r * 82 / 100 - i;
                    self.fill_circle(ax1, ay, 2, fg);
                    self.fill_circle(ax2, ay, 2, fg);
                }
            }
            OsKind::Linux => {
                // Terminal prompt >_
                let pad = size * 24 / 100;
                let mid_y = y + size / 2;
                let x0 = x + pad;
                let stroke = (size / 16).max(3);
                for i in 0..(size * 18 / 100) {
                    self.fill_rect(x0 + i, mid_y - size * 18 / 100 + i, stroke, stroke, fg);
                    self.fill_rect(x0 + i, mid_y + size * 18 / 100 - i, stroke, stroke, fg);
                }
                self.fill_rect(
                    x + size * 52 / 100,
                    mid_y + size * 14 / 100,
                    size * 24 / 100,
                    stroke + 1,
                    fg,
                );
            }
            OsKind::Tools => {
                // Gear / ring with inner hole + 4 teeth
                let cx = (x + size / 2) as i32;
                let cy = (y + size / 2) as i32;
                let r_out = (size * 24 / 100) as i32;
                let r_in = (size * 11 / 100) as i32;
                let tooth = (size * 10 / 100).max(4);
                self.fill_rect(
                    (cx as usize).saturating_sub(tooth / 2),
                    (cy - r_out - tooth as i32 / 2) as usize,
                    tooth,
                    (r_out * 2 + tooth as i32) as usize,
                    fg,
                );
                self.fill_rect(
                    (cx - r_out - tooth as i32 / 2) as usize,
                    (cy as usize).saturating_sub(tooth / 2),
                    (r_out * 2 + tooth as i32) as usize,
                    tooth,
                    fg,
                );
                self.fill_circle(cx, cy, r_out, fg);
                self.fill_circle(cx, cy, r_in, bg);
            }
            OsKind::Generic => {
                let pad = size * 28 / 100;
                self.stroke_rounded_rect(x + pad, y + pad, size - pad * 2, size - pad * 2, 6, 4, fg);
            }
        }
    }

    pub fn present_full(&mut self) {
        self.last_cursor = None;
        let Ok(mut gop) = (unsafe {
            boot::open_protocol::<GraphicsOutput>(
                OpenProtocolParams {
                    handle: self.gop_handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            return;
        };

        let _ = gop.blt(BltOp::BufferToVideo {
            buffer: &self.buf,
            src: BltRegion::Full,
            dest: (0, 0),
            dims: (self.w, self.h),
        });
    }

    pub fn present_rect(&self, x: usize, y: usize, w: usize, h: usize) {
        let rx = x.min(self.w);
        let ry = y.min(self.h);
        let rw = w.min(self.w - rx);
        let rh = h.min(self.h - ry);
        if rw == 0 || rh == 0 {
            return;
        }
        let Ok(mut gop) = (unsafe {
            boot::open_protocol::<GraphicsOutput>(
                OpenProtocolParams {
                    handle: self.gop_handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            return;
        };

        let _ = gop.blt(BltOp::BufferToVideo {
            buffer: &self.buf,
            src: BltRegion::SubRectangle {
                coords: (rx, ry),
                px_stride: self.w,
            },
            dest: (rx, ry),
            dims: (rw, rh),
        });
    }

    /// Smooth cursor overlay: restores previous cursor rect from backbuffer, then blits cursor at new pos.
    pub fn update_cursor(&mut self, pos: Option<(i32, i32)>) {
        if let Some((ox, oy)) = self.last_cursor {
            self.present_rect(ox.max(0) as usize, oy.max(0) as usize, CURSOR_W, CURSOR_H);
        }
        self.last_cursor = pos;

        let Some((nx, ny)) = pos else {
            return;
        };
        let ux = nx.clamp(0, (self.w as i32) - 1) as usize;
        let uy = ny.clamp(0, (self.h as i32) - 1) as usize;
        let cw = CURSOR_W.min(self.w - ux);
        let ch = CURSOR_H.min(self.h - uy);
        if cw == 0 || ch == 0 {
            return;
        }

        // Copy background under cursor into scratch buffer and composite 2x-scaled arrow
        for dy in 0..ch {
            let src_row = (uy + dy) * self.w + ux;
            let dst_row = dy * cw;
            for dx in 0..cw {
                let mut px = self.buf[src_row + dx];
                let sy = dy / 2;
                let sx = dx / 2;
                if sy < CURSOR_SHAPE.len() && sx < CURSOR_SHAPE[sy].len() {
                    match CURSOR_SHAPE[sy][sx] {
                        b'1' => px = BltPixel::new(248, 250, 252),
                        b'2' => px = BltPixel::new(15, 23, 42),
                        _ => {}
                    }
                }
                self.cursor_scratch[dst_row + dx] = px;
            }
        }

        if let Ok(mut gop) = unsafe {
            boot::open_protocol::<GraphicsOutput>(
                OpenProtocolParams {
                    handle: self.gop_handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            let _ = gop.blt(BltOp::BufferToVideo {
                buffer: &self.cursor_scratch[..cw * ch],
                src: BltRegion::Full,
                dest: (ux, uy),
                dims: (cw, ch),
            });
        }
    }
}
