//! Hand-written Win32 bindings, just enough to host a Vulkan surface.
//!
//! Only the symbols actually used by the renderer are declared here.
//! All declarations follow the standard `windows.h` ABI, so this module
//! can be cross-checked against MSDN one for one.
//!
//! Window state shared with the rest of the program (close flag, last
//! known client size, resize signal) lives in process-wide atomics so
//! that the C-side `WndProc` callback does not need access to any Rust
//! object. This keeps the FFI boundary trivial.

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
    pub hwnd:    HWND,
    pub message: UINT,
    pub wParam:  WPARAM,
    pub lParam:  LPARAM,
    pub time:    DWORD,
    pub pt:      POINT,
}

pub type WNDPROC = unsafe extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
pub struct WNDCLASSW {
    pub style:         UINT,
    pub lpfnWndProc:   WNDPROC,
    pub cbClsExtra:    c_int,
    pub cbWndExtra:    c_int,
    pub hInstance:     HINSTANCE,
    pub hIcon:         HICON,
    pub hCursor:       HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName:  LPCWSTR,
    pub lpszClassName: LPCWSTR,
}

pub const WS_OVERLAPPEDWINDOW: DWORD = 0x00CF0000;
pub const WS_VISIBLE:          DWORD = 0x10000000;
pub const SW_SHOW:             c_int = 5;
pub const CW_USEDEFAULT:       c_int = 0x80000000u32 as c_int;
pub const PM_REMOVE:           UINT  = 0x0001;

pub const WM_DESTROY: UINT   = 0x0002;
pub const WM_SIZE:    UINT   = 0x0005;
pub const WM_CLOSE:   UINT   = 0x0010;
pub const WM_QUIT:    UINT   = 0x0012;
pub const WM_KEYDOWN: UINT   = 0x0100;

pub const VK_ESCAPE:  WPARAM = 0x1B;
pub const IDC_ARROW:  LPCWSTR = 32512usize as LPCWSTR;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleW(lpModuleName: LPCWSTR) -> HMODULE;
}

#[link(name = "user32")]
unsafe extern "system" {
    pub fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> ATOM;

    pub fn CreateWindowExW(
        dwExStyle:    DWORD,
        lpClassName:  LPCWSTR,
        lpWindowName: LPCWSTR,
        dwStyle:      DWORD,
        X: c_int, Y: c_int,
        nWidth: c_int, nHeight: c_int,
        hWndParent: HWND,
        hMenu:      HMENU,
        hInstance:  HINSTANCE,
        lpParam:    *mut c_void,
    ) -> HWND;

    pub fn DefWindowProcW(hWnd: HWND, Msg: UINT, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    pub fn ShowWindow(hWnd: HWND, nCmdShow: c_int) -> BOOL;
    pub fn UpdateWindow(hWnd: HWND) -> BOOL;
    pub fn PeekMessageW(lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: UINT, wMsgFilterMax: UINT, wRemoveMsg: UINT) -> BOOL;
    pub fn TranslateMessage(lpMsg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(lpMsg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(nExitCode: c_int);
    pub fn GetClientRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    pub fn LoadCursorW(hInstance: HINSTANCE, lpCursorName: LPCWSTR) -> HCURSOR;
}

/// Set when the user requests the window to close (Alt+F4, click X, or Esc).
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);
/// Most recently observed client width / height in pixels.
static CLIENT_W: AtomicI32 = AtomicI32::new(0);
static CLIENT_H: AtomicI32 = AtomicI32::new(0);
/// Sticky flag the renderer drains via `take_resized` to know it should
/// rebuild its swapchain.
static RESIZED: AtomicBool = AtomicBool::new(false);

/// The C-side window procedure. Kept tiny on purpose: it only translates
/// the few messages the renderer cares about into atomic side effects.
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: UINT, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE | WM_DESTROY => {
            SHOULD_CLOSE.store(true, Ordering::SeqCst);
            PostQuitMessage(0);
            0
        }
        WM_KEYDOWN if w == VK_ESCAPE => {
            SHOULD_CLOSE.store(true, Ordering::SeqCst);
            0
        }
        WM_SIZE => {
            // LOWORD / HIWORD of lParam carry the new client size.
            let nw = (l        & 0xFFFF) as i32;
            let nh = ((l >> 16) & 0xFFFF) as i32;
            CLIENT_W.store(nw, Ordering::SeqCst);
            CLIENT_H.store(nh, Ordering::SeqCst);
            RESIZED.store(true, Ordering::SeqCst);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// Convert a Rust `&str` to a null terminated UTF-16 buffer suitable for
/// the `*W` family of Win32 calls.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// RAII-ish handle to a top level Win32 window. Owns nothing the OS would
/// not free on process exit, so a missing Drop impl is acceptable for the
/// skeleton.
pub struct Window {
    pub hinstance: HINSTANCE,
    pub hwnd:      HWND,
    /// Kept alive so that the class name pointer in WNDCLASSW remains valid.
    _class_name:   Vec<u16>,
}

impl Window {
    /// Create and show a top level window with the given caption and initial
    /// client size hint. The system may pick a different actual size; the
    /// renderer must always re-query.
    pub fn new(title: &str, width: i32, height: i32) -> Self {
        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());
            let class_name = wide("HexRSWindowClass");
            let title_w = wide(title);
            let cursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);

            let wc = WNDCLASSW {
                style:         0,
                lpfnWndProc:   wnd_proc,
                cbClsExtra:    0,
                cbWndExtra:    0,
                hInstance:     hinstance,
                hIcon:         ptr::null_mut(),
                hCursor:       cursor,
                hbrBackground: ptr::null_mut(),
                lpszMenuName:  ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            assert!(RegisterClassW(&wc) != 0, "RegisterClassW failed");

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title_w.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT, CW_USEDEFAULT,
                width, height,
                ptr::null_mut(), ptr::null_mut(), hinstance, ptr::null_mut(),
            );
            assert!(!hwnd.is_null(), "CreateWindowExW failed");

            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);

            // Seed the size atomics with the requested values. WM_SIZE will
            // overwrite them with the truthful numbers very shortly.
            CLIENT_W.store(width, Ordering::SeqCst);
            CLIENT_H.store(height, Ordering::SeqCst);

            Window { hinstance, hwnd, _class_name: class_name }
        }
    }

    /// Drain every pending message in the queue without blocking.
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

    /// True once the user (or our Esc handler) has asked the program to quit.
    pub fn should_close(&self) -> bool {
        SHOULD_CLOSE.load(Ordering::SeqCst)
    }

    /// Current client size, clamped away from zero so callers can divide
    /// by it safely. The renderer additionally checks for a minimized
    /// window and bails out before allocating a degenerate swapchain.
    pub fn client_size(&self) -> (u32, u32) {
        (CLIENT_W.load(Ordering::SeqCst).max(1) as u32,
         CLIENT_H.load(Ordering::SeqCst).max(1) as u32)
    }

    /// Atomically read the resize flag and clear it. The renderer calls
    /// this once per frame to decide whether to rebuild the swapchain.
    pub fn take_resized(&self) -> bool {
        RESIZED.swap(false, Ordering::SeqCst)
    }
}