//! `tools/list` で返すツール定義（名前・説明・引数スキーマ）が変わっていないことを固定する。
//! Owned by the designer role; implementers must not edit.
//!
//! 文字列ではなく JSON の値で比べる。Windows の autocrlf で改行が変わっても誤検知しないため。
//! 意図してツール定義を変えたときだけ `COMP_UPDATE_SNAPSHOTS=1 cargo test tool_schema` で書き直す。
use super::*;

const SNAPSHOT_PATH: &str = "src/mcp/testdata/tool_definitions.json";

// cargo test の作業フォルダはクレートの直下（daemon/）なので、相対パスで読める。
fn load_snapshot() -> Value {
    let text = std::fs::read_to_string(SNAPSHOT_PATH).expect("snapshot file exists");
    serde_json::from_str(&text).expect("snapshot is valid JSON")
}

#[test]
fn tool_definitions_match_snapshot() {
    let actual = MCPServer::tool_definitions();

    if std::env::var("COMP_UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        let pretty = serde_json::to_string_pretty(&actual).unwrap() + "\n";
        std::fs::write(SNAPSHOT_PATH, pretty).unwrap();
        return;
    }

    let expected: Value = load_snapshot();
    assert_eq!(actual, expected);
}

#[test]
fn tool_definitions_snapshot_lists_every_tool() {
    let expected: Value = load_snapshot();
    let names: Vec<&str> = expected["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"run_pipeline"));
    assert!(names.contains(&"session_recall"));
    assert!(names.contains(&"get_impact_graph"));
}
