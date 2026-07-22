# cue — architecture (as built)

Minimalist Windows selection assistant: one hotkey, one window, one config
file. Rust + tao/wry (system WebView2 renders the UI, giving browser-native
text selection in responses), ~1.7 MB exe + WebView2Loader.dll. Flat square
style: no rounded corners, hairline borders, Segoe UI + system CJK fallback.

## Behavior

- Resident background process with a tray icon (left-click opens the
  assistant; menu: settings / quit). `autostart` syncs an HKCU Run key.
- One global hotkey (default alt+q). On press the hotkey thread captures the
  selection - UI Automation first (no keystrokes, no clipboard; a definitive
  "no selection" sends nothing), simulated Ctrl+C with clipboard save/restore
  only when UIA can't answer (non-text clipboard is left untouched). VS Code
  (Code.exe / Code - Insiders.exe) is special-cased: with editor a11y off,
  Monaco's text pattern lies empty, so those processes use Ctrl+C plus VS
  Code's `vscode-editor-data` (`isFromEmptySelection`) to tell real selection
  from a caret-only line copy; integrated terminal focus skips Ctrl+C. Then
  shows the window at the cursor on the cursor's monitor and messages the UI.
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
- WebDAV: each backup uploads a timestamped `cue-settings-YYYYMMDD-HHMMSS.toml`
  (sanitized: inline keys stripped) to a folder URL with Basic auth; restore
  lists the folder (PROPFIND) and offers a picker, validates before
  overwriting, and keeps the connection it was performed with. A url ending
  in .toml is used verbatim (single file, no picker). Redirects are errors.

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

| File             | Role                                                        |
|------------------|-------------------------------------------------------------|
| `main.rs`        | tao event loop; single `Ev::Wake` user event                |
| `ui.rs`          | window + WebView2, ipc dispatch, streaming, dav, settings   |
| `assets/ui.html` | the whole UI: assist + settings views, styling, page logic  |
| `hotkey.rs`      | dedicated Win32 thread: global hotkey, tray icon, shows the |
|                  | window via HWND, wakes the event loop                       |
| `config.rs`      | TOML schema, template, secrets, autostart registry sync     |
| `api.rs`         | OpenAI-compatible SSE streaming client (ureq + schannel TLS)|
| `dav.rs`         | WebDAV PUT/GET with Basic auth                              |
| `capture.rs`     | UIA/clipboard selection capture, cursor position (enigo)    |
| `win.rs`         | ShowWindow/SetWindowPos, per-monitor DPI, message pump      |

UI split: Rust owns all state and logic; the page renders it. Rust calls page
functions (`assist`, `settings`, `resp`, `notice`, `davlist`, ...) via
`evaluate_script` with JSON-literal arguments; the page posts `{cmd, ...}`
ipc messages (`run`, `followup`, `save`, `drag`, `close`, ...). Markdown is
converted to HTML in Rust (pulldown-cmark; raw HTML neutralized to text) and
set as `innerHTML`, so responses select/copy like any web page. Unsaved
settings edits live in the page (`dirty`) and survive assist activations.
WebView2 profile data lives in `%APPDATA%\cue\webview2`.

Deps: tao, wry, pulldown-cmark, global-hotkey, tray-icon, arboard, enigo,
ureq/native-tls (schannel), base64, serde/serde_json/toml, windows,
windows-sys, winreg. Build with `build.ps1` (w64devkit supplies the binutils
the windows-gnu toolchain lacks; it also copies WebView2Loader.dll beside the
exe - a hard import, the app won't start without it).
