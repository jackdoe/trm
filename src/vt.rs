use std::collections::VecDeque;

pub const BOLD: u32 = 1;
pub const FAINT: u32 = 2;
pub const UNDER: u32 = 8;
pub const REV: u32 = 16;
pub const STRIKE: u32 = 32;
pub const WIDE: u32 = 64;
pub const TAIL: u32 = 128;

const PALETTE: [u32; 16] = [
    0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5,
    0x666666, 0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
];

pub type Span = Option<((u64, usize), (u64, usize))>;

#[derive(Clone, Copy)]
pub struct Cell {
    pub cp: u32,
    pub fg: u32,
    pub bg: u32,
    pub attr: u32,
}

#[derive(Clone)]
pub struct Line {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

#[derive(Default, Clone, Copy)]
pub struct Modes {
    pub ckm: bool,
    pub awm: bool,
    pub tcem: bool,
    pub origin: bool,
    pub paste: bool,
    pub focus: bool,
    pub sync: bool,
    pub mouse: u32,
    pub sgr_mouse: bool,
}

#[derive(Default, Clone, Copy)]
struct Saved {
    x: usize,
    y: usize,
    fg: u32,
    bg: u32,
    attr: u32,
    g0: bool,
    g1: bool,
    cs: usize,
    origin: bool,
    set: bool,
}

#[derive(PartialEq, Clone, Copy)]
enum St {
    Ground,
    Esc,
    EscInt(u8),
    Csi,
    CsiInt,
    Osc,
    Str,
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    screens: [Vec<Line>; 2],
    pub alt: usize,
    pub sb: VecDeque<Line>,
    sb_cap: usize,
    pub sb_base: u64,
    pub view: usize,
    pub dirty: Vec<bool>,
    pub all_dirty: bool,
    pub x: usize,
    pub y: usize,
    pub fg: u32,
    pub bg: u32,
    pub attr: u32,
    pub def_fg: u32,
    pub def_bg: u32,
    top: usize,
    bot: usize,
    pend: bool,
    pub modes: Modes,
    saved: [Saved; 2],
    tabs: Vec<bool>,
    g0: bool,
    g1: bool,
    cs: usize,
    last: u32,
    st: St,
    params: [u32; 16],
    np: usize,
    seen: bool,
    colon: u32,
    privc: u8,
    inter: u8,
    osc: Vec<u8>,
    esc_str: bool,
    ucp: u32,
    uneed: u8,
    uhave: u8,
    pub out: Vec<u8>,
    pub title: Option<String>,
}

const ZERO_W: &[(u32, u32)] = &[
    (0x300, 0x36f), (0x483, 0x489), (0x591, 0x5c7), (0x610, 0x61a), (0x64b, 0x65f),
    (0x670, 0x670), (0x6d6, 0x6dc), (0x6df, 0x6e4), (0x816, 0x82d), (0x900, 0x902),
    (0x93c, 0x93c), (0x941, 0x948), (0x1ab0, 0x1aff), (0x1dc0, 0x1dff), (0x200b, 0x200f),
    (0x202a, 0x202e), (0x2060, 0x2064), (0x20d0, 0x20ff), (0xfe00, 0xfe0f),
    (0xfe20, 0xfe2f), (0xfeff, 0xfeff), (0xe0100, 0xe01ef),
];

const WIDE_W: &[(u32, u32)] = &[
    (0x1100, 0x115f), (0x2329, 0x232a), (0x2e80, 0x303e), (0x3041, 0x33ff),
    (0x3400, 0x4dbf), (0x4e00, 0x9fff), (0xa000, 0xa4cf), (0xa960, 0xa97f),
    (0xac00, 0xd7a3), (0xf900, 0xfaff), (0xfe10, 0xfe19), (0xfe30, 0xfe6f),
    (0xff00, 0xff60), (0xffe0, 0xffe6), (0x1f300, 0x1f64f), (0x1f680, 0x1f6ff),
    (0x1f900, 0x1faff), (0x20000, 0x2fffd), (0x30000, 0x3fffd),
];

fn in_ranges(r: &[(u32, u32)], cp: u32) -> bool {
    r.binary_search_by(|&(lo, hi)| {
        if hi < cp { std::cmp::Ordering::Less } else if lo > cp { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Equal }
    })
    .is_ok()
}

pub fn width(cp: u32) -> usize {
    if cp < 0x300 { 1 } else if in_ranges(ZERO_W, cp) { 0 } else if in_ranges(WIDE_W, cp) { 2 } else { 1 }
}

pub fn encode_utf8(cp: u32, buf: &mut Vec<u8>) {
    let c = char::from_u32(cp).unwrap_or('\u{fffd}');
    let mut b = [0u8; 4];
    buf.extend(c.encode_utf8(&mut b).as_bytes());
}

const GFX: [u16; 31] = [
    0x25c6, 0x2592, 0x2409, 0x240c, 0x240d, 0x240a, 0x00b0, 0x00b1, 0x2424, 0x240b,
    0x2518, 0x2510, 0x250c, 0x2514, 0x253c, 0x23ba, 0x23bb, 0x2500, 0x23bc, 0x23bd,
    0x251c, 0x2524, 0x2534, 0x252c, 0x2502, 0x2264, 0x2265, 0x03c0, 0x2260, 0x00a3, 0x00b7,
];

impl Term {
    pub fn new(cols: usize, rows: usize, sb_cap: usize, fg: u32, bg: u32) -> Term {
        let blank = Cell { cp: 0, fg, bg, attr: 0 };
        let mk = || (0..rows).map(|_| Line { cells: vec![blank; cols], wrapped: false }).collect();
        let mut t = Term {
            cols, rows,
            screens: [mk(), mk()],
            alt: 0,
            sb: VecDeque::new(),
            sb_cap, sb_base: 0, view: 0,
            dirty: vec![true; rows],
            all_dirty: true,
            x: 0, y: 0, fg, bg, attr: 0, def_fg: fg, def_bg: bg,
            top: 0, bot: rows - 1, pend: false,
            modes: Modes { awm: true, tcem: true, ..Default::default() },
            saved: [Saved::default(); 2],
            tabs: vec![],
            g0: false, g1: false, cs: 0, last: 0,
            st: St::Ground,
            params: [0; 16], np: 0, seen: false, colon: 0, privc: 0, inter: 0,
            osc: vec![], esc_str: false,
            ucp: 0, uneed: 0, uhave: 0,
            out: vec![],
            title: None,
        };
        t.reset_tabs();
        t
    }

    fn blank(&self) -> Cell {
        Cell { cp: 0, fg: self.fg, bg: self.bg, attr: 0 }
    }

    fn reset_tabs(&mut self) {
        self.tabs = (0..self.cols).map(|i| i % 8 == 0).collect();
    }

    pub fn mark(&mut self, y: usize) {
        if y < self.rows { self.dirty[y] = true }
    }

    #[cfg(test)]
    pub fn line(&self, y: usize) -> &Line {
        &self.screens[self.alt][y]
    }

    fn line_mut(&mut self, y: usize) -> &mut Line {
        &mut self.screens[self.alt][y]
    }

    pub fn line_id(&self, d: usize) -> u64 {
        self.sb_base + (self.sb.len() - self.view + d) as u64
    }

    pub fn line_at(&self, id: u64) -> Option<&Line> {
        let rel = id.checked_sub(self.sb_base)? as usize;
        if rel < self.sb.len() { self.sb.get(rel) } else { self.screens[self.alt].get(rel - self.sb.len()) }
    }

    fn clear_cells(&mut self, y: usize, x0: usize, x1: usize) {
        let b = self.blank();
        let cols = self.cols;
        let line = self.line_mut(y);
        for c in &mut line.cells[x0.min(cols)..x1.min(cols)] { *c = b }
        self.mark(y);
    }

    fn non_blank(c: &Cell, def_bg: u32) -> bool {
        (c.cp != 0 && c.cp != b' ' as u32) || c.bg != def_bg || c.attr & (UNDER | STRIKE | REV | TAIL) != 0
    }

    fn trim_blank(cells: &mut Vec<Cell>, def_bg: u32) {
        while cells.last().is_some_and(|c| !Self::non_blank(c, def_bg)) { cells.pop(); }
    }

    fn sb_push(&mut self, mut line: Line) {
        if self.sb_cap == 0 || self.alt != 0 { return }
        if !line.wrapped { Self::trim_blank(&mut line.cells, self.def_bg) }
        if self.sb.len() == self.sb_cap {
            self.sb.pop_front();
            self.sb_base += 1;
        }
        self.sb.push_back(line);
        if self.view > 0 { self.view = (self.view + 1).min(self.sb.len()) }
    }

    fn scroll_up(&mut self, top: usize, bot: usize, n: usize, to_sb: bool) {
        let n = n.min(bot - top + 1);
        if n == 0 { return }
        if to_sb && self.alt == 0 && top == 0 && bot == self.rows - 1 {
            for i in 0..n {
                let l = self.screens[self.alt][top + i].clone();
                self.sb_push(l);
            }
        }
        let b = self.blank();
        self.screens[self.alt][top..=bot].rotate_left(n);
        for l in &mut self.screens[self.alt][bot + 1 - n..=bot] {
            l.cells.fill(b);
            l.wrapped = false;
        }
        self.all_dirty = true;
    }

    fn scroll_down(&mut self, top: usize, bot: usize, n: usize) {
        let n = n.min(bot - top + 1);
        if n == 0 { return }
        let b = self.blank();
        self.screens[self.alt][top..=bot].rotate_right(n);
        for l in &mut self.screens[self.alt][top..top + n] {
            l.cells.fill(b);
            l.wrapped = false;
        }
        self.all_dirty = true;
    }

    fn use_alt(&mut self, alt: bool) {
        if self.alt == alt as usize { return }
        self.alt = alt as usize;
        if alt {
            let b = self.blank();
            for l in &mut self.screens[1] {
                l.cells.fill(b);
                l.wrapped = false;
            }
        }
        self.view = 0;
        self.all_dirty = true;
    }

    fn move_to(&mut self, y: i64, x: i64) {
        let (ymin, ymax) = if self.modes.origin { (self.top as i64, self.bot as i64) } else { (0, self.rows as i64 - 1) };
        self.y = y.clamp(ymin, ymax) as usize;
        self.x = x.clamp(0, self.cols as i64 - 1) as usize;
        self.pend = false;
    }

    fn linefeed(&mut self) {
        if self.y == self.bot {
            self.scroll_up(self.top, self.bot, 1, true);
        } else if self.y < self.rows - 1 {
            self.y += 1;
            self.mark(self.y);
        }
        self.pend = false;
    }

    fn reverse_index(&mut self) {
        if self.y == self.top { self.scroll_down(self.top, self.bot, 1) } else if self.y > 0 { self.y -= 1 }
        self.pend = false;
    }

    fn unlink_wide(&mut self, y: usize, x: usize) {
        let cols = self.cols;
        let line = self.line_mut(y);
        if line.cells[x].attr & TAIL != 0 && x > 0 {
            line.cells[x - 1].cp = b' ' as u32;
            line.cells[x - 1].attr &= !WIDE;
        }
        if line.cells[x].attr & WIDE != 0 && x + 1 < cols {
            line.cells[x + 1].cp = b' ' as u32;
            line.cells[x + 1].attr &= !TAIL;
        }
    }

    fn put_char(&mut self, cp: u32) {
        let cp = self.map_charset(cp);
        let w = width(cp);
        if w == 0 { return }
        if self.pend && self.modes.awm {
            self.line_mut(self.y).wrapped = true;
            self.x = 0;
            self.linefeed();
        }
        self.pend = false;
        if w == 2 && self.x >= self.cols - 1 {
            if self.modes.awm {
                self.line_mut(self.y).wrapped = true;
                self.x = 0;
                self.linefeed();
            } else {
                self.x = self.cols.saturating_sub(2);
            }
        }
        self.unlink_wide(self.y, self.x);
        let cell = Cell { cp, fg: self.fg, bg: self.bg, attr: self.attr | if w == 2 { WIDE } else { 0 } };
        let (x, y) = (self.x, self.y);
        self.line_mut(y).cells[x] = cell;
        if w == 2 && x + 1 < self.cols {
            self.unlink_wide(y, x + 1);
            self.line_mut(y).cells[x + 1] = Cell { cp: 0, fg: self.fg, bg: self.bg, attr: self.attr | TAIL };
        }
        self.mark(y);
        self.last = cp;
        if x + w >= self.cols {
            self.x = self.cols - 1;
            self.pend = true;
        } else {
            self.x += w;
        }
    }

    fn map_charset(&self, cp: u32) -> u32 {
        let gfx = if self.cs == 1 { self.g1 } else { self.g0 };
        if gfx && (0x60..=0x7e).contains(&cp) { GFX[(cp - 0x60) as usize] as u32 } else { cp }
    }

    fn tab_forward(&mut self, n: usize) {
        for _ in 0..n {
            self.x += 1;
            while self.x < self.cols && !self.tabs[self.x] { self.x += 1 }
            if self.x >= self.cols { self.x = self.cols - 1; break }
        }
        self.pend = false;
    }

    fn tab_back(&mut self, n: usize) {
        for _ in 0..n {
            while self.x > 0 { self.x -= 1; if self.tabs[self.x] { break } }
        }
        self.pend = false;
    }

    fn ctrl(&mut self, b: u8) {
        match b {
            8 => { self.x = self.x.saturating_sub(1); self.pend = false }
            9 => self.tab_forward(1),
            10..=12 => self.linefeed(),
            13 => { self.x = 0; self.pend = false }
            14 => self.cs = 1,
            15 => self.cs = 0,
            _ => {}
        }
    }

    fn save_cursor(&mut self) {
        self.saved[self.alt] = Saved {
            x: self.x, y: self.y, fg: self.fg, bg: self.bg, attr: self.attr,
            g0: self.g0, g1: self.g1, cs: self.cs, origin: self.modes.origin, set: true,
        };
    }

    fn restore_cursor(&mut self) {
        let s = self.saved[self.alt];
        if !s.set { self.move_to(0, 0); return }
        self.fg = s.fg;
        self.bg = s.bg;
        self.attr = s.attr;
        self.g0 = s.g0;
        self.g1 = s.g1;
        self.cs = s.cs;
        self.modes.origin = s.origin;
        self.move_to(s.y as i64, s.x as i64);
    }

    fn sgr_reset(&mut self) {
        self.fg = self.def_fg;
        self.bg = self.def_bg;
        self.attr = 0;
    }

    fn reset_all(&mut self) {
        self.use_alt(false);
        self.sgr_reset();
        for y in 0..self.rows { self.clear_cells(y, 0, self.cols) }
        self.top = 0;
        self.bot = self.rows - 1;
        self.modes = Modes { awm: true, tcem: true, ..Default::default() };
        self.g0 = false;
        self.g1 = false;
        self.cs = 0;
        self.saved = [Saved::default(); 2];
        self.reset_tabs();
        self.move_to(0, 0);
        self.all_dirty = true;
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if cols == self.cols && rows == self.rows { return }
        let blank = Cell { cp: 0, fg: self.def_fg, bg: self.def_bg, attr: 0 };
        for scr in &mut self.screens {
            for l in scr.iter_mut() { l.cells.resize(cols, blank) }
            scr.resize_with(rows, || Line { cells: vec![blank; cols], wrapped: false });
        }
        self.cols = cols;
        self.rows = rows;
        self.dirty.resize(rows, true);
        self.reset_tabs();
        self.top = 0;
        self.bot = rows - 1;
        self.x = self.x.min(cols - 1);
        self.y = self.y.min(rows - 1);
        self.view = 0;
        self.pend = false;
        self.all_dirty = true;
    }

    fn palette(&self, idx: u32) -> u32 {
        match idx {
            0..=15 => PALETTE[idx as usize],
            16..=231 => {
                let c = idx - 16;
                let f = |v: u32| if v > 0 { v * 40 + 55 } else { 0 };
                f(c / 36) << 16 | f(c / 6 % 6) << 8 | f(c % 6)
            }
            232..=255 => {
                let v = (idx - 232) * 10 + 8;
                v << 16 | v << 8 | v
            }
            _ => self.def_fg,
        }
    }

    fn param(&self, i: usize, def: u32) -> u32 {
        let count = self.np + (self.seen || self.np > 0) as usize;
        if i >= count || self.params[i] == 0 { def } else { self.params[i] }
    }

    fn param_count(&self) -> usize {
        self.np + (self.seen || self.np > 0) as usize
    }

    fn colon_run(&self, i: usize) -> usize {
        (i + 1..16).take_while(|&j| self.colon >> j & 1 != 0).count()
    }

    fn sgr_color(&self, count: usize, i: usize) -> (usize, Option<u32>) {
        let run = self.colon_run(i);
        if i + 1 >= count { return (1, None) }
        match self.params[i + 1] {
            5 => (3, (i + 2 < count).then(|| self.palette(self.params[i + 2]))),
            2 => {
                let base = if run >= 5 { i + 3 } else { i + 2 };
                let rgb = (base + 2 < count).then(|| {
                    (self.params[base] & 255) << 16 | (self.params[base + 1] & 255) << 8 | (self.params[base + 2] & 255)
                });
                (base + 3 - i, rgb)
            }
            _ => (2, None),
        }
    }

    fn sgr(&mut self) {
        let count = self.param_count();
        if count == 0 { self.sgr_reset(); return }
        let mut i = 0;
        while i < count {
            match self.params[i] {
                0 => self.sgr_reset(),
                1 => self.attr |= BOLD,
                2 => self.attr |= FAINT,
                4 => {
                    let run = self.colon_run(i);
                    if run > 0 && self.params[i + 1] == 0 { self.attr &= !UNDER } else { self.attr |= UNDER }
                    i += run;
                }
                7 => self.attr |= REV,
                9 => self.attr |= STRIKE,
                21 => self.attr |= UNDER,
                22 => self.attr &= !(BOLD | FAINT),
                24 => self.attr &= !UNDER,
                27 => self.attr &= !REV,
                29 => self.attr &= !STRIKE,
                38 => { let (adv, c) = self.sgr_color(count, i); if let Some(c) = c { self.fg = c } i += adv - 1 }
                39 => self.fg = self.def_fg,
                48 => { let (adv, c) = self.sgr_color(count, i); if let Some(c) = c { self.bg = c } i += adv - 1 }
                49 => self.bg = self.def_bg,
                p @ 30..=37 => self.fg = self.palette(p - 30),
                p @ 40..=47 => self.bg = self.palette(p - 40),
                p @ 90..=97 => self.fg = self.palette(p - 82),
                p @ 100..=107 => self.bg = self.palette(p - 92),
                _ => {}
            }
            i += 1;
        }
    }

    fn mode_value(&self, m: u32) -> Option<bool> {
        Some(match m {
            1 => self.modes.ckm,
            6 => self.modes.origin,
            7 => self.modes.awm,
            25 => self.modes.tcem,
            47 | 1047 | 1049 => self.alt != 0,
            1000 | 1002 | 1003 => self.modes.mouse == m,
            1004 => self.modes.focus,
            1006 => self.modes.sgr_mouse,
            2004 => self.modes.paste,
            2026 => self.modes.sync,
            _ => return None,
        })
    }

    fn dec_mode(&mut self, m: u32, set: bool) {
        match m {
            1 => self.modes.ckm = set,
            6 => {
                self.modes.origin = set;
                self.move_to(if set { self.top as i64 } else { 0 }, 0);
            }
            7 => self.modes.awm = set,
            25 => { self.modes.tcem = set; self.mark(self.y) }
            47 | 1047 => self.use_alt(set),
            1048 => if set { self.save_cursor() } else { self.restore_cursor() },
            1049 => {
                if set { self.save_cursor(); self.use_alt(true) } else { self.use_alt(false); self.restore_cursor() }
            }
            1000 | 1002 | 1003 => {
                if set { self.modes.mouse = m } else if self.modes.mouse == m { self.modes.mouse = 0 }
            }
            1004 => self.modes.focus = set,
            1006 => self.modes.sgr_mouse = set,
            2004 => self.modes.paste = set,
            2026 => self.modes.sync = set,
            _ => {}
        }
    }

    fn csi(&mut self, fin: u8) {
        let n = self.param(0, 1) as usize;
        let ni = n as i64;
        match fin {
            b'@' => {
                let n = n.min(self.cols - self.x);
                let (x, cols) = (self.x, self.cols);
                self.line_mut(self.y).cells[x..cols].rotate_right(n);
                self.clear_cells(self.y, x, x + n);
            }
            b'A' => { let lim = if self.y >= self.top { self.top } else { 0 }; self.y = self.y.saturating_sub(n).max(lim); self.pend = false }
            b'B' | b'e' => { let lim = if self.y <= self.bot { self.bot } else { self.rows - 1 }; self.y = (self.y + n).min(lim); self.pend = false }
            b'C' | b'a' => { self.x = (self.x + n).min(self.cols - 1); self.pend = false }
            b'D' => { self.x = self.x.saturating_sub(n); self.pend = false }
            b'E' => { self.x = 0; for _ in 0..n { self.linefeed() } }
            b'F' => { self.x = 0; for _ in 0..n { self.reverse_index() } }
            b'G' | b'`' => { let y = self.y as i64 - if self.modes.origin { self.top as i64 } else { 0 }; self.move_to(y, ni - 1) }
            b'H' | b'f' => self.move_to(self.param(0, 1) as i64 - 1, self.param(1, 1) as i64 - 1),
            b'I' => self.tab_forward(n),
            b'J' => match self.param(0, 0) {
                0 => {
                    self.clear_cells(self.y, self.x, self.cols);
                    for y in self.y + 1..self.rows { self.clear_cells(y, 0, self.cols) }
                }
                1 => {
                    for y in 0..self.y { self.clear_cells(y, 0, self.cols) }
                    self.clear_cells(self.y, 0, self.x + 1);
                }
                2 => for y in 0..self.rows { self.clear_cells(y, 0, self.cols) },
                3 => {
                    self.sb_base += self.sb.len() as u64;
                    self.sb.clear();
                    self.view = 0;
                    self.all_dirty = true;
                }
                _ => {}
            },
            b'K' => match self.param(0, 0) {
                0 => self.clear_cells(self.y, self.x, self.cols),
                1 => self.clear_cells(self.y, 0, self.x + 1),
                2 => self.clear_cells(self.y, 0, self.cols),
                _ => {}
            },
            b'L' => if (self.top..=self.bot).contains(&self.y) { self.scroll_down(self.y, self.bot, n) },
            b'M' => if (self.top..=self.bot).contains(&self.y) { self.scroll_up(self.y, self.bot, n, false) },
            b'P' => {
                let n = n.min(self.cols - self.x);
                let (x, cols) = (self.x, self.cols);
                self.line_mut(self.y).cells[x..cols].rotate_left(n);
                self.clear_cells(self.y, cols - n, cols);
            }
            b'S' => self.scroll_up(self.top, self.bot, n, true),
            b'T' => self.scroll_down(self.top, self.bot, n),
            b'X' => self.clear_cells(self.y, self.x, self.x + n),
            b'Z' => self.tab_back(n),
            b'b' => {
                if self.last != 0 {
                    for _ in 0..n.min(self.cols * self.rows) { self.put_char(self.last) }
                }
            }
            b'c' => {
                if self.privc == b'>' { self.out.extend(b"\x1b[>0;0;0c") }
                else if self.privc == 0 && self.param(0, 0) == 0 { self.out.extend(b"\x1b[?6c") }
            }
            b'd' => self.move_to(ni - 1, self.x as i64),
            b'g' => {
                if self.param(0, 0) == 3 { self.tabs.fill(false) } else { self.tabs[self.x] = false }
            }
            b'h' | b'l' => {
                if self.privc == b'?' {
                    for i in 0..self.param_count() { self.dec_mode(self.params[i], fin == b'h') }
                }
            }
            b'm' => if self.privc == 0 { self.sgr() },
            b'n' => match self.param(0, 0) {
                5 => self.out.extend(b"\x1b[0n"),
                6 => {
                    let r = self.y - if self.modes.origin { self.top } else { 0 } + 1;
                    self.out.extend(format!("\x1b[{};{}R", r, self.x + 1).as_bytes());
                }
                _ => {}
            },
            b'p' => {
                if self.inter == b'!' {
                    self.modes.tcem = true;
                    self.modes.origin = false;
                    self.modes.awm = true;
                    self.modes.ckm = false;
                    self.top = 0;
                    self.bot = self.rows - 1;
                    self.sgr_reset();
                    self.g0 = false;
                    self.g1 = false;
                    self.cs = 0;
                    self.saved = [Saved::default(); 2];
                } else if self.inter == b'$' && self.privc == b'?' {
                    let m = self.param(0, 0);
                    let v = match self.mode_value(m) { Some(true) => 1, Some(false) => 2, None => 0 };
                    self.out.extend(format!("\x1b[?{};{}$y", m, v).as_bytes());
                }
            }
            b'r' => {
                if self.privc == 0 {
                    let top = (self.param(0, 1) as usize).saturating_sub(1);
                    let bot = (self.param(1, self.rows as u32) as usize - 1).min(self.rows - 1);
                    if top < bot {
                        self.top = top;
                        self.bot = bot;
                        self.move_to(if self.modes.origin { top as i64 } else { 0 }, 0);
                    }
                }
            }
            b's' => if self.privc == 0 { self.save_cursor() },
            b'u' => if self.privc == 0 { self.restore_cursor() },
            _ => {}
        }
    }

    fn osc_dispatch(&mut self) {
        let osc = std::mem::take(&mut self.osc);
        let mut it = osc.splitn(2, |&b| b == b';');
        let num = it.next().unwrap_or(&[]);
        if !num.iter().all(|b| b.is_ascii_digit()) { return }
        let cmd: u32 = std::str::from_utf8(num).ok().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
        if let (0 | 2, Some(payload)) = (cmd, it.next()) {
            let mut s = String::from_utf8_lossy(payload).into_owned();
            s.retain(|c| !c.is_control());
            s.truncate(255);
            self.title = Some(s);
        }
    }

    fn esc(&mut self, b: u8) {
        self.st = St::Ground;
        match b {
            b'[' => {
                self.params = [0; 16];
                self.np = 0;
                self.seen = false;
                self.colon = 0;
                self.privc = 0;
                self.inter = 0;
                self.st = St::Csi;
            }
            b']' => { self.osc.clear(); self.esc_str = false; self.st = St::Osc }
            b'P' | b'X' | b'^' | b'_' => { self.esc_str = false; self.st = St::Str }
            b'(' | b')' | b'#' => self.st = St::EscInt(b),
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.linefeed(),
            b'E' => { self.x = 0; self.linefeed() }
            b'H' => self.tabs[self.x] = true,
            b'M' => self.reverse_index(),
            b'c' => self.reset_all(),
            0x1b => self.st = St::Esc,
            _ => {}
        }
    }

    fn step(&mut self, b: u8) {
        match self.st {
            St::Ground => {
                if self.uneed > 0 {
                    match self.utf8_push(b) {
                        Ok(Some(cp)) => self.put_char(cp),
                        Ok(None) => {}
                        Err(()) => {
                            self.put_char(0xfffd);
                            if b < 0x80 || b >= 0xc0 { self.step(b) }
                        }
                    }
                    return;
                }
                match b {
                    0x1b => self.st = St::Esc,
                    0..=0x1f | 0x7f => self.ctrl(b),
                    0x20..=0x7e => self.put_char(b as u32),
                    _ => match self.utf8_push(b) {
                        Ok(Some(cp)) => self.put_char(cp),
                        Ok(None) => {}
                        Err(()) => self.put_char(0xfffd),
                    },
                }
            }
            St::Esc => self.esc(b),
            St::EscInt(i) => {
                match i {
                    b'(' => self.g0 = b == b'0',
                    b')' => self.g1 = b == b'0',
                    _ => {}
                }
                self.st = St::Ground;
            }
            St::Csi | St::CsiInt => {
                match b {
                    0x1b => { self.st = St::Esc; return }
                    0x18 | 0x1a => { self.st = St::Ground; return }
                    0..=0x1f => { self.ctrl(b); return }
                    _ => {}
                }
                if self.st == St::Csi {
                    match b {
                        b'0'..=b'9' => {
                            self.params[self.np] = (self.params[self.np] * 10 + (b - b'0') as u32).min(65535);
                            self.seen = true;
                            return;
                        }
                        b';' | b':' => {
                            if self.np < 15 {
                                self.np += 1;
                                if b == b':' { self.colon |= 1 << self.np }
                            }
                            self.seen = true;
                            return;
                        }
                        b'<'..=b'?' => {
                            if !self.seen && self.np == 0 { self.privc = b }
                            return;
                        }
                        _ => {}
                    }
                }
                match b {
                    0x20..=0x2f => { self.inter = b; self.st = St::CsiInt }
                    0x40..=0x7e => { self.st = St::Ground; self.csi(b) }
                    _ => {}
                }
            }
            St::Osc => {
                if self.esc_str {
                    self.esc_str = false;
                    self.st = St::Ground;
                    if b == b'\\' { self.osc_dispatch() } else { self.st = St::Esc; self.step(b) }
                    return;
                }
                match b {
                    7 => { self.st = St::Ground; self.osc_dispatch() }
                    0x1b => self.esc_str = true,
                    0x18 | 0x1a => self.st = St::Ground,
                    _ => if self.osc.len() < 4096 { self.osc.push(b) },
                }
            }
            St::Str => {
                if self.esc_str {
                    self.esc_str = b == 0x1b;
                    if !self.esc_str { self.st = St::Ground }
                    return;
                }
                match b {
                    0x1b => self.esc_str = true,
                    7 | 0x18 | 0x1a => self.st = St::Ground,
                    _ => {}
                }
            }
        }
    }

    fn utf8_push(&mut self, b: u8) -> Result<Option<u32>, ()> {
        if self.uneed == 0 {
            match b {
                0..=0x7f => return Ok(Some(b as u32)),
                0xc0..=0xdf => { self.ucp = (b & 0x1f) as u32; self.uneed = 1 }
                0xe0..=0xef => { self.ucp = (b & 0x0f) as u32; self.uneed = 2 }
                0xf0..=0xf7 => { self.ucp = (b & 0x07) as u32; self.uneed = 3 }
                _ => return Err(()),
            }
            self.uhave = 0;
            return Ok(None);
        }
        if b & 0xc0 != 0x80 { self.uneed = 0; return Err(()) }
        self.ucp = self.ucp << 6 | (b & 0x3f) as u32;
        self.uhave += 1;
        if self.uhave < self.uneed { return Ok(None) }
        let (cp, n) = (self.ucp, self.uneed);
        self.uneed = 0;
        let bad = (n == 1 && cp < 0x80) || (n == 2 && cp < 0x800) || (n == 3 && cp < 0x10000)
            || (0xd800..=0xdfff).contains(&cp) || cp > 0x10ffff;
        Ok(Some(if bad { 0xfffd } else { cp }))
    }

    fn put_run(&mut self, run: &[u8]) {
        let mut i = 0;
        while i < run.len() {
            if self.pend || self.x >= self.cols {
                self.put_char(run[i] as u32);
                i += 1;
                continue;
            }
            let n = (self.cols - self.x).min(run.len() - i);
            let (x, y) = (self.x, self.y);
            self.unlink_wide(y, x);
            self.unlink_wide(y, x + n - 1);
            let (fg, bg, attr) = (self.fg, self.bg, self.attr);
            for (cell, &b) in self.line_mut(y).cells[x..x + n].iter_mut().zip(&run[i..i + n]) {
                *cell = Cell { cp: b as u32, fg, bg, attr };
            }
            self.mark(y);
            self.last = run[i + n - 1] as u32;
            self.x += n;
            if self.x >= self.cols {
                self.x = self.cols - 1;
                self.pend = true;
            }
            i += n;
        }
    }

    pub fn feed(&mut self, buf: &[u8]) {
        let mut i = 0;
        while i < buf.len() {
            let b = buf[i];
            if b >= 0x20 && b < 0x7f && self.st == St::Ground && self.uneed == 0
                && !if self.cs == 1 { self.g1 } else { self.g0 }
            {
                let end = buf[i..]
                    .iter()
                    .position(|&c| !(0x20..0x7f).contains(&c))
                    .map_or(buf.len(), |p| i + p);
                self.put_run(&buf[i..end]);
                i = end;
            } else {
                self.step(b);
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(t: &Term) -> String {
        let mut s = String::new();
        for y in 0..t.rows {
            let mut row = String::new();
            let mut keep = 0;
            for c in &t.line(y).cells {
                if c.attr & TAIL != 0 { continue }
                if c.cp == 0 { row.push('.') } else {
                    row.push(char::from_u32(c.cp).unwrap());
                    keep = row.len();
                }
            }
            row.truncate(keep);
            s.push_str(&row);
            if y < t.rows - 1 { s.push('\n') }
        }
        s + &format!("@{},{}", t.y, t.x)
    }

    fn run(cols: usize, rows: usize, input: &str) -> Term {
        let mut t = Term::new(cols, rows, 100, 0xffffff, 0);
        t.feed(input.as_bytes());
        t
    }

    #[test]
    fn golden() {
        for (cols, rows, input, want) in [
            (10, 3, "hello", "hello\n\n@0,5"),
            (10, 3, "ab\r\ncd", "ab\ncd\n@1,2"),
            (10, 3, "0123456789X", "0123456789\nX\n@1,1"),
            (10, 3, "0123456789", "0123456789\n\n@0,9"),
            (10, 3, "\x1b[2;2HX", "\n.X\n@1,2"),
            (10, 3, "junk\x1b[2J\x1b[2;2HX", "\n.X\n@1,2"),
            (10, 3, "abcdef\x1b[1;3H\x1b[K", "ab\n\n@0,2"),
            (10, 4, "\x1b[2;3r\x1b[3;1Ha\nb", "\na\n.b\n@2,2"),
            (10, 3, "A\x1b[?1049hZAP\x1b[?1049l", "A\n\n@0,1"),
            (10, 3, "abcd\r\x1b[2@\x1b[2P", "abcd\n\n@0,0"),
            (10, 4, "a\r\nb\r\nc\x1b[2;1H\x1b[1L", "a\n\nb\nc@1,0"),
            (8, 3, "漢X", "漢X\n\n@0,3"),
            (4, 3, "abc漢", "abc\n漢\n@1,2"),
            (10, 3, "\u{1b}(0qx\u{1b}(B", "─│\n\n@0,2"),
            (10, 3, "a\x1b[3b", "aaaa\n\n@0,4"),
        ] {
            assert_eq!(dump(&run(cols, rows, input)), want, "input {:?}", input);
        }
        let mut t = Term::new(10, 3, 100, 0xffffff, 0);
        t.feed(b"\xc3(z");
        assert_eq!(dump(&t), "\u{fffd}(z\n\n@0,3");
    }

    #[test]
    fn responses() {
        for (input, want) in [
            ("\x1b[c", "\x1b[?6c"),
            ("\x1b[>c", "\x1b[>0;0;0c"),
            ("\x1b[5n", "\x1b[0n"),
            ("\x1b[5;7H\x1b[6n", "\x1b[5;7R"),
            ("\x1b[?2026$p", "\x1b[?2026;2$y"),
            ("\x1b[?2026h\x1b[?2026$p", "\x1b[?2026;1$y"),
            ("\x1b[?999$p", "\x1b[?999;0$y"),
        ] {
            let t = run(80, 24, input);
            assert_eq!(String::from_utf8_lossy(&t.out), want, "input {:?}", input);
        }
    }

    #[test]
    fn colors_scrollback_title() {
        let t = run(10, 2, "\x1b[31mA\x1b[48;2;1;2;3mB\x1b[38;5;255mC\x1b[38:2::10:20:30mD");
        let row = &t.line(0).cells;
        assert_eq!(row[0].fg, 0xcd3131);
        assert_eq!(row[1].bg, 0x010203);
        assert_eq!(row[2].fg, 0xeeeeee);
        assert_eq!(row[3].fg, 0x0a141e);

        let t = run(10, 2, "a\r\nb\r\nc");
        assert_eq!(t.sb.len(), 1);
        assert_eq!(t.sb[0].cells[0].cp, b'a' as u32);

        let t = run(10, 2, "\x1b]2;hi there\x07");
        assert_eq!(t.title.as_deref(), Some("hi there"));
    }

    #[test]
    fn bench_feed() {
        let mut t = Term::new(120, 40, 1000, 0xffffff, 0);
        let line = "the quick brown fox jumps over the lazy dog 0123456789 !@#$%^&*() ~fin\r\n".repeat(1000);
        let n = 200;
        let start = std::time::Instant::now();
        for _ in 0..n { t.feed(line.as_bytes()) }
        let mbs = (line.len() * n) as f64 / start.elapsed().as_secs_f64() / 1e6;
        println!("feed: {:.0} MB/s", mbs);
    }

    #[test]
    fn fuzz_smoke() {
        let mut state = 0x9e3779b97f4a7c15u64;
        let mut rng = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let n = 512 + (rng() % 2048) as usize;
            let buf: Vec<u8> = (0..n).map(|_| (rng() >> 8) as u8).collect();
            let mut t = Term::new((rng() % 120 + 2) as usize, (rng() % 60 + 2) as usize, 50, 0xffffff, 0);
            t.feed(b"\x1b[");
            t.feed(&buf);
            t.resize((rng() % 120 + 2) as usize, (rng() % 60 + 2) as usize);
            t.feed(&buf[..n / 2]);
        }
    }
}
