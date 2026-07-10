use enigo::{Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::thread::sleep;
use std::time::Duration;

/// Captures the current selection by simulating Ctrl+C and reading the
/// clipboard, then restores the previous clipboard text. Non-text clipboard
/// content (images, files) is left alone: it can't be cleared-and-restored,
/// and any text read after the copy is necessarily fresh.
pub fn grab_selection() -> Option<String> {
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
