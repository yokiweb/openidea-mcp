//! The IDE's MCP server: AI agents (any MCP client) manage the project's run
//! configurations ("services") and query its data sources over stdio.
//!
//! - Launch: `<openidea> --mcp [--project <path>] [--allow-write]`, handled
//!   before the UI starts; this process has no windows. `--project` defaults
//!   to the current directory.
//! - It shares the desktop app's config / data directories, so project trust
//!   and saved data source passwords apply exactly as configured in the UI.
//! - Tools: services (`list_services`, `start_service`, `stop_service`,
//!   `restart_service`, `get_service_logs`) and database
//!   (`list_datasources`, `execute_query`). Queries are read-only unless the
//!   server was started with `--allow-write`.
//!
//! Limitations: runs started by the tools are children of this process, so
//! the GUI's Services tool window does not show them, and `get_service_logs`
//! only covers runs these tools started (the IDE streams console output to
//! the UI without keeping it).

pub mod guard;
pub mod http;
mod server;
mod tools;

use ide_host::{Host, HostConfig};
use ide_language::LanguageRegistry;
use rmcp::ServiceExt;
use std::path::PathBuf;
use std::sync::Arc;

/// The one project this process serves (its window label too).
const WINDOW: &str = "mcp";

/// The desktop app's identifier; its config / data directories are shared.
const IDENTIFIER: &str = "ai.l0ng.openidea";

/// The MCP mode's CLI. [`None`] when `--mcp` is absent, so the caller starts
/// the UI as usual; otherwise returns the process exit code.
pub fn try_main() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--mcp") {
        return None;
    }
    let mut project: Option<PathBuf> = None;
    let mut allow_write = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--mcp" => {}
            "--allow-write" => allow_write = true,
            "--project" => match rest.next() {
                Some(path) => project = Some(PathBuf::from(path)),
                None => {
                    eprintln!("openidea --mcp: --project needs a path");
                    return Some(2);
                }
            },
            other => {
                eprintln!("openidea --mcp: unknown argument {other}");
                eprintln!("usage: openidea --mcp [--project <path>] [--allow-write]");
                return Some(2);
            }
        }
    }
    let project = project.map_or_else(std::env::current_dir, |p| {
        if p.is_absolute() { Ok(p) } else { std::env::current_dir().map(|cwd| cwd.join(p)) }
    });
    let project = match project {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openidea --mcp: cannot resolve the project path: {e}");
            return Some(1);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build();
    let runtime = match runtime {
        Ok(r) => r,
        Err(e) => {
            eprintln!("openidea --mcp: {e}");
            return Some(1);
        }
    };
    let code = runtime.block_on(serve(project, allow_write));
    // Nothing started for the client may outlive it.
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));
    Some(code)
}

async fn serve(project: PathBuf, allow_write: bool) -> i32 {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("OPENIDEA_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let (config_dir, data_dir, local_data_dir) = match app_dirs(IDENTIFIER) {
        Some(dirs) => dirs,
        None => {
            eprintln!("openidea --mcp: cannot locate the app data directories");
            return 1;
        }
    };
    for dir in [&config_dir, &data_dir, &local_data_dir] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("openidea --mcp: cannot create {}: {e}", dir.display());
            return 1;
        }
    }
    // An MCP client's environment is minimal: take the login shell's, plus the
    // IDE-managed tools, so services find `java`, `mvn`, `node`…
    ide_host::shell_env::inherit_login_shell_env("openidea-mcp");
    ide_tools::extend_process_path(&local_data_dir);
    ide_sdk::java::add_jdk_root(&ide_tools::jdk::jdks_dir(&local_data_dir));
    let languages = match LanguageRegistry::load(Some(&config_dir.join("languages"))) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            eprintln!("openidea --mcp: language definitions: {e}");
            return 1;
        }
    };
    let host = Arc::new(Host::new(HostConfig {
        config_dir,
        data_dir,
        local_data_dir,
        languages,
        syntax: Arc::default(),
        sink_for: Box::new(|_| Arc::new(ide_core::NullSink)),
        owner: |w| w,
        runtime: tokio::runtime::Handle::current(),
        project_environment: false,
    }));
    if let Err(e) = ide_host::ops::project::open(&host, WINDOW, &project).await {
        eprintln!("openidea --mcp: cannot open {}: {e}", project.display());
        return 1;
    }
    if let Err(e) = host.require_trust(WINDOW) {
        eprintln!("openidea --mcp: {} is not trusted; open it in OpenIDEA once and trust it ({e})", project.display());
        return 1;
    }
    let server = server::OpenIdeaMcp::new(host, allow_write, WINDOW);
    match server.serve(rmcp::transport::stdio()).await {
        Ok(running) => {
            let _ = running.waiting().await;
            0
        }
        Err(e) => {
            eprintln!("openidea --mcp: {e}");
            1
        }
    }
}

/// The desktop app's config / data / local-data directories (`Tauri`'s
/// `app_config_dir` / `app_data_dir` / `app_local_data_dir`).
fn app_dirs(identifier: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    #[cfg(windows)]
    {
        Some((
            dirs::config_dir()?.join(identifier),
            dirs::data_dir()?.join(identifier),
            dirs::data_local_dir()?.join(identifier),
        ))
    }
    #[cfg(target_os = "macos")]
    {
        let base = dirs::data_dir()?.join(identifier);
        Some((base.clone(), base.clone(), base))
    }
    #[cfg(target_os = "linux")]
    {
        Some((
            dirs::config_dir()?.join(identifier),
            dirs::data_dir()?.join(identifier),
            dirs::data_dir()?.join(identifier),
        ))
    }
}
