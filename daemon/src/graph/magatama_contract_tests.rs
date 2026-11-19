//! MAGATAMA（別リポジトリ）は `.comp/index.db` を SQL で直接読む。そのとき使う列が
//! comP のスキーマから消えたり名前が変わったりすると、MAGATAMA は何も言わずに壊れる。
//! MAGATAMA 側のテストは手書きの CREATE TABLE を使うので、この食い違いを検知できない。
//! そのため、読まれている列を comP 側で固定する。
//! Owned by the designer role; implementers must not edit.
//!
//! 列の出典: MAGATAMA packages/magatama-core/src/magatama_core/infrastructure/storage/comp_index_reader.py
//! の SELECT 文（2026-11-20 時点）。MAGATAMA が読む列を増やしたら、ここにも足す。
use super::*;
use std::collections::HashSet;

const COLUMNS_READ_BY_MAGATAMA: &[(&str, &[&str])] = &[
    ("files", &["id", "path", "hash", "language"]),
    (
        "nodes",
        &["id", "file_id", "name", "kind", "line", "col", "scope", "is_exported", "signature"],
    ),
    ("edges", &["from_id", "to_id", "kind"]),
    ("metadata", &["key", "value"]),
];

fn columns_of(conn: &rusqlite::Connection, table: &str) -> HashSet<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("table_info");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("query table_info")
        .map(|c| c.expect("column name"))
        .collect()
}

#[test]
fn schema_keeps_every_column_magatama_reads() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    Schema::apply_all(&conn).unwrap();

    let mut missing = Vec::new();
    for (table, cols) in COLUMNS_READ_BY_MAGATAMA {
        let actual = columns_of(&conn, table);
        for col in *cols {
            if !actual.contains(*col) {
                missing.push(format!("{table}.{col}"));
            }
        }
    }
    assert_eq!(missing, Vec::<String>::new(), "MAGATAMA が読む列がスキーマにない");
}

#[test]
fn contract_check_detects_a_missing_column() {
    // 検査そのものが空振りしていないことを確かめる（列のない表で必ず検出されるか）
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE files (id INTEGER, path TEXT);").unwrap();
    let actual = columns_of(&conn, "files");
    assert!(actual.contains("path"));
    assert!(!actual.contains("hash"));
}
