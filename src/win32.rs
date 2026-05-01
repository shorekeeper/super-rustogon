//! Hand-written Win32 bindings, just enough to host a Vulkan surface
//! and report keyboard plus mouse input.
//!
//! (See original module documentation above.)
//!
//! This revision adds the Shift key (for the dash / slow-mo
//! ability) and extends `Input` with a `shift` field. Esc, arrows,
//! Space, Enter, Up, Down are unchanged.

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub type HINSTANCE = *mut c_void;
pub type HWND      = *mut c_void;
pub type HMENU     = *mut c_void;
pub type HICON     = *mut c_void;
pub type HCURSOR   = *mut c_void;
pub type HBRUSH    = *mut c_void;
pub type HMODULE   = *mut c_void;
pub type LPCWSTR   = *const u16;
pub type WPARAM    = usize;
pub type LPARAM    = isize;
pub type LRESULT   = isize;
pub type DWORD     = u32;
pub type BOOL      = i32;
pub type ATOM      = u16;
pub type UINT      = u32;
pub type LONG      = i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct POINT { pub x: LONG, pub y: LONG }
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RECT { pub left: LONG, pub top: LONG, pub right: LONG, pub bottom: LONG }

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND, pub message: UINT,
    pub wParam: WPARAM, pub lParam: LPARAM,
    pub time: DWORD, pub pt: POINT,
}

pub type WNDPROC = unsafe extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
pub struct WNDCLASSW {
    pub style: UINT,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: c_int, pub cbWndExtra: c_int,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON, pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: LPCWSTR, pub lpszClassName: LPCWSTR,
}

pub const WS_OVERLAPPEDWINDOW: DWORD = 0x00CF0000;
pub const WS_VISIBLE:          DWORD = 0x10000000;
pub const SW_SHOW:             c_int = 5;
pub const CW_USEDEFAULT:       c_int = 0x80000000u32 as c_int;
pub const PM_REMOVE:           UINT  = 0x0001;

pub const WM_DESTROY:     UINT = 0x0002;
pub const WM_SIZE:        UINT = 0x0005;
pub const WM_CLOSE:       UINT = 0x0010;
pub const WM_QUIT:        UINT = 0x0012;
pub const WM_KEYDOWN:     UINT = 0x0100;
pub const WM_KEYUP:       UINT = 0x0101;
pub const WM_MOUSEMOVE:   UINT = 0x0200;
pub const WM_LBUTTONDOWN: UINT = 0x0201;
pub const WM_LBUTTONUP:   UINT = 0x0202;
pub const WM_MOUSEWHEEL:  UINT = 0x020A;

pub const VK_ESCAPE: WPARAM = 0x1B;
pub const VK_LEFT:   WPARAM = 0x25;
pub const VK_UP:     WPARAM = 0x26;
pub const VK_RIGHT:  WPARAM = 0x27;
pub const VK_DOWN:   WPARAM = 0x28;
pub const VK_SPACE:  WPARAM = 0x20;
pub const VK_RETURN: WPARAM = 0x0D;
pub const VK_SHIFT:  WPARAM = 0x10;
pub const VK_LSHIFT: WPARAM = 0xA0;
pub const VK_RSHIFT: WPARAM = 0xA1;

pub const VK_DELETE: WPARAM = 0x2E;

pub const IDC_ARROW: LPCWSTR = 32512usize as LPCWSTR;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleW(lpModuleName: LPCWSTR) -> HMODULE;
}

#[link(name = "user32")]
unsafe extern "system" {
    pub fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> ATOM;
    pub fn CreateWindowExW(
        dwExStyle: DWORD, lpClassName: LPCWSTR, lpWindowName: LPCWSTR,
        dwStyle: DWORD, X: c_int, Y: c_int, nWidth: c_int, nHeight: c_int,
        hWndParent: HWND, hMenu: HMENU, hInstance: HINSTANCE,
        lpParam: *mut c_void,
    ) -> HWND;
    pub fn DefWindowProcW(hWnd: HWND, Msg: UINT, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    pub fn ShowWindow(hWnd: HWND, nCmdShow: c_int) -> BOOL;
    pub fn UpdateWindow(hWnd: HWND) -> BOOL;
    pub fn PeekMessageW(lpMsg: *mut MSG, hWnd: HWND,
        wMsgFilterMin: UINT, wMsgFilterMax: UINT, wRemoveMsg: UINT) -> BOOL;
    pub fn TranslateMessage(lpMsg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(lpMsg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(nExitCode: c_int);
    pub fn GetClientRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    pub fn LoadCursorW(hInstance: HINSTANCE, lpCursorName: LPCWSTR) -> HCURSOR;
}

static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);
static CLIENT_W: AtomicI32 = AtomicI32::new(0);
static CLIENT_H: AtomicI32 = AtomicI32::new(0);
static RESIZED: AtomicBool = AtomicBool::new(false);

static KEY_LEFT:   AtomicBool = AtomicBool::new(false);
static KEY_RIGHT:  AtomicBool = AtomicBool::new(false);
static KEY_UP:     AtomicBool = AtomicBool::new(false);
static KEY_DOWN:   AtomicBool = AtomicBool::new(false);
static KEY_SPACE:  AtomicBool = AtomicBool::new(false);
static KEY_ENTER:  AtomicBool = AtomicBool::new(false);
static KEY_ESCAPE: AtomicBool = AtomicBool::new(false);
static KEY_SHIFT:  AtomicBool = AtomicBool::new(false);

static MOUSE_X: AtomicI32 = AtomicI32::new(0);
static MOUSE_Y: AtomicI32 = AtomicI32::new(0);
static MOUSE_LEFT: AtomicBool = AtomicBool::new(false);

/// Accumulated mouse wheel delta since the last `take_scroll_delta`
/// call. Positive values are scroll up / away from the user,
/// negative are scroll down / toward the user. Integer units of
/// WHEEL_DELTA = 120 on a typical PC mouse; callers divide by 120
/// to get notch counts.
static SCROLL_DELTA: AtomicI32 = AtomicI32::new(0);

/// Is the Delete key currently down?
static KEY_DELETE:  AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Default, Debug)]
pub struct Input {
    pub left:   bool,
    pub right:  bool,
    pub up:     bool,
    pub down:   bool,
    pub space:  bool,
    pub enter:  bool,
    pub escape: bool,
    pub shift:  bool,
    pub delete: bool,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Mouse {
    pub x: i32, pub y: i32,
    pub left_down: bool,
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: UINT, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE | WM_DESTROY => {
            SHOULD_CLOSE.store(true, Ordering::SeqCst);
            unsafe { PostQuitMessage(0); }
            0
        }
        WM_KEYDOWN => {
            match w {
                VK_ESCAPE => KEY_ESCAPE.store(true, Ordering::SeqCst),
                VK_LEFT   => KEY_LEFT.store(true, Ordering::SeqCst),
                VK_RIGHT  => KEY_RIGHT.store(true, Ordering::SeqCst),
                VK_UP     => KEY_UP.store(true, Ordering::SeqCst),
                VK_DOWN   => KEY_DOWN.store(true, Ordering::SeqCst),
                VK_SPACE  => KEY_SPACE.store(true, Ordering::SeqCst),
                VK_RETURN => KEY_ENTER.store(true, Ordering::SeqCst),
                VK_DELETE => KEY_DELETE.store(true, Ordering::SeqCst),
                VK_SHIFT | VK_LSHIFT | VK_RSHIFT
                          => KEY_SHIFT.store(true, Ordering::SeqCst),
                _ => {}
            }
            0
        }
        WM_KEYUP => {
            match w {
                VK_ESCAPE => KEY_ESCAPE.store(false, Ordering::SeqCst),
                VK_LEFT   => KEY_LEFT.store(false, Ordering::SeqCst),
                VK_RIGHT  => KEY_RIGHT.store(false, Ordering::SeqCst),
                VK_UP     => KEY_UP.store(false, Ordering::SeqCst),
                VK_DOWN   => KEY_DOWN.store(false, Ordering::SeqCst),
                VK_SPACE  => KEY_SPACE.store(false, Ordering::SeqCst),
                VK_RETURN => KEY_ENTER.store(false, Ordering::SeqCst),
                VK_DELETE => KEY_DELETE.store(false, Ordering::SeqCst),
                VK_SHIFT | VK_LSHIFT | VK_RSHIFT
                          => KEY_SHIFT.store(false, Ordering::SeqCst),
                _ => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            let nx = (l & 0xFFFF) as i16 as i32;
            let ny = ((l >> 16) & 0xFFFF) as i16 as i32;
            MOUSE_X.store(nx, Ordering::SeqCst);
            MOUSE_Y.store(ny, Ordering::SeqCst);
            0
        }
        WM_LBUTTONDOWN => { MOUSE_LEFT.store(true, Ordering::SeqCst); 0 }
        WM_LBUTTONUP   => { MOUSE_LEFT.store(false, Ordering::SeqCst); 0 }
        WM_MOUSEWHEEL => {
            // WM_MOUSEWHEEL packs the delta into the high word of
            // wparam as a signed 16 bit value. Unpack explicitly
            // so a downward notch (negative delta) stays negative
            // when cast back to i32.
            let raw = ((w >> 16) & 0xFFFF) as u16;
            let delta = raw as i16 as i32;
            SCROLL_DELTA.fetch_add(delta, Ordering::SeqCst);
            0
        }
        WM_SIZE => {
            let nw = (l & 0xFFFF) as i32;
            let nh = ((l >> 16) & 0xFFFF) as i32;
            CLIENT_W.store(nw, Ordering::SeqCst);
            CLIENT_H.store(nh, Ordering::SeqCst);
            RESIZED.store(true, Ordering::SeqCst);
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct Window {
    pub hinstance: HINSTANCE,
    pub hwnd: HWND,
    _class_name: Vec<u16>,
}

impl Window {
    pub fn new(title: &str, width: i32, height: i32) -> Self {
        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());
            let class_name = wide("HexRSWindowClass");
            let title_w = wide(title);
            let cursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);

            let wc = WNDCLASSW {
                style: 0, lpfnWndProc: wnd_proc,
                cbClsExtra: 0, cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(), hCursor: cursor,
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(), lpszClassName: class_name.as_ptr(),
            };
            assert!(RegisterClassW(&wc) != 0, "RegisterClassW failed");

            let hwnd = CreateWindowExW(
                0, class_name.as_ptr(), title_w.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT, CW_USEDEFAULT, width, height,
                ptr::null_mut(), ptr::null_mut(), hinstance, ptr::null_mut(),
            );
            assert!(!hwnd.is_null(), "CreateWindowExW failed");

            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);

            CLIENT_W.store(width, Ordering::SeqCst);
            CLIENT_H.store(height, Ordering::SeqCst);

            Window { hinstance, hwnd, _class_name: class_name }
        }
    }

    pub fn poll_events(&self) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    SHOULD_CLOSE.store(true, Ordering::SeqCst);
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn should_close(&self) -> bool { SHOULD_CLOSE.load(Ordering::SeqCst) }

    pub fn client_size(&self) -> (u32, u32) {
        (CLIENT_W.load(Ordering::SeqCst).max(1) as u32,
         CLIENT_H.load(Ordering::SeqCst).max(1) as u32)
    }

    pub fn take_resized(&self) -> bool { RESIZED.swap(false, Ordering::SeqCst) }

    pub fn input(&self) -> Input {
        Input {
            left:   KEY_LEFT.load  (Ordering::SeqCst),
            right:  KEY_RIGHT.load (Ordering::SeqCst),
            up:     KEY_UP.load    (Ordering::SeqCst),
            down:   KEY_DOWN.load  (Ordering::SeqCst),
            space:  KEY_SPACE.load (Ordering::SeqCst),
            enter:  KEY_ENTER.load (Ordering::SeqCst),
            escape: KEY_ESCAPE.load(Ordering::SeqCst),
            shift:  KEY_SHIFT.load (Ordering::SeqCst),
            delete: KEY_DELETE.load(Ordering::SeqCst),
        }
    }

    pub fn mouse(&self) -> Mouse {
        Mouse {
            x: MOUSE_X.load(Ordering::SeqCst),
            y: MOUSE_Y.load(Ordering::SeqCst),
            left_down: MOUSE_LEFT.load(Ordering::SeqCst),
        }
    }

    /// Pull the accumulated mouse wheel delta since the last
    /// call. Units are raw WHEEL_DELTA (120 per notch). The
    /// editor divides by 120 to convert to clean integer notch
    /// counts for canvas zoom and scroll lists.
    pub fn take_scroll_delta(&self) -> i32 {
        SCROLL_DELTA.swap(0, Ordering::SeqCst)
    }
    
}