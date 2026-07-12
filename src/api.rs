use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

pub enum Delta {
    Chunk(String),
    Done,
    Error(String),
}

#[derive(Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// true = model default, false = disabled
    pub reasoning: bool,
}

pub fn tls_agent(
    total_timeout: Option<Duration>,
    follow_redirects: bool,
) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    // timeout_read is per read call, so it can't kill a healthy long stream,
    // but it unblocks the worker thread when the connection silently dies
    let mut b = ureq::builder()
        .tls_connector(Arc::new(tls))
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(120));
    if let Some(t) = total_timeout {
        b = b.timeout(t);
    }
    if !follow_redirects {
        b = b.redirects(0);
    }
    Ok(b.build())
}

/// Streams an OpenAI-compatible chat completion on a background thread,
/// sending deltas through `tx` and waking the UI after each one.
pub fn stream(
    ep: Endpoint,
    messages: Vec<serde_json::Value>,
    tx: Sender<Delta>,
    abort: Arc<AtomicBool>,
    proxy: tao::event_loop::EventLoopProxy<crate::Ev>,
) {
    std::thread::spawn(move || {
        let send = |d: Delta| {
            let _ = tx.send(d);
            let _ = proxy.send_event(crate::Ev::Wake);
        };
        let agent = match tls_agent(None, true) {
            Ok(a) => a,
            Err(e) => return send(Delta::Error(e)),
        };
        let url = format!("{}/chat/completions", ep.base_url.trim_end_matches('/'));
        let mut body = serde_json::json!({ "model": ep.model, "messages": messages, "stream": true });
        // OpenRouter-style disable; enabled = leave the provider default
        if !ep.reasoning {
            body["reasoning"] = serde_json::json!({ "enabled": false });
        }
        let resp = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", ep.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                return send(Delta::Error(format!("HTTP {code}: {detail}")));
            }
            Err(e) => return send(Delta::Error(e.to_string())),
        };
        for line in BufReader::new(resp.into_reader()).lines() {
            if abort.load(Ordering::Relaxed) {
                return;
            }
            let line = match line {
                Ok(l) => l,
                Err(e) => return send(Delta::Error(e.to_string())),
            };
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(err) = v["error"]["message"].as_str() {
                return send(Delta::Error(err.to_string()));
            }
            if let Some(s) = v["choices"][0]["delta"]["content"].as_str()
                && !s.is_empty()
            {
                send(Delta::Chunk(s.to_string()));
            }
        }
        send(Delta::Done);
    });
}

pub fn user(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "user", "content": text })
}

pub fn assistant(text: &str) -> serde_json::Value {
    serde_json::json!({ "role": "assistant", "content": text })
}
