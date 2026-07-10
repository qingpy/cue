use crate::hotkey::Msg;
use crate::{api, config, dav, hotkey, win};
use eframe::egui::{
    Align2, Button, CentralPanel, Color32, Context, CornerRadius, DragValue, FontId, Frame, Id,
    Key, LayerId, Margin, Modifiers, Order, Panel, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextEdit, Theme, Ui, ViewportCommand, vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

const BG: Color32 = Color32::from_rgb(250, 250, 249);
const BORDER: Color32 = Color32::from_rgb(160, 160, 155);
const WEAK: Color32 = Color32::from_rgb(130, 130, 125);

#[derive(PartialEq)]
enum Mode {
    Assist,
    Settings,
}

struct DraftAction {
    name: String,
    prompt: String,
    model: String,
    /// None = inherit the global; the checkbox shows the effective value and
    /// toggling it pins an explicit override
    reasoning: Option<bool>,
    base_url: Option<String>,
    api_key: Option<String>,
    key: Option<String>,
}

struct Draft {
    hotkey: String,
    autostart: bool,
    width: f32,
    height: f32,
    base_url: String,
    model: String,
    reasoning: bool,
    secret: String,
    dav_url: String,
    dav_user: String,
    dav_pass: String,
    actions: Vec<DraftAction>,
    expanded: Option<usize>,
    focus_expanded: bool,
}

impl Draft {
    fn from(cfg: &config::Config) -> Self {
        let name = cfg.api.key_name();
        Self {
            hotkey: cfg.hotkey.clone(),
            autostart: cfg.autostart,
            width: cfg.width,
            height: cfg.height,
            base_url: cfg.api.base_url.clone(),
            model: cfg.api.model.clone(),
            reasoning: cfg.api.reasoning,
            secret: cfg.secrets.get(name).cloned().unwrap_or_default(),
            dav_url: cfg.webdav.url.clone(),
            dav_user: cfg.webdav.user.clone(),
            dav_pass: cfg.secrets.get("webdav").cloned().unwrap_or_default(),
            actions: cfg
                .actions
                .iter()
                .map(|a| DraftAction {
                    name: a.name.clone(),
                    prompt: a.prompt.clone(),
                    model: a.model.clone().unwrap_or_default(),
                    reasoning: a.reasoning,
                    base_url: a.base_url.clone(),
                    api_key: a.api_key.clone(),
                    key: a.key.clone(),
                })
                .collect(),
            expanded: None,
            focus_expanded: false,
        }
    }
}

pub struct App {
    hwnd: win::Hwnd,
    act_rx: Receiver<Msg>,
    startup_hotkey: String,
    cfg: config::Config,
    mode: Mode,
    draft: Option<Draft>,
    notice: Option<String>,
    input: String,
    focus_input: bool,
    followup: String,
    response: String,
    transcript: String,
    messages: Vec<serde_json::Value>,
    endpoint: Option<api::Endpoint>,
    streaming: bool,
    rx: Option<Receiver<api::Delta>>,
    abort: Arc<AtomicBool>,
    cache: CommonMarkCache,
    had_focus: bool,
    want_visible: bool,
    dav_rx: Option<Receiver<dav::Done>>,
}

fn square_style(ctx: &Context) {
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::ZERO;
        }
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.panel_fill = BG;
        v.extreme_bg_color = Color32::WHITE;
    });
}

/// Title strip: draggable, with a close (hide) button. Returns close clicked.
fn drag_header(ui: &mut Ui, ctx: &Context, title: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let drag_rect = rect.with_max_x(rect.right() - 24.0);
    let drag = ui.interact(drag_rect, ui.id().with("drag"), Sense::click_and_drag());
    if drag.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    let x_rect = eframe::egui::Rect::from_center_size(
        eframe::egui::pos2(rect.right() - 8.0, rect.center().y),
        vec2(16.0, 16.0),
    );
    let x = ui.interact(x_rect, ui.id().with("close"), Sense::click());
    let p = ui.painter();
    p.text(rect.left_center(), Align2::LEFT_CENTER, title, FontId::monospace(11.0), WEAK);
    let x_color = if x.hovered() { Color32::from_rgb(40, 40, 38) } else { WEAK };
    p.text(x_rect.center(), Align2::CENTER_CENTER, "×", FontId::proportional(14.0), x_color);
    x.clicked()
}

fn section(ui: &mut Ui, label: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(label).monospace().size(10.0).color(WEAK));
    ui.add_space(2.0);
}

fn restart() -> ! {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.set_theme(Theme::Light);
        square_style(ctx);
        crate::font::install(ctx);

        let hwnd: win::Hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => 0,
        };

        let mut notes: Vec<String> = Vec::new();
        let (cfg, created) = match config::load() {
            Ok(pair) => pair,
            Err(e) => {
                notes.push(e);
                (config::Config::default(), false)
            }
        };
        config::sync_autostart(cfg.autostart);

        let (act_tx, act_rx) = channel();
        if let Some(e) = hotkey::spawn(hwnd, ctx.clone(), &cfg, act_tx) {
            notes.push(e);
        }
        if created {
            notes.push(format!(
                "welcome - set your api key in settings (tray icon), then press {} on any selected text",
                cfg.hotkey
            ));
        }

        // First run or broken config: surface the window. Delayed so eframe
        // finishes window setup first (the UI of hidden windows never runs,
        // so this can't be done from the update loop).
        let log = config::path().with_file_name("cue.log");
        if notes.is_empty() {
            let _ = std::fs::remove_file(log);
            // guarantee a frame after eframe's forced first show, so the
            // want_visible enforcement below can re-hide the window
            let wake = ctx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                wake.request_repaint();
            });
        } else {
            let _ = std::fs::write(log, notes.join("\n"));
            let wake = ctx.clone();
            let size = cfg.size();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(400));
                win::show_centered(hwnd, size, true);
                wake.request_repaint();
            });
        }

        Self {
            hwnd,
            act_rx,
            startup_hotkey: cfg.hotkey.clone(),
            want_visible: !notes.is_empty(),
            notice: (!notes.is_empty()).then(|| notes.join("\n")),
            cfg,
            mode: Mode::Assist,
            draft: None,
            input: String::new(),
            focus_input: false,
            followup: String::new(),
            response: String::new(),
            transcript: String::new(),
            messages: Vec::new(),
            endpoint: None,
            streaming: false,
            rx: None,
            abort: Arc::new(AtomicBool::new(false)),
            cache: CommonMarkCache::default(),
            had_focus: false,
            dav_rx: None,
        }
    }

    fn dav_cred(d: &Draft) -> dav::Cred {
        dav::Cred {
            url: d.dav_url.trim().to_string(),
            user: d.dav_user.trim().to_string(),
            pass: d.dav_pass.clone(),
        }
    }

    fn poll_dav(&mut self) {
        let Some(rx) = self.dav_rx.take() else { return };
        match rx.try_recv() {
            Ok(dav::Done::Backup(Ok(()))) => self.notice = Some("backup uploaded".into()),
            Ok(dav::Done::Backup(Err(e))) => self.notice = Some(format!("backup failed: {e}")),
            Ok(dav::Done::Restore(Ok(body))) => match toml::from_str::<config::Config>(&body) {
                Ok(mut c) => {
                    // a restore must not lose the webdav connection it was
                    // performed with, even if the backup predates it
                    let (url, user, pass) = match &self.draft {
                        Some(d) => {
                            let c = Self::dav_cred(d);
                            (c.url, c.user, c.pass)
                        }
                        None => (
                            self.cfg.webdav.url.clone(),
                            self.cfg.webdav.user.clone(),
                            self.cfg.secrets.get("webdav").cloned().unwrap_or_default(),
                        ),
                    };
                    if c.webdav.url.is_empty() {
                        c.webdav.url = url;
                        c.webdav.user = user;
                    }
                    let merged = toml::to_string_pretty(&c).unwrap_or(body);
                    let _ = config::save_raw(&merged);
                    if let Ok((cfg, _)) = config::load() {
                        self.cfg = cfg;
                    }
                    if self.mode == Mode::Settings {
                        let mut d = Draft::from(&self.cfg);
                        if d.dav_pass.is_empty() {
                            d.dav_pass = pass;
                        }
                        self.draft = Some(d);
                    }
                    self.notice = Some("restored from webdav".into());
                }
                Err(e) => self.notice = Some(format!("restore: not a valid config: {e}")),
            },
            Ok(dav::Done::Restore(Err(e))) => self.notice = Some(format!("restore failed: {e}")),
            Err(std::sync::mpsc::TryRecvError::Empty) => self.dav_rx = Some(rx),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }
    }

    /// Window is already visible (hotkey/tray thread showed it); reset state
    /// around the captured text and reload the config. An unsaved settings
    /// draft is kept in the background and restored on the next settings open.
    fn activate(&mut self, text: String) {
        self.abort.store(true, Ordering::Relaxed);
        self.mode = Mode::Assist;
        match config::load() {
            Ok((cfg, _)) => {
                config::sync_autostart(cfg.autostart);
                self.notice = (cfg.hotkey != self.startup_hotkey)
                    .then(|| "hotkey changed - restart cue to apply".to_string());
                self.cfg = config::Config { hotkey: self.startup_hotkey.clone(), ..cfg };
            }
            Err(e) => self.notice = Some(e),
        }
        // with captured text, leave focus on the window so number keys fire;
        // with nothing captured, put the caret in the input for typing
        self.focus_input = text.is_empty();
        self.input = text;
        self.had_focus = false;
        self.response.clear();
        self.transcript.clear();
        self.followup.clear();
        self.messages.clear();
        self.streaming = false;
        self.rx = None;
    }

    fn open_settings(&mut self) {
        if self.draft.is_none() {
            if let Ok((cfg, _)) = config::load() {
                self.cfg = cfg;
            }
            self.draft = Some(Draft::from(&self.cfg));
        }
        self.mode = Mode::Settings;
        self.had_focus = false;
        self.notice = None;
    }

    fn save_settings(&mut self) {
        let Some(d) = self.draft.take() else { return };
        let mut cfg = self.cfg.clone();
        cfg.hotkey = d.hotkey.trim().to_string();
        cfg.autostart = d.autostart;
        cfg.width = d.width;
        cfg.height = d.height;
        cfg.api.base_url = d.base_url.trim().to_string();
        cfg.api.model = d.model.trim().to_string();
        cfg.api.reasoning = d.reasoning;
        cfg.webdav.url = d.dav_url.trim().to_string();
        cfg.webdav.user = d.dav_user.trim().to_string();
        let dav_pass = d.dav_pass;
        // unnamed actions can't be saved; keep them in the form, don't erase
        let (named, unnamed): (Vec<_>, Vec<_>) =
            d.actions.into_iter().partition(|a| !a.name.trim().is_empty());
        cfg.actions = named
            .into_iter()
            .map(|a| config::Action {
                name: a.name.trim().to_string(),
                prompt: a.prompt,
                model: {
                    let m = a.model.trim();
                    (!m.is_empty()).then(|| m.to_string())
                },
                // an override equal to the global collapses to inherit
                reasoning: a.reasoning.filter(|v| *v != d.reasoning),
                base_url: a.base_url,
                api_key: a.api_key,
                key: a.key,
            })
            .collect();

        let name = cfg.api.key_name().to_string();
        let secret = d.secret.trim().to_string();
        let mut err = config::set_secret(&name, &secret).err();
        cfg.secrets.insert(name, secret);
        if let Err(e) = config::set_secret("webdav", &dav_pass) {
            err = Some(e);
        }
        cfg.secrets.insert("webdav".into(), dav_pass);
        if let Err(e) = config::save(&cfg) {
            err = Some(e);
        }
        config::sync_autostart(cfg.autostart);

        if err.is_none() && cfg.hotkey != self.startup_hotkey {
            restart();
        }

        // stay open in settings after save; cancel/Esc/x close
        self.notice = Some(match err {
            Some(e) => e,
            None if unnamed.is_empty() => "saved".into(),
            None => "saved - give the unnamed action a name".into(),
        });
        self.cfg = cfg;
        let mut fresh = Draft::from(&self.cfg);
        fresh.actions.extend(unnamed);
        self.draft = Some(fresh);
    }

    fn hide(&mut self) {
        self.abort.store(true, Ordering::Relaxed);
        self.streaming = false;
        self.rx = None;
        self.want_visible = false;
        win::hide(self.hwnd);
    }

    fn start_stream(&mut self, ctx: &Context) {
        let Some(ep) = self.endpoint.clone() else { return };
        self.abort.store(true, Ordering::Relaxed);
        self.abort = Arc::new(AtomicBool::new(false));
        self.response.clear();
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.streaming = true;
        api::stream(ep, self.messages.clone(), tx, self.abort.clone(), ctx.clone());
    }

    fn run_action(&mut self, idx: usize, ctx: &Context) {
        let Some(a) = self.cfg.actions.get(idx) else { return };
        let prompt = a.prompt.replace("{{text}}", self.input.trim());
        self.endpoint = Some(self.cfg.endpoint(a));
        self.messages = vec![api::user(&prompt)];
        self.transcript.clear();
        self.start_stream(ctx);
    }

    fn send_followup(&mut self, ctx: &Context) {
        let q = self.followup.trim().to_string();
        if q.is_empty() || self.streaming || self.response.is_empty() {
            return;
        }
        self.messages.push(api::assistant(&self.response));
        self.messages.push(api::user(&q));
        self.transcript.push_str(&format!("{}\n\n---\n\n**{}**\n\n", self.response, q));
        self.followup.clear();
        self.start_stream(ctx);
    }

    fn poll_stream(&mut self) {
        let Some(rx) = self.rx.take() else { return };
        let mut keep = true;
        loop {
            match rx.try_recv() {
                Ok(api::Delta::Chunk(s)) => self.response.push_str(&s),
                Ok(api::Delta::Done) => {
                    self.streaming = false;
                    keep = false;
                    break;
                }
                Ok(api::Delta::Error(e)) => {
                    self.streaming = false;
                    keep = false;
                    self.response.push_str(&format!("\n\n**error:** {e}"));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.streaming = false;
                    keep = false;
                    break;
                }
            }
        }
        if keep {
            self.rx = Some(rx);
        }
    }

    fn assist_ui(&mut self, ui: &mut Ui, ctx: &Context) {
        let pad = Frame::new().fill(BG).inner_margin(Margin::same(12));

        let mut close = false;
        Panel::top("head").frame(pad).show_separator_line(false).show(ui, |ui| {
            close = drag_header(ui, ctx, "CUE");
            ui.add_space(6.0);

            // consume Enter before the TextEdit sees it, so it dispatches the
            // action instead of inserting a newline at the caret
            let input_id = Id::new("assist-input");
            let enter = ctx.memory(|m| m.has_focus(input_id))
                && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            let input_resp = ui.add(
                TextEdit::multiline(&mut self.input)
                    .id(input_id)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("enter runs action 1"),
            );
            if self.focus_input {
                self.focus_input = false;
                input_resp.request_focus();
            }
            if enter {
                self.run_action(0, ctx);
            }
            ui.add_space(6.0);

            ui.horizontal_wrapped(|ui| {
                for i in 0..self.cfg.actions.len() {
                    let label = format!("{} {}", i + 1, self.cfg.actions[i].name);
                    let b = ui.button(label);
                    if b.clicked() {
                        // a focused button would swallow the number keys and
                        // re-fire on Enter
                        b.surrender_focus();
                        self.run_action(i, ctx);
                    }
                }
            });
            const NUMS: [Key; 9] = [
                Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5,
                Key::Num6, Key::Num7, Key::Num8, Key::Num9,
            ];
            if ctx.memory(|m| m.focused()).is_none() {
                for (i, k) in NUMS.iter().enumerate().take(self.cfg.actions.len()) {
                    if ctx.input(|inp| inp.key_pressed(*k)) {
                        self.run_action(i, ctx);
                    }
                }
            }
            if let Some(n) = &self.notice {
                ui.add_space(4.0);
                ui.colored_label(WEAK, n);
            }
        });
        if close {
            self.hide();
        }

        Panel::bottom("foot").frame(pad).show_separator_line(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                let has_resp = !self.response.is_empty();
                if ui.add_enabled(has_resp, Button::new("copy")).clicked() {
                    ctx.copy_text(self.response.clone());
                }
                let fu = ui.add_sized(
                    vec2(ui.available_width(), 20.0),
                    TextEdit::singleline(&mut self.followup).hint_text("follow up"),
                );
                if fu.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.send_followup(ctx);
                    fu.request_focus();
                }
            });
        });

        CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.transcript.is_empty() {
                            if !self.response.is_empty() {
                                CommonMarkViewer::new().show(ui, &mut self.cache, &self.response);
                            }
                        } else {
                            let display = format!("{}{}", self.transcript, self.response);
                            CommonMarkViewer::new().show(ui, &mut self.cache, &display);
                        }
                        if self.streaming {
                            ui.spinner();
                        }
                    });
            });
    }

    fn settings_ui(&mut self, ui: &mut Ui, ctx: &Context) {
        let pad = Frame::new().fill(BG).inner_margin(Margin::same(12));
        let mut save = false;
        let mut cancel = false;
        let mut do_backup = false;
        let mut do_restore = false;
        let dav_idle = self.dav_rx.is_none();

        Panel::top("shead").frame(pad).show_separator_line(false).show(ui, |ui| {
            cancel = drag_header(ui, ctx, "CUE · SETTINGS");
        });

        Panel::bottom("sfoot").frame(pad).show_separator_line(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                save = ui.button("save").clicked();
                cancel |= ui.button("cancel").clicked();
            });
            if let Some(n) = &self.notice {
                ui.colored_label(WEAK, n);
            }
        });

        let Some(d) = self.draft.as_mut() else { return };
        CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("hotkey");
                        ui.add(TextEdit::singleline(&mut d.hotkey).desired_width(110.0));
                        ui.checkbox(&mut d.autostart, "start at login");
                    });
                    ui.horizontal(|ui| {
                        ui.label("window");
                        ui.add(DragValue::new(&mut d.width).range(200.0..=2000.0).speed(4));
                        ui.label("x");
                        ui.add(DragValue::new(&mut d.height).range(150.0..=2000.0).speed(4));
                    });

                    section(ui, "API");
                    ui.add(
                        TextEdit::singleline(&mut d.base_url)
                            .desired_width(f32::INFINITY)
                            .hint_text("base_url, e.g. https://openrouter.ai/api/v1"),
                    );
                    ui.add(
                        TextEdit::singleline(&mut d.secret)
                            .desired_width(f32::INFINITY)
                            .password(true)
                            .hint_text("api key (stored in %APPDATA%\\cue, outside the project)"),
                    );
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.reasoning, "reasoning");
                        ui.add(
                            TextEdit::singleline(&mut d.model)
                                .desired_width(ui.available_width())
                                .hint_text("model, e.g. openai/gpt-4o-mini"),
                        );
                    });

                    section(ui, "ACTIONS");
                    let mut remove = None;
                    let global_reasoning = d.reasoning;
                    for (i, a) in d.actions.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(
                                TextEdit::singleline(&mut a.name)
                                    .desired_width(110.0)
                                    .hint_text("name"),
                            );
                            if ui.button("x").clicked() {
                                remove = Some(i);
                            }
                            let mut eff = a.reasoning.unwrap_or(global_reasoning);
                            if ui.checkbox(&mut eff, "reasoning").changed() {
                                a.reasoning = Some(eff);
                            }
                            ui.add(
                                TextEdit::singleline(&mut a.model)
                                    .desired_width(ui.available_width())
                                    .hint_text("model override (optional)"),
                            );
                        });
                        // prompts fold to a one-line preview; clicking swaps
                        // in the editor (desired_rows alone can't fold: it is
                        // a minimum, content always grows the field)
                        let pid = Id::new(("prompt", i));
                        if d.expanded == Some(i) {
                            let te = ui.add(
                                TextEdit::multiline(&mut a.prompt)
                                    .id(pid)
                                    .desired_rows(4)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("prompt; {{text}} = captured text"),
                            );
                            if d.focus_expanded {
                                d.focus_expanded = false;
                                te.request_focus();
                            }
                            if te.lost_focus() && d.expanded == Some(i) {
                                d.expanded = None;
                            }
                        } else {
                            let mut preview = {
                                let mut lines = a.prompt.lines().filter(|l| !l.trim().is_empty());
                                let first = lines.next().unwrap_or("").to_string();
                                if lines.next().is_some() { format!("{first} ...") } else { first }
                            };
                            let r = ui.add(
                                TextEdit::singleline(&mut preview)
                                    .id(pid.with("preview"))
                                    .desired_width(f32::INFINITY)
                                    .hint_text("prompt; {{text}} = captured text"),
                            );
                            if r.gained_focus() {
                                d.expanded = Some(i);
                                d.focus_expanded = true;
                            }
                        }
                        ui.add_space(8.0);
                    }
                    if let Some(i) = remove {
                        d.actions.remove(i);
                    }
                    if ui.button("+ action").clicked() {
                        d.actions.push(DraftAction {
                            name: String::new(),
                            prompt: "{{text}}".into(),
                            model: String::new(),
                            reasoning: None,
                            base_url: None,
                            api_key: None,
                            key: None,
                        });
                    }

                    section(ui, "WEBDAV BACKUP");
                    ui.add(
                        TextEdit::singleline(&mut d.dav_url)
                            .desired_width(f32::INFINITY)
                            .hint_text("folder url, e.g. https://dav.example.com/cue"),
                    );
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut d.dav_user)
                                .desired_width(140.0)
                                .hint_text("user"),
                        );
                        ui.add(
                            TextEdit::singleline(&mut d.dav_pass)
                                .desired_width(ui.available_width())
                                .password(true)
                                .hint_text("password"),
                        );
                    });
                    ui.horizontal(|ui| {
                        let ready = !d.dav_url.trim().is_empty() && dav_idle;
                        if ui.add_enabled(ready, Button::new("backup")).clicked() {
                            do_backup = true;
                        }
                        if ui.add_enabled(ready, Button::new("restore")).clicked() {
                            do_restore = true;
                        }
                        if !dav_idle {
                            ui.spinner();
                        }
                    });
                    ui.add_space(12.0);
                });
            });

        if save {
            self.save_settings();
        } else if cancel {
            self.draft = None;
            self.mode = Mode::Assist;
            self.hide();
        } else if do_backup || do_restore {
            if let Some(d) = &self.draft {
                let cred = Self::dav_cred(d);
                let (tx, rx) = channel();
                self.dav_rx = Some(rx);
                if do_backup {
                    // back up the saved state on disk (sanitized), but carry
                    // the webdav connection currently typed in the form -
                    // otherwise a backup made before saving uploads a config
                    // without it, and a later restore wipes the fields
                    let (url, user) = (cred.url.clone(), cred.user.clone());
                    match config::load().and_then(|(mut c, _)| {
                        c.webdav.url = url;
                        c.webdav.user = user;
                        config::sanitized_toml(&c)
                    }) {
                        Ok(body) => dav::backup(cred, body, tx, ctx.clone()),
                        Err(e) => {
                            self.dav_rx = None;
                            self.notice = Some(e);
                        }
                    }
                } else {
                    dav::restore(cred, tx, ctx.clone());
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        // dav results first, so a same-frame activation's fresh notice wins
        self.poll_dav();
        while let Ok(msg) = self.act_rx.try_recv() {
            self.want_visible = true;
            match msg {
                Msg::Activate(text) => self.activate(text),
                Msg::Settings => self.open_settings(),
            }
        }
        // eframe force-shows the window after its first painted frame; undo
        // any show we didn't ask for
        if !self.want_visible && win::is_visible(self.hwnd) {
            win::hide(self.hwnd);
        }
        self.poll_stream();
        // Esc with a focused field only unfocuses it (egui side); a second
        // Esc closes - prevents one keypress from discarding settings edits
        if ctx.input(|i| i.key_pressed(Key::Escape)) && ctx.memory(|m| m.focused()).is_none() {
            if self.mode == Mode::Settings {
                self.draft = None;
                self.mode = Mode::Assist;
            }
            self.hide();
        }
        // auto-hide once focus moves elsewhere (assist only: settings should
        // survive a trip to the browser to copy an api key). had_focus guards
        // the frames between showing and SetForegroundWindow landing.
        if self.mode == Mode::Assist {
            let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
            if focused {
                self.had_focus = true;
            } else if self.had_focus {
                self.had_focus = false;
                self.hide();
            }
        }

        match self.mode {
            Mode::Assist => self.assist_ui(ui, ctx),
            Mode::Settings => self.settings_ui(ui, ctx),
        }

        // hairline window border
        ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("border"))).rect_stroke(
            ctx.content_rect(),
            CornerRadius::ZERO,
            Stroke::new(1.0, BORDER),
            StrokeKind::Inside,
        );
    }
}
