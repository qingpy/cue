use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, HWND_NOTOPMOST, HWND_TOPMOST, IsWindowVisible, MSG, SW_HIDE,
    SW_SHOW, SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage,
};

pub type Hwnd = isize;
type RawHwnd = windows_sys::Win32::Foundation::HWND;

fn raw(hwnd: Hwnd) -> RawHwnd {
    hwnd as RawHwnd
}

pub fn hide(hwnd: Hwnd) {
    unsafe {
        ShowWindow(raw(hwnd), SW_HIDE);
    }
}

pub fn is_visible(hwnd: Hwnd) -> bool {
    unsafe { IsWindowVisible(raw(hwnd)) != 0 }
}

/// Scale of the target monitor, not the window's current one - they differ
/// when the hotkey fires on another monitor in a mixed-DPI setup.
fn scale(mon: HMONITOR) -> f32 {
    let (mut x, mut y) = (96u32, 96u32);
    if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y) } == 0 {
        x as f32 / 96.0
    } else {
        1.0
    }
}

fn place(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32, topmost: bool) {
    unsafe {
        let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        SetWindowPos(raw(hwnd), after, x, y, w, h, 0);
        ShowWindow(raw(hwnd), SW_SHOW);
        SetForegroundWindow(raw(hwnd));
    }
}

fn work_area(mon: windows_sys::Win32::Graphics::Gdi::HMONITOR) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) != 0 {
            let r = mi.rcWork;
            Some((r.left, r.top, r.right, r.bottom))
        } else {
            None
        }
    }
}

/// Shows the window near a screen point (physical px), clamped to that
/// monitor's work area.
pub fn show_at(hwnd: Hwnd, x: i32, y: i32, logical: (f32, f32)) {
    let mon = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    let s = scale(mon);
    let (w, h) = ((logical.0 * s) as i32, (logical.1 * s) as i32);
    let (mut px, mut py) = (x + 8, y + 8);
    if let Some((l, t, r, b)) = work_area(mon) {
        px = px.min(r - w - 8).max(l + 8);
        py = py.min(b - h - 8).max(t + 8);
    }
    place(hwnd, px, py, w, h, true);
}

/// Settings opens as a regular (non-topmost) window; the assistant popup is
/// topmost.
pub fn show_centered(hwnd: Hwnd, logical: (f32, f32), topmost: bool) {
    let mon = unsafe { MonitorFromWindow(raw(hwnd), MONITOR_DEFAULTTONEAREST) };
    let s = scale(mon);
    let (w, h) = ((logical.0 * s) as i32, (logical.1 * s) as i32);
    let (mut px, mut py) = (200, 150);
    if let Some((l, t, r, b)) = work_area(mon) {
        px = l + (r - l - w) / 2;
        py = t + (b - t - h) / 2;
    }
    place(hwnd, px, py, w, h, topmost);
}

/// Blocks forever pumping Win32 messages; required on the thread that owns
/// the hotkey registrations.
pub fn message_pump() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
