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
const COLS: usize = 100;
const ROWS: usize = 30;
const SCROLLBACK: usize = 10000;
const FG: u32 = 0xd4d4d4;
const BG: u32 = 0x0e0e12;

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
    pty: c_int,
    fdref: *mut c_void,
    outq: VecDeque<u8>,
    frame_scheduled: bool,
    sync_since: f64,
    sel: input::Sel,
    last_cursor: (usize, usize),
    scroll_acc: f64,
    focused: bool,
    ready: bool,
    view_w: f64,
    view_h: f64,
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
        let drew = render::frame(&mut a.t, &mut a.atlas, &mut fb, &a.sel.on, a.focused, &mut a.drawn);
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

fn set_font(delta: f64) {
    let (pt, scale) = {
        let a = app();
        let pt = (a.font_pt + delta).clamp(6.0, 32.0);
        if pt == a.font_pt { return }
        a.font_pt = pt;
        (pt, a.scale)
    };
    let atlas = Atlas::new(pt * scale as f64);
    app().atlas = atlas;
    relayout();
}

fn copy_selection() {
    let text = {
        let a = app();
        a.sel.text(&a.t)
    };
    let Some(text) = text else { return };
    unsafe {
        let pb: Id = msg![Id: cls("NSPasteboard"), "generalPasteboard"];
        let _: i64 = msg![i64: pb, "clearContents"];
        let s = nsstr(&String::from_utf8_lossy(&text));
        let _: bool = msg![bool: pb, "setString:forType:", Id: s, Id: NSPasteboardTypeString];
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
    let (out, title) = {
        let a = app();
        a.t.feed(buf);
        if a.sel.on.is_some() && !a.sel.dragging {
            a.sel.on = None;
            a.t.all_dirty = true;
        }
        (std::mem::take(&mut a.t.out), a.t.title.take())
    };
    if !out.is_empty() { pty_write(&out) }
    if let Some(s) = title {
        unsafe { msg![(): app().win, "setTitle:", Id: nsstr(&s)] }
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

fn pty_spawn(cols: usize, rows: usize) -> c_int {
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
            .filter(|(k, _)| k != "TERM" && k != "COLORTERM" && k != "TERMCAP" && !k.starts_with("DYLD_"))
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
        m
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

extern "C" fn v_key_down(this: Id, _c: SEL, ev: Id) {
    let (ch, mods) = unsafe { (ev_char(ev), msg![u64: ev, "modifierFlags"]) };
    let act = {
        let a = app();
        input::key(ch, mods, &a.t.modes)
    };
    match act {
        Act::Write(b) => send(&b),
        Act::Copy => copy_selection(),
        Act::Paste => paste(),
        Act::Font(d) => set_font(d),
        Act::NewWindow => {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
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
        b"deleteBackward:" => send(b"\x7f"),
        b"cancelOperation:" => send(b"\x1b"),
        _ => {}
    }
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
                let id = a.t.line_id(gy);
                match kind {
                    input::DOWN => a.sel.begin(&a.t, id, gx, clicks),
                    input::DRAG => a.sel.drag(&a.t, id, gx),
                    _ => a.sel.finish(id, gx),
                }
                a.t.all_dirty = true;
            }
            None
        }
    };
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
        let atlas = Atlas::new(FONT_PT * scale as f64);

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
        let _: () = msg![(): view, "setWantsLayer:", bool: true];
        let layer: Id = msg![Id: view, "layer"];
        let _: () = msg![(): layer, "setContentsGravity:", Id: kCAGravityTopLeft];
        let _: () = msg![(): layer, "setContentsScale:", f64: scale as f64];

        let t = Term::new(COLS, ROWS, SCROLLBACK, FG, BG);
        let pty = pty_spawn(COLS, ROWS);
        G = Box::into_raw(Box::new(App {
            t, atlas,
            surf: [null_mut(); 2], cur: 0,
            stale: [vec![], vec![]], stale_all: [true; 2], drawn: vec![],
            fbw: 0, fbh: 0,
            win, layer, view,
            scale, font_pt: FONT_PT,
            pty, fdref: null_mut(),
            outq: VecDeque::new(),
            frame_scheduled: false, sync_since: 0.0,
            sel: input::Sel::default(),
            last_cursor: (0, 0),
            scroll_acc: 0.0, focused: true, ready: false,
            view_w: w, view_h: h,
        }));

        let fdref = CFFileDescriptorCreate(null(), pty, false, pty_cb, null_mut());
        app().fdref = fdref;
        let src = CFFileDescriptorCreateRunLoopSource(null(), fdref, 0);
        CFRunLoopAddSource(CFRunLoopGetMain(), src, kCFRunLoopCommonModes);
        CFRelease(src);
        CFFileDescriptorEnableCallBacks(fdref, FD_READ_CB);

        app().ready = true;
        relayout();
        let _: () = msg![(): win, "center"];
        let _: () = msg![(): win, "makeKeyAndOrderFront:", Id: null_mut()];
        let _: () = msg![(): nsapp, "activateIgnoringOtherApps:", bool: true];
        let _: () = msg![(): nsapp, "run"];
    }
}
