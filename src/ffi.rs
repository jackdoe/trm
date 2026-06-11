#![allow(non_snake_case, non_upper_case_globals, clippy::upper_case_acronyms)]
use std::ffi::{c_char, c_int, c_void};

pub type Id = *mut c_void;
pub type SEL = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGSize {
    pub w: f64,
    pub h: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NSRange {
    pub loc: u64,
    pub len: u64,
}

#[repr(C)]
pub struct ObjcSuper {
    pub receiver: Id,
    pub class: Id,
}

#[repr(C)]
pub struct Winsize {
    pub row: u16,
    pub col: u16,
    pub xpix: u16,
    pub ypix: u16,
}

#[repr(C)]
pub struct Passwd {
    pub name: *mut c_char,
    pub passwd: *mut c_char,
    pub uid: u32,
    pub gid: u32,
    pub change: i64,
    pub class: *mut c_char,
    pub gecos: *mut c_char,
    pub dir: *mut c_char,
    pub shell: *mut c_char,
    pub expire: i64,
}

pub const TIOCSWINSZ: u64 = 0x80087467;
pub const TIOCSCTTY: u64 = 0x20007461;
pub const O_RDWR: c_int = 2;
pub const O_NOCTTY: c_int = 0x20000;
pub const F_SETFL: c_int = 4;
pub const O_NONBLOCK: c_int = 4;
pub const EAGAIN: c_int = 35;
pub const EINTR: c_int = 4;
pub const SIGCHLD: c_int = 20;
pub const SIGPIPE: c_int = 13;
pub const SIG_IGN: usize = 1;

pub const FD_READ_CB: u64 = 1;
pub const FD_WRITE_CB: u64 = 2;

#[link(name = "objc")]
extern "C" {
    pub fn objc_msgSend();
    pub fn objc_msgSendSuper();
    pub fn objc_getClass(name: *const c_char) -> Id;
    pub fn sel_registerName(name: *const c_char) -> SEL;
    pub fn sel_getName(sel: SEL) -> *const c_char;
    pub fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    pub fn objc_registerClassPair(cls: Id);
    pub fn class_addMethod(cls: Id, sel: SEL, imp: *const c_void, types: *const c_char) -> bool;
    pub fn class_getSuperclass(cls: Id) -> Id;
    pub fn object_getClass(obj: Id) -> Id;
    pub fn objc_getProtocol(name: *const c_char) -> Id;
    pub fn class_addProtocol(cls: Id, proto: Id) -> bool;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    pub static NSPasteboardTypeString: Id;
    pub static NSPasteboardTypeFileURL: Id;
}

#[link(name = "QuartzCore", kind = "framework")]
extern "C" {
    pub static kCAGravityTopLeft: Id;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRelease(cf: *const c_void);
    pub fn CFAbsoluteTimeGetCurrent() -> f64;
    pub fn CFRunLoopGetMain() -> *mut c_void;
    pub fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: Id);
    pub fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: Id);
    pub fn CFRunLoopTimerCreate(
        alloc: *const c_void, fire: f64, interval: f64, flags: u64, order: isize,
        cb: extern "C" fn(*mut c_void, *mut c_void), ctx: *mut c_void,
    ) -> *mut c_void;
    pub fn CFFileDescriptorCreate(
        alloc: *const c_void, fd: c_int, close_on_invalidate: bool,
        cb: extern "C" fn(*mut c_void, u64, *mut c_void), ctx: *mut c_void,
    ) -> *mut c_void;
    pub fn CFFileDescriptorCreateRunLoopSource(alloc: *const c_void, f: *mut c_void, order: isize) -> *mut c_void;
    pub fn CFFileDescriptorEnableCallBacks(f: *mut c_void, which: u64);
    pub fn CFDictionaryCreate(
        alloc: *const c_void, keys: *const Id, vals: *const Id, n: isize,
        kcb: *const c_void, vcb: *const c_void,
    ) -> *const c_void;
    pub fn CFNumberCreate(alloc: *const c_void, ty: isize, val: *const c_void) -> Id;
    pub static kCFRunLoopCommonModes: Id;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
}

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    pub fn IOSurfaceCreate(props: *const c_void) -> *mut c_void;
    pub fn IOSurfaceGetBaseAddress(s: *mut c_void) -> *mut c_void;
    pub fn IOSurfaceGetBytesPerRow(s: *mut c_void) -> usize;
    pub fn IOSurfaceLock(s: *mut c_void, opts: u32, seed: *mut u32) -> i32;
    pub fn IOSurfaceUnlock(s: *mut c_void, opts: u32, seed: *mut u32) -> i32;
    pub static kIOSurfaceWidth: Id;
    pub static kIOSurfaceHeight: Id;
    pub static kIOSurfaceBytesPerElement: Id;
    pub static kIOSurfacePixelFormat: Id;
}

extern "C" {
    pub fn posix_openpt(flags: c_int) -> c_int;
    pub fn grantpt(fd: c_int) -> c_int;
    pub fn unlockpt(fd: c_int) -> c_int;
    pub fn ptsname(fd: c_int) -> *mut c_char;
    pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    pub fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    pub fn ioctl(fd: c_int, req: u64, ...) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn fork() -> c_int;
    pub fn setsid() -> c_int;
    pub fn dup2(from: c_int, to: c_int) -> c_int;
    pub fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub fn _exit(code: c_int) -> !;
    pub fn signal(sig: c_int, handler: usize) -> usize;
    pub fn getuid() -> u32;
    pub fn getpwuid(uid: u32) -> *mut Passwd;
    pub fn __error() -> *mut c_int;
}

pub fn errno() -> c_int {
    unsafe { *__error() }
}

#[macro_export]
macro_rules! msg {
    ($ret:ty : $obj:expr, $sel:expr $(, $aty:ty : $arg:expr)*) => {{
        let f: extern "C" fn($crate::ffi::Id, $crate::ffi::SEL $(, $aty)*) -> $ret =
            std::mem::transmute($crate::ffi::objc_msgSend as *const std::ffi::c_void);
        f($obj, $crate::sel($sel) $(, $arg)*)
    }};
}
