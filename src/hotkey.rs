use crate::{capture, config, win};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

pub enum Msg {
    Activate(String),
    Settings,
}

/// Flat square ring with a center block, light-on-dark for the tray.
fn tray_img() -> tray_icon::Icon {
    const S: usize = 32;
    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let in_square = (3..29).contains(&x) && (3..29).contains(&y);
            let ring = in_square && (x < 6 || x >= 26 || y < 6 || y >= 26);
            let block = (12..20).contains(&x) && (12..20).contains(&y);
            if ring || block {
                let i = (y * S + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[235, 235, 230, 255]);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, S as u32, S as u32).expect("32x32 rgba")
}

/// Runs hotkeys and the tray icon on a dedicated Win32 thread, independent
/// of the render loop (eframe skips the UI of hidden windows, so they can't
/// poll anything). Showing/positioning the window happens here via HWND; the
/// app just receives `Msg`. A failing hotkey degrades to a returned notice.
/// Hotkey changes take effect on restart.
pub fn spawn(
    hwnd: win::Hwnd,
    ctx: eframe::egui::Context,
    cfg: &config::Config,
    tx: Sender<Msg>,
) -> Option<String> {
    let mut notes: Vec<String> = Vec::new();
    let main: Option<HotKey> = match cfg.hotkey.parse() {
        Ok(h) => Some(h),
        Err(e) => {
            notes.push(format!("hotkey \"{}\": {e}", cfg.hotkey));
            None
        }
    };

    let (res_tx, res_rx) = channel::<Vec<String>>();
    std::thread::spawn(move || {
        let mut notes: Vec<String> = Vec::new();
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                let _ = res_tx.send(vec![format!("hotkeys unavailable: {e}")]);
                return;
            }
        };
        let mut main_id = None;
        if let Some(h) = main {
            match manager.register(h) {
                Ok(()) => main_id = Some(h.id()),
                Err(e) => notes.push(format!("hotkey: {e}")),
            }
        }
        let _ = res_tx.send(notes);

        // shared "notify the app and show the window" for all event sources.
        // Send before showing: showing first can trigger a UI frame that sees
        // an unexplained visible window and hides it again.
        let send = {
            let ctx = ctx.clone();
            Arc::new(move |msg: Msg, at: Option<(i32, i32)>, size: (f32, f32), topmost: bool| {
                let _ = tx.send(msg);
                match at {
                    Some((x, y)) => win::show_at(hwnd, x, y, size),
                    None => win::show_centered(hwnd, size, topmost),
                }
                ctx.request_repaint();
            })
        };
        let cfg_size = || config::load().map(|(c, _)| c.size()).unwrap_or((560.0, 600.0));

        {
            let send = send.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                if e.state == HotKeyState::Pressed && Some(e.id) == main_id {
                    let text = capture::grab_selection().unwrap_or_default();
                    let at = capture::cursor_pos().unwrap_or((300, 300));
                    send(Msg::Activate(text), Some(at), cfg_size(), true);
                }
            }));
        }

        let menu = Menu::new();
        let m_settings = MenuItem::new("settings", true, None);
        let m_quit = MenuItem::new("quit", true, None);
        let _ = menu.append_items(&[&m_settings, &m_quit]);
        let (settings_id, quit_id) = (m_settings.id().clone(), m_quit.id().clone());
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("cue")
            .with_icon(tray_img())
            .build();

        {
            let send = send.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if e.id == quit_id {
                    std::process::exit(0);
                } else if e.id == settings_id {
                    // settings: fixed default size, regular (non-topmost) window
                    send(Msg::Settings, None, (560.0, 600.0), false);
                }
            }));
        }
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                send(Msg::Activate(String::new()), None, cfg_size(), true);
            }
        }));

        win::message_pump();
        drop(manager);
        drop(tray);
    });

    match res_rx.recv() {
        Ok(thread_notes) => notes.extend(thread_notes),
        Err(_) => notes.push("hotkey thread failed to start".into()),
    }
    (!notes.is_empty()).then(|| notes.join("\n"))
}
