#![windows_subsystem = "windows"]

mod api;
mod capture;
mod config;
mod dav;
mod hotkey;
mod ui;
mod win;

/// Sole user event: "state changed, drain your queues".
#[derive(Debug)]
pub enum Ev {
    Wake,
}

fn main() {
    let event_loop = tao::event_loop::EventLoopBuilder::<Ev>::with_user_event().build();
    let mut app = ui::App::new(&event_loop);
    event_loop.run(move |event, _, flow| app.handle(event, flow));
}
