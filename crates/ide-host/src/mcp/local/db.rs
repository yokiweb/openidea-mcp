//! 数据库工具：项目的数据源与只读查询（上游 21 个工具里没有任何 DB 能力）。

use super::super::tool::{Output, Param, ParamType, Tool};
use super::allow_write;
use crate::Host;
use ide_core::{IdeError, RequestId};
use ide_db::split::SqlStatement;
use serde_json::json;

fn err(e: ide_core::IdeError) -> String {
    e.to_string()
}

fn find_source(host: &Host, window: &str, name: &str) -> Result<ide_db::DataSource, String> {
    let sources = crate::ops::db::sources(host, window).map_err(err)?;
    if let Some(source) = sources.iter().find(|s| s.id == name) {
        return Ok(source.clone());
    }
    let by_name: Vec<&ide_db::DataSource> = sources.iter().filter(|s| s.name.eq_ignore_ascii_case(name)).collect();
    match by_name.len() {
        1 => Ok(by_name[0].clone()),
        0 => Err(format!("no data source named {name:?}; call list_datasources for the available ones")),
        _ => {
            let ids = by_name.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ");
            Err(format!("several data sources are named {name:?}; use their id instead: {ids}"))
        }
    }
}

pub async fn list_datasources(host: &Host, window: &str) -> Result<serde_json::Value, String> {
    let sources = crate::ops::db::sources(host, window).map_err(err)?;
    let connected = host.project(window).map(|p| p.db.connected()).unwrap_or_default();
    let data_sources = sources
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.name,
                "driver": s.driver,
                "host": s.host,
                "port": s.port,
                "database": s.database,
                "readOnly": s.read_only,
                "connected": connected.iter().any(|c| c == &s.id),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "dataSources": data_sources }))
}

/// Splits `sql` (it may hold several statements), checks the read-only gate,
/// runs the statements in order, and stops at the first error.
pub async fn execute_query(
    host: &Host,
    window: &str,
    datasource: &str,
    sql: &str,
    max_rows: Option<u32>,
) -> Result<serde_json::Value, String> {
    let source = find_source(host, window, datasource)?;
    let utf16_len = sql.encode_utf16().count() as u32;
    let statements = ide_db::split::statements_in(sql, 0, utf16_len, source.driver);
    if statements.is_empty() {
        return Err("the input contains no SQL statement".into());
    }
    check(&statements, allow_write())?;
    let max_rows = max_rows.unwrap_or(200).clamp(1, 1000);
    // 上游 execute 新增 schema 上下文；MCP 没有 console 的活动 schema，传 None 表示不切换。
    let outcomes = crate::ops::db::execute(host, window, RequestId::next(), &source.id, statements, 0, max_rows, None)
        .await
        .map_err(err)?;
    let mut results = Vec::new();
    for outcome in outcomes {
        if let Some(error) = outcome.error {
            let sample: String = outcome.statement.text.lines().next().unwrap_or_default().chars().take(120).collect();
            return Err(format!("the statement failed ({sample}): {error}"));
        }
        if let Some(result) = outcome.result {
            results.push(json!({
                "statement": result.statement,
                "columns": result.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
                "rows": result.rows,
                "rowCount": result.rows.len(),
                "affected": result.affected,
                "hasMore": result.has_more,
                "elapsedMs": result.elapsed_ms,
            }));
        }
    }
    Ok(json!({
        "dataSource": source.id,
        "readOnly": source.read_only,
        "maxRows": max_rows,
        "results": results,
    }))
}

/// Statements that only read: queries, plans, listings, and the (usually
/// read-only) pragmas of SQLite / Postgres / MySQL.
const READ_ONLY: [&str; 7] = ["select", "with", "explain", "show", "describe", "desc", "pragma"];

/// Rejects statements that are not read-only unless the settings page turned on
/// `allowWrite`. The check is a coarse first line: the driver and the data
/// source's `read_only` flag remain the real enforcement.
fn check(statements: &[SqlStatement], allow_write: bool) -> Result<(), String> {
    if allow_write {
        return Ok(());
    }
    for statement in statements {
        let keyword = leading_keyword(&statement.text);
        if !READ_ONLY.contains(&keyword.as_str()) {
            let sample = statement.text.lines().next().unwrap_or_default();
            let sample: String = sample.chars().take(120).collect();
            return Err(format!(
                "the statement is not read-only (it starts with {keyword:?}): {sample}\n\
                 Only SELECT / EXPLAIN / SHOW / DESCRIBE / WITH / PRAGMA are allowed.\n\
                 Turn on \"Allow write statements\" in Settings › Tools › MCP Server to run other statements."
            ));
        }
    }
    Ok(())
}

/// The first word of `sql`, past whitespace and comments, lowercased; empty
/// when there is none.
fn leading_keyword(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            break;
        }
    }
    sql[i..].chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase()
}

pub fn list_datasources_tool() -> Tool {
    Tool::new(
        "list_datasources",
        "List the project's configured data sources: id, name, driver, host, port, database, whether it is \
read-only, and whether it is connected right now. Call this before execute_query to get the exact names.",
        |call| async move {
            let host = call.host();
            let value = list_datasources(host.as_ref(), call.window()).await.map_err(IdeError::invalid)?;
            Output::json(value)
        },
    )
}

pub fn execute_query_tool() -> Tool {
    Tool::new(
        "execute_query",
        "Run SQL against one of the project's data sources and return the columns and rows. Several statements \
are allowed (separated by `;`) and run in order; the call stops at the first error. Read-only statements \
(SELECT / EXPLAIN / SHOW / DESCRIBE / WITH / PRAGMA) are always allowed; anything else needs \"Allow write \
statements\" turned on in Settings › Tools › MCP Server. Rows are capped by `maxRows`.",
        |call| async move {
            let host = call.host();
            let datasource = call.str("datasource")?;
            let sql = call.str("sql")?;
            let max_rows = call.opt_i64("maxRows")?.map(|n| n.clamp(1, 1000) as u32);
            let value = execute_query(host.as_ref(), call.window(), &datasource, &sql, max_rows)
                .await
                .map_err(IdeError::invalid)?;
            Output::json(value)
        },
    )
    .write()
    .param(Param::required("datasource", ParamType::String, "The data source's id or name"))
    .param(Param::required("sql", ParamType::String, "The SQL to run"))
    .param(Param::optional(
        "maxRows",
        ParamType::Integer,
        "Maximum rows returned per statement (default 200, at most 1000)",
    ))
}

#[cfg(test)]
mod tests {
    use super::check;
    use ide_db::split::statements_in;

    fn denied(sql: &str, allow_write: bool) -> bool {
        let statements = statements_in(sql, 0, sql.encode_utf16().count() as u32, ide_db::DbDriver::Postgres);
        check(&statements, allow_write).is_err()
    }

    #[test]
    fn read_only_statements_pass() {
        assert!(!denied("SELECT 1", false));
        assert!(!denied("-- look up users\nWITH t AS (SELECT 1) SELECT * FROM t", false));
        assert!(!denied("/* note */   explain SELECT 1", false));
        assert!(!denied("show tables;", false));
        assert!(!denied("PRAGMA foreign_keys;", false));
    }

    #[test]
    fn writes_are_denied_without_allow_write() {
        assert!(denied("DELETE FROM t", false));
        assert!(denied("UPDATE t SET x = 1", false));
        assert!(denied("-- clean up\nDROP TABLE t", false));
        assert!(denied("INSERT INTO t VALUES (1)", false));
        assert!(denied("ALTER TABLE t ADD COLUMN c INT", false));
    }

    #[test]
    fn mixed_statements_are_denied() {
        assert!(denied("SELECT 1; DELETE FROM t", false));
    }

    #[test]
    fn allow_write_admits_everything() {
        assert!(!denied("DELETE FROM t", true));
        assert!(!denied("CREATE TABLE t (id INT)", true));
    }

    #[test]
    fn comments_are_skipped() {
        assert!(!denied("-- update the docs, really\nSELECT 1", false));
        assert!(denied("/* SELECT */ DELETE FROM t", false));
    }
}
