use std::sync::mpsc::Sender;
use std::time::Duration;

pub struct Cred {
    pub url: String,
    pub user: String,
    pub pass: String,
}

pub enum Done {
    Backup(Result<(), String>),
    Restore(Result<String, String>),
}

fn file_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with(".toml") {
        base.to_string()
    } else {
        format!("{base}/cue-settings.toml")
    }
}

fn auth(c: &Cred) -> String {
    use base64::Engine as _;
    let raw = format!("{}:{}", c.user, c.pass);
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

fn fmt_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        e => e.to_string(),
    }
}

/// Redirects are disabled (ureq would turn PUT into a body-less GET and
/// report success); surface them as errors instead.
fn check(resp: ureq::Response) -> Result<ureq::Response, String> {
    if resp.status() >= 300 {
        Err(format!("HTTP {} (redirect - check the url)", resp.status()))
    } else {
        Ok(resp)
    }
}

fn put(c: &Cred, body: &str) -> Result<(), String> {
    let agent = crate::api::tls_agent(Some(Duration::from_secs(20)), false)?;
    agent
        .put(&file_url(&c.url))
        .set("Authorization", &auth(c))
        .send_string(body)
        .map_err(fmt_err)
        .and_then(check)
        .map(|_| ())
}

fn get(c: &Cred) -> Result<String, String> {
    let agent = crate::api::tls_agent(Some(Duration::from_secs(20)), false)?;
    let resp = agent
        .get(&file_url(&c.url))
        .set("Authorization", &auth(c))
        .call()
        .map_err(fmt_err)
        .and_then(check)?;
    resp.into_string().map_err(|e| e.to_string())
}

pub fn backup(c: Cred, body: String, tx: Sender<Done>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let _ = tx.send(Done::Backup(put(&c, &body)));
        ctx.request_repaint();
    });
}

pub fn restore(c: Cred, tx: Sender<Done>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let _ = tx.send(Done::Restore(get(&c)));
        ctx.request_repaint();
    });
}
