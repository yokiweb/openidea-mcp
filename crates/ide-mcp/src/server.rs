//! The MCP service: the seven tools of the crate docs, served with `rmcp`.

use crate::tools::{db, services};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameParams {
    /// The run configuration's id or name, as `list_services` reports it.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogsParams {
    /// The run configuration's id or name.
    pub name: String,
    /// How many lines to return, counting back from the last output
    /// (default 100).
    #[serde(default)]
    pub tail: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryParams {
    /// The data source's id or name, as `list_datasources` reports it.
    pub datasource: String,
    /// One or more SQL statements; without `--allow-write` they must all be
    /// read-only (SELECT / EXPLAIN / SHOW / DESCRIBE / WITH / PRAGMA).
    pub sql: String,
    /// Maximum rows per result set, 1-1000 (default 200); extra rows are
    /// marked in `hasMore` instead of being returned.
    #[serde(default)]
    pub max_rows: Option<u32>,
}

/// The MCP service over one project of the IDE host.
#[derive(Clone)]
pub struct OpenIdeaMcp {
    host: Arc<ide_host::Host>,
    /// The window label whose project the tools serve: `"mcp"` in the stdio
    /// mode (see `crate::WINDOW`), the GUI window's label in the HTTP mode.
    window: Arc<str>,
    allow_write: bool,
    runs: Arc<services::RunRecord>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl OpenIdeaMcp {
    pub fn new(host: Arc<ide_host::Host>, allow_write: bool, window: impl Into<Arc<str>>) -> Self {
        Self { host, window: window.into(), allow_write, runs: Arc::default(), tool_router: Self::tool_router() }
    }

    #[tool(description = "List the project's run configurations (services): id, name, kind, \
         whether each is running, and the run ids behind them.")]
    async fn list_services(&self) -> Result<CallToolResult, McpError> {
        Self::reply(services::list_services(&self.host, &self.window).await)
    }

    #[tool(description = "Start the run configuration `name` (a service). Fails when it is \
         already running (unless it allows parallel instances). The process is a child of \
         this MCP server; list_services reports its state.")]
    async fn start_service(&self, Parameters(params): Parameters<NameParams>) -> Result<CallToolResult, McpError> {
        Self::reply(services::start_service(&self.host, &self.window, &self.runs, &params.name).await)
    }

    #[tool(description = "Stop the running service `name` (a run configuration's id or name, \
         or a runId number from list_services / start_service).")]
    async fn stop_service(&self, Parameters(params): Parameters<NameParams>) -> Result<CallToolResult, McpError> {
        Self::reply(services::stop_service(&self.host, &self.window, &params.name).await)
    }

    #[tool(description = "Restart the run configuration `name`: stop its running instances, \
         then start it again.")]
    async fn restart_service(&self, Parameters(params): Parameters<NameParams>) -> Result<CallToolResult, McpError> {
        Self::reply(services::restart_service(&self.host, &self.window, &self.runs, &params.name).await)
    }

    #[tool(description = "The console output of the service `name`'s latest run, counting \
         back `tail` lines (default 100). Only covers runs started by these tools.")]
    async fn get_service_logs(&self, Parameters(params): Parameters<LogsParams>) -> Result<CallToolResult, McpError> {
        Self::reply(services::get_service_logs(
                &self.host,
                &self.window,
                &self.runs,
                &params.name,
                params.tail.unwrap_or(100),
            ).await)
    }

    #[tool(description = "List the project's data sources: id, name, driver, host, port, \
         database, whether they are read-only, and whether they are connected.")]
    async fn list_datasources(&self) -> Result<CallToolResult, McpError> {
        Self::reply(db::list_datasources(&self.host, &self.window).await)
    }

    #[tool(description = "Run read-only SQL (SELECT / EXPLAIN / SHOW / DESCRIBE / WITH / \
         PRAGMA) on data source `datasource`. Rows are capped at `max_rows` (1-1000). \
         Other statements need the server started with --allow-write.")]
    async fn execute_query(&self, Parameters(params): Parameters<QueryParams>) -> Result<CallToolResult, McpError> {
        Self::reply(
            db::execute_query(&self.host, &self.window, self.allow_write, &params.datasource, &params.sql, params.max_rows).await,
        )
    }

    /// A tool result: JSON text on success, a failed tool call on error.
    fn reply(result: Result<serde_json::Value, String>) -> Result<CallToolResult, McpError> {
        match result {
            Ok(value) => match serde_json::to_string_pretty(&value) {
                Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            },
            Err(message) => Err(McpError::internal_error(message, None)),
        }
    }
}

#[tool_handler]
impl ServerHandler for OpenIdeaMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "These tools manage the run configurations and data sources of the OpenIDEA \
             project given at startup. Call list_services / list_datasources first; names \
             from their answers feed the other tools. Queries are read-only unless the \
             server was started with --allow-write.",
        )
    }
}
