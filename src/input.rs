use crate::vt::{self, Modes, Span, Term};

pub const SHIFT: u64 = 1 << 17;
pub const CTRL: u64 = 1 << 18;
pub const ALT: u64 = 1 << 19;
pub const CMD: u64 = 1 << 20;

pub const DOWN: u8 = 0;
pub const DRAG: u8 = 1;
pub const UP: u8 = 2;
pub const WHEEL_UP: i64 = 64;
pub const WHEEL_DOWN: i64 = 65;

pub enum Act {
    Write(Vec<u8>),
    Copy,
    Paste,
    Font(f64),
    NewWindow,
    Quit,
    Eat,
    Pass,
}

fn mod_code(m: u64) -> u32 {
    1 + (m & SHIFT != 0) as u32 + 2 * (m & ALT != 0) as u32 + 4 * (m & CTRL != 0) as u32
}

fn write(s: String) -> Act {
    Act::Write(s.into_bytes())
}

fn letter_key(l: char, mods: u64, app_mode: bool) -> Act {
    match mod_code(mods) {
        1 => write(format!("\x1b{}{}", if app_mode { 'O' } else { '[' }, l)),
        mc => write(format!("\x1b[1;{}{}", mc, l)),
    }
}

fn tilde_key(code: u32, mods: u64) -> Act {
    match mod_code(mods) {
        1 => write(format!("\x1b[{}~", code)),
        mc => write(format!("\x1b[{};{}~", code, mc)),
    }
}

pub fn key(ch: u16, mods: u64, m: &Modes) -> Act {
    if mods & CMD != 0 {
        return match ch as u8 {
            b'q' => Act::Quit,
            b'n' => Act::NewWindow,
            b'c' => Act::Copy,
            b'v' => Act::Paste,
            b'=' | b'+' => Act::Font(1.0),
            b'-' => Act::Font(-1.0),
            _ => Act::Eat,
        };
    }
    match ch {
        0xf700..=0xf703 => letter_key(['A', 'B', 'D', 'C'][(ch - 0xf700) as usize], mods, m.ckm),
        0xf729 => letter_key('H', mods, m.ckm),
        0xf72b => letter_key('F', mods, m.ckm),
        0xf704..=0xf707 => letter_key(['P', 'Q', 'R', 'S'][(ch - 0xf704) as usize], mods, true),
        0xf708..=0xf70f => tilde_key([15, 17, 18, 19, 20, 21, 23, 24][(ch - 0xf708) as usize], mods),
        0xf728 => tilde_key(3, mods),
        0xf72c => tilde_key(5, mods),
        0xf72d => tilde_key(6, mods),
        0xf710..=0xf8ff => Act::Eat,
        0x0d => Act::Write(if mods & ALT != 0 { b"\x1b\r".into() } else { b"\r".into() }),
        0x09 if mods & SHIFT != 0 => Act::Write(b"\x1b[Z".into()),
        0x09 => Act::Write(b"\t".into()),
        0x7f if mods & ALT != 0 => Act::Write(b"\x1b\x7f".into()),
        0x7f if mods & CTRL != 0 => Act::Write(b"\x08".into()),
        0x7f => Act::Write(b"\x7f".into()),
        0x1b if mods & ALT != 0 => Act::Write(b"\x1b\x1b".into()),
        0x1b => Act::Write(b"\x1b".into()),
        _ if mods & CTRL != 0 => {
            let b = match ch as u8 {
                b' ' | b'@' | b'2' => 0,
                c @ (b'a'..=b'z' | b'A'..=b'Z' | b'[' | b'\\' | b']' | b'^' | b'_') => c & 0x1f,
                b'/' => 0x1f,
                b'?' => 0x7f,
                _ => return Act::Eat,
            };
            Act::Write(if mods & ALT != 0 { vec![0x1b, b] } else { vec![b] })
        }
        _ if mods & ALT != 0 && ch >= 0x20 => {
            let mut buf = vec![0x1b];
            vt::encode_utf8(ch as u32, &mut buf);
            Act::Write(buf)
        }
        _ => Act::Pass,
    }
}

pub fn clean_paste(raw: &[u8], bracketed: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 12);
    if bracketed { out.extend(b"\x1b[200~") }
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        i += 1;
        match c {
            b'\r' if raw.get(i) == Some(&b'\n') => {}
            b'\n' => out.push(b'\r'),
            0x7f => {}
            0..=0x1f if c != b'\t' && c != b'\r' => {}
            0xc2 if matches!(raw.get(i), Some(0x80..=0x9f)) => i += 1,
            _ => out.push(c),
        }
    }
    if bracketed { out.extend(b"\x1b[201~") }
    out
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./+:@%=,".contains(&b)) {
        return s.into();
    }
    let mut q = String::with_capacity(s.len() + 2);
    q.push('\'');
    for c in s.chars() {
        if c == '\'' { q.push_str("'\\''") } else { q.push(c) }
    }
    q.push('\'');
    q
}

pub fn drop_paths<I: IntoIterator<Item = String>>(paths: I, bracketed: bool) -> Vec<u8> {
    let joined: String = paths
        .into_iter()
        .map(|p| {
            let clean: String = p.chars().filter(|c| !c.is_control()).collect();
            shell_quote(&clean)
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = Vec::new();
    if bracketed { out.extend(b"\x1b[200~") }
    out.extend(joined.as_bytes());
    out.push(b' ');
    if bracketed { out.extend(b"\x1b[201~") }
    out
}

pub fn mouse_report(btn: i64, kind: u8, mods: u64, gx: usize, gy: usize, m: &Modes) -> Option<Vec<u8>> {
    if !m.sgr_mouse { return None }
    let mut b = btn;
    if mods & SHIFT != 0 { b += 4 }
    if mods & ALT != 0 { b += 8 }
    if mods & CTRL != 0 { b += 16 }
    if kind == DRAG {
        if m.mouse < 1002 { return None }
        b += 32;
    }
    let fin = if kind == UP { 'm' } else { 'M' };
    Some(format!("\x1b[<{};{};{}{}", b, gx + 1, gy + 1, fin).into_bytes())
}

pub fn cell(px: f64, py: f64, scale: usize, pad: usize, cw: usize, ch: usize, cols: usize, rows: usize) -> (usize, usize) {
    let gx = ((px * scale as f64) as usize).saturating_sub(pad);
    let gy = ((py * scale as f64) as usize).saturating_sub(pad);
    ((gx / cw).min(cols - 1), (gy / ch).min(rows - 1))
}

#[derive(Default)]
pub struct Sel {
    pub on: Span,
    pub dragging: bool,
    anchor: (u64, usize),
    mode: u8,
}

fn word_char(cp: u32) -> bool {
    cp > 127 || (cp as u8 as char).is_ascii_alphanumeric() || b"_-./~+@$%".contains(&(cp as u8))
}

impl Sel {
    pub fn begin(&mut self, t: &Term, l: u64, c: usize, clicks: i64) {
        self.mode = (clicks.clamp(1, 3) - 1) as u8;
        self.anchor = (l, c);
        self.dragging = true;
        self.update(t, l, c);
    }

    pub fn drag(&mut self, t: &Term, l: u64, c: usize) {
        if self.dragging { self.update(t, l, c) }
    }

    pub fn finish(&mut self, l: u64, c: usize) {
        if !self.dragging { return }
        self.dragging = false;
        if self.mode == 0 && self.on == Some(((l, c), (l, c))) { self.on = None }
    }

    fn update(&mut self, t: &Term, l: u64, c: usize) {
        let (mut lo, mut hi) = if (l, c) < self.anchor { ((l, c), self.anchor) } else { (self.anchor, (l, c)) };
        if self.mode == 2 {
            lo.1 = 0;
            hi.1 = t.cols - 1;
        } else if self.mode == 1 {
            if let Some(line) = t.line_at(lo.0) {
                while lo.1 > 0 && lo.1 <= line.cells.len() && word_char(line.cells[lo.1 - 1].cp) { lo.1 -= 1 }
            }
            if let Some(line) = t.line_at(hi.0) {
                while hi.1 + 1 < line.cells.len() && word_char(line.cells[hi.1 + 1].cp) { hi.1 += 1 }
            }
        }
        self.on = Some((lo, hi));
    }

    pub fn text(&self, t: &Term) -> Option<Vec<u8>> {
        let ((al, ac), (bl, bc)) = self.on?;
        let mut out = Vec::new();
        for id in al..=bl {
            let Some(line) = t.line_at(id) else { break };
            if line.cells.is_empty() {
                if id != bl { out.push(b'\n') }
                continue;
            }
            let x0 = if id == al { ac.min(line.cells.len() - 1) } else { 0 };
            let x1 = if id == bl { bc.min(line.cells.len() - 1) } else { line.cells.len() - 1 };
            let mut keep = out.len();
            for c in &line.cells[x0..=x1] {
                if c.attr & vt::TAIL != 0 { continue }
                let cp = if c.cp == 0 { b' ' as u32 } else { c.cp };
                vt::encode_utf8(cp, &mut out);
                if cp != b' ' as u32 { keep = out.len() }
            }
            out.truncate(keep);
            if id != bl && !line.wrapped { out.push(b'\n') }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_hygiene() {
        assert_eq!(clean_paste(b"ab\x1b[201~cd", false), b"ab[201~cd");
        assert_eq!(clean_paste(b"a\xc2\x9b0;1mb", false), b"a0;1mb");
        assert_eq!(clean_paste(b"a\r\nb\nc\td", false), b"a\rb\rc\td");
        assert_eq!(clean_paste(b"x", true), b"\x1b[200~x\x1b[201~");
        assert_eq!(clean_paste(b"\xc2\xa9", false), b"\xc2\xa9");
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/usr/bin/cc"), "/usr/bin/cc");
        assert_eq!(shell_quote("/tmp/my file.txt"), "'/tmp/my file.txt'");
        assert_eq!(shell_quote("/a/it's"), "'/a/it'\\''s'");
    }

    #[test]
    fn drop_security() {
        assert_eq!(drop_paths(["/bin/ls".into()], false), b"/bin/ls ");
        // shell metacharacters are inert (single-quoted)
        assert_eq!(drop_paths(["/t/$(reboot);x".into()], false), b"'/t/$(reboot);x' ");
        // a newline in a filename must NOT survive as CR/LF (no line submission)
        let evil = drop_paths(["a\nrm -rf ~".into()], false);
        assert!(!evil.contains(&b'\n') && !evil.contains(&b'\r'));
        assert_eq!(evil, "'arm -rf ~' ".as_bytes());
        // bracketed wrapping when paste mode is on
        assert_eq!(drop_paths(["/a".into()], true), b"\x1b[200~/a \x1b[201~");
    }

    #[test]
    fn selection_text() {
        let mut t = Term::new(10, 3, 10, 0xffffff, 0);
        t.feed(b"hello \r\nworld");
        let mut s = Sel::default();
        s.begin(&t, t.line_id(0), 0, 1);
        s.drag(&t, t.line_id(1), 4);
        s.finish(t.line_id(1), 4);
        assert_eq!(s.text(&t).unwrap(), b"hello\nworld");
        let mut s = Sel::default();
        s.begin(&t, t.line_id(1), 2, 2);
        assert_eq!(s.text(&t).unwrap(), b"world");
    }
}
