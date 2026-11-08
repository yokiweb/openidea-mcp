//! Services tools: the project's run configurations and their processes.

use ide_core::RunId;
use ide_host::Host;
use ide_host::ops::Emit;
use ide_run::{OutputChunk, RunConfig, RunInfo};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// The runs this server started, for `get_service_logs` (see the crate docs).
pub type RunRecord = Mutex<Vec<RunInfo>>;

fn err(e: ide_core::IdeError) -> String {
    e.to_string()
}

fn find_config(host: &Host, window: &str, name: &str) -> Result<RunConfig, String> {
    let configs = ide_host::ops::run::configs(host, window).map_err(err)?;
    if let Some(config) = configs.iter().find(|c| c.id == name) {
        return Ok(config.clone());
    }
    let by_name: Vec<&RunConfig> = configs.iter().filter(|c| c.name.eq_ignore_ascii_case(name)).collect();
    match by_name.len() {
        1 => Ok(by_name[0].clone()),
        0 => Err(format!("no run configuration named {name:?}; call list_services for the available ones")),
        _ => {
            let ids = by_name.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(", ");
            Err(format!("several configurations are named {name:?}; use their id instead: {ids}"))
        }
    }
}

fn runs_of(host: &Host, config_id: &str) -> Vec<RunInfo> {
    host.processes.list().into_iter().filter(|r| r.config_id == config_id).collect()
}

fn no_op_output() -> Emit<OutputChunk> {
    Arc::new(|_| {})
}

/// JVM application kinds (`application` / `spring-boot`): their command line keeps the
/// `${classpath}` / `${testClasspath}` placeholders and a bare `java` launcher; the IDE
/// frontend resolves them through jdtls before it calls run start (see
/// `src/platform/javaLaunch.ts`). `start_service` has no frontend in front of it, so it
/// resolves them here.
fn is_jvm_application(kind: &str) -> bool {
    matches!(kind, "application" | "spring-boot")
}

/// The configuration with its classpath placeholder and `java` launcher resolved
/// through jdtls (a mirror of the frontend's `resolveJavaCommand`).
async fn resolve_java_command(host: &Host, window: &str, config: &RunConfig) -> Result<RunConfig, String> {
    let module = config
        .extras
        .module
        .as_deref()
        .filter(|m| !m.is_empty())
        .ok_or_else(|| format!("\"{}\" has no module; the classpath cannot be resolved", config.name))?;
    let test = config.args.iter().any(|a| a == "${testClasspath}");
    let request = ide_core::RequestId::next();
    let info =
        ide_host::ops::java::java_resolve_launch(host, window, std::path::PathBuf::from(module), test, true, request)
            .await
            .map_err(err)?;
    if info.class_paths.is_empty() {
        return Err(format!(
            "the classpath of module {module:?} is empty; is it imported by the Java language server?"
        ));
    }
    let sep = info.path_list_separator.as_str();
    let cp = info.class_paths.join(sep);
    let mut args: Vec<String> = Vec::new();
    // The console renders ANSI colours, but Spring only detects a terminal; the
    // frontend's Spring Boot launches add this too (javaLaunch.ts `consoleVmArgs`).
    if config.kind == "spring-boot"
        && !config.args.iter().any(|a| a.starts_with("-Dspring.output.ansi.enabled="))
    {
        args.push("-Dspring.output.ansi.enabled=always".into());
    }
    if !info.module_paths.is_empty() && !config.args.iter().any(|a| a == "-p" || a == "--module-path") {
        args.push("--module-path".into());
        args.push(info.module_paths.join(sep));
    }
    args.extend(
        config
            .args
            .iter()
            .map(|a| if a == "${classpath}" || a == "${testClasspath}" { cp.clone() } else { a.clone() }),
    );
    let command = match config.command.as_str() {
        "java" if info.java_exec.is_some() => info.java_exec.expect("checked above"),
        _ => config.command.clone(),
    };
    Ok(RunConfig { command, args, ..config.clone() })
}

pub async fn list_services(host: &Host, window: &str) -> Result<serde_json::Value, String> {
    let configs = ide_host::ops::run::configs(host, window).map_err(err)?;
    let runs = host.processes.list();
    let services = configs
        .iter()
        .map(|c| {
            let mine: Vec<&RunInfo> = runs.iter().filter(|r| r.config_id == c.id).collect();
            json!({
                "id": c.id,
                "name": c.name,
                "kind": c.kind,
                "detected": c.detected,
                "isTest": c.is_test,
                "running": mine.iter().any(|r| r.running),
                "runs": mine.iter().map(|r| json!({
                    "runId": r.id.0,
                    "running": r.running,
                    "pid": r.pid,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let project = host.project(window).map(|p| p.root.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(json!({ "project": project, "services": services }))
}

pub async fn start_service(host: &Host, window: &str, record: &RunRecord, name: &str) -> Result<serde_json::Value, String> {
    let config = find_config(host, window, name)?;
    let running = runs_of(host, &config.id).into_iter().find(|r| r.running);
    if let Some(run) = running
        && !config.allow_parallel
    {
        return Err(format!(
            "\"{}\" is already running (run {}); stop it first or use restart_service",
            config.name, run.id
        ));
    }
    let config = if is_jvm_application(&config.kind) { resolve_java_command(host, window, &config).await? } else { config };
    let info = ide_host::ops::run::start(host, window, config.clone(), no_op_output()).await.map_err(err)?;
    record.lock().expect("run record mutex").push(info.clone());
    Ok(json!({
        "started": true,
        "runId": info.id.0,
        "name": config.name,
        "pid": info.pid,
        "running": info.running,
        "note": "the process is a child of this MCP server; the IDE's Services window does not list it",
    }))
}

pub async fn stop_service(host: &Host, window: &str, name: &str) -> Result<serde_json::Value, String> {
    let id = resolve_run(host, window, name)?;
    ide_host::ops::run::stop(host, window, id).map_err(err)?;
    Ok(json!({ "stopped": true, "runId": id.0 }))
}

pub async fn restart_service(host: &Host, window: &str, record: &RunRecord, name: &str) -> Result<serde_json::Value, String> {
    let config = find_config(host, window, name)?;
    for run in runs_of(host, &config.id).into_iter().filter(|r| r.running) {
        ide_host::ops::run::stop(host, window, run.id).map_err(err)?;
    }
    start_service(host, window, record, &config.name).await
}

pub async fn get_service_logs(
    host: &Host,
    window: &str,
    record: &RunRecord,
    name: &str,
    tail: u32,
) -> Result<serde_json::Value, String> {
    let config = find_config(host, window, name)?;
    let mut candidates: Vec<RunId> =
        record.lock().expect("run record mutex").iter().filter(|r| r.config_id == config.id).map(|r| r.id).collect();
    candidates.extend(runs_of(host, &config.id).into_iter().map(|r| r.id));
    if let Some(id) = candidates.into_iter().max() {
        return memory_logs(id, tail);
    }
    shared_logs(host, &config.id, tail)
}

/// Logs of runs this process started: complete, kept in memory.
fn memory_logs(id: RunId, tail: u32) -> Result<serde_json::Value, String> {
    let chunks = ide_host::ops::run::output_tail(id, 10_000);
    let mut text = String::new();
    for chunk in chunks {
        match chunk {
            OutputChunk::Stdout { text: t } | OutputChunk::Stderr { text: t } | OutputChunk::System { text: t } => {
                text.push_str(&t)
            }
        }
    }
    let text = strip_ansi(&text);
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(tail.max(1) as usize);
    Ok(json!({
        "source": "memory",
        "runId": id.0,
        "totalLines": lines.len(),
        "text": lines[skip..].join("\n"),
    }))
}

/// Logs of runs other processes started (the GUI's runs, Spring Boot included):
/// from the shared run-logs directory (see `ide_host::ops::run::run_logs_dir`).
fn shared_logs(host: &Host, config_id: &str, tail: u32) -> Result<serde_json::Value, String> {
    let dir = ide_host::ops::run::run_logs_dir(host);
    let manifest = dir.join(ide_host::ops::run::RUN_LOG_MANIFEST);
    let index = std::fs::read_to_string(&manifest).map_err(|_| {
        "no shared run logs; start the service with start_service, or run it from the IDE on this machine once"
            .to_string()
    })?;
    let run_id = index
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry.get("configId").and_then(|c| c.as_str()) == Some(config_id))
        .filter_map(|entry| entry.get("runId").and_then(|r| r.as_u64()))
        .max()
        .ok_or_else(|| format!("no logged runs of {config_id:?} yet"))?;
    let path = dir.join(format!("{run_id}.log"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read run log {run_id}: {e}"))?;
    let text = strip_ansi(&text);
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(tail.max(1) as usize);
    Ok(json!({
        "source": "shared-file",
        "runId": run_id,
        "totalLines": lines.len(),
        "text": lines[skip..].join("\n"),
    }))
}

/// `name` is a run id (a number), or a run configuration's id / name whose
/// currently running run is meant.
fn resolve_run(host: &Host, window: &str, name: &str) -> Result<RunId, String> {
    if let Ok(n) = name.parse::<u32>() {
        return Ok(RunId(n));
    }
    let config = find_config(host, window, name)?;
    runs_of(host, &config.id)
        .into_iter()
        .find(|r| r.running)
        .map(|r| r.id)
        .ok_or_else(|| format!("\"{}\" is not running", config.name))
}

/// Drops ANSI escape sequences (colours, cursor moves): console output of
/// builds and dev servers is for people, agents read plain text.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            // CSI: `ESC [ params letter`; OSC: `ESC ] … BEL or ESC \`.
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
            } else if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                i += 2;
                while i < bytes.len() && bytes[i] != 0x07 {
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
            } else {
                i += 1;
            }
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != 0x1b {
                i += 1;
            }
            out.push_str(&text[start..i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn colours_and_cursor_moves_are_dropped() {
        assert_eq!(strip_ansi("\x1b[32m√ Compiled\x1b[0m in \x1b[1;33m2s\x1b[0m"), "√ Compiled in 2s");
        assert_eq!(strip_ansi("\x1b[2J\x1b[Hclean"), "clean");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("spring \x1b[38;5;39mboot\x1b[m started"), "spring boot started");
    }

    #[test]
    fn osc_sequences_are_dropped() {
        assert_eq!(strip_ansi("\x1b]0;title\x07text"), "text");
        assert_eq!(strip_ansi("\x1b]8;;url\x1b\\link"), "link");
    }
}
