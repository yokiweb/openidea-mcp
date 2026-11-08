//! Read-only statement gate for `execute_query`.

use ide_db::split::SqlStatement;

/// Statements that only read: queries, plans, listings, and the (usually
/// read-only) pragmas of SQLite / Postgres / MySQL.
const READ_ONLY: [&str; 7] = ["select", "with", "explain", "show", "describe", "desc", "pragma"];

/// Rejects statements that are not read-only unless `allow_write`. The check
/// is a coarse first line: the driver and the data source's `read_only` flag
/// remain the real enforcement.
pub fn check(statements: &[SqlStatement], allow_write: bool) -> Result<(), String> {
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
                 The server must be started with --allow-write to run other statements."
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
