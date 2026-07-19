use crate::hotkey::Msg;
use crate::{Ev, api, config, dav, hotkey, win};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopProxy};
use tao::platform::windows::WindowExtWindows;
use tao::window::{Window, WindowBuilder};
use wry::WebViewBuilder;

#[derive(PartialEq)]
enum Mode {
    Assist,
    Settings,
}

/// Settings form as the page submits it.
#[derive(serde::Deserialize)]
struct Form {
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
    actions: Vec<FormAction>,
}

#[derive(serde::Deserialize)]
struct FormAction {
    name: String,
    prompt: String,
    model: String,
    reasoning: Option<bool>,
    base_url: Option<String>,
    api_key: Option<String>,
    key: Option<String>,
}

pub struct App {
    window: Window,
    webview: wry::WebView,
    _web_context: wry::WebContext,
    hwnd: win::Hwnd,
    proxy: EventLoopProxy<Ev>,
    ipc_rx: Receiver<Value>,
    act_rx: Receiver<Msg>,
    startup_hotkey: String,
    /// welcome/error notes shown once the page reports ready
    startup_notice: Option<String>,
    cfg: config::Config,
    mode: Mode,
    response: String,
    transcript: String,
    messages: Vec<Value>,
    endpoint: Option<api::Endpoint>,
    streaming: bool,
    rx: Option<Receiver<api::Delta>>,
    abort: Arc<AtomicBool>,
    dav_rx: Option<Receiver<dav::Done>>,
    /// credentials of an in-flight restore, merged into the restored config
    dav_cred: Option<(String, String, String)>,
    had_focus: bool,
    /// focus the empty input once, on the first tick after the foreground
    /// lands: repeated MoveFocus calls blur (and re-fold) the input the page
    /// just focused, and calling it before the foreground arrives is a no-op
    focus_pending: bool,
}

/// Markdown to HTML. Raw HTML in model output is neutralized to text: the
/// page runs with an ipc bridge, so the response must stay inert markup.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Markdown to HTML. Follow-up user lines are wrapped in U+001E record
/// separators in the transcript and rendered as muted `<p class="ask">`.
fn md_html(text: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, html};
    const SEP: char = '\u{1e}';
    let md_chunk = |chunk: &str| -> String {
        let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
        let parser = Parser::new_ext(chunk, opts).map(|ev| match ev {
            Event::Html(h) => Event::Text(h),
            Event::InlineHtml(h) => Event::Text(h),
            ev => ev,
        });
        let mut out = String::new();
        html::push_html(&mut out, parser);
        out
    };
    // odd split parts are ask lines: md \x1e ask \x1e md \x1e ask \x1e md
    let mut out = String::new();
    for (i, part) in text.split(SEP).enumerate() {
        if i % 2 == 1 {
            out.push_str("<p class=\"ask\">");
            out.push_str(&html_escape(part));
            out.push_str("</p>\n");
        } else if !part.is_empty() {
            out.push_str(&md_chunk(part));
        }
    }
    out
}

fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let op: Vec<u16> = "open".encode_utf16().chain([0]).collect();
    let wide: Vec<u16> = url.encode_utf16().chain([0]).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
}

fn restart() -> ! {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

impl App {
    pub fn new(event_loop: &EventLoop<Ev>) -> Self {
        let proxy = event_loop.create_proxy();
        let window = WindowBuilder::new()
            .with_title("cue")
            .with_inner_size(tao::dpi::LogicalSize::new(560.0, 600.0))
            .with_decorations(false)
            .with_always_on_top(true)
            .with_visible(false)
            .build(event_loop)
            .expect("window");
        let hwnd = window.hwnd() as win::Hwnd;

        let data_dir = config::secrets_path().with_file_name("webview2");
        let mut web_context = wry::WebContext::new(Some(data_dir));
        let (ipc_tx, ipc_rx) = channel::<Value>();
        let webview = {
            let proxy = proxy.clone();
            WebViewBuilder::new_with_web_context(&mut web_context)
                .with_background_color((250, 250, 249, 255))
                .with_html(include_str!("../assets/ui.html"))
                .with_ipc_handler(move |req| {
                    if let Ok(v) = serde_json::from_str::<Value>(req.body()) {
                        let _ = ipc_tx.send(v);
                        let _ = proxy.send_event(Ev::Wake);
                    }
                })
                .build(&window)
                .expect("webview")
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
        if let Some(e) = hotkey::spawn(hwnd, proxy.clone(), &cfg, act_tx) {
            notes.push(e);
        }
        if created {
            notes.push(format!(
                "welcome - set your api key in settings (tray icon), then press {} on any selected text",
                cfg.hotkey
            ));
        }

        let log = config::path().with_file_name("cue.log");
        if notes.is_empty() {
            let _ = std::fs::remove_file(log);
        } else {
            let _ = std::fs::write(log, notes.join("\n"));
            win::show_centered(hwnd, cfg.size(), true);
        }

        Self {
            window,
            webview,
            _web_context: web_context,
            hwnd,
            proxy,
            ipc_rx,
            act_rx,
            startup_hotkey: cfg.hotkey.clone(),
            startup_notice: (!notes.is_empty()).then(|| notes.join("\n")),
            cfg,
            mode: Mode::Assist,
            response: String::new(),
            transcript: String::new(),
            messages: Vec::new(),
            endpoint: None,
            streaming: false,
            rx: None,
            abort: Arc::new(AtomicBool::new(false)),
            dav_rx: None,
            dav_cred: None,
            had_focus: false,
            focus_pending: false,
        }
    }

    pub fn handle(&mut self, event: Event<Ev>, flow: &mut ControlFlow) {
        *flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            self.hide();
        }
        while let Ok(v) = self.ipc_rx.try_recv() {
            self.on_ipc(v);
        }
        while let Ok(msg) = self.act_rx.try_recv() {
            match msg {
                Msg::Activate(text) => self.activate(text),
                Msg::Settings => self.open_settings(),
            }
        }
        self.poll_stream();
        self.poll_dav();
        // auto-hide once focus moves to another app (assist only: settings
        // should survive a trip to the browser to copy an api key). Polled,
        // because Win32 focus lives on the webview child, not on this window,
        // so tao never reports focus changes. had_focus guards the moments
        // between showing and SetForegroundWindow landing.
        if self.mode == Mode::Assist && self.window.is_visible() {
            if win::foreground_is_ours() {
                self.had_focus = true;
            } else if self.had_focus {
                self.had_focus = false;
                self.hide();
                return;
            }
            if self.focus_pending && self.had_focus {
                self.focus_pending = false;
                let _ = self.webview.focus();
                let _ = self.webview.evaluate_script("document.getElementById('inp').focus()");
            }
            let wake = std::time::Instant::now() + std::time::Duration::from_millis(150);
            *flow = ControlFlow::WaitUntil(wake);
        }
    }

    /// evaluate `func(...args)` in the page; JSON doubles as JS literals
    fn call(&self, func: &str, args: &[Value]) {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let _ = self.webview.evaluate_script(&format!("{func}({})", args.join(",")));
    }

    fn notice(&self, text: &str) {
        self.call("notice", &[json!(text)]);
    }

    fn on_ipc(&mut self, v: Value) {
        match v["cmd"].as_str().unwrap_or("") {
            "ready" => {
                if let Some(n) = self.startup_notice.take() {
                    let names: Vec<&str> = self.cfg.actions.iter().map(|a| a.name.as_str()).collect();
                    self.call("assist", &[json!(""), json!(names), json!(n), json!(true)]);
                }
            }
            "drag" => {
                let _ = self.window.drag_window();
            }
            "close" => self.close(),
            "run" => {
                let idx = v["idx"].as_u64().unwrap_or(0) as usize;
                let input = v["input"].as_str().unwrap_or("").to_string();
                self.run_action(idx, &input);
            }
            "followup" => {
                let q = v["text"].as_str().unwrap_or("").trim().to_string();
                self.send_followup(q);
            }
            "copy" => {
                if !self.response.is_empty()
                    && let Ok(mut cb) = arboard::Clipboard::new()
                {
                    let _ = cb.set_text(self.response.clone());
                }
            }
            "copytext" => {
                if let (Some(t), Ok(mut cb)) = (v["text"].as_str(), arboard::Clipboard::new()) {
                    let _ = cb.set_text(t.to_string());
                }
            }
            "open" => open_url(v["url"].as_str().unwrap_or("")),
            "save" => match serde_json::from_value::<Form>(v["form"].clone()) {
                Ok(f) => self.save(f),
                Err(e) => self.notice(&format!("save failed: {e}")),
            },
            "backup" | "davlist" | "restore" => self.dav_op(&v),
            _ => {}
        }
    }

    /// Window is already visible (hotkey/tray thread showed it); reset state
    /// around the captured text and reload the config. Unsaved settings edits
    /// stay in the page and reappear on the next settings open.
    fn activate(&mut self, text: String) {
        self.abort.store(true, Ordering::Relaxed);
        self.mode = Mode::Assist;
        let mut notice = String::new();
        match config::load() {
            Ok((cfg, _)) => {
                config::sync_autostart(cfg.autostart);
                if cfg.hotkey != self.startup_hotkey {
                    notice = "hotkey changed - restart cue to apply".into();
                }
                self.cfg = config::Config { hotkey: self.startup_hotkey.clone(), ..cfg };
            }
            Err(e) => notice = e,
        }
        self.response.clear();
        self.transcript.clear();
        self.messages.clear();
        self.streaming = false;
        self.rx = None;
        self.had_focus = false;
        // with captured text, keep focus free so number keys fire; with
        // nothing captured, expand the input with the caret in it
        let expand = text.is_empty();
        let names: Vec<&str> = self.cfg.actions.iter().map(|a| a.name.as_str()).collect();
        self.call("assist", &[json!(text), json!(names), json!(notice), json!(expand)]);
        self.focus_pending = expand;
    }

    fn open_settings(&mut self) {
        if let Ok((cfg, _)) = config::load() {
            self.cfg = cfg;
        }
        self.mode = Mode::Settings;
        self.had_focus = false;
        self.call("settings", &[self.settings_payload(None), json!(false)]);
        let _ = self.webview.focus();
    }

    fn settings_payload(&self, dav_pass: Option<&str>) -> Value {
        let c = &self.cfg;
        let secret = |n: &str| c.secrets.get(n).cloned().unwrap_or_default();
        json!({
            "hotkey": c.hotkey, "autostart": c.autostart,
            "width": c.width, "height": c.height,
            "base_url": c.api.base_url, "model": c.api.model, "reasoning": c.api.reasoning,
            "secret": secret(c.api.key_name()),
            "dav_url": c.webdav.url, "dav_user": c.webdav.user,
            "dav_pass": dav_pass.map(String::from).unwrap_or_else(|| secret("webdav")),
            "actions": c.actions.iter().map(|a| json!({
                "name": a.name, "prompt": a.prompt,
                "model": a.model.clone().unwrap_or_default(),
                "reasoning": a.reasoning,
                "base_url": a.base_url, "api_key": a.api_key, "key": a.key,
            })).collect::<Vec<_>>(),
        })
    }

    fn save(&mut self, f: Form) {
        let mut cfg = self.cfg.clone();
        cfg.hotkey = f.hotkey.trim().to_string();
        cfg.autostart = f.autostart;
        cfg.width = f.width.clamp(200.0, 2000.0);
        cfg.height = f.height.clamp(150.0, 2000.0);
        cfg.api.base_url = f.base_url.trim().to_string();
        cfg.api.model = f.model.trim().to_string();
        cfg.api.reasoning = f.reasoning;
        cfg.webdav.url = f.dav_url.trim().to_string();
        cfg.webdav.user = f.dav_user.trim().to_string();
        // unnamed actions can't be saved; they stay in the form for naming
        let unnamed = f.actions.iter().any(|a| a.name.trim().is_empty());
        cfg.actions = f
            .actions
            .into_iter()
            .filter(|a| !a.name.trim().is_empty())
            .map(|a| config::Action {
                name: a.name.trim().to_string(),
                prompt: a.prompt,
                model: {
                    let m = a.model.trim();
                    (!m.is_empty()).then(|| m.to_string())
                },
                // an override equal to the global collapses to inherit
                reasoning: a.reasoning.filter(|v| *v != f.reasoning),
                base_url: a.base_url,
                api_key: a.api_key,
                key: a.key,
            })
            .collect();

        let name = cfg.api.key_name().to_string();
        let secret = f.secret.trim().to_string();
        let mut err = config::set_secret(&name, &secret).err();
        cfg.secrets.insert(name, secret);
        if let Err(e) = config::set_secret("webdav", &f.dav_pass) {
            err = Some(e);
        }
        cfg.secrets.insert("webdav".into(), f.dav_pass);
        if let Err(e) = config::save(&cfg) {
            err = Some(e);
        }
        config::sync_autostart(cfg.autostart);

        if err.is_none() && cfg.hotkey != self.startup_hotkey {
            restart();
        }

        // stay open in settings after save; cancel/Esc/x close
        self.cfg = cfg;
        let notice = match err {
            Some(e) => e,
            None if unnamed => "saved - give the unnamed action a name".into(),
            None => "saved".to_string(),
        };
        self.call("saved", &[json!(notice)]);
    }

    fn close(&mut self) {
        if self.mode == Mode::Settings {
            // discard the draft; Esc paths reach here with dirty still set
            let _ = self.webview.evaluate_script("dirty=false");
            self.mode = Mode::Assist;
        }
        self.hide();
    }

    fn hide(&mut self) {
        self.abort.store(true, Ordering::Relaxed);
        self.streaming = false;
        self.rx = None;
        self.focus_pending = false;
        win::hide(self.hwnd);
    }

    fn start_stream(&mut self) {
        let Some(ep) = self.endpoint.clone() else { return };
        self.abort.store(true, Ordering::Relaxed);
        self.abort = Arc::new(AtomicBool::new(false));
        self.response.clear();
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.streaming = true;
        api::stream(ep, self.messages.clone(), tx, self.abort.clone(), self.proxy.clone());
        self.render_resp();
    }

    fn run_action(&mut self, idx: usize, input: &str) {
        let Some(a) = self.cfg.actions.get(idx) else { return };
        let prompt = a.prompt.replace("{{text}}", input.trim());
        self.endpoint = Some(self.cfg.endpoint(a));
        self.messages = vec![api::user(&prompt)];
        self.transcript.clear();
        self.start_stream();
    }

    fn send_followup(&mut self, q: String) {
        if q.is_empty() || self.streaming || self.response.is_empty() {
            return;
        }
        self.messages.push(api::assistant(&self.response));
        self.messages.push(api::user(&q));
        // muted plain line (class=ask), not bold — bold collides with model emphasis
        let q = q.replace('\u{1e}', "");
        self.transcript
            .push_str(&format!("{}\n\n---\n\n\u{1e}{q}\u{1e}\n\n", self.response));
        self.start_stream();
    }

    fn render_resp(&self) {
        let display = if self.transcript.is_empty() {
            self.response.clone()
        } else {
            format!("{}{}", self.transcript, self.response)
        };
        let html = if display.is_empty() { String::new() } else { md_html(&display) };
        self.call("resp", &[json!(html), json!(self.streaming)]);
    }

    fn poll_stream(&mut self) {
        let Some(rx) = self.rx.take() else { return };
        let mut changed = false;
        let mut keep = true;
        loop {
            match rx.try_recv() {
                Ok(api::Delta::Chunk(s)) => {
                    self.response.push_str(&s);
                    changed = true;
                }
                Ok(api::Delta::Done) => {
                    self.streaming = false;
                    keep = false;
                    changed = true;
                    break;
                }
                Ok(api::Delta::Error(e)) => {
                    self.streaming = false;
                    keep = false;
                    changed = true;
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
        if changed {
            self.render_resp();
        }
    }

    fn dav_op(&mut self, v: &Value) {
        if self.dav_rx.is_some() {
            return; // one operation at a time
        }
        let cred = dav::Cred {
            url: v["dav"]["url"].as_str().unwrap_or("").trim().to_string(),
            user: v["dav"]["user"].as_str().unwrap_or("").trim().to_string(),
            pass: v["dav"]["pass"].as_str().unwrap_or("").to_string(),
        };
        if cred.url.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        match v["cmd"].as_str().unwrap_or("") {
            "backup" => {
                // back up the saved state on disk (sanitized), but carry the
                // webdav connection currently typed in the form - otherwise a
                // backup made before saving uploads a config without it, and
                // a later restore wipes the fields
                let (url, user) = (cred.url.clone(), cred.user.clone());
                match config::load().and_then(|(mut c, _)| {
                    c.webdav.url = url;
                    c.webdav.user = user;
                    config::sanitized_toml(&c)
                }) {
                    Ok(body) => dav::backup(cred, body, tx, self.proxy.clone()),
                    Err(e) => return self.notice(&e),
                }
            }
            "davlist" => dav::list_backups(cred, tx, self.proxy.clone()),
            "restore" => {
                let name = v["name"].as_str().unwrap_or("").to_string();
                self.dav_cred = Some((cred.url.clone(), cred.user.clone(), cred.pass.clone()));
                dav::restore(cred, name, tx, self.proxy.clone());
            }
            _ => return,
        }
        self.dav_rx = Some(rx);
        self.call("davbusy", &[json!(true)]);
    }

    fn poll_dav(&mut self) {
        let Some(rx) = self.dav_rx.take() else { return };
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.dav_rx = Some(rx);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            Ok(dav::Done::Backup(Ok(()))) => self.notice("backup uploaded"),
            Ok(dav::Done::Backup(Err(e))) => self.notice(&format!("backup failed: {e}")),
            Ok(dav::Done::List(Ok(names))) => {
                if names.is_empty() {
                    self.notice("no backups found");
                }
                self.call("davlist", &[json!(names)]);
            }
            Ok(dav::Done::List(Err(e))) => self.notice(&format!("list failed: {e}")),
            Ok(dav::Done::Restore(Err(e))) => self.notice(&format!("restore failed: {e}")),
            Ok(dav::Done::Restore(Ok(body))) => match toml::from_str::<config::Config>(&body) {
                Ok(mut c) => {
                    // a restore must not lose the webdav connection it was
                    // performed with, even if the backup predates it
                    let (url, user, pass) = self.dav_cred.take().unwrap_or_default();
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
                        let pass = (!pass.is_empty()).then_some(pass);
                        self.call(
                            "settings",
                            &[self.settings_payload(pass.as_deref()), json!(true)],
                        );
                    }
                    self.notice("restored from webdav");
                }
                Err(e) => self.notice(&format!("restore: not a valid config: {e}")),
            },
        }
        self.call("davbusy", &[json!(false)]);
    }
}
