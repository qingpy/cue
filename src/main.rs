#![windows_subsystem = "windows"]

mod api;
mod capture;
mod config;
mod dav;
mod font;
mod hotkey;
mod ui;
mod win;

fn main() {
    let opts = eframe::NativeOptions {
        // Off-screen: eframe force-shows the window after its first painted
        // frame regardless of with_visible(false); App::ui re-hides it, and
        // out here nobody sees the one-frame flash. No with_taskbar(false):
        // winit implements it via ITaskbarList::DeleteTab, and windows the
        // shell doesn't track appear on every virtual desktop.
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([560.0, 600.0])
            .with_position([-20000.0, -20000.0])
            .with_decorations(false)
            .with_always_on_top()
            .with_visible(false),
        ..Default::default()
    };
    let _ = eframe::run_native("cue", opts, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))));
}
