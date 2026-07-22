use enigo::{Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

enum Sel {
    Text(String),
    /// UIA exposes a text pattern and reports no selection - authoritative.
    Empty,
    /// The focused app exposes no text pattern; UIA can't tell.
    Unknown,
}

/// Captures the current selection via UI Automation, falling back to a
/// simulated Ctrl+C only when UIA can't answer. UIA reporting an empty
/// selection is trusted (nothing selected -> open empty, no keystroke), so
/// terminals aren't interrupted and editors don't copy the caret's line.
///
/// VS Code is special-cased: with `editor.accessibilitySupport` off, Monaco
/// still exposes a text pattern that always reports empty, so UIA cannot be
/// trusted. For Code.exe only we use Ctrl+C and VS Code's clipboard metadata
/// (`isFromEmptySelection`) to tell a real selection from a caret-only line
/// copy. Other apps keep the UIA path unchanged.
pub fn grab_selection() -> Option<String> {
    let uia = uia_selection();
    if is_vscode_foreground() {
        return grab_vscode(uia);
    }
    match uia {
        Sel::Text(t) => Some(t),
        Sel::Empty => None,
        Sel::Unknown => clipboard_grab(),
    }
}

fn grab_vscode(uia: Sel) -> Option<String> {
    // Accessibility mode still works when enabled; prefer it (no keystroke).
    if let Sel::Text(t) = uia {
        return Some(t);
    }
    // Integrated terminal: Ctrl+C is SIGINT when nothing is selected.
    if focus_looks_like_vscode_terminal() {
        return None;
    }
    vscode_editor_clipboard_grab()
}

fn uia_selection() -> Sel {
    unsafe {
        // repeat calls return S_FALSE; harmless on the hotkey thread
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(auto) =
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        else {
            return Sel::Unknown;
        };
        let Ok(el) = auto.GetFocusedElement() else {
            return Sel::Unknown;
        };
        let Ok(pat) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) else {
            return Sel::Unknown;
        };
        let Ok(sel) = pat.GetSelection() else {
            return Sel::Unknown;
        };
        let n = sel.Length().unwrap_or(0);
        let mut out = String::new();
        for i in 0..n {
            if let Ok(range) = sel.GetElement(i)
                && let Ok(t) = range.GetText(-1)
            {
                out.push_str(&t.to_string());
            }
        }
        let out = out.trim();
        if out.is_empty() {
            Sel::Empty
        } else {
            Sel::Text(out.to_string())
        }
    }
}

/// True when the focused UIA node (or a near parent) looks like the
/// integrated terminal — used only to avoid sending Ctrl+C into a shell.
fn focus_looks_like_vscode_terminal() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(auto) =
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        else {
            return false;
        };
        let Ok(walker) = auto.RawViewWalker() else {
            return false;
        };
        let Ok(el) = auto.GetFocusedElement() else {
            return false;
        };
        let mut cur = el;
        for _ in 0..10 {
            if let Ok(name) = cur.CurrentName() {
                // Workbench labels: "Terminal 1", "powershell, Terminal", etc.
                if name.to_string().to_ascii_lowercase().contains("terminal") {
                    return true;
                }
            }
            match walker.GetParentElement(&cur) {
                Ok(p) => cur = p,
                Err(_) => break,
            }
        }
        false
    }
}

fn is_vscode_foreground() -> bool {
    let Some(path) = foreground_exe_path() else {
        return false;
    };
    let Some(stem) = Path::new(&path).file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let s = stem.to_ascii_lowercase();
    // Stable + Insiders only; leave Cursor/VSCodium/etc. on the normal UIA path.
    s == "code" || s == "code - insiders"
}

fn foreground_exe_path() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        windows_sys::Win32::Foundation::CloseHandle(h);
        if ok == 0 || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Ctrl+C for VS Code editor; discard caret-only line copies via the
/// `vscode-editor-data` clipboard format (`isFromEmptySelection`).
fn vscode_editor_clipboard_grab() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let saved = cb.get_text().ok();
    if saved.is_some() {
        let _ = cb.clear();
    }

    synth_ctrl_c()?;

    let from_empty = clipboard_vscode_from_empty_selection();
    let text = cb.get_text().ok().filter(|t| !t.trim().is_empty());
    if let Some(s) = saved {
        let _ = cb.set_text(s);
    }
    if from_empty == Some(true) {
        return None;
    }
    text
}

/// Simulates Ctrl+C and reads the clipboard, then restores the previous
/// clipboard text. Non-text clipboard content (images, files) is left alone:
/// it can't be cleared-and-restored, and any text read after the copy is
/// necessarily fresh.
fn clipboard_grab() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let saved = cb.get_text().ok();
    if saved.is_some() {
        let _ = cb.clear();
    }

    synth_ctrl_c()?;

    let text = cb.get_text().ok().filter(|t| !t.trim().is_empty());
    if let Some(s) = saved {
        let _ = cb.set_text(s);
    }
    text
}

fn synth_ctrl_c() -> Option<()> {
    let mut enigo = Enigo::new(&Settings::default()).ok()?;
    // The user may still hold the hotkey's modifiers; release them so the
    // synthesized Ctrl+C is not read as Ctrl+Alt+C.
    for k in [Key::Alt, Key::Shift, Key::Meta, Key::Control] {
        let _ = enigo.key(k, Direction::Release);
    }
    sleep(Duration::from_millis(50));
    let _ = enigo.key(Key::Control, Direction::Press);
    let _ = enigo.key(Key::Unicode('c'), Direction::Click);
    let _ = enigo.key(Key::Control, Direction::Release);
    sleep(Duration::from_millis(150));
    Some(())
}

/// Reads VS Code's `isFromEmptySelection` flag from the copy payload.
/// Electron puts Monaco's `vscode-editor-data` JSON inside Chromium's
/// "Chromium Web Custom MIME Data Format" pickle (not a bare MIME format).
fn clipboard_vscode_from_empty_selection() -> Option<bool> {
    let bytes = clipboard_format_bytes("Chromium Web Custom MIME Data Format")?;
    let json = chromium_custom_mime_value(&bytes, "vscode-editor-data")?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    v.get("isFromEmptySelection")?.as_bool()
}

fn clipboard_format_bytes(format_name: &str) -> Option<Vec<u8>> {
    unsafe {
        let mut name: Vec<u16> = format_name.encode_utf16().collect();
        name.push(0);
        let fmt = RegisterClipboardFormatW(name.as_ptr());
        if fmt == 0 || IsClipboardFormatAvailable(fmt) == 0 {
            return None;
        }
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(fmt);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle as *mut _);
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }
        let size = GlobalSize(handle as *mut _);
        let bytes = if size == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(ptr as *const u8, size).to_vec()
        };
        GlobalUnlock(handle as *mut _);
        CloseClipboard();
        Some(bytes)
    }
}

/// Chromium `base::Pickle` of custom MIME map written on copy:
/// `u32 payload_size`, then `u32 count`, then count × (`String16` key, `String16` value).
/// Each String16 is `u32` length in code units + UTF-16LE payload, 4-byte aligned.
fn chromium_custom_mime_value(bytes: &[u8], want_key: &str) -> Option<String> {
    if bytes.len() < 8 {
        return None;
    }
    let payload_size = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    let payload = bytes.get(4..4 + payload_size.min(bytes.len() - 4))?;
    let mut i = 0usize;
    let count = u32::from_le_bytes(payload.get(i..i + 4)?.try_into().ok()?) as usize;
    i += 4;
    for _ in 0..count {
        let key = pickle_read_string16(payload, &mut i)?;
        let val = pickle_read_string16(payload, &mut i)?;
        if key == want_key {
            return Some(val);
        }
    }
    None
}

fn pickle_read_string16(buf: &[u8], i: &mut usize) -> Option<String> {
    let len = u32::from_le_bytes(buf.get(*i..*i + 4)?.try_into().ok()?) as usize;
    *i += 4;
    let nbytes = len.checked_mul(2)?;
    let slice = buf.get(*i..*i + nbytes)?;
    let u16s: Vec<u16> = slice
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    *i += nbytes;
    // base::Pickle pads each write to 4-byte alignment.
    *i = (*i + 3) & !3;
    Some(String::from_utf16_lossy(&u16s))
}

/// Current mouse position in physical screen pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    Enigo::new(&Settings::default()).ok()?.location().ok()
}
