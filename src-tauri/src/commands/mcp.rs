//! Settings › Tools › MCP Server: the HTTP MCP server that runs inside the app.
//!
//! The stdio mode (`openidea --mcp`, see `ide_mcp`) is a separate process; this
//! one serves the project of the window that turns it on, so its tools act on
//! exactly what the Services and Database views of that window show.
//!
//! State lives in `<config_dir>/mcp.json` (enabled, port, token, allowWrite) so
//! the address and credentials survive restarts. Starting is explicit: the app
//! does not resume the server on launch (the project to serve is not known
//! before a window opens).

use crate::state::AppState;
use ide_core::{IdeError, IdeResult};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{State, Window};

/// The port offered on first use; any free port can be configured.
const DEFAULT_PORT: u16 = 41521;

/// What the settings page shows: the saved configuration plus live state.
#[derive(Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    /// Saved switch: start the server on the next Apply of this page's scope.
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub allow_write: bool,
    /// Bearer token MCP clients must send; regenerated never (a stable one keeps
    /// client configurations valid across restarts).
    pub token: String,
    /// The root of the project the running server serves.
    pub project: Option<String>,
    /// Why the last start failed, until the next attempt.
    pub error: Option<String>,
    /// `[exe, "--mcp", "--project", <root>]`: the stdio client configuration.
    pub stdio_command: Vec<String>,
}

/// `<config_dir>/mcp.json`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Config {
    enabled: bool,
    port: u16,
    allow_write: bool,
    token: String,
}

impl Default for Config {
    fn default() -> Self {
        Self { enabled: false, port: DEFAULT_PORT, allow_write: false, token: ide_mcp::http::new_token() }
    }
}

/// The server of this process (one per app, like the port it binds).
struct Running {
    handle: ide_mcp::http::Handle,
    project: String,
    port: u16,
    allow_write: bool,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);
/// The last start failure, shown by the page until the next Apply.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn config_path(state: &AppState) -> PathBuf {
    state.host.config_dir.join("mcp.json")
}

fn load(state: &AppState) -> Config {
    let path = config_path(state);
    let Ok(text) = std::fs::read_to_string(&path) else { return Config::default() };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!("mcp.json is malformed, using the defaults: {e}");
        Config::default()
    })
}

fn save(state: &AppState, config: &Config) -> IdeResult<()> {
    let path = config_path(state);
    let text = serde_json::to_string_pretty(config).map_err(|e| IdeError::internal(e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| IdeError::internal(format!("cannot write {}: {e}", path.display())))
}

/// The status the page renders, for `window` (a settings window resolves to the
/// project window it belongs to).
fn status(state: &AppState, window: &str) -> McpServerStatus {
    let config = load(state);
    let running = SERVER.lock();
    let project = match running.as_ref() {
        Some(server) => Some(server.project.clone()),
        None => state.host.project(window).ok().map(|p| p.root.to_string_lossy().into_owned()),
    };
    let stdio_command = std::env::current_exe()
        .map(|exe| {
            let mut command = vec![exe.to_string_lossy().into_owned(), "--mcp".into()];
            if let Some(root) = &project {
                command.push("--project".into());
                command.push(root.clone());
            }
            command
        })
        .unwrap_or_default();
    McpServerStatus {
        enabled: config.enabled,
        running: running.is_some(),
        port: running.as_ref().map_or(config.port, |s| s.port),
        allow_write: running.as_ref().map_or(config.allow_write, |s| s.allow_write),
        token: config.token,
        project,
        error: LAST_ERROR.lock().clone(),
        stdio_command,
    }
}

/// Stops the server, if it runs (idempotent).
fn stop() {
    if let Some(server) = SERVER.lock().take() {
        server.handle.stop();
    }
}

#[tauri::command]
#[specta::specta]
pub fn mcp_server_status(window: Window, state: State<'_, AppState>) -> IdeResult<McpServerStatus> {
    Ok(status(&state, window.label()))
}

/// Saves `enabled` / `port` / `allow_write` and applies them: starts the server
/// on the calling window's project, or stops it. Restarts when the address or
/// the project changes while it runs.
#[tauri::command]
#[specta::specta]
pub async fn mcp_server_apply(
    window: Window,
    state: State<'_, AppState>,
    enabled: bool,
    port: u16,
    allow_write: bool,
) -> IdeResult<McpServerStatus> {
    let label = AppState::owner_window(window.label()).to_owned();
    let mut config = load(&state);
    config.enabled = enabled;
    config.port = port;
    config.allow_write = allow_write;
    if config.token.is_empty() {
        config.token = ide_mcp::http::new_token();
    }
    save(&state, &config)?;
    if !enabled {
        stop();
        *LAST_ERROR.lock() = None;
        return Ok(status(&state, &label));
    }
    let project = state.host.project(&label)?;
    let root = project.root.to_string_lossy().into_owned();
    state.host.require_trust(&label)?;
    // A running server keeps serving what it started with: only an unchanged
    // port + project + write mode can stay.
    let unchanged = SERVER
        .lock()
        .as_ref()
        .is_some_and(|s| s.port == port && s.project == root && s.allow_write == allow_write);
    if !unchanged {
        stop();
        match ide_mcp::http::start(state.host.clone(), &label, port, config.token.clone(), allow_write).await {
            Ok(handle) => {
                *LAST_ERROR.lock() = None;
                *SERVER.lock() = Some(Running { handle, project: root, port, allow_write });
            }
            Err(message) => {
                *LAST_ERROR.lock() = Some(message);
            }
        }
    }
    Ok(status(&state, &label))
}
