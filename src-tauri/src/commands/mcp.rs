//! Tools › MCP Server: runs `ide_mcp::McpServer` over the built-in host tools
//! while it is turned on in Settings, and reports where it listens.
//!
//! 与上游的差异（用户 2026-11-10 确认）：开关、端口、token 全部持久化在
//! `<config_dir>/mcp.json`，server 用固定端口 + 稳定 token 启动。
//! 上游每次启动随机端口 + 新 token，MCP 客户端的配置重启即失效，必须重新复制。

use crate::state::AppState;
use ide_core::{IdeError, IdeResult, IpcPath};
use ide_host::mcp::{FileProblems, FocusedEditor};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{State, Window};

/// 首次使用的端口；之后可在设置页改。
const DEFAULT_PORT: u16 = 41521;

/// `<config_dir>/mcp.json`：本页唯一的真源（不再读全局设置里的 mcpServer.enabled，
/// 避免设置页与配置文件两处状态打架）。
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
        Self { enabled: false, port: DEFAULT_PORT, allow_write: false, token: ide_mcp::new_token().unwrap_or_default() }
    }
}

fn config_path(state: &AppState) -> PathBuf {
    state.host.config_dir.join("mcp.json")
}

/// 读配置。空 token / 0 端口视为损坏（手改或旧文件），补默认值并写回，
/// 否则客户端会拿到空凭据。
fn load(state: &AppState) -> Config {
    let path = config_path(state);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    let mut config: Config = serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!("mcp.json is malformed, using the defaults: {e}");
        Config::default()
    });
    let mut repaired = false;
    if config.token.is_empty() {
        config.token = ide_mcp::new_token().unwrap_or_default();
        repaired = true;
    }
    if config.port == 0 {
        config.port = DEFAULT_PORT;
        repaired = true;
    }
    if repaired {
        let _ = save(state, &config);
    }
    config
}

fn save(state: &AppState, config: &Config) -> IdeResult<()> {
    let path = config_path(state);
    let text = serde_json::to_string_pretty(config).map_err(|e| IdeError::internal(e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| IdeError::internal(format!("cannot write {}: {e}", path.display())))
}

/// 正在运行的 server 及其绑定参数（端口/写权限变了要重启）。
struct Running {
    server: ide_mcp::McpServer,
    port: u16,
    allow_write: bool,
}

/// The server while it runs, or why it doesn't.
#[derive(Default)]
pub struct McpService {
    running: Mutex<Option<Running>>,
    error: Mutex<Option<String>>,
}

fn stop_running(service: &McpService) {
    if let Some(running) = service.running.lock().take() {
        running.server.stop();
    }
}

/// 启动（或按新配置重启）server。
fn start(state: &AppState, config: &Config) {
    let mut running = state.mcp.running.lock();
    if let Some(current) = running.as_ref() {
        // 端口与写权限没变就不重来（重启会断掉客户端连接）。
        if current.port == config.port && current.allow_write == config.allow_write {
            return;
        }
    }
    if let Some(previous) = running.take() {
        previous.server.stop();
    }
    ide_host::mcp::local::set_allow_write(config.allow_write);
    let discovery = ide_mcp::discovery_path(&state.data_dir);
    match ide_mcp::McpServer::start_with(
        Arc::new(ide_host::mcp::Registry::new(state.host.clone())),
        Some(discovery),
        &state.host.runtime,
        Some(config.port),
        Some(config.token.clone()),
    ) {
        Ok(started) => {
            let exe = std::env::current_exe().unwrap_or_default();
            ide_agent::set_mcp_executable(Some(exe));
            *running = Some(Running { server: started, port: config.port, allow_write: config.allow_write });
            *state.mcp.error.lock() = None;
        }
        Err(e) => {
            tracing::warn!("MCP server: {e}");
            *state.mcp.error.lock() = Some(e.to_string());
        }
    }
}

/// Starts or stops the server to match `mcp.json`.
/// 调用点：lib.rs 启动、设置变更、命令层 apply。
pub fn apply(state: &AppState) {
    let config = load(state);
    if !config.enabled {
        stop_running(&state.mcp);
        ide_agent::set_mcp_executable(None);
        *state.mcp.error.lock() = None;
        return;
    }
    start(state, &config);
}

/// Stops the server on exit (its discovery file goes with it).
pub fn stop(state: &AppState) {
    ide_agent::set_mcp_executable(None);
    stop_running(&state.mcp);
}

/// What Settings › Tools › MCP Server shows.
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    /// 保存的开关（`mcp.json`）。
    pub enabled: bool,
    pub running: bool,
    /// Why it isn't running though turned on.
    pub error: Option<String>,
    pub port: u16,
    pub allow_write: bool,
    /// Bearer token MCP clients send; stable across restarts.
    pub token: String,
    /// The window's local project, which the URL and command below are for.
    pub project: Option<IpcPath>,
    /// `http://127.0.0.1:<port>/mcp?project=<root>`.
    pub url: Option<String>,
    /// The stdio bridge: this executable and its arguments.
    pub stdio_command: Vec<String>,
}

fn status(state: &AppState, window: &str) -> McpServerStatus {
    let config = load(state);
    let running = state.mcp.running.lock();
    let root: Option<PathBuf> = state.host.project(window).ok().map(|p| p.root.clone());
    let exe = std::env::current_exe().unwrap_or_default();
    McpServerStatus {
        enabled: config.enabled,
        running: running.is_some(),
        error: state.mcp.error.lock().clone(),
        port: running.as_ref().map_or(config.port, |r| r.port),
        allow_write: running.as_ref().map_or(config.allow_write, |r| r.allow_write),
        token: config.token.clone(),
        url: root.as_ref().map(|r| ide_mcp::endpoint_url(config.port, r)),
        stdio_command: root.as_ref().map(|r| ide_mcp::stdio_command(&exe, r)).unwrap_or_default(),
        project: root.map(IpcPath::from),
    }
}

/// 失败也返回 Ok(status)：设置页保留用户填的值，错误显示在状态行，
/// 而不是整页退回 fallback（会把「没有打开项目」误显示成「构建太旧」）。
fn soft_fail(state: &AppState, window: &str, message: String) -> IdeResult<McpServerStatus> {
    *state.mcp.error.lock() = Some(message);
    Ok(status(state, window))
}

#[tauri::command]
#[specta::specta]
pub fn mcp_server_status(window: Window, state: State<'_, AppState>) -> IdeResult<McpServerStatus> {
    // 设置页可能开在独立的 settings 窗口，映射回它所属的项目窗口。
    let label = AppState::owner_window(window.label()).to_owned();
    Ok(status(&state, &label))
}

/// Saves `enabled` / `port` / `allow_write` and applies them: starts the
/// server, restarts it when the address changed, or stops it.
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
    if let Err(e) = save(&state, &config) {
        return soft_fail(&state, &label, e.message);
    }
    if enabled {
        if let Err(e) = state.host.require_trust(&label) {
            return soft_fail(&state, &label, e.message);
        }
    }
    apply(&state);
    Ok(status(&state, &label))
}

#[tauri::command]
#[specta::specta]
pub fn mcp_set_open_editors(window: Window, state: State<'_, AppState>, paths: Vec<IpcPath>) -> IdeResult<()> {
    state.project(window.label())?;
    state.host.mcp_reports.set_open_editors(window.label(), paths.into_iter().map(PathBuf::from).collect());
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn mcp_set_focused_editor(
    window: Window,
    state: State<'_, AppState>,
    editor: Option<FocusedEditor>,
    focused: bool,
) -> IdeResult<()> {
    state.project(window.label())?;
    state.host.mcp_reports.set_focused_editor(window.label(), editor, focused);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn mcp_report_problems(
    window: Window,
    state: State<'_, AppState>,
    reset: bool,
    files: Vec<FileProblems>,
) -> IdeResult<()> {
    state.project(window.label())?;
    state.host.mcp_reports.report_problems(window.label(), reset, files);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn mcp_ui_ready(window: Window, state: State<'_, AppState>, ready: bool) -> IdeResult<()> {
    state.project(window.label())?;
    state.host.mcp_requests.set_ready(window.label(), ready);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn mcp_respond(
    window: Window,
    state: State<'_, AppState>,
    id: u32,
    allow: bool,
    error: Option<String>,
) -> IdeResult<()> {
    state.host.mcp_requests.answer(window.label(), id, allow, error)
}
