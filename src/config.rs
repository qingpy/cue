use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const TEMPLATE: &str = r#"hotkey    = "alt+q"
autostart = false
width     = 560
height    = 600

[api]
base_url = "https://openrouter.ai/api/v1"
model    = "openai/gpt-4o-mini"
# api key: put it in the secrets file (see key below), or inline: api_key = "sk-..."
key      = "default"

[[actions]]
name   = "translate"
prompt = """
Translate the following into Chinese (into English if it is already Chinese).
Output only the translation.

{{text}}"""

[[actions]]
name   = "explain"
prompt = """
Explain the following concisely:

{{text}}"""

[[actions]]
name   = "ask"
prompt = "{{text}}"
# per-action overrides:
# model    = "..."
# base_url = "..."
# key      = "..."   # a name in the secrets file
# api_key  = "..."   # or an inline key
"#;

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct Config {
    #[serde(default = "d_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "d_width")]
    pub width: f32,
    #[serde(default = "d_height")]
    pub height: f32,
    #[serde(default)]
    pub api: Api,
    #[serde(default, skip_serializing_if = "Webdav::is_empty")]
    pub webdav: Webdav,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(skip)]
    pub secrets: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct Webdav {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub user: String,
}

impl Webdav {
    fn is_empty(&self) -> bool {
        self.url.is_empty() && self.user.is_empty()
    }
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct Api {
    #[serde(default)]
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default)]
    pub model: String,
    /// true = model default, false = disabled; per-action override wins
    #[serde(
        default = "d_true",
        deserialize_with = "de_reasoning",
        skip_serializing_if = "is_true"
    )]
    pub reasoning: bool,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Action {
    pub name: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// None = inherit the global setting; tolerates the old string form
    #[serde(
        default,
        deserialize_with = "de_reasoning_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning: Option<bool>,
}

fn d_true() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}

fn de_reasoning_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    de_reasoning(d).map(Some)
}

fn de_reasoning<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct V;
    impl serde::de::Visitor<'_> for V {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bool or string")
        }
        fn visit_bool<E>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }
        fn visit_str<E>(self, v: &str) -> Result<bool, E> {
            Ok(!(v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("false")))
        }
    }
    d.deserialize_any(V)
}

fn d_hotkey() -> String {
    "alt+q".into()
}
fn d_width() -> f32 {
    560.0
}
fn d_height() -> f32 {
    600.0
}

impl Api {
    /// Secrets-file entry name for the global key.
    pub fn key_name(&self) -> &str {
        if self.key.is_empty() { "default" } else { &self.key }
    }
}

impl Config {
    pub fn size(&self) -> (f32, f32) {
        (self.width.max(200.0), self.height.max(150.0))
    }

    pub fn endpoint(&self, action: &Action) -> crate::api::Endpoint {
        let secret = |name: &str| self.secrets.get(name).cloned().unwrap_or_default();
        // action-level settings win over global ones; within a level an
        // inline api_key wins over a secrets-file name
        let api_key = match (&action.api_key, &action.key) {
            (Some(k), _) if !k.is_empty() => k.clone(),
            (_, Some(name)) => secret(name),
            _ if !self.api.api_key.is_empty() => self.api.api_key.clone(),
            _ => secret(self.api.key_name()),
        };
        crate::api::Endpoint {
            base_url: action.base_url.clone().unwrap_or_else(|| self.api.base_url.clone()),
            api_key,
            model: action.model.clone().unwrap_or_else(|| self.api.model.clone()),
            reasoning: action.reasoning.unwrap_or(self.api.reasoning),
        }
    }
}

pub fn path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default()
        .join("settings.toml")
}

/// Secrets live outside the project so settings.toml stays shareable:
/// %APPDATA%\cue\secrets.toml with entries like `default = "sk-..."`.
pub fn secrets_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("cue")
        .join("secrets.toml")
}

fn load_secrets() -> BTreeMap<String, String> {
    let p = secrets_path();
    if !p.exists() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, "default = \"\"\n");
    }
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Atomic write: the hotkey thread reads these files at any moment.
fn write_atomic(p: &std::path::Path, s: &str) -> Result<(), String> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, s).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, p).map_err(|e| e.to_string())
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let s = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    write_atomic(&path(), &s)
}

pub fn save_raw(s: &str) -> Result<(), String> {
    write_atomic(&path(), s)
}

/// Serialization with inline api keys stripped; the only form that ever
/// leaves this machine.
pub fn sanitized_toml(cfg: &Config) -> Result<String, String> {
    let mut c = cfg.clone();
    c.api.api_key.clear();
    for a in &mut c.actions {
        a.api_key = None;
    }
    toml::to_string_pretty(&c).map_err(|e| e.to_string())
}

pub fn set_secret(name: &str, value: &str) -> Result<(), String> {
    let mut map = load_secrets();
    map.insert(name.into(), value.into());
    let s = toml::to_string_pretty(&map).map_err(|e| e.to_string())?;
    write_atomic(&secrets_path(), &s)
}

/// Loads settings.toml next to the exe; writes the template on first run.
/// Returns (config, created).
pub fn load() -> Result<(Config, bool), String> {
    let p = path();
    let mut created = false;
    if !p.exists() {
        std::fs::write(&p, TEMPLATE).map_err(|e| e.to_string())?;
        created = true;
    }
    let s = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut cfg: Config = toml::from_str(&s).map_err(|e| format!("settings.toml: {e}"))?;
    cfg.secrets = load_secrets();
    Ok((cfg, created))
}

pub fn sync_autostart(enabled: bool) {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok((key, _)) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") else {
        return;
    };
    if enabled {
        if let Ok(exe) = std::env::current_exe() {
            let _ = key.set_value("cue", &exe.to_string_lossy().to_string());
        }
    } else {
        let _ = key.delete_value("cue");
    }
}
