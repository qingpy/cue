use enigo::{Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};

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
/// Requires accessibility to be exposed by the app (e.g. VS Code needs
/// `editor.accessibilitySupport: on`); apps without a text pattern still use
/// Ctrl+C.
pub fn grab_selection() -> Option<String> {
    match uia_selection() {
        Sel::Text(t) => Some(t),
        Sel::Empty => None,
        Sel::Unknown => clipboard_grab(),
    }
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

    let text = cb.get_text().ok().filter(|t| !t.trim().is_empty());
    if let Some(s) = saved {
        let _ = cb.set_text(s);
    }
    text
}

/// Current mouse position in physical screen pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    Enigo::new(&Settings::default()).ok()?.location().ok()
}
