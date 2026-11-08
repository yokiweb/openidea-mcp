//! Database tools: the project's data sources and read-only queries.

use crate::guard;
use ide_core::RequestId;
use ide_host::Host;
use serde_json::json;

fn err(e: ide_core::IdeError) -> String {
    e.to_string()
}

fn find_source(host: &Host, window: &str, name: &str) -> Result<ide_db::DataSource, String> {
    let sources = ide_host::ops::db::sources(host, window).map_err(err)?;
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
    let sources = ide_host::ops::db::sources(host, window).map_err(err)?;
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
    allow_write: bool,
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
    guard::check(&statements, allow_write)?;
    let max_rows = max_rows.unwrap_or(200).clamp(1, 1000);
    let outcomes =
        ide_host::ops::db::execute(host, window, RequestId::next(), &source.id, statements, 0, max_rows)
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
