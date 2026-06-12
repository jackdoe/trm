use crate::font::{self, Font};
use crate::vt::{self, Span, Term};
use std::collections::HashMap;

pub struct Atlas {
    pub cw: usize,
    pub ch: usize,
    pub base: usize,
    px_em: f32,
    font: Font,
    ascii: [u16; 95],
    glyphs: Vec<Option<Vec<u8>>>,
    proc: HashMap<(u32, bool), Vec<u8>>,
}

const HOLLOW: u32 = 0;
const OUTLINE: u32 = 1;
const PHOSPHOR: (f32, f32, f32) = (255.0, 176.0, 0.0);

fn luma(rgb: u32) -> f32 {
    ((rgb >> 16 & 255) * 54 + (rgb >> 8 & 255) * 183 + (rgb & 255) * 19) as f32 / 65280.0
}

fn phosphor(l: f32) -> u32 {
    ((PHOSPHOR.0 * l) as u32) << 16 | ((PHOSPHOR.1 * l) as u32) << 8 | (PHOSPHOR.2 * l) as u32
}

fn amber_luts(gamma: f32) -> ([u32; 256], [u32; 256]) {
    (
        std::array::from_fn(|i| {
            let l = i as f32 / 255.0;
            phosphor(if l < 0.02 { l } else { l.powf(gamma) })
        }),
        std::array::from_fn(|i| phosphor((i as f32 / 255.0).powf(1.3))),
    )
}

fn amber(lut: &[u32; 256], rgb: u32) -> u32 {
    lut[(luma(rgb) * 255.0 + 0.5) as usize]
}

fn is_proc(cp: u32) -> bool {
    matches!(cp, 0x2500..=0x259f | 0x2800..=0x28ff | 0x23ba..=0x23bd | 0x23bf | 0x23fa)
}

impl Atlas {
    pub fn new(px_em: f64) -> Atlas {
        let font = Font::parse(font::DATA).expect("embedded font unreadable");
        let px_em = px_em as f32;
        let scale = px_em / font.units_per_em as f32;
        let m = font.glyph_index('M' as u32);
        let cw = ((font.advance(m) as f32 * scale).ceil() as usize).max(1);
        let asc = font.ascent as f32 * scale;
        let desc = font.descent as f32 * scale;
        let gap = font.line_gap as f32 * scale;
        let ch = ((asc - desc + gap).ceil() as usize).max(1);
        let base = (asc + gap / 2.0).round() as usize;
        let ascii: [u16; 95] = std::array::from_fn(|i| font.glyph_index(i as u32 + 0x20));
        let glyphs = vec![None; font.num_glyphs as usize];
        Atlas { cw, ch, base, px_em, font, ascii, glyphs, proc: HashMap::new() }
    }

    fn ensure(&mut self, cp: u32) -> u16 {
        let gi = if (0x20..0x7f).contains(&cp) {
            self.ascii[(cp - 0x20) as usize]
        } else {
            self.font.glyph_index(cp)
        };
        if gi == 0 || gi as usize >= self.glyphs.len() { return 0 }
        if self.glyphs[gi as usize].is_none() {
            let scale = self.px_em / self.font.units_per_em as f32;
            let adv = self.font.advance(gi) as f32 * scale;
            let x_off = ((self.cw as f32 - adv) / 2.0).max(0.0);
            self.glyphs[gi as usize] = Some(font::rasterize(&self.font, gi, scale, self.cw, self.ch, x_off, self.base as f32));
        }
        gi
    }

    fn glyph(&self, gi: u16) -> &[u8] {
        self.glyphs[gi as usize].as_deref().unwrap_or(&[])
    }

    fn proc_cached(&mut self, cp: u32, wide: bool, lt: i64) -> &[u8] {
        let (w, h) = (if wide { self.cw * 2 } else { self.cw }, self.ch);
        self.proc.entry((cp, wide)).or_insert_with(|| {
            let mut buf = vec![0u8; w * h];
            let mut cov = Cov { px: &mut buf, w, h };
            let (wi, hi) = (w as i64, h as i64);
            match cp {
                HOLLOW => cov.hollow(lt),
                OUTLINE => {
                    cov.rect(0, 0, wi, lt);
                    cov.rect(0, hi - lt, wi, hi);
                    cov.rect(0, 0, lt, hi);
                    cov.rect(wi - lt, 0, wi, hi);
                }
                _ => { cov.proc_glyph(cp, lt); }
            }
            buf
        })
    }
}

const fn b(l: u16, r: u16, u: u16, d: u16) -> u16 {
    l | r << 2 | u << 4 | d << 6
}
const RND: u16 = 0x100;

#[rustfmt::skip]
const BOX: [u16; 128] = [
    b(1,1,0,0), b(2,2,0,0), b(0,0,1,1), b(0,0,2,2), b(1,1,0,0), b(2,2,0,0), b(0,0,1,1), b(0,0,2,2),
    b(1,1,0,0), b(2,2,0,0), b(0,0,1,1), b(0,0,2,2), b(0,1,0,1), b(0,2,0,1), b(0,1,0,2), b(0,2,0,2),
    b(1,0,0,1), b(2,0,0,1), b(1,0,0,2), b(2,0,0,2), b(0,1,1,0), b(0,2,1,0), b(0,1,2,0), b(0,2,2,0),
    b(1,0,1,0), b(2,0,1,0), b(1,0,2,0), b(2,0,2,0), b(0,1,1,1), b(0,2,1,1), b(0,1,2,1), b(0,1,1,2),
    b(0,1,2,2), b(0,2,2,1), b(0,2,1,2), b(0,2,2,2), b(1,0,1,1), b(2,0,1,1), b(1,0,2,1), b(1,0,1,2),
    b(1,0,2,2), b(2,0,2,1), b(2,0,1,2), b(2,0,2,2), b(1,1,0,1), b(2,1,0,1), b(1,2,0,1), b(2,2,0,1),
    b(1,1,0,2), b(2,1,0,2), b(1,2,0,2), b(2,2,0,2), b(1,1,1,0), b(2,1,1,0), b(1,2,1,0), b(2,2,1,0),
    b(1,1,2,0), b(2,1,2,0), b(1,2,2,0), b(2,2,2,0), b(1,1,1,1), b(2,1,1,1), b(1,2,1,1), b(2,2,1,1),
    b(1,1,2,1), b(1,1,1,2), b(1,1,2,2), b(2,1,2,1), b(1,2,2,1), b(2,1,1,2), b(1,2,1,2), b(2,2,2,1),
    b(2,2,1,2), b(2,1,2,2), b(1,2,2,2), b(2,2,2,2), b(1,1,0,0), b(2,2,0,0), b(0,0,1,1), b(0,0,2,2),
    b(3,3,0,0), b(0,0,3,3), b(0,3,0,1), b(0,1,0,3), b(0,3,0,3), b(3,0,0,1), b(1,0,0,3), b(3,0,0,3),
    b(0,3,1,0), b(0,1,3,0), b(0,3,3,0), b(3,0,1,0), b(1,0,3,0), b(3,0,3,0), b(0,3,1,1), b(0,1,3,3),
    b(0,3,3,3), b(3,0,1,1), b(1,0,3,3), b(3,0,3,3), b(3,3,0,1), b(1,1,0,3), b(3,3,0,3), b(3,3,1,0),
    b(1,1,3,0), b(3,3,3,0), b(3,3,1,1), b(1,1,3,3), b(3,3,3,3),
    b(0,1,0,1) | RND, b(1,0,0,1) | RND, b(1,0,1,0) | RND, b(0,1,1,0) | RND,
    0, 0, 0,
    b(1,0,0,0), b(0,0,1,0), b(0,1,0,0), b(0,0,0,1), b(2,0,0,0), b(0,0,2,0), b(0,2,0,0), b(0,0,0,2),
    b(1,2,0,0), b(0,0,1,2), b(2,1,0,0), b(0,0,2,1),
];

const QUAD: [u8; 10] = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14];

struct Cov<'a> {
    px: &'a mut [u8],
    w: usize,
    h: usize,
}

impl Cov<'_> {
    fn rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64) {
        let (x0, y0) = (x0.max(0) as usize, y0.max(0) as usize);
        let (x1, y1) = ((x1.max(0) as usize).min(self.w), (y1.max(0) as usize).min(self.h));
        for y in y0..y1 {
            self.px[y * self.w + x0.min(x1)..y * self.w + x1].fill(255);
        }
    }

    fn max(&mut self, x: usize, y: usize, v: f64) {
        if v <= 0.0 { return }
        let a = (v.min(1.0) * 255.0) as u8;
        let p = &mut self.px[y * self.w + x];
        *p = (*p).max(a);
    }

    fn seg(&mut self, side: usize, style: u16, lt: i64) {
        let (cx, cy, w, h) = (self.w as i64 / 2, self.h as i64 / 2, self.w as i64, self.h as i64);
        let t = if style == 2 { lt * 2 } else { lt };
        match (side, style) {
            (0, 3) => { self.rect(0, cy - 2 * lt, cx + 2 * lt, cy - lt); self.rect(0, cy + lt, cx + 2 * lt, cy + 2 * lt) }
            (0, _) => self.rect(0, cy - t / 2, cx + t / 2 + t % 2, cy - t / 2 + t),
            (1, 3) => { self.rect(cx - 2 * lt, cy - 2 * lt, w, cy - lt); self.rect(cx - 2 * lt, cy + lt, w, cy + 2 * lt) }
            (1, _) => self.rect(cx - t / 2, cy - t / 2, w, cy - t / 2 + t),
            (2, 3) => { self.rect(cx - 2 * lt, 0, cx - lt, cy + 2 * lt); self.rect(cx + lt, 0, cx + 2 * lt, cy + 2 * lt) }
            (2, _) => self.rect(cx - t / 2, 0, cx - t / 2 + t, cy + t / 2 + t % 2),
            (3, 3) => { self.rect(cx - 2 * lt, cy - 2 * lt, cx - lt, h); self.rect(cx + lt, cy - 2 * lt, cx + 2 * lt, h) }
            (_, _) => self.rect(cx - t / 2, cy - t / 2, cx - t / 2 + t, h),
        }
    }

    fn arc(&mut self, corner: usize, lt: i64) {
        let (cx, cy, w, h) = (self.w as i64 / 2, self.h as i64 / 2, self.w as i64, self.h as i64);
        let r = cx.min(cy).max(2);
        let ax = if corner == 0 || corner == 3 { cx + r } else { cx - r } as f64;
        let ay = if corner < 2 { cy + r } else { cy - r } as f64;
        for y in 0..self.h {
            for x in 0..self.w {
                let inx = if corner == 0 || corner == 3 { (x as f64) <= ax } else { (x as f64) >= ax };
                let iny = if corner < 2 { (y as f64) <= ay } else { (y as f64) >= ay };
                if !inx || !iny { continue }
                let d = ((x as f64 + 0.5 - ax).hypot(y as f64 + 0.5 - ay) - r as f64).abs();
                self.max(x, y, lt as f64 / 2.0 + 0.5 - d);
            }
        }
        let (hl, hr) = (cy - lt / 2, cy - lt / 2 + lt);
        let (vl, vr) = (cx - lt / 2, cx - lt / 2 + lt);
        match corner {
            0 => { self.rect(cx + r, hl, w, hr); self.rect(vl, cy + r, vr, h) }
            1 => { self.rect(0, hl, cx - r, hr); self.rect(vl, cy + r, vr, h) }
            2 => { self.rect(0, hl, cx - r, hr); self.rect(vl, 0, vr, cy - r) }
            _ => { self.rect(cx + r, hl, w, hr); self.rect(vl, 0, vr, cy - r) }
        }
    }

    fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, lt: i64) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = dx * dx + dy * dy;
        for y in 0..self.h {
            for x in 0..self.w {
                let (px, py) = (x as f64 + 0.5 - x0, y as f64 + 0.5 - y0);
                let t = ((px * dx + py * dy) / len2).clamp(0.0, 1.0);
                let d = (px - t * dx).hypot(py - t * dy);
                self.max(x, y, lt as f64 / 2.0 + 0.5 - d);
            }
        }
    }

    fn dot(&mut self, dx: f64, dy: f64, r: f64) {
        for y in 0..self.h {
            for x in 0..self.w {
                let d = (x as f64 + 0.5 - dx).hypot(y as f64 + 0.5 - dy);
                self.max(x, y, r + 0.5 - d);
            }
        }
    }

    fn hollow(&mut self, lt: i64) {
        let (w, h) = (self.w as i64, self.h as i64);
        let (ix, iy) = (w / 8 + 1, h / 8 + 1);
        self.rect(ix, iy, w - ix, iy + lt);
        self.rect(ix, h - iy - lt, w - ix, h - iy);
        self.rect(ix, iy, ix + lt, h - iy);
        self.rect(w - ix - lt, iy, w - ix, h - iy);
    }

    fn proc_glyph(&mut self, cp: u32, lt: i64) -> bool {
        let (w, h) = (self.w as i64, self.h as i64);
        match cp {
            0x2571..=0x2573 => {
                if cp != 0x2572 { self.line(0.0, h as f64, w as f64, 0.0, lt) }
                if cp != 0x2571 { self.line(0.0, 0.0, w as f64, h as f64, lt) }
                true
            }
            0x2500..=0x257f => {
                let v = BOX[(cp - 0x2500) as usize];
                if v & RND != 0 {
                    let corner = if v & 3 != 0 { if v >> 4 & 3 != 0 { 2 } else { 1 } } else if v >> 4 & 3 != 0 { 3 } else { 0 };
                    self.arc(corner, lt);
                    return true;
                }
                if v == 0 { return false }
                for side in 0..4 {
                    let style = v >> (side * 2) & 3;
                    if style != 0 { self.seg(side, style, lt) }
                }
                true
            }
            0x2580..=0x259f => {
                match cp - 0x2580 {
                    0 => self.rect(0, 0, w, h / 2),
                    i @ 1..=8 => self.rect(0, h - h * i as i64 / 8, w, h),
                    i @ 9..=15 => self.rect(0, 0, w * (16 - i as i64) / 8, h),
                    16 => self.rect(w / 2, 0, w, h),
                    i @ 17..=19 => self.px.fill(64 * (i as u8 - 16)),
                    20 => self.rect(0, 0, w, h / 8),
                    21 => self.rect(w - w / 8, 0, w, h),
                    i => {
                        let q = QUAD[(i - 22) as usize];
                        if q & 1 != 0 { self.rect(0, 0, w / 2, h / 2) }
                        if q & 2 != 0 { self.rect(w / 2, 0, w, h / 2) }
                        if q & 4 != 0 { self.rect(0, h / 2, w / 2, h) }
                        if q & 8 != 0 { self.rect(w / 2, h / 2, w, h) }
                    }
                }
                true
            }
            0x2800..=0x28ff => {
                let bits = (cp - 0x2800) as u8;
                let r = w as f64 / 7.0 + 0.5;
                const POS: [(usize, usize); 8] = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
                for (bit, &(col, row)) in POS.iter().enumerate() {
                    if bits >> bit & 1 != 0 {
                        self.dot(w as f64 * if col == 1 { 0.72 } else { 0.28 }, h as f64 * (0.14 + 0.24 * row as f64), r);
                    }
                }
                true
            }
            0x23ba..=0x23bd => {
                let y = (h - lt) * (cp - 0x23ba) as i64 / 3;
                self.rect(0, y, w, y + lt);
                true
            }
            0x23bf => {
                self.seg(1, 1, lt);
                self.seg(2, 1, lt);
                true
            }
            0x23fa => {
                let r = w.min(h) as f64 * 0.38;
                self.dot(w as f64 / 2.0, h as f64 / 2.0, r);
                true
            }
            _ => false,
        }
    }
}

fn blend(p: &mut u32, fg: u32, a: u32) {
    if a == 0 { return }
    if a >= 255 {
        *p = 0xff00_0000 | fg;
        return;
    }
    let d = *p;
    let mix = |s: u32, d: u32| (s * a + d * (255 - a) + 127) / 255;
    *p = 0xff00_0000
        | mix(fg >> 16 & 255, d >> 16 & 255) << 16
        | mix(fg >> 8 & 255, d >> 8 & 255) << 8
        | mix(fg & 255, d & 255);
}

pub struct Fb<'a> {
    pub px: &'a mut [u32],
    pub w: usize,
    pub h: usize,
    pub stride: usize,
    pub ox: usize,
    pub oy: usize,
}

impl Fb<'_> {
    fn fill_at(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: u32) {
        let v = 0xff00_0000 | c;
        let x0 = x0.min(self.w);
        let x1 = (x0 + w).min(self.w);
        for y in y0..(y0 + h).min(self.h) {
            self.px[y * self.stride + x0..y * self.stride + x1].fill(v);
        }
    }

    fn fill(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: u32) {
        self.fill_at(self.ox + x0, self.oy + y0, w, h, c);
    }

    fn blend_cov(&mut self, x0: usize, y0: usize, cov: &[u8], cw: usize, chh: usize, fg: u32, shift: usize) {
        let xs = self.ox + x0 + shift;
        if xs >= self.w { return }
        let w = cw.min(self.w - xs);
        for y in 0..chh {
            let fy = self.oy + y0 + y;
            if fy >= self.h { break }
            let row = fy * self.stride + xs;
            for (d, &a) in self.px[row..row + w].iter_mut().zip(&cov[y * cw..y * cw + w]) {
                blend(d, fg, a as u32);
            }
        }
    }
}

fn sel_contains(sel: &Span, id: u64, x: usize) -> bool {
    let Some(((al, ac), (bl, bc))) = *sel else { return false };
    (id > al || (id == al && x >= ac)) && (id < bl || (id == bl && x <= bc))
}

pub fn frame(t: &mut Term, atlas: &mut Atlas, fb: &mut Fb, sel: &Span, focused: bool, gamma: f32, drawn: &mut Vec<bool>) -> bool {
    let (cw, ch) = (atlas.cw, atlas.ch);
    let lt = (ch as i64 / 14).max(1);
    drawn.clear();
    drawn.resize(t.rows, false);
    let mut drew = t.all_dirty;
    let (fg_lut, bg_lut) = amber_luts(gamma);
    let def_bg = amber(&bg_lut, t.def_bg);
    if t.all_dirty {
        fb.fill_at(0, 0, fb.w, fb.oy, def_bg);
        let bot = fb.oy + t.rows * ch;
        fb.fill_at(0, bot, fb.w, fb.h.saturating_sub(bot), def_bg);
    }
    for d in 0..t.rows {
        let live = d as i64 - t.view as i64;
        let row_dirty = t.all_dirty || (live >= 0 && (live as usize) < t.rows && t.dirty[live as usize]);
        if !row_dirty { continue }
        drew = true;
        drawn[d] = true;
        let id = t.line_id(d);
        let line = t.line_at(id);
        let cursor_row = t.view == 0 && d == t.y && t.modes.tcem;
        let py = d * ch;
        fb.fill_at(0, fb.oy + py, fb.ox, ch, def_bg);
        let mut x = 0;
        while x < t.cols {
            let c = line
                .and_then(|l| l.cells.get(x))
                .copied()
                .unwrap_or(vt::Cell { cp: 0, fg: t.def_fg, bg: t.def_bg, attr: 0 });
            if c.attr & vt::TAIL != 0 { x += 1; continue }
            let wide = c.attr & vt::WIDE != 0 && x + 1 < t.cols;
            let cell_w = if wide { cw * 2 } else { cw };
            let px = x * cw;
            let (mut fg, mut bg) = (c.fg, c.bg);
            if c.attr & vt::REV != 0 { std::mem::swap(&mut fg, &mut bg) }
            if c.attr & vt::FAINT != 0 { fg = fg >> 1 & 0x7f7f7f }
            if sel_contains(sel, id, x) {
                fg = 0x000000;
                bg = 0xffffff;
            }
            let cursor = cursor_row && x == t.x;
            if cursor && focused { std::mem::swap(&mut fg, &mut bg) }
            let (fg, bg) = (amber(&fg_lut, fg), amber(&bg_lut, bg));
            fb.fill(px, py, cell_w, ch, bg);
            if c.cp != 0 && c.cp != b' ' as u32 {
                if is_proc(c.cp) {
                    let g = atlas.proc_cached(c.cp, wide, lt);
                    fb.blend_cov(px, py, g, cell_w, ch, fg, 0);
                } else {
                    let gi = if wide { 0 } else { atlas.ensure(c.cp) };
                    if gi != 0 {
                        let g = atlas.glyph(gi);
                        fb.blend_cov(px, py, g, cw, ch, fg, 0);
                        if c.attr & vt::BOLD != 0 {
                            fb.blend_cov(px, py, g, cw, ch, fg, 1);
                        }
                    } else {
                        let g = atlas.proc_cached(HOLLOW, wide, lt);
                        fb.blend_cov(px, py, g, cell_w, ch, fg, 0);
                    }
                }
            }
            if c.attr & vt::UNDER != 0 {
                let uy = (py + atlas.base + lt as usize).min(py + ch - lt as usize);
                fb.fill(px, uy, cell_w, lt as usize, fg);
            }
            if c.attr & vt::STRIKE != 0 {
                fb.fill(px, py + ch / 2 - lt as usize / 2, cell_w, lt as usize, fg);
            }
            if cursor && !focused {
                let g = atlas.proc_cached(OUTLINE, wide, lt);
                let c = amber(&fg_lut, t.def_fg);
                fb.blend_cov(px, py, g, cell_w, ch, c, 0);
            }
            x += if wide { 2 } else { 1 };
        }
        let edge = t.cols * cw;
        if edge < fb.w { fb.fill(edge, py, fb.w - edge, ch, def_bg) }
    }
    if t.all_dirty && t.rows * ch < fb.h {
        fb.fill(0, t.rows * ch, fb.w, fb.h - t.rows * ch, def_bg);
    }
    t.dirty.fill(false);
    t.all_dirty = false;
    drew
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_full_frame() {
        let mut atlas = Atlas::new(26.0);
        let mut t = Term::new(200, 60, 100, 0xd4d4d4, 0x0e0e12);
        for _ in 0..60 {
            t.feed("│⠋⠙ hello ╭──────╮ wörld ✻ ▓▒░ 0123456789 ║═╗ ─┼─ │\r\n".as_bytes());
        }
        let (w, h) = (200 * atlas.cw + 20, 60 * atlas.ch + 20);
        let mut px = vec![0u32; w * h];
        let mut drawn = vec![];
        let n = 200u32;
        let start = std::time::Instant::now();
        for _ in 0..n {
            t.all_dirty = true;
            let mut fb = Fb { px: &mut px, w, h, stride: w, ox: 10, oy: 10 };
            frame(&mut t, &mut atlas, &mut fb, &None, true, 0.45, &mut drawn);
        }
        println!("full frame: {:?}", start.elapsed() / n);
    }
}
