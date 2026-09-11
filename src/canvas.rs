//! Tiny CPU rasteriser shared by the simulated desktop painter and the
//! sprite foundry, plus the xorshift RNG used by every procedural generator
//! in the engine (painters, glitches, and the DSP noise sources).

pub struct PixelCanvas {
    pub w: usize,
    pub h: usize,
    pub buf: Vec<u8>, // RGBA8, straight alpha
}

impl PixelCanvas {
    pub fn new(w: usize, h: usize) -> Self {
        PixelCanvas { w, h, buf: vec![0; w * h * 4] }
    }

    pub fn into_buf(self) -> Vec<u8> {
        self.buf
    }

    pub fn fill(&mut self, c: [u8; 4]) {
        for px in self.buf.chunks_exact_mut(4) {
            px.copy_from_slice(&c);
        }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        if c[3] >= 255 {
            self.buf[i..i + 4].copy_from_slice(&c);
            return;
        }
        let a = c[3] as f32 / 255.0;
        for k in 0..3 {
            let d = self.buf[i + k] as f32;
            let s = c[k] as f32;
            self.buf[i + k] = (s * a + d * (1.0 - a)).round() as u8;
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, c);
            }
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 4]) {
        let r2 = r * r;
        let x0 = (cx - r).floor() as i32;
        let x1 = (cx + r).ceil() as i32;
        let y0 = (cy - r).floor() as i32;
        let y1 = (cy + r).ceil() as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                if dx * dx + dy * dy <= r2 {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Barycentric triangle fill (scanline over the bounding box).
    pub fn fill_triangle(
        &mut self,
        ax: f32, ay: f32,
        bx: f32, by: f32,
        cx: f32, cy: f32,
        c: [u8; 4],
    ) {
        let d = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy);
        if d.abs() < 1e-9 {
            return;
        }
        let minx = ax.min(bx).min(cx).floor() as i32;
        let maxx = ax.max(bx).max(cx).ceil() as i32;
        let miny = ay.min(by).min(cy).floor() as i32;
        let maxy = ay.max(by).max(cy).ceil() as i32;
        for y in miny..=maxy {
            for x in minx..=maxx {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let wa = ((by - cy) * (px - cx) + (cx - bx) * (py - cy)) / d;
                let wb = ((cy - ay) * (px - cx) + (ax - cx) * (py - cy)) / d;
                let wc = 1.0 - wa - wb;
                if wa >= 0.0 && wb >= 0.0 && wc >= 0.0 {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Horizontally displace a band of scanlines with wraparound —
    /// THE glitch primitive.
    pub fn shift_rows(&mut self, y0: i32, rows: i32, dx: i32) {
        let h = self.h as i32;
        let w = self.w as i32;
        for y in y0..(y0 + rows) {
            if y < 0 || y >= h {
                continue;
            }
            let row_start = y as usize * self.w * 4;
            let row = self.buf[row_start..row_start + self.w * 4].to_vec();
            for x in 0..w {
                let sx = (x - dx).rem_euclid(w) as usize;
                let di = row_start + x as usize * 4;
                let si = sx * 4;
                self.buf[di] = row[si];
                self.buf[di + 1] = row[si + 1];
                self.buf[di + 2] = row[si + 2];
                self.buf[di + 3] = 255;
            }
        }
    }

    pub fn invert_region(&mut self, y0: i32, rows: i32) {
        let h = self.h as i32;
        for y in y0..(y0 + rows) {
            if y < 0 || y >= h {
                continue;
            }
            let row_start = y as usize * self.w * 4;
            for x in 0..self.w {
                let i = row_start + x * 4;
                self.buf[i] = 255 - self.buf[i];
                self.buf[i + 1] = 255 - self.buf[i + 1];
                self.buf[i + 2] = 255 - self.buf[i + 2];
            }
        }
    }

    /// 5x7 bitmap text with '\n' support. Unknown glyphs advance narrowly.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, c: [u8; 4], scale: i32) {
        let mut cx = x;
        let mut cy = y;
        for ch in text.chars() {
            if ch == '\n' {
                cx = x;
                cy += 8 * scale;
                continue;
            }
            if let Some(g) = crate::font::glyph(ch) {
                for (ry, row) in g.iter().enumerate() {
                    for rx in 0..5 {
                        if row & (1 << (4 - rx)) != 0 {
                            self.fill_rect(cx + rx * scale, cy + ry * scale, scale, scale, c);
                        }
                    }
                }
                cx += 6 * scale;
            } else {
                cx += 4 * scale;
            }
        }
    }

    pub fn text_width(text: &str, scale: i32) -> i32 {
        text.lines()
            .map(|l| (l.chars().count() as i32 * 6 - 1) * scale)
            .max()
            .unwrap_or(0)
    }
}

/// xorshift32 — deterministic, allocation-free noise.
pub struct TinyRng {
    pub s: u32,
}

impl TinyRng {
    pub fn new(seed: u32) -> Self {
        TinyRng { s: if seed == 0 { 0x9E37_79B9 } else { seed } }
    }
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.s;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.s = x;
        x
    }
    pub fn next_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
    pub fn next_range(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_f32() * n as f32) as usize % n
        }
    }
}
