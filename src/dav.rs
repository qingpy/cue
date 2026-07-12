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
    List(Result<Vec<String>, String>),
}

/// A url ending in .toml is used verbatim (single-file mode); otherwise it
/// is a folder holding timestamped backups.
pub fn is_file_url(base: &str) -> bool {
    base.trim_end_matches('/').ends_with(".toml")
}

fn folder(base: &str) -> &str {
    base.trim_end_matches('/')
}

fn stamp() -> String {
    unsafe {
        let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
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
/// report success); surface them as errors instead. 207 is PROPFIND's
/// Multi-Status success.
fn check(resp: ureq::Response) -> Result<ureq::Response, String> {
    if resp.status() >= 300 && resp.status() != 207 {
        Err(format!("HTTP {} (redirect - check the url)", resp.status()))
    } else {
        Ok(resp)
    }
}

fn put(c: &Cred, body: &str) -> Result<(), String> {
    let url = if is_file_url(&c.url) {
        folder(&c.url).to_string()
    } else {
        format!("{}/cue-settings-{}.toml", folder(&c.url), stamp())
    };
    let agent = crate::api::tls_agent(Some(Duration::from_secs(20)), false)?;
    agent
        .put(&url)
        .set("Authorization", &auth(c))
        .send_string(body)
        .map_err(fmt_err)
        .and_then(check)
        .map(|_| ())
}

fn get(c: &Cred, name: &str) -> Result<String, String> {
    let url = if name.is_empty() {
        folder(&c.url).to_string()
    } else {
        format!("{}/{name}", folder(&c.url))
    };
    let agent = crate::api::tls_agent(Some(Duration::from_secs(20)), false)?;
    let resp = agent
        .get(&url)
        .set("Authorization", &auth(c))
        .call()
        .map_err(fmt_err)
        .and_then(check)?;
    resp.into_string().map_err(|e| e.to_string())
}

/// Backup file names in the folder, newest first (names sort by timestamp).
fn list(c: &Cred) -> Result<Vec<String>, String> {
    let agent = crate::api::tls_agent(Some(Duration::from_secs(20)), false)?;
    let resp = agent
        .request("PROPFIND", folder(&c.url))
        .set("Authorization", &auth(c))
        .set("Depth", "1")
        .call()
        .map_err(fmt_err)
        .and_then(check)?;
    let xml = resp.into_string().map_err(|e| e.to_string())?;
    let mut names: Vec<String> = xml
        .split("href>")
        .filter_map(|seg| seg.find("</").map(|end| &seg[..end]))
        .filter_map(|href| href.trim_end_matches('/').rsplit('/').next())
        .filter(|n| n.starts_with("cue-settings") && n.ends_with(".toml"))
        .map(String::from)
        .collect();
    names.sort();
    names.dedup();
    names.reverse();
    Ok(names)
}

type Wake = tao::event_loop::EventLoopProxy<crate::Ev>;

pub fn backup(c: Cred, body: String, tx: Sender<Done>, proxy: Wake) {
    std::thread::spawn(move || {
        let _ = tx.send(Done::Backup(put(&c, &body)));
        let _ = proxy.send_event(crate::Ev::Wake);
    });
}

/// name = "" restores the url itself (single-file mode).
pub fn restore(c: Cred, name: String, tx: Sender<Done>, proxy: Wake) {
    std::thread::spawn(move || {
        let _ = tx.send(Done::Restore(get(&c, &name)));
        let _ = proxy.send_event(crate::Ev::Wake);
    });
}

pub fn list_backups(c: Cred, tx: Sender<Done>, proxy: Wake) {
    std::thread::spawn(move || {
        let _ = tx.send(Done::List(list(&c)));
        let _ = proxy.send_event(crate::Ev::Wake);
    });
}
