#[macro_use]
mod ffi;
#[forbid(unsafe_code)]
mod font;
#[forbid(unsafe_code)]
mod input;
#[forbid(unsafe_code)]
mod render;
#[forbid(unsafe_code)]
mod vt;

use ffi::*;
use input::Act;
use render::{Atlas, Fb};
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::mem::transmute;
use std::ptr::{null, null_mut};
use vt::Term;

const FONT_PT: f64 = 13.0;
const GAMMA: f64 = 0.45;
const COLS: usize = 100;
const ROWS: usize = 30;
const SCROLLBACK: usize = 10000;
const FG: u32 = 0xffffff;
const BG: u32 = 0x000000;

pub fn sel(s: &str) -> SEL {
    let c = CString::new(s).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}

fn cls(s: &str) -> Id {
    let c = CString::new(s).unwrap();
    unsafe { objc_getClass(c.as_ptr()) }
}

fn nsstr(s: &str) -> Id {
    let c = CString::new(s.replace('\0', "")).unwrap();
    unsafe { msg![Id: cls("NSString"), "stringWithUTF8String:", *const c_char: c.as_ptr()] }
}

struct App {
    t: Term,
    atlas: Atlas,
    surf: [*mut c_void; 2],
    cur: usize,
    stale: [Vec<bool>; 2],
    stale_all: [bool; 2],
    drawn: Vec<bool>,
    fbw: usize,
    fbh: usize,
    win: Id,
    layer: Id,
    view: Id,
    scale: usize,
    font_pt: f64,
    gamma: f64,
    phosphor: usize,
    pty: c_int,
    child: c_int,
    fdref: *mut c_void,
    outq: VecDeque<u8>,
    frame_scheduled: bool,
    sync_since: f64,
    sel: input::Sel,
    drag_pt: CGPoint,
    autoscroll: bool,
    last_cursor: (usize, usize),
    scroll_acc: f64,
    focused: bool,
    ready: bool,
    view_w: f64,
    view_h: f64,
    cwd: String,
    cmd: String,
}

impl App {
    fn pad(&self) -> usize {
        (self.atlas.cw / 3).max(2)
    }
}

static mut G: *mut App = null_mut();

#[allow(static_mut_refs)]
fn app() -> &'static mut App {
    unsafe { &mut *G }
}

fn pty_write(buf: &[u8]) {
    let a = app();
    let mut off = 0;
    while off < buf.len() && a.outq.is_empty() {
        let n = unsafe { write(a.pty, buf[off..].as_ptr() as *const c_void, buf.len() - off) };
        if n > 0 { off += n as usize } else if errno() == EINTR { continue } else { break }
    }
    if off < buf.len() {
        a.outq.extend(&buf[off..]);
        unsafe { CFFileDescriptorEnableCallBacks(a.fdref, FD_WRITE_CB) }
    }
}

fn flush_outq() {
    let a = app();
    while !a.outq.is_empty() {
        let (head, _) = a.outq.as_slices();
        let n = unsafe { write(a.pty, head.as_ptr() as *const c_void, head.len()) };
        if n > 0 { a.outq.drain(..n as usize); } else if errno() == EINTR { continue } else { break }
    }
}

fn snap_bottom() {
    let a = app();
    if a.t.view != 0 {
        a.t.view = 0;
        a.t.all_dirty = true;
        schedule_frame();
    }
}

fn send(buf: &[u8]) {
    snap_bottom();
    pty_write(buf);
}

extern "C" fn frame_cb(_t: *mut c_void, _info: *mut c_void) {
    {
        let a = app();
        a.frame_scheduled = false;
        if a.t.modes.sync {
            let now = unsafe { CFAbsoluteTimeGetCurrent() };
            if a.sync_since == 0.0 { a.sync_since = now }
            if now - a.sync_since < 0.15 {
                a.frame_scheduled = false;
                schedule_frame();
                return;
            }
        }
        a.sync_since = 0.0;
    }
    present();
}

fn schedule_frame() {
    let a = app();
    if a.frame_scheduled || !a.ready { return }
    a.frame_scheduled = true;
    unsafe {
        let t = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent() + 0.008, 0.0, 0, 0, frame_cb, null_mut());
        CFRunLoopAddTimer(CFRunLoopGetMain(), t, kCFRunLoopCommonModes);
        CFRelease(t);
    }
}

unsafe fn make_surface(w: usize, h: usize) -> *mut c_void {
    let vals = [w as i32, h as i32, 4, 0x42475241u32 as i32];
    let keys = [kIOSurfaceWidth, kIOSurfaceHeight, kIOSurfaceBytesPerElement, kIOSurfacePixelFormat];
    let nums: Vec<Id> = vals.iter().map(|v| CFNumberCreate(null(), 3, v as *const i32 as *const c_void)).collect();
    let dict = CFDictionaryCreate(
        null(), keys.as_ptr(), nums.as_ptr(), 4,
        &kCFTypeDictionaryKeyCallBacks as *const c_void,
        &kCFTypeDictionaryValueCallBacks as *const c_void,
    );
    for n in nums { CFRelease(n) }
    let s = IOSurfaceCreate(dict);
    CFRelease(dict);
    assert!(!s.is_null(), "IOSurfaceCreate failed");
    s
}

fn present() {
    let a = app();
    let cur = (a.t.y, a.t.x);
    if cur != a.last_cursor {
        a.t.mark(a.last_cursor.0);
        a.t.mark(cur.0);
        a.last_cursor = cur;
    }
    let cs = a.cur;
    if a.stale_all[cs] || (a.t.view != 0 && a.stale[cs].iter().any(|&d| d)) {
        a.t.all_dirty = true;
    } else {
        for d in 0..a.t.rows.min(a.stale[cs].len()) {
            if a.stale[cs][d] { a.t.dirty[d] = true }
        }
    }
    let was_all = a.t.all_dirty;
    let pad = a.pad();
    let surf = a.surf[cs];
    let drew = unsafe {
        IOSurfaceLock(surf, 0, null_mut());
        let stride = IOSurfaceGetBytesPerRow(surf) / 4;
        let px = std::slice::from_raw_parts_mut(IOSurfaceGetBaseAddress(surf) as *mut u32, stride * a.fbh);
        let mut fb = Fb { px, w: a.fbw, h: a.fbh, stride, ox: pad, oy: pad };
        let drew = render::frame(&mut a.t, &mut a.atlas, &mut fb, &a.sel.on, a.focused, a.gamma as f32, a.phosphor, &mut a.drawn);
        IOSurfaceUnlock(surf, 0, null_mut());
        drew
    };
    if !drew { return }
    a.stale[cs].fill(false);
    a.stale_all[cs] = false;
    let o = 1 - cs;
    if was_all {
        a.stale_all[o] = true;
    } else {
        if a.stale[o].len() != a.drawn.len() { a.stale[o].resize(a.drawn.len(), true) }
        for (s, &d) in a.stale[o].iter_mut().zip(&a.drawn) { *s |= d }
    }
    a.cur = o;
    unsafe {
        let cat = cls("CATransaction");
        let _: () = msg![(): cat, "begin"];
        let _: () = msg![(): cat, "setDisableActions:", bool: true];
        let _: () = msg![(): a.layer, "setContents:", Id: surf as Id];
        let _: () = msg![(): cat, "commit"];
    }
}

fn relayout() {
    let a = app();
    let fbw = ((a.view_w * a.scale as f64) as usize).max(a.atlas.cw * 2);
    let fbh = ((a.view_h * a.scale as f64) as usize).max(a.atlas.ch * 2);
    if fbw != a.fbw || fbh != a.fbh || a.surf[0].is_null() {
        unsafe {
            for s in a.surf {
                if !s.is_null() { CFRelease(s) }
            }
            a.surf = [make_surface(fbw, fbh), make_surface(fbw, fbh)];
        }
        a.fbw = fbw;
        a.fbh = fbh;
    }
    let pad = a.pad();
    a.t.resize(
        fbw.saturating_sub(2 * pad).max(a.atlas.cw) / a.atlas.cw,
        fbh.saturating_sub(2 * pad).max(a.atlas.ch) / a.atlas.ch,
    );
    let ws = Winsize { row: a.t.rows as u16, col: a.t.cols as u16, xpix: 0, ypix: 0 };
    unsafe { ioctl(a.pty, TIOCSWINSZ, &ws) };
    a.stale = [vec![false; a.t.rows], vec![false; a.t.rows]];
    a.stale_all = [true, true];
    a.sel.on = None;
    a.t.all_dirty = true;
    present();
}

fn config_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join(".config/trm/config"))
}

fn setting(conf: &str, key: &str) -> Option<f64> {
    conf.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')?.trim().parse().ok())
}

fn save_settings() {
    let Some(p) = config_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let a = app();
    let _ = std::fs::write(p, format!("font_pt={}\ngamma={}\nphosphor={}\n", a.font_pt, a.gamma, a.phosphor));
}

fn set_font(delta: f64) {
    let (pt, scale) = {
        let a = app();
        let pt = (a.font_pt + delta).max(1.0);
        if pt == a.font_pt { return }
        a.font_pt = pt;
        (pt, a.scale)
    };
    let atlas = Atlas::new(pt * scale as f64);
    app().atlas = atlas;
    relayout();
    save_settings();
}

fn set_gamma(delta: f64) {
    let a = app();
    let g = (a.gamma + delta).clamp(0.1, 1.0);
    if g == a.gamma { return }
    a.gamma = g;
    a.t.all_dirty = true;
    schedule_frame();
    save_settings();
}

fn cycle_phosphor() {
    let a = app();
    a.phosphor = (a.phosphor + 1) % render::PHOSPHORS.len();
    a.t.all_dirty = true;
    a.cwd.clear();
    schedule_frame();
    save_settings();
    poll_cb(null_mut(), null_mut());
}

fn pasteboard_set(text: &[u8]) {
    unsafe {
        let pb: Id = msg![Id: cls("NSPasteboard"), "generalPasteboard"];
        let _: i64 = msg![i64: pb, "clearContents"];
        let s = nsstr(&String::from_utf8_lossy(text));
        let _: bool = msg![bool: pb, "setString:forType:", Id: s, Id: NSPasteboardTypeString];
    }
}

fn copy_selection() {
    let text = {
        let a = app();
        a.sel.text(&a.t)
    };
    let Some(text) = text else { return };
    pasteboard_set(&text);
}

extern "C" fn flash_cb(_t: *mut c_void, _info: *mut c_void) {
    let a = app();
    unsafe { msg![(): a.win, "setTitle:", Id: nsstr(&tilde(&a.cwd))] }
}

fn clip_flash(n: usize) {
    unsafe {
        msg![(): app().win, "setTitle:", Id: nsstr(&format!("\u{29c9} clipboard \u{2190} {} bytes", n))];
        let t = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent() + 1.5, 0.0, 0, 0, flash_cb, null_mut());
        CFRunLoopAddTimer(CFRunLoopGetMain(), t, kCFRunLoopCommonModes);
        CFRelease(t);
    }
}

fn paste() {
    let raw = unsafe {
        let pb: Id = msg![Id: cls("NSPasteboard"), "generalPasteboard"];
        let s: Id = msg![Id: pb, "stringForType:", Id: NSPasteboardTypeString];
        if s.is_null() { return }
        let u: *const c_char = msg![*const c_char: s, "UTF8String"];
        if u.is_null() { return }
        CStr::from_ptr(u).to_bytes().to_vec()
    };
    let payload = input::clean_paste(&raw, app().t.modes.paste);
    send(&payload);
}

fn app_feed(buf: &[u8]) {
    let (out, clip) = {
        let a = app();
        a.t.feed(buf);
        if a.sel.on.is_some() && !a.sel.dragging {
            a.sel.on = None;
            a.t.all_dirty = true;
        }
        (std::mem::take(&mut a.t.out), a.t.clip.take())
    };
    if !out.is_empty() { pty_write(&out) }
    if let Some(c) = clip {
        pasteboard_set(&c);
        clip_flash(c.len());
    }
    schedule_frame();
}

extern "C" fn pty_cb(fdref: *mut c_void, flags: u64, _info: *mut c_void) {
    if flags & FD_WRITE_CB != 0 { flush_outq() }
    if flags & FD_READ_CB != 0 {
        let fd = app().pty;
        let mut buf = [0u8; 65536];
        let mut total = 0usize;
        loop {
            let n = unsafe { read(fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
            if n > 0 {
                app_feed(&buf[..n as usize]);
                total += n as usize;
                if total > 1 << 20 { break }
            } else if n < 0 && (errno() == EAGAIN || errno() == EINTR) {
                break;
            } else {
                std::process::exit(0);
            }
        }
    }
    unsafe {
        CFFileDescriptorEnableCallBacks(fdref, FD_READ_CB);
        if !app().outq.is_empty() { CFFileDescriptorEnableCallBacks(fdref, FD_WRITE_CB) }
    }
}

// proc_vnodepathinfo: pvi_cdir.vip_path (1024 bytes) sits at offset 152, after
// the vnode_info header; pvi_rdir follows, for 2352 bytes total.
fn pid_cwd(pid: c_int) -> Option<String> {
    if pid <= 0 { return None }
    let mut buf = [0u8; 2352];
    let n = unsafe { proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, buf.as_mut_ptr() as *mut c_void, buf.len() as c_int) };
    if n < 1176 { return None }
    let path = &buf[152..1176];
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    if end == 0 { return None }
    Some(String::from_utf8_lossy(&path[..end]).into_owned())
}

fn fg_cwd() -> Option<String> {
    let a = app();
    pid_cwd(unsafe { tcgetpgrp(a.pty) }).or_else(|| pid_cwd(a.child))
}

// KERN_PROCARGS2 layout: i32 argc, exec_path, nul padding, then argv[0]
fn argv0(pid: c_int) -> Option<String> {
    let mut mib = [CTL_KERN, KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; 262144];
    let mut len = buf.len();
    let r = unsafe { sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut c_void, &mut len, null_mut(), 0) };
    if r != 0 || len < 5 { return None }
    let buf = &buf[..len];
    let mut i = 4;
    while i < buf.len() && buf[i] != 0 { i += 1 }
    while i < buf.len() && buf[i] == 0 { i += 1 }
    let start = i;
    while i < buf.len() && buf[i] != 0 { i += 1 }
    if start == i { return None }
    Some(String::from_utf8_lossy(&buf[start..i]).into_owned())
}

// kinfo_proc.kp_proc.p_comm sits at offset 243 of the 648-byte struct;
// readable for any pid (ps's mechanism), unlike procargs2, which the kernel
// refuses for setuid processes like top
fn kcomm(pid: c_int) -> Option<String> {
    let mut mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid];
    let mut buf = [0u8; 648];
    let mut len = buf.len();
    let r = unsafe { sysctl(mib.as_mut_ptr(), 4, buf.as_mut_ptr() as *mut c_void, &mut len, null_mut(), 0) };
    if r != 0 || len < 260 { return None }
    let comm = &buf[243..260];
    let end = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
    if end == 0 { return None }
    Some(String::from_utf8_lossy(&comm[..end]).into_owned())
}

fn fg_cmd() -> Option<String> {
    let a = app();
    let pid = unsafe { tcgetpgrp(a.pty) };
    if pid <= 0 || pid == a.child { return None }
    let name = argv0(pid).or_else(|| kcomm(pid))?;
    let base = name.rsplit('/').next().unwrap_or(&name).trim_start_matches('-');
    if base.is_empty() { return None }
    Some(base.into())
}

// the byte the backspace key sends: the tty's current VERASE control char,
// so line editors that bind backspace from `stty erase` (e.g. python 3.13's
// pyrepl) agree with us even when a dotfile remaps erase to ^H
fn erase_byte(fd: c_int) -> u8 {
    let mut tio = Termios { iflag: 0, oflag: 0, cflag: 0, lflag: 0, cc: [0; 20], ispeed: 0, ospeed: 0 };
    let e = if unsafe { tcgetattr(fd, &mut tio) } == 0 { tio.cc[VERASE] } else { 0x7f };
    if e == 0 || e == 0xff { 0x7f } else { e }
}

fn erase_char() -> u8 {
    erase_byte(app().pty)
}

fn tilde(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if p == h => "~".into(),
        Ok(h) if p.starts_with(&h) && p.as_bytes().get(h.len()) == Some(&b'/') => format!("~{}", &p[h.len()..]),
        _ => p.into(),
    }
}

unsafe fn icon_attrs(pt: f64, dim: bool) -> Id {
    let font: Id = msg![Id: cls("NSFont"), "monospacedSystemFontOfSize:weight:", f64: pt, f64: 0.4];
    let (r, g, b) = render::PHOSPHORS[app().phosphor];
    let l = if dim { 0.6 } else { 1.0 };
    let color: Id = msg![Id: cls("NSColor"), "colorWithSRGBRed:green:blue:alpha:",
        f64: r as f64 * l / 255.0, f64: g as f64 * l / 255.0, f64: b as f64 * l / 255.0, f64: 1.0];
    let keys = [NSFontAttributeName, NSForegroundColorAttributeName];
    let vals = [font, color];
    msg![Id: cls("NSDictionary"), "dictionaryWithObjects:forKeys:count:",
        *const Id: vals.as_ptr(), *const Id: keys.as_ptr(), u64: 2]
}

fn icon8(text: &str) -> String {
    if text.chars().count() <= 8 { return text.into() }
    let mut s: String = text.chars().take(7).collect();
    s.push('~');
    s
}

unsafe fn set_app_icon(dir: &str, cmd: Option<&str>) {
    let img: Id = msg![Id: msg![Id: cls("NSImage"), "alloc"], "initWithSize:", CGSize: CGSize { w: 512.0, h: 512.0 }];
    let _: () = msg![(): img, "lockFocus"];
    let _: () = msg![(): msg![Id: cls("NSColor"), "blackColor"], "setFill"];
    let rect = CGRect { origin: CGPoint { x: 32.0, y: 32.0 }, size: CGSize { w: 448.0, h: 448.0 } };
    let path: Id = msg![Id: cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", CGRect: rect, f64: 96.0, f64: 96.0];
    let _: () = msg![(): path, "fill"];
    let probe: CGSize = msg![CGSize: nsstr("00000000"), "sizeWithAttributes:", Id: icon_attrs(100.0, false)];
    let dir_pt = 100.0 * 380.0 / probe.w;
    let cmd_pt = dir_pt * 0.55;
    let (dir_h, cmd_h) = (probe.h * dir_pt / 100.0, probe.h * cmd_pt / 100.0);
    let top = (512.0 + dir_h + cmd_h) / 2.0;
    for (text, pt, dim, y) in [(dir, dir_pt, false, top - dir_h), (cmd.unwrap_or(""), cmd_pt, true, top - dir_h - cmd_h)] {
        if text.is_empty() { continue }
        let attrs = icon_attrs(pt, dim);
        let s = nsstr(text);
        let sz: CGSize = msg![CGSize: s, "sizeWithAttributes:", Id: attrs];
        let _: () = msg![(): s, "drawAtPoint:withAttributes:", CGPoint: CGPoint { x: (512.0 - sz.w) / 2.0, y }, Id: attrs];
    }
    let _: () = msg![(): img, "unlockFocus"];
    let nsapp: Id = msg![Id: cls("NSApplication"), "sharedApplication"];
    let _: () = msg![(): nsapp, "setApplicationIconImage:", Id: img];
    let _: () = msg![(): img, "release"];
}

extern "C" fn poll_cb(_t: *mut c_void, _info: *mut c_void) {
    let Some(cwd) = fg_cwd() else { return };
    let cmd = fg_cmd().unwrap_or_default();
    let a = app();
    if cwd == a.cwd && cmd == a.cmd { return }
    let dir_changed = cwd != a.cwd;
    a.cwd = cwd;
    a.cmd = cmd;
    let title = tilde(&a.cwd);
    let base = match title.rsplit('/').next() {
        Some("") | None => "/",
        Some(b) => b,
    };
    unsafe {
        if dir_changed {
            msg![(): a.win, "setTitle:", Id: nsstr(&title)];
            _LSSetApplicationInformationItem(-2, _LSGetCurrentApplicationASN(), _kLSDisplayNameKey, nsstr(&title), null_mut());
        }
        set_app_icon(&icon8(base), (!a.cmd.is_empty()).then(|| icon8(&a.cmd)).as_deref());
    }
}

#[cfg(test)]
mod cwd_tests {
    use super::*;

    fn row(t: &Term, y: usize) -> String {
        let mut s = String::new();
        for c in &t.line(y).cells {
            if c.attr & vt::TAIL != 0 { continue }
            s.push(char::from_u32(if c.cp == 0 { '.' as u32 } else { c.cp }).unwrap_or('?'));
        }
        s.trim_end_matches('.').to_string()
    }

    // full pipeline: real python under a real pty, output pumped through Term
    // exactly like pty_cb does, terminal responses written back
    fn repl_backspace(python: &str, extra_arg: Option<&str>) -> (String, usize) {
        unsafe {
            let m = posix_openpt(O_RDWR | O_NOCTTY);
            assert!(m >= 0 && grantpt(m) == 0 && unlockpt(m) == 0);
            let sname = CStr::from_ptr(ptsname(m)).to_owned();
            let ws = Winsize { row: 30, col: 100, xpix: 0, ypix: 0 };
            ioctl(m, TIOCSWINSZ, &ws);
            let pid = fork();
            assert!(pid >= 0);
            if pid == 0 {
                setsid();
                let s = open(sname.as_ptr(), O_RDWR);
                ioctl(s, TIOCSCTTY, 0u64);
                dup2(s, 0); dup2(s, 1); dup2(s, 2);
                let path = CString::new(python).unwrap();
                let arg0 = CString::new(python).unwrap();
                let q = CString::new(extra_arg.unwrap_or("-q")).unwrap();
                let argv = [arg0.as_ptr(), q.as_ptr(), null()];
                let term = CString::new("TERM=xterm-256color").unwrap();
                let home = CString::new(format!("HOME={}", std::env::var("HOME").unwrap())).unwrap();
                let envp = [term.as_ptr(), home.as_ptr(), null()];
                execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
                _exit(127);
            }
            fcntl(m, F_SETFL, O_NONBLOCK);
            let mut t = Term::new(100, 30, 100, 0xffffff, 0);
            let pump = |t: &mut Term, secs: f64| {
                let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
                while std::time::Instant::now() < end {
                    let mut buf = [0u8; 65536];
                    let n = read(m, buf.as_mut_ptr() as *mut c_void, buf.len());
                    if n > 0 {
                        t.feed(&buf[..n as usize]);
                        let out = std::mem::take(&mut t.out);
                        if !out.is_empty() { write(m, out.as_ptr() as *const c_void, out.len()); }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            };
            pump(&mut t, 3.0);
            // push the prompt to the bottom row so scrollback is in play
            let cmd: &[u8] = b"print('x\\n' * 40)\r";
            write(m, cmd.as_ptr() as *const c_void, cmd.len());
            pump(&mut t, 2.0);
            write(m, b"abc".as_ptr() as *const c_void, 3);
            pump(&mut t, 1.0);
            write(m, b"\x7f".as_ptr() as *const c_void, 1);
            pump(&mut t, 1.0);
            for y in 0..t.rows { eprintln!("{:2}|{}|", y, row(&t, y)) }
            eprintln!("cursor: y={} x={}", t.y, t.x);
            let r = (row(&t, t.y), t.x);
            write(m, b"\x04".as_ptr() as *const c_void, 1);
            pump(&mut t, 0.3);
            close(m);
            r
        }
    }

    // type past the right margin so the input wraps, then backspace across
    // the wrap boundary — classic ghost-space territory
    #[test]
    fn python_wrapped_backspace_e2e() {
        unsafe {
            let m = posix_openpt(O_RDWR | O_NOCTTY);
            assert!(m >= 0 && grantpt(m) == 0 && unlockpt(m) == 0);
            let sname = CStr::from_ptr(ptsname(m)).to_owned();
            let ws = Winsize { row: 30, col: 100, xpix: 0, ypix: 0 };
            ioctl(m, TIOCSWINSZ, &ws);
            let pid = fork();
            if pid == 0 {
                setsid();
                let s = open(sname.as_ptr(), O_RDWR);
                ioctl(s, TIOCSCTTY, 0u64);
                dup2(s, 0); dup2(s, 1); dup2(s, 2);
                let path = CString::new("/opt/homebrew/bin/python3").unwrap();
                let q = CString::new("-q").unwrap();
                let argv = [path.as_ptr(), q.as_ptr(), null()];
                let term = CString::new("TERM=xterm-256color").unwrap();
                let envp = [term.as_ptr(), null()];
                execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
                _exit(127);
            }
            fcntl(m, F_SETFL, O_NONBLOCK);
            let mut t = Term::new(100, 30, 100, FG, BG);
            let pump = |t: &mut Term, secs: f64| {
                let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
                while std::time::Instant::now() < end {
                    let mut buf = [0u8; 65536];
                    let n = read(m, buf.as_mut_ptr() as *mut c_void, buf.len());
                    if n > 0 {
                        t.feed(&buf[..n as usize]);
                        let out = std::mem::take(&mut t.out);
                        if !out.is_empty() { write(m, out.as_ptr() as *const c_void, out.len()); }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            };
            pump(&mut t, 2.0);
            let line = [b'a'; 110];
            write(m, line.as_ptr() as *const c_void, line.len());
            pump(&mut t, 1.5);
            // delete back across the wrap boundary: 110 - 20 = 90 chars left
            for _ in 0..20 {
                write(m, b"\x7f".as_ptr() as *const c_void, 1);
                pump(&mut t, 0.05);
            }
            pump(&mut t, 1.0);
            for y in 0..t.rows { eprintln!("{:2}|{}|", y, row(&t, y)) }
            eprintln!("cursor: y={} x={}", t.y, t.x);
            // pyrepl soft-wraps with a `\` continuation marker at its margin;
            // strip the markers and check the surviving content
            let want = format!(">>> {}", "a".repeat(90));
            let pr = (0..t.rows).rev().find(|&y| row(&t, y).starts_with(">>> ")).unwrap();
            let got: String = (pr..=t.y).map(|y| row(&t, y)).collect::<Vec<_>>().join("").replace('\\', "");
            write(m, b"\x04\x04".as_ptr() as *const c_void, 2);
            pump(&mut t, 0.3);
            close(m);
            assert_eq!(got, want);
        }
    }

    #[test]
    fn ipython_backspace_e2e() {
        let ipy = "/opt/homebrew/bin/ipython";
        if !std::path::Path::new(ipy).exists() { return }
        let (line, x) = repl_backspace(ipy, Some("--no-banner"));
        assert_eq!(line, "In [2]: ab");
        assert_eq!(x, 10);
    }

    // replay a pyrepl session split at every chunk granularity through
    // present()'s double-buffer stale logic; displayed pixels must equal a
    // from-scratch full redraw of the same final state
    #[test]
    fn incremental_render_matches_full() {
        let session: Vec<u8> = [
            &b"\x1b[?2004h\x1b[?1h\x1b=\x1b[?25l\x1b[1A\n\x1b[1;35m>>> \x1b[0m\x1b[4D\x1b[?12l\x1b[?25h\x1b[4C"[..],
            b"\x1b[?25l\x1b[4D\x1b[1;35m>>> \x1b[0ma\x1b[5D\x1b[?12l\x1b[?25h\x1b[5C",
            b"\x1b[?25l\x1b[5D\x1b[1;35m>>> \x1b[0mab\x1b[6D\x1b[?12l\x1b[?25h\x1b[6C",
            b"\x1b[?25l\x1b[6D\x1b[1;35m>>> \x1b[0mabc\x1b[7D\x1b[?12l\x1b[?25h\x1b[7C",
            b"\x1b[?25l\x1b[7D\x1b[K\x1b[1;35m>>> \x1b[0mab\x1b[6D\x1b[?12l\x1b[?25h\x1b[6C",
        ]
        .concat();
        let (cols, rows) = (100usize, 30usize);
        for chunk in [1usize, 2, 3, 5, 7, 16, 64, session.len()] {
            let mut atlas = Atlas::new(13.0);
            let (w, h) = (cols * atlas.cw + 8, rows * atlas.ch + 8);
            let mut t = Term::new(cols, rows, 100, FG, BG);
            let mut bufs = [vec![0u32; w * h], vec![0u32; w * h]];
            let mut stale = [vec![false; rows], vec![false; rows]];
            let mut stale_all = [true, true];
            let mut drawn = vec![];
            let mut cur = 0usize;
            let mut last_cursor = (0usize, 0usize);
            let mut shown = 1usize;
            for piece in session.chunks(chunk) {
                t.feed(piece);
                let cpos = (t.y, t.x);
                if cpos != last_cursor {
                    t.mark(last_cursor.0);
                    t.mark(cpos.0);
                    last_cursor = cpos;
                }
                if stale_all[cur] {
                    t.all_dirty = true;
                } else {
                    for d in 0..rows {
                        if stale[cur][d] { t.dirty[d] = true }
                    }
                }
                let was_all = t.all_dirty;
                let mut fb = Fb { px: &mut bufs[cur], w, h, stride: w, ox: 4, oy: 4 };
                let drew = render::frame(&mut t, &mut atlas, &mut fb, &None, true, 0.45, 0, &mut drawn);
                if !drew { continue }
                stale[cur].fill(false);
                stale_all[cur] = false;
                let o = 1 - cur;
                if was_all {
                    stale_all[o] = true;
                } else {
                    for (s, &d) in stale[o].iter_mut().zip(&drawn) { *s |= d }
                }
                shown = cur;
                cur = o;
            }
            let mut rt = Term::new(cols, rows, 100, FG, BG);
            rt.feed(&session);
            rt.all_dirty = true;
            let mut rbuf = vec![0u32; w * h];
            let mut fb = Fb { px: &mut rbuf, w, h, stride: w, ox: 4, oy: 4 };
            render::frame(&mut rt, &mut atlas, &mut fb, &None, true, 0.45, 0, &mut drawn);
            let diff = bufs[shown].iter().zip(&rbuf).filter(|(a, b)| a != b).count();
            assert_eq!(diff, 0, "chunk={}: {} pixels differ from full redraw", chunk, diff);
        }
    }

    #[test]
    fn python_backspace_e2e() {
        for py in ["/opt/homebrew/bin/python3", "/usr/bin/python3"] {
            if !std::path::Path::new(py).exists() { continue }
            let (line, x) = repl_backspace(py, None);
            assert_eq!(line, ">>> ab", "python {}", py);
            assert_eq!(x, 6, "python {}", py);
        }
    }

    // user dotfiles run `stty erase ^H`; pyrepl binds backspace to VERASE, so
    // the backspace key must send the tty's erase char, not a hardcoded 0x7f
    #[test]
    fn python_backspace_stty_erase_e2e() {
        unsafe {
            let m = posix_openpt(O_RDWR | O_NOCTTY);
            assert!(m >= 0 && grantpt(m) == 0 && unlockpt(m) == 0);
            let sname = CStr::from_ptr(ptsname(m)).to_owned();
            let ws = Winsize { row: 30, col: 100, xpix: 0, ypix: 0 };
            ioctl(m, TIOCSWINSZ, &ws);
            let pid = fork();
            if pid == 0 {
                setsid();
                let s = open(sname.as_ptr(), O_RDWR);
                ioctl(s, TIOCSCTTY, 0u64);
                let mut tio = Termios { iflag: 0, oflag: 0, cflag: 0, lflag: 0, cc: [0; 20], ispeed: 0, ospeed: 0 };
                tcgetattr(s, &mut tio);
                tio.cc[VERASE] = 0x08;
                tcsetattr(s, 0, &tio);
                dup2(s, 0); dup2(s, 1); dup2(s, 2);
                let path = CString::new("/opt/homebrew/bin/python3").unwrap();
                let q = CString::new("-q").unwrap();
                let argv = [path.as_ptr(), q.as_ptr(), null()];
                let term = CString::new("TERM=xterm-256color").unwrap();
                let envp = [term.as_ptr(), null()];
                execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
                _exit(127);
            }
            fcntl(m, F_SETFL, O_NONBLOCK);
            let mut t = Term::new(100, 30, 100, FG, BG);
            let pump = |t: &mut Term, secs: f64| {
                let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
                while std::time::Instant::now() < end {
                    let mut buf = [0u8; 65536];
                    let n = read(m, buf.as_mut_ptr() as *mut c_void, buf.len());
                    if n > 0 {
                        t.feed(&buf[..n as usize]);
                        let out = std::mem::take(&mut t.out);
                        if !out.is_empty() { write(m, out.as_ptr() as *const c_void, out.len()); }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            };
            pump(&mut t, 2.0);
            write(m, b"abc".as_ptr() as *const c_void, 3);
            pump(&mut t, 1.0);
            // what the backspace key now sends: the tty's VERASE, read off the master
            let erase = erase_byte(m);
            assert_eq!(erase, 0x08, "expected remapped VERASE visible on master");
            write(m, [erase].as_ptr() as *const c_void, 1);
            pump(&mut t, 1.0);
            let r = (row(&t, t.y), t.x);
            write(m, b"\x04".as_ptr() as *const c_void, 1);
            pump(&mut t, 0.3);
            close(m);
            assert_eq!(r.0, ">>> ab");
            assert_eq!(r.1, 6);
        }
    }

    #[test]
    fn pid_cwd_offsets() {
        let cwd = pid_cwd(std::process::id() as c_int).unwrap();
        assert_eq!(cwd, std::env::current_dir().unwrap().to_string_lossy());
    }

    #[test]
    fn argv0_of_self() {
        let name = argv0(std::process::id() as c_int).unwrap();
        assert!(name.rsplit('/').next().unwrap().starts_with("trm"), "{}", name);
    }

    #[test]
    fn kcomm_of_setuid_top() {
        assert!(kcomm(std::process::id() as c_int).unwrap().starts_with("trm"));
        let out = std::process::Command::new("/usr/bin/top")
            .args(["-l", "1", "-n", "0"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = out.id() as c_int;
        std::thread::sleep(std::time::Duration::from_millis(200));
        let name = kcomm(pid);
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        assert_eq!(name.as_deref(), Some("top"));
    }

    #[test]
    fn settings_parse() {
        let conf = "font_pt=14.5\ngamma=0.5\njunk\nfont_ptx=9\n";
        assert_eq!(setting(conf, "font_pt"), Some(14.5));
        assert_eq!(setting(conf, "gamma"), Some(0.5));
        assert_eq!(setting(conf, "missing"), None);
        assert_eq!(setting("font_pt=abc\n", "font_pt"), None);
    }

    #[test]
    fn icon_truncate() {
        assert_eq!(icon8("trm"), "trm");
        assert_eq!(icon8("12345678"), "12345678");
        assert_eq!(icon8("superpowers"), "superpo~");
        assert_eq!(icon8("customer-match"), "custome~");
        assert_eq!(icon8("~"), "~");
    }

    #[test]
    fn tilde_abbrev() {
        let h = std::env::var("HOME").unwrap();
        assert_eq!(tilde(&h), "~");
        assert_eq!(tilde(&format!("{}/work", h)), "~/work");
        assert_eq!(tilde(&format!("{}work", h)), format!("{}work", h));
        assert_eq!(tilde("/tmp"), "/tmp");
    }
}

fn pty_spawn(cols: usize, rows: usize) -> (c_int, c_int) {
    unsafe {
        let m = posix_openpt(O_RDWR | O_NOCTTY);
        assert!(m >= 0 && grantpt(m) == 0 && unlockpt(m) == 0, "pty failed");
        let sname = CStr::from_ptr(ptsname(m)).to_owned();
        let ws = Winsize { row: rows as u16, col: cols as u16, xpix: 0, ypix: 0 };
        ioctl(m, TIOCSWINSZ, &ws);
        let pw = getpwuid(getuid());
        let shell = if !pw.is_null() && !(*pw).shell.is_null() && *(*pw).shell != 0 {
            CStr::from_ptr((*pw).shell).to_string_lossy().into_owned()
        } else {
            "/bin/zsh".into()
        };
        let base = shell.rsplit('/').next().unwrap_or(&shell);
        let arg0 = CString::new(format!("-{}", base)).unwrap();
        let path = CString::new(shell.clone()).unwrap();
        let argv = [arg0.as_ptr(), null()];
        let mut env: Vec<CString> = std::env::vars()
            .filter(|(k, _)| k != "TERM" && k != "COLORTERM" && k != "TERMCAP" && !k.starts_with("DYLD_") && !k.starts_with("TRM_"))
            .map(|(k, v)| CString::new(format!("{}={}", k, v)).unwrap())
            .collect();
        env.push(CString::new("TERM=xterm-256color").unwrap());
        env.push(CString::new("COLORTERM=truecolor").unwrap());
        if std::env::var("LANG").is_err() { env.push(CString::new("LANG=en_US.UTF-8").unwrap()) }
        let mut envp: Vec<*const c_char> = env.iter().map(|c| c.as_ptr()).collect();
        envp.push(null());
        let pid = fork();
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            setsid();
            let s = open(sname.as_ptr(), O_RDWR);
            if s < 0 { _exit(127) }
            ioctl(s, TIOCSCTTY, 0u64);
            dup2(s, 0);
            dup2(s, 1);
            dup2(s, 2);
            if s > 2 { close(s); }
            close(m);
            signal(SIGCHLD, 0);
            signal(SIGPIPE, 0);
            execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
            _exit(127);
        }
        fcntl(m, F_SETFL, O_NONBLOCK);
        (m, pid)
    }
}

unsafe fn ev_char(ev: Id) -> u16 {
    let s: Id = msg![Id: ev, "charactersIgnoringModifiers"];
    if s.is_null() || msg![u64: s, "length"] == 0 { return 0 }
    msg![u16: s, "characterAtIndex:", u64: 0]
}

unsafe fn ev_point(ev: Id) -> CGPoint {
    let p: CGPoint = msg![CGPoint: ev, "locationInWindow"];
    msg![CGPoint: app().view, "convertPoint:fromView:", CGPoint: p, Id: null_mut()]
}

extern "C" fn v_flipped(_s: Id, _c: SEL) -> bool { true }
extern "C" fn v_first_responder(_s: Id, _c: SEL) -> bool { true }
extern "C" fn v_has_marked(_s: Id, _c: SEL) -> bool { false }
extern "C" fn v_range(_s: Id, _c: SEL) -> NSRange { NSRange { loc: u64::MAX >> 1, len: 0 } }
extern "C" fn v_attr_sub(_s: Id, _c: SEL, _r: NSRange, _a: *mut NSRange) -> Id { null_mut() }
extern "C" fn v_valid_attrs(_s: Id, _c: SEL) -> Id { unsafe { msg![Id: cls("NSArray"), "array"] } }
extern "C" fn v_first_rect(_s: Id, _c: SEL, _r: NSRange, _a: *mut NSRange) -> CGRect {
    CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } }
}
extern "C" fn v_char_index(_s: Id, _c: SEL, _p: CGPoint) -> u64 { 0 }
extern "C" fn v_unmark(_s: Id, _c: SEL) {}
extern "C" fn v_set_marked(_s: Id, _c: SEL, _t: Id, _a: NSRange, _b: NSRange) {}

extern "C" fn v_drag_entered(_s: Id, _c: SEL, _info: Id) -> u64 { 1 }
extern "C" fn v_drag_prepare(_s: Id, _c: SEL, _info: Id) -> bool { true }

extern "C" fn v_drag_perform(_s: Id, _c: SEL, info: Id) -> bool {
    let paths = unsafe {
        let pb: Id = msg![Id: info, "draggingPasteboard"];
        if pb.is_null() { return false }
        let classes: Id = msg![Id: cls("NSArray"), "arrayWithObject:", Id: cls("NSURL")];
        let urls: Id = msg![Id: pb, "readObjectsForClasses:options:", Id: classes, Id: null_mut()];
        if urls.is_null() { return false }
        let n: u64 = msg![u64: urls, "count"];
        let mut paths = Vec::new();
        for i in 0..n {
            let url: Id = msg![Id: urls, "objectAtIndex:", u64: i];
            let path: Id = msg![Id: url, "path"];
            if path.is_null() { continue }
            let u: *const c_char = msg![*const c_char: path, "UTF8String"];
            if u.is_null() { continue }
            paths.push(CStr::from_ptr(u).to_string_lossy().into_owned());
        }
        paths
    };
    if paths.is_empty() { return false }
    let payload = input::drop_paths(paths, app().t.modes.paste);
    send(&payload);
    true
}

extern "C" fn v_key_down(this: Id, _c: SEL, ev: Id) {
    let (ch, mods) = unsafe { (ev_char(ev), msg![u64: ev, "modifierFlags"]) };
    let act = {
        let erase = erase_char();
        let a = app();
        input::key(ch, mods, &a.t.modes, erase)
    };
    match act {
        Act::Write(b) => send(&b),
        Act::Copy => copy_selection(),
        Act::Paste => paste(),
        Act::Font(d) => set_font(d),
        Act::Gamma(d) => set_gamma(d),
        Act::Phosphor => cycle_phosphor(),
        Act::NewWindow => {
            if let Ok(exe) = std::env::current_exe() {
                let (pt, gamma, phos) = {
                    let a = app();
                    (format!("{}", a.font_pt), format!("{}", a.gamma), format!("{}", a.phosphor))
                };
                let mut cmd = std::process::Command::new(&exe);
                cmd.env("TRM_FONT_PT", &pt).env("TRM_GAMMA", &gamma).env("TRM_PHOSPHOR", &phos);
                if let Some(cwd) = fg_cwd() { cmd.current_dir(cwd); }
                if cmd.spawn().is_err() {
                    let _ = std::process::Command::new(exe).env("TRM_FONT_PT", &pt).env("TRM_GAMMA", &gamma).env("TRM_PHOSPHOR", &phos).spawn();
                }
            }
        }
        Act::Quit => std::process::exit(0),
        Act::Eat => {}
        Act::Pass => unsafe {
            let arr: Id = msg![Id: cls("NSArray"), "arrayWithObject:", Id: ev];
            msg![(): this, "interpretKeyEvents:", Id: arr]
        },
    }
}

extern "C" fn v_insert_text(_s: Id, _c: SEL, mut text: Id, _r: NSRange) {
    unsafe {
        if msg![bool: text, "isKindOfClass:", Id: cls("NSAttributedString")] {
            text = msg![Id: text, "string"];
        }
        let u: *const c_char = msg![*const c_char: text, "UTF8String"];
        if !u.is_null() { send(CStr::from_ptr(u).to_bytes()) }
    }
}

extern "C" fn v_do_command(_s: Id, _c: SEL, cmd: SEL) {
    let name = unsafe { CStr::from_ptr(sel_getName(cmd)) }.to_bytes();
    match name {
        b"insertNewline:" | b"insertLineBreak:" => send(b"\r"),
        b"insertTab:" => send(b"\t"),
        b"deleteBackward:" => send(&[erase_char()]),
        b"cancelOperation:" => send(b"\x1b"),
        _ => {}
    }
}

extern "C" fn autoscroll_cb(_t: *mut c_void, _info: *mut c_void) {
    app().autoscroll = false;
    drag_sel();
}

fn drag_sel() {
    let a = app();
    if !a.sel.dragging { return }
    let step = a.atlas.ch as f64 / a.scale as f64;
    let lines = if a.t.alt != 0 {
        0
    } else if a.drag_pt.y < 0.0 {
        ((-a.drag_pt.y / step) as i64 + 1).min(30)
    } else if a.drag_pt.y > a.view_h {
        (((a.view_h - a.drag_pt.y) / step) as i64 - 1).max(-30)
    } else {
        0
    };
    a.t.view = (a.t.view as i64 + lines).clamp(0, a.t.sb.len() as i64) as usize;
    let (gx, gy) = input::cell(a.drag_pt.x, a.drag_pt.y, a.scale, a.pad(), a.atlas.cw, a.atlas.ch, a.t.cols, a.t.rows);
    a.sel.drag(&a.t, a.t.line_id(gy), gx);
    a.t.all_dirty = true;
    if lines != 0 && !a.autoscroll {
        a.autoscroll = true;
        unsafe {
            let t = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent() + 0.05, 0.0, 0, 0, autoscroll_cb, null_mut());
            CFRunLoopAddTimer(CFRunLoopGetMain(), t, kCFRunLoopCommonModes);
            CFRelease(t);
        }
    }
    schedule_frame();
}

extern "C" fn v_mouse(_s: Id, cmd: SEL, ev: Id) {
    let name = unsafe { CStr::from_ptr(sel_getName(cmd)) }.to_bytes();
    let btn: i64 = match name[0] {
        b'r' => 2,
        b'o' => 1,
        _ => 0,
    };
    let kind = if name.ends_with(b"Down:") { input::DOWN } else if name.ends_with(b"Up:") { input::UP } else { input::DRAG };
    let (p, mods) = unsafe { (ev_point(ev), msg![u64: ev, "modifierFlags"]) };
    let clicks: i64 = if kind == input::DOWN { unsafe { msg![i64: ev, "clickCount"] } } else { 0 };
    let report = {
        let a = app();
        let (gx, gy) = input::cell(p.x, p.y, a.scale, a.pad(), a.atlas.cw, a.atlas.ch, a.t.cols, a.t.rows);
        if a.t.modes.mouse != 0 && mods & input::SHIFT == 0 && a.t.view == 0 {
            input::mouse_report(btn, kind, mods, gx, gy, &a.t.modes)
        } else {
            if btn == 0 {
                a.drag_pt = p;
                let id = a.t.line_id(gy);
                match kind {
                    input::DOWN => a.sel.begin(&a.t, id, gx, clicks),
                    input::DRAG => {}
                    _ => a.sel.finish(id, gx),
                }
                a.t.all_dirty = true;
            }
            None
        }
    };
    if btn == 0 && kind == input::DRAG && report.is_none() { drag_sel() }
    match report {
        Some(b) => pty_write(&b),
        None => schedule_frame(),
    }
}

extern "C" fn v_scroll(_s: Id, _c: SEL, ev: Id) {
    let (p, dy, precise, mods) = unsafe {
        (
            ev_point(ev),
            msg![f64: ev, "scrollingDeltaY"],
            msg![bool: ev, "hasPreciseScrollingDeltas"],
            msg![u64: ev, "modifierFlags"],
        )
    };
    let out = {
        let a = app();
        let step = a.atlas.ch as f64 / a.scale as f64;
        a.scroll_acc += if precise { dy } else { dy * 3.0 * step };
        let lines = (a.scroll_acc / step) as i64;
        a.scroll_acc -= lines as f64 * step;
        if lines == 0 { return }
        let n = lines.unsigned_abs().min(30) as usize;
        if a.t.modes.mouse != 0 && mods & input::SHIFT == 0 && a.t.view == 0 {
            let (gx, gy) = input::cell(p.x, p.y, a.scale, a.pad(), a.atlas.cw, a.atlas.ch, a.t.cols, a.t.rows);
            let btn = if lines > 0 { input::WHEEL_UP } else { input::WHEEL_DOWN };
            input::mouse_report(btn, input::DOWN, mods, gx, gy, &a.t.modes).map(|b| b.repeat(n))
        } else if a.t.alt != 0 {
            let arrow: &[u8] = match (lines > 0, a.t.modes.ckm) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            Some(arrow.repeat(n))
        } else {
            let v = (a.t.view as i64 + lines).clamp(0, a.t.sb.len() as i64) as usize;
            if v != a.t.view {
                a.t.view = v;
                a.t.all_dirty = true;
            }
            None
        }
    };
    match out {
        Some(b) => pty_write(&b),
        None => schedule_frame(),
    }
}

extern "C" fn v_set_frame_size(this: Id, cmd: SEL, size: CGSize) {
    unsafe {
        let sup = ObjcSuper { receiver: this, class: class_getSuperclass(object_getClass(this)) };
        let f: extern "C" fn(*const ObjcSuper, SEL, CGSize) = transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd, size);
        if G.is_null() { return }
    }
    let ready = {
        let a = app();
        a.view_w = size.w;
        a.view_h = size.h;
        a.ready
    };
    if ready { relayout() }
}

extern "C" fn v_backing_changed(this: Id, _c: SEL) {
    let scale = unsafe {
        let win: Id = msg![Id: this, "window"];
        if win.is_null() || G.is_null() { return }
        let sf: f64 = msg![f64: win, "backingScaleFactor"];
        if sf > 1.5 { 2usize } else { 1 }
    };
    let rebuild = {
        let a = app();
        if scale != a.scale && a.ready {
            a.scale = scale;
            Some(a.font_pt * scale as f64)
        } else {
            None
        }
    };
    let Some(pt) = rebuild else { return };
    let atlas = Atlas::new(pt);
    app().atlas = atlas;
    unsafe { msg![(): app().layer, "setContentsScale:", f64: scale as f64] }
    relayout();
}

extern "C" fn w_will_close(_s: Id, _c: SEL, _n: Id) {
    std::process::exit(0);
}

extern "C" fn w_focus(_s: Id, cmd: SEL, _n: Id) {
    let became = unsafe { CStr::from_ptr(sel_getName(cmd)) }.to_bytes() == b"windowDidBecomeKey:";
    let notify = {
        let a = app();
        a.focused = became;
        let y = a.t.y;
        a.t.mark(y);
        a.t.modes.focus
    };
    if notify { pty_write(if became { b"\x1b[I" } else { b"\x1b[O" }) }
    schedule_frame();
}

fn make_view_class() -> Id {
    unsafe {
        let name = CString::new("TrmView").unwrap();
        let c = objc_allocateClassPair(cls("NSView"), name.as_ptr(), 0);
        let add = |s: &str, imp: *const c_void, ty: &str| {
            let t = CString::new(ty).unwrap();
            class_addMethod(c, sel(s), imp, t.as_ptr());
        };
        add("isFlipped", v_flipped as *const c_void, "c@:");
        add("acceptsFirstResponder", v_first_responder as *const c_void, "c@:");
        add("keyDown:", v_key_down as *const c_void, "v@:@");
        add("insertText:replacementRange:", v_insert_text as *const c_void, "v@:@{_NSRange=QQ}");
        add("doCommandBySelector:", v_do_command as *const c_void, "v@::");
        add("setMarkedText:selectedRange:replacementRange:", v_set_marked as *const c_void, "v@:@{_NSRange=QQ}{_NSRange=QQ}");
        add("unmarkText", v_unmark as *const c_void, "v@:");
        add("selectedRange", v_range as *const c_void, "{_NSRange=QQ}@:");
        add("markedRange", v_range as *const c_void, "{_NSRange=QQ}@:");
        add("hasMarkedText", v_has_marked as *const c_void, "c@:");
        add("attributedSubstringForProposedRange:actualRange:", v_attr_sub as *const c_void, "@@:{_NSRange=QQ}^{_NSRange=QQ}");
        add("validAttributesForMarkedText", v_valid_attrs as *const c_void, "@@:");
        add("firstRectForCharacterRange:actualRange:", v_first_rect as *const c_void, "{CGRect={CGPoint=dd}{CGSize=dd}}@:{_NSRange=QQ}^{_NSRange=QQ}");
        add("characterIndexForPoint:", v_char_index as *const c_void, "Q@:{CGPoint=dd}");
        for s in [
            "mouseDown:", "mouseDragged:", "mouseUp:",
            "rightMouseDown:", "rightMouseDragged:", "rightMouseUp:",
            "otherMouseDown:", "otherMouseDragged:", "otherMouseUp:",
        ] {
            add(s, v_mouse as *const c_void, "v@:@");
        }
        add("scrollWheel:", v_scroll as *const c_void, "v@:@");
        add("draggingEntered:", v_drag_entered as *const c_void, "Q@:@");
        add("prepareForDragOperation:", v_drag_prepare as *const c_void, "c@:@");
        add("performDragOperation:", v_drag_perform as *const c_void, "c@:@");
        add("setFrameSize:", v_set_frame_size as *const c_void, "v@:{CGSize=dd}");
        add("viewDidChangeBackingProperties", v_backing_changed as *const c_void, "v@:");
        add("windowWillClose:", w_will_close as *const c_void, "v@:@");
        add("windowDidBecomeKey:", w_focus as *const c_void, "v@:@");
        add("windowDidResignKey:", w_focus as *const c_void, "v@:@");
        let pname = CString::new("NSTextInputClient").unwrap();
        let proto = objc_getProtocol(pname.as_ptr());
        if !proto.is_null() { class_addProtocol(c, proto); }
        objc_registerClassPair(c);
        c
    }
}

fn main() {
    unsafe {
        signal(SIGCHLD, SIG_IGN);
        signal(SIGPIPE, SIG_IGN);
        let nsapp: Id = msg![Id: cls("NSApplication"), "sharedApplication"];
        let _: () = msg![(): nsapp, "setActivationPolicy:", i64: 0];

        let screen: Id = msg![Id: cls("NSScreen"), "mainScreen"];
        let sf: f64 = if screen.is_null() { 2.0 } else { msg![f64: screen, "backingScaleFactor"] };
        let scale = if sf > 1.5 { 2usize } else { 1 };
        let conf = config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let font_pt = std::env::var("TRM_FONT_PT").ok()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| setting(&conf, "font_pt"))
            .filter(|p| p.is_finite())
            .map_or(FONT_PT, |p| p.max(1.0));
        let gamma = std::env::var("TRM_GAMMA").ok()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| setting(&conf, "gamma"))
            .filter(|g| g.is_finite())
            .map_or(GAMMA, |g| g.clamp(0.1, 1.0));
        let phosphor = std::env::var("TRM_PHOSPHOR").ok()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| setting(&conf, "phosphor"))
            .filter(|p| p.is_finite())
            .map_or(0, |p| (p.max(0.0) as usize).min(render::PHOSPHORS.len() - 1));
        let atlas = Atlas::new(font_pt * scale as f64);

        let pad = (atlas.cw / 3).max(2);
        let w = (COLS * atlas.cw + 2 * pad) as f64 / scale as f64;
        let h = (ROWS * atlas.ch + 2 * pad) as f64 / scale as f64;
        let rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w, h } };

        let vc = make_view_class();
        let view: Id = msg![Id: msg![Id: vc, "alloc"], "initWithFrame:", CGRect: rect];
        let win: Id = msg![Id: msg![Id: cls("NSWindow"), "alloc"],
            "initWithContentRect:styleMask:backing:defer:",
            CGRect: rect, u64: 15, u64: 2, bool: false];
        let _: () = msg![(): win, "setReleasedWhenClosed:", bool: false];
        let _: () = msg![(): win, "setTitle:", Id: nsstr("trm")];
        let _: () = msg![(): win, "setContentView:", Id: view];
        let _: () = msg![(): win, "setDelegate:", Id: view];
        let _: () = msg![(): win, "makeFirstResponder:", Id: view];
        let drag_types: Id = msg![Id: cls("NSArray"), "arrayWithObject:", Id: NSPasteboardTypeFileURL];
        let _: () = msg![(): view, "registerForDraggedTypes:", Id: drag_types];
        let _: () = msg![(): view, "setWantsLayer:", bool: true];
        let layer: Id = msg![Id: view, "layer"];
        let _: () = msg![(): layer, "setContentsGravity:", Id: kCAGravityTopLeft];
        let _: () = msg![(): layer, "setContentsScale:", f64: scale as f64];

        let t = Term::new(COLS, ROWS, SCROLLBACK, FG, BG);
        let (pty, child) = pty_spawn(COLS, ROWS);
        G = Box::into_raw(Box::new(App {
            t, atlas,
            surf: [null_mut(); 2], cur: 0,
            stale: [vec![], vec![]], stale_all: [true; 2], drawn: vec![],
            fbw: 0, fbh: 0,
            win, layer, view,
            scale, font_pt, gamma, phosphor,
            pty, child, fdref: null_mut(),
            outq: VecDeque::new(),
            frame_scheduled: false, sync_since: 0.0,
            sel: input::Sel::default(),
            drag_pt: CGPoint { x: 0.0, y: 0.0 },
            autoscroll: false,
            last_cursor: (0, 0),
            scroll_acc: 0.0, focused: true, ready: false,
            view_w: w, view_h: h,
            cwd: String::new(),
            cmd: String::new(),
        }));

        let fdref = CFFileDescriptorCreate(null(), pty, false, pty_cb, null_mut());
        app().fdref = fdref;
        let src = CFFileDescriptorCreateRunLoopSource(null(), fdref, 0);
        CFRunLoopAddSource(CFRunLoopGetMain(), src, kCFRunLoopCommonModes);
        CFRelease(src);
        CFFileDescriptorEnableCallBacks(fdref, FD_READ_CB);

        let cwt = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent(), 0.5, 0, 0, poll_cb, null_mut());
        CFRunLoopAddTimer(CFRunLoopGetMain(), cwt, kCFRunLoopCommonModes);
        CFRelease(cwt);

        app().ready = true;
        relayout();
        let _: () = msg![(): win, "center"];
        let _: () = msg![(): win, "makeKeyAndOrderFront:", Id: null_mut()];
        let _: () = msg![(): nsapp, "activateIgnoringOtherApps:", bool: true];
        let _: () = msg![(): nsapp, "run"];
    }
}
