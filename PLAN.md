# cue — architecture (as built)

Minimalist Windows selection assistant: one hotkey, one window, one config
file. Rust + eframe/egui 0.35, single ~6 MB exe. Flat square style: no
rounded corners, hairline borders, Inter (embedded) + system CJK fallback.

## Behavior

- Resident background process with a tray icon (left-click opens the
  assistant; menu: settings / quit). `autostart` syncs an HKCU Run key.
- One global hotkey (default alt+q). On press the hotkey thread captures the
  selection (save clipboard text, simulate Ctrl+C, read, restore; non-text
  clipboard is left untouched), shows the window at the cursor on the
  cursor's monitor, and messages the UI.
- Assist window (topmost, per-desktop): captured text on top (empty = input
  focused for typing), actions as numbered buttons (`1`-`9`, click, or Enter
  for action 1), streamed Markdown response, copy button, follow-up input
  for multi-turn. Esc/×/focus-loss hides; Esc first unfocuses a field.
- Settings window (regular, non-topmost, fixed 560x600, opened from tray):
  hotkey, start at login, window size, API (base_url / key / model / global
  reasoning toggle), actions (name, per-action reasoning override, model
  override, prompt folded to one line until clicked), WebDAV backup/restore.
  Save keeps the window open; unsaved drafts survive hotkey activations.
  Changing the hotkey restarts the app to re-register it.
- Reasoning: checkbox per action inheriting the global; off sends
  OpenRouter-style `reasoning: {enabled: false}`, on sends nothing.
- WebDAV: uploads `cue-settings.toml` (sanitized: inline keys stripped) to a
  folder URL with Basic auth; restore validates before overwriting and keeps
  the connection it was performed with. Redirects are treated as errors.

## Config & secrets

- `settings.toml` next to the exe, template written on first run, re-read on
  every hotkey press (hand-edits apply instantly; GUI saves rewrite it, so
  comments don't survive a save). Writes are atomic (tmp + rename).
- Secrets in `%APPDATA%\cue\secrets.toml`: API keys by name (`default`, or
  per-action `key = "name"`), WebDAV password under `webdav`. Never uploaded;
  settings.toml stays credential-free unless an inline `api_key` is used.
  Key resolution: action inline > action named > global inline > global named.
- Startup problems are written to `cue.log` next to the exe and shown in a
  centered window; error-free launches are fully silent.

## Structure

| File         | Role                                                         |
|--------------|--------------------------------------------------------------|
| `main.rs`    | eframe launch; window boots hidden off-screen (eframe force- |
|              | shows after the first frame; the UI re-hides unwanted shows) |
| `ui.rs`      | App state machine: assist/settings modes, streaming, drafts  |
| `hotkey.rs`  | dedicated Win32 thread: global hotkey, tray icon, shows the  |
|              | window via HWND (eframe skips hidden windows' UI entirely)   |
| `config.rs`  | TOML schema, template, secrets, autostart registry sync      |
| `api.rs`     | OpenAI-compatible SSE streaming client (ureq + schannel TLS) |
| `dav.rs`     | WebDAV PUT/GET with Basic auth                               |
| `capture.rs` | clipboard-based selection capture, cursor position (enigo)   |
| `win.rs`     | ShowWindow/SetWindowPos, per-monitor DPI, message pump       |
| `font.rs`    | embedded Inter + runtime system CJK fallback                 |

Deps: eframe, egui_commonmark, global-hotkey, tray-icon, arboard, enigo,
ureq/native-tls (schannel), base64, serde/serde_json/toml, windows-sys,
raw-window-handle, winreg. Build with `build.ps1` (w64devkit supplies the
binutils the windows-gnu toolchain lacks).
