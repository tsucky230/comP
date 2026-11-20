//! index.db の形式の版（metadata.schema_version）。MAGATAMA が読み込み時に、自分が対応している形式かを
//! 確かめるために使う。既存の metadata.version（移行の番号、INSERT OR IGNORE で更新されない）とは別のキー。
//! Owned by the designer role; implementers must not edit.
use super::*;

fn schema_version(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

#[test]
fn fresh_database_records_current_schema_version() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    Schema::apply_all(&conn).unwrap();
    assert_eq!(schema_version(&conn).as_deref(), Some("1"));
    assert_eq!(COMP_INDEX_SCHEMA_VERSION, "1");
}

#[test]
fn existing_database_gets_schema_version_overwritten_with_current() {
    // 古い形式の版が残っている DB を開き直しても、今の版に置き換わる（INSERT OR IGNORE だと古いまま残る）
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    Schema::apply_all(&conn).unwrap();
    conn.execute("UPDATE metadata SET value = '0' WHERE key = 'schema_version'", [])
        .unwrap();
    Schema::apply_all(&conn).unwrap();
    assert_eq!(schema_version(&conn).as_deref(), Some("1"));
}

#[test]
fn legacy_version_key_is_left_untouched() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    Schema::apply_all(&conn).unwrap();
    let legacy: String = conn
        .query_row("SELECT value FROM metadata WHERE key = 'version'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(legacy, "2");
}
