pub static DATA: &[u8] = include_bytes!("../font/3270NerdFont-Regular.ttf");

fn rd8(d: &[u8], o: usize) -> Option<u8> {
    d.get(o).copied()
}

fn rd16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn rdi16(d: &[u8], o: usize) -> Option<i16> {
    rd16(d, o).map(|v| v as i16)
}

fn rd32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

pub struct Font {
    data: &'static [u8],
    glyf: usize,
    loca: usize,
    hmtx: usize,
    cmap4: Option<usize>,
    cmap12: Option<usize>,
    pub num_glyphs: u16,
    num_h_metrics: u16,
    long_loca: bool,
    pub units_per_em: u16,
    pub ascent: i16,
    pub descent: i16,
    pub line_gap: i16,
}

impl Font {
    pub fn parse(data: &'static [u8]) -> Option<Font> {
        let n = rd16(data, 4)? as usize;
        let table = |tag: &[u8; 4]| -> Option<usize> {
            (0..n).find_map(|i| {
                let r = 12 + i * 16;
                (data.get(r..r + 4)? == tag).then(|| rd32(data, r + 8).map(|o| o as usize))?
            })
        };
        let head = table(b"head")?;
        let hhea = table(b"hhea")?;
        let maxp = table(b"maxp")?;
        let cmap = table(b"cmap")?;
        let f = Font {
            data,
            glyf: table(b"glyf")?,
            loca: table(b"loca")?,
            hmtx: table(b"hmtx")?,
            cmap4: None,
            cmap12: None,
            num_glyphs: rd16(data, maxp + 4)?,
            num_h_metrics: rd16(data, hhea + 34)?,
            long_loca: rdi16(data, head + 50)? == 1,
            units_per_em: rd16(data, head + 18)?,
            ascent: rdi16(data, hhea + 4)?,
            descent: rdi16(data, hhea + 6)?,
            line_gap: rdi16(data, hhea + 8)?,
        };
        let mut f = f;
        let subs = rd16(data, cmap + 2)? as usize;
        for i in 0..subs {
            let off = cmap + rd32(data, cmap + 8 + i * 8)? as usize;
            match rd16(data, off)? {
                4 if f.cmap4.is_none() => f.cmap4 = Some(off),
                12 if f.cmap12.is_none() => f.cmap12 = Some(off),
                _ => {}
            }
        }
        Some(f)
    }

    pub fn glyph_index(&self, cp: u32) -> u16 {
        let d = self.data;
        if let Some(o) = self.cmap12 {
            let n = rd32(d, o + 12).unwrap_or(0) as usize;
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = o + 16 + mid * 12;
                let (start, end) = match (rd32(d, g), rd32(d, g + 4)) {
                    (Some(s), Some(e)) => (s, e),
                    _ => return 0,
                };
                if cp < start { hi = mid }
                else if cp > end { lo = mid + 1 }
                else {
                    return rd32(d, g + 8).map_or(0, |sg| (sg + (cp - start)) as u16);
                }
            }
        }
        if cp > 0xffff { return 0 }
        let Some(o) = self.cmap4 else { return 0 };
        let segx2 = rd16(d, o + 6).unwrap_or(0) as usize;
        let ends = o + 14;
        let starts = ends + segx2 + 2;
        let deltas = starts + segx2;
        let ranges = deltas + segx2;
        let cp16 = cp as u16;
        for i in (0..segx2 / 2).map(|i| i * 2) {
            let end = rd16(d, ends + i).unwrap_or(0);
            if cp16 > end { continue }
            let start = rd16(d, starts + i).unwrap_or(0xffff);
            if cp16 < start { return 0 }
            let delta = rd16(d, deltas + i).unwrap_or(0);
            let range = rd16(d, ranges + i).unwrap_or(0);
            if range == 0 { return cp16.wrapping_add(delta) }
            let addr = ranges + i + range as usize + 2 * (cp16 - start) as usize;
            let g = rd16(d, addr).unwrap_or(0);
            return if g == 0 { 0 } else { g.wrapping_add(delta) };
        }
        0
    }

    pub fn advance(&self, gi: u16) -> u16 {
        let i = gi.min(self.num_h_metrics.saturating_sub(1));
        rd16(self.data, self.hmtx + i as usize * 4).unwrap_or(self.units_per_em / 2)
    }

    fn glyf_range(&self, gi: u16) -> Option<(usize, usize)> {
        if gi >= self.num_glyphs { return None }
        let d = self.data;
        let i = gi as usize;
        let (a, b) = if self.long_loca {
            (rd32(d, self.loca + i * 4)? as usize, rd32(d, self.loca + i * 4 + 4)? as usize)
        } else {
            (rd16(d, self.loca + i * 2)? as usize * 2, rd16(d, self.loca + i * 2 + 2)? as usize * 2)
        };
        (a < b).then_some((self.glyf + a, self.glyf + b))
    }

    fn outline(&self, gi: u16, t: [f32; 6], r: &mut Raster, depth: u8) {
        if depth > 4 { return }
        let Some((off, _end)) = self.glyf_range(gi) else { return };
        let d = self.data;
        let Some(ncont) = rdi16(d, off) else { return };
        if ncont >= 0 {
            self.simple_glyph(off, ncont as usize, t, r);
            return;
        }
        let mut o = off + 10;
        loop {
            let Some(flags) = rd16(d, o) else { return };
            let Some(cgi) = rd16(d, o + 2) else { return };
            o += 4;
            let (dx, dy);
            if flags & 1 != 0 {
                dx = rdi16(d, o).unwrap_or(0) as f32;
                dy = rdi16(d, o + 2).unwrap_or(0) as f32;
                o += 4;
            } else {
                dx = rd8(d, o).unwrap_or(0) as i8 as f32;
                dy = rd8(d, o + 1).unwrap_or(0) as i8 as f32;
                o += 2;
            }
            let f2d = |v: i16| v as f32 / 16384.0;
            let (a, b, c, dd) = if flags & 8 != 0 {
                let s = f2d(rdi16(d, o).unwrap_or(0));
                o += 2;
                (s, 0.0, 0.0, s)
            } else if flags & 0x40 != 0 {
                let sx = f2d(rdi16(d, o).unwrap_or(0));
                let sy = f2d(rdi16(d, o + 2).unwrap_or(0));
                o += 4;
                (sx, 0.0, 0.0, sy)
            } else if flags & 0x80 != 0 {
                let m: Vec<f32> = (0..4).map(|i| f2d(rdi16(d, o + i * 2).unwrap_or(0))).collect();
                o += 8;
                (m[0], m[1], m[2], m[3])
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };
            let local = [a, b, c, dd, dx, dy];
            let ct = [
                t[0] * local[0] + t[2] * local[1],
                t[1] * local[0] + t[3] * local[1],
                t[0] * local[2] + t[2] * local[3],
                t[1] * local[2] + t[3] * local[3],
                t[0] * local[4] + t[2] * local[5] + t[4],
                t[1] * local[4] + t[3] * local[5] + t[5],
            ];
            if flags & 2 != 0 { self.outline(cgi, ct, r, depth + 1) }
            if flags & 0x20 == 0 { return }
        }
    }

    fn simple_glyph(&self, off: usize, ncont: usize, t: [f32; 6], r: &mut Raster) {
        let d = self.data;
        let Some(npts) = rd16(d, off + 10 + (ncont - 1) * 2).map(|v| v as usize + 1) else { return };
        let mut ends = Vec::with_capacity(ncont);
        for i in 0..ncont {
            match rd16(d, off + 10 + i * 2) {
                Some(e) => ends.push(e as usize),
                None => return,
            }
        }
        let ins = match rd16(d, off + 10 + ncont * 2) {
            Some(v) => v as usize,
            None => return,
        };
        let mut o = off + 12 + ncont * 2 + ins;
        let mut flags = Vec::with_capacity(npts);
        while flags.len() < npts {
            let Some(f) = rd8(d, o) else { return };
            o += 1;
            flags.push(f);
            if f & 8 != 0 {
                let Some(rep) = rd8(d, o) else { return };
                o += 1;
                for _ in 0..rep { flags.push(f) }
            }
        }
        flags.truncate(npts);
        let mut xs = Vec::with_capacity(npts);
        let mut v = 0i32;
        for &f in &flags {
            if f & 2 != 0 {
                let Some(b) = rd8(d, o) else { return };
                o += 1;
                v += if f & 16 != 0 { b as i32 } else { -(b as i32) };
            } else if f & 16 == 0 {
                let Some(w) = rdi16(d, o) else { return };
                o += 2;
                v += w as i32;
            }
            xs.push(v as f32);
        }
        let mut ys = Vec::with_capacity(npts);
        v = 0;
        for &f in &flags {
            if f & 4 != 0 {
                let Some(b) = rd8(d, o) else { return };
                o += 1;
                v += if f & 32 != 0 { b as i32 } else { -(b as i32) };
            } else if f & 32 == 0 {
                let Some(w) = rdi16(d, o) else { return };
                o += 2;
                v += w as i32;
            }
            ys.push(v as f32);
        }
        let map = |i: usize| -> (f32, f32, bool) {
            let (x, y) = (xs[i], ys[i]);
            (t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5], flags[i] & 1 != 0)
        };
        let mut start = 0;
        for &end in &ends {
            if end >= npts || end < start { return }
            let n = end - start + 1;
            if n < 2 {
                start = end + 1;
                continue;
            }
            let pt = |k: usize| map(start + k % n);
            let mut anchor = pt(0);
            let mut first_off = None;
            if !anchor.2 {
                let last = pt(n - 1);
                anchor = if last.2 {
                    last
                } else {
                    ((anchor.0 + last.0) / 2.0, (anchor.1 + last.1) / 2.0, true)
                };
                first_off = Some(pt(0));
            }
            let mut prev = (anchor.0, anchor.1);
            let mut ctrl: Option<(f32, f32)> = first_off.map(|p| (p.0, p.1));
            for k in 1..n {
                let cur = pt(k);
                if cur.2 {
                    match ctrl.take() {
                        Some(c) => r.quad(prev, c, (cur.0, cur.1)),
                        None => r.line(prev, (cur.0, cur.1)),
                    }
                    prev = (cur.0, cur.1);
                } else {
                    if let Some(c) = ctrl {
                        let mid = ((c.0 + cur.0) / 2.0, (c.1 + cur.1) / 2.0);
                        r.quad(prev, c, mid);
                        prev = mid;
                    }
                    ctrl = Some((cur.0, cur.1));
                }
            }
            match ctrl.take() {
                Some(c) => r.quad(prev, c, (anchor.0, anchor.1)),
                None => r.line(prev, (anchor.0, anchor.1)),
            }
            start = end + 1;
        }
    }
}

pub struct Raster {
    a: Vec<f32>,
    w: usize,
    h: usize,
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Raster {
        Raster { a: vec![0.0; (w + 1) * h], w, h }
    }

    pub fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if (p0.1 - p1.1).abs() < 1e-9 { return }
        let (dir, top, bot) = if p0.1 < p1.1 { (1.0f32, p0, p1) } else { (-1.0, p1, p0) };
        let dxdy = (bot.0 - top.0) / (bot.1 - top.1);
        let y_first = top.1.max(0.0);
        let y_last = bot.1.min(self.h as f32);
        if y_first >= y_last { return }
        let mut x = top.0 + dxdy * (y_first - top.1);
        let mut yf = y_first;
        let stride = self.w + 1;
        let xmax = self.w as f32 - 0.001;
        while yf < y_last {
            let yi = yf as usize;
            let dy = ((yi + 1) as f32).min(y_last) - yf;
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (xa, xb) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let xa = xa.clamp(0.0, xmax);
            let xb = xb.clamp(0.0, xmax);
            let row = yi * stride;
            let x0i = xa.floor();
            let x0 = x0i as usize;
            let x1c = xb.ceil();
            if x1c <= x0i + 1.0 {
                let xm = (xa + xb) / 2.0 - x0i;
                self.a[row + x0] += d * (1.0 - xm);
                self.a[row + x0 + 1] += d * xm;
            } else {
                let s = 1.0 / (xb - xa);
                let x0f = xa - x0i;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = xb - (x1c - 1.0);
                let am = 0.5 * s * x1f * x1f;
                let x1 = x1c as usize;
                self.a[row + x0] += d * a0;
                if x1 == x0 + 2 {
                    self.a[row + x0 + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.a[row + x0 + 1] += d * (a1 - a0);
                    for xi in x0 + 2..x1 - 1 {
                        self.a[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1 - x0 - 3) as f32 * s;
                    self.a[row + x1 - 1] += d * (1.0 - a2 - am);
                }
                self.a[row + x1] += d * am;
            }
            x = xnext;
            yf = (yi + 1) as f32;
        }
    }

    pub fn quad(&mut self, p0: (f32, f32), c: (f32, f32), p1: (f32, f32)) {
        let devx = p0.0 - 2.0 * c.0 + p1.0;
        let devy = p0.1 - 2.0 * c.1 + p1.1;
        let dev = devx * devx + devy * devy;
        let n = ((dev * 16.0).sqrt().sqrt().ceil() as usize).clamp(1, 24);
        let mut prev = p0;
        for i in 1..=n {
            let tt = i as f32 / n as f32;
            let mt = 1.0 - tt;
            let p = (
                mt * mt * p0.0 + 2.0 * mt * tt * c.0 + tt * tt * p1.0,
                mt * mt * p0.1 + 2.0 * mt * tt * c.1 + tt * tt * p1.1,
            );
            self.line(prev, p);
            prev = p;
        }
    }

    pub fn finish(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h];
        let stride = self.w + 1;
        for y in 0..self.h {
            let mut acc = 0.0f32;
            for x in 0..self.w {
                acc += self.a[y * stride + x];
                out[y * self.w + x] = (acc.abs().min(1.0) * 255.0) as u8;
            }
        }
        out
    }
}

pub fn rasterize(f: &Font, gi: u16, scale: f32, w: usize, h: usize, x_off: f32, base: f32) -> Vec<u8> {
    let mut r = Raster::new(w, h);
    f.outline(gi, [scale, 0.0, 0.0, -scale, x_off, base], &mut r, 0);
    r.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_rasterize() {
        let f = Font::parse(DATA).unwrap();
        assert!(f.units_per_em > 0);
        assert!(f.ascent > 0 && f.descent < 0);
        for cp in ['M', 'g', 'é', 'λ', '→', '\u{e0b0}'] {
            let gi = f.glyph_index(cp as u32);
            assert!(gi != 0, "no glyph for {:?}", cp);
            let scale = 26.0 / f.units_per_em as f32;
            let g = rasterize(&f, gi, scale, 16, 31, 0.0, 24.0);
            let ink: u32 = g.iter().map(|&v| v as u32).sum();
            assert!(ink > 200, "no ink for {:?}", cp);
        }
        assert_eq!(f.glyph_index(0x0378), 0);
        assert!(f.advance(f.glyph_index('M' as u32)) > 0);
    }
}
