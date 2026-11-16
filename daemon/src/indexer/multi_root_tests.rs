//! `index_all_roots`: one index over the main root plus additional roots.

use super::roots::normalize_abs;
use super::{index_all_roots, IndexSummary};
use crate::graph::GraphDB;
use anyhow::Result;
use std::path::Path;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Write `<main>/.comp/config.json` with the given roots under `key`.
fn set_roots(main: &Path, key: &str, extras: &[&Path]) {
    let list: Vec<String> = extras.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let mut config = serde_json::Map::new();
    config.insert(key.to_string(), serde_json::json!(list));
    write(main, ".comp/config.json", &serde_json::Value::Object(config).to_string());
}

/// Stored path of `rel` inside an additional root.
fn abs(root: &Path, rel: &str) -> String {
    format!("{}/{}", normalize_abs(&root.to_string_lossy()).unwrap(), rel)
}

/// Every stored path in the index, sorted.
fn stored(db: &GraphDB) -> Vec<String> {
    let mut v: Vec<String> = db.list_files().unwrap().into_iter().map(|(_, p, _)| p).collect();
    v.sort();
    v
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn symbol_names(db: &GraphDB, stored_path: &str) -> Vec<String> {
    let id = db.get_file_id_by_path(stored_path).unwrap().expect("file is indexed");
    db.get_file_symbols_sorted(id).unwrap().into_iter().map(|n| n.name).collect()
}

fn root_str(dir: &TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

/// The defect this module exists for: the additional-root pass used to treat
/// every main-root file as deleted, and the next start did the reverse.
#[tokio::test]
async fn both_roots_survive_repeated_passes() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "main_only.rs", "fn main_fn() {}\n");
    write(extra.path(), "extra_only.rs", "fn extra_fn() {}\n");
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;

    let first = index_all_roots(&root_str(&main), &db).await?;
    let want = sorted(vec!["main_only.rs".to_string(), abs(extra.path(), "extra_only.rs")]);
    assert_eq!(stored(&db), want);
    assert_eq!((first.total_files, first.indexed_files), (2, 2));
    assert_eq!(first.symbols, 2);

    // Two more passes stand in for two daemon restarts.
    for _ in 0..2 {
        let again = index_all_roots(&root_str(&main), &db).await?;
        assert_eq!(stored(&db), want);
        assert_eq!(again, IndexSummary { total_files: 2, indexed_files: 0, symbols: 0 });
    }
    assert_eq!(symbol_names(&db, "main_only.rs"), vec!["main_fn"]);
    assert_eq!(symbol_names(&db, &abs(extra.path(), "extra_only.rs")), vec!["extra_fn"]);
    Ok(())
}

#[tokio::test]
async fn workspace_folders_key_is_indexed_too() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "a.rs", "fn a() {}\n");
    write(extra.path(), "src/b.rs", "fn b() {}\n");
    set_roots(main.path(), "workspace_folders", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;

    index_all_roots(&root_str(&main), &db).await?;

    assert_eq!(stored(&db), sorted(vec!["a.rs".to_string(), abs(extra.path(), "src/b.rs")]));
    Ok(())
}

#[tokio::test]
async fn same_relative_path_in_two_roots_stays_two_files() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "src/shared.rs", "fn main_side() {}\n");
    write(extra.path(), "src/shared.rs", "fn extra_side() {}\n");
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;

    index_all_roots(&root_str(&main), &db).await?;
    index_all_roots(&root_str(&main), &db).await?;

    let extra_stored = abs(extra.path(), "src/shared.rs");
    assert_eq!(stored(&db), sorted(vec!["src/shared.rs".to_string(), extra_stored.clone()]));
    assert_eq!(symbol_names(&db, "src/shared.rs"), vec!["main_side"]);
    assert_eq!(symbol_names(&db, &extra_stored), vec!["extra_side"]);
    Ok(())
}

#[tokio::test]
async fn deleting_a_file_removes_only_that_entry() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "keep.rs", "fn keep_main() {}\n");
    write(main.path(), "gone.rs", "fn gone_main() {}\n");
    write(extra.path(), "keep.rs", "fn keep_extra() {}\n");
    write(extra.path(), "gone.rs", "fn gone_extra() {}\n");
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;
    index_all_roots(&root_str(&main), &db).await?;
    assert_eq!(stored(&db).len(), 4);

    std::fs::remove_file(extra.path().join("gone.rs"))?;
    index_all_roots(&root_str(&main), &db).await?;
    assert_eq!(
        stored(&db),
        sorted(vec!["gone.rs".to_string(), "keep.rs".to_string(), abs(extra.path(), "keep.rs")])
    );

    std::fs::remove_file(main.path().join("gone.rs"))?;
    index_all_roots(&root_str(&main), &db).await?;
    assert_eq!(stored(&db), sorted(vec!["keep.rs".to_string(), abs(extra.path(), "keep.rs")]));
    Ok(())
}

#[tokio::test]
async fn changed_file_in_additional_root_is_reparsed() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "a.rs", "fn a() {}\n");
    write(extra.path(), "b.rs", "fn before() {}\n");
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;
    index_all_roots(&root_str(&main), &db).await?;

    write(extra.path(), "b.rs", "fn after() {}\n");
    let pass = index_all_roots(&root_str(&main), &db).await?;

    assert_eq!((pass.total_files, pass.indexed_files), (2, 1));
    assert_eq!(symbol_names(&db, &abs(extra.path(), "b.rs")), vec!["after"]);
    assert_eq!(symbol_names(&db, "a.rs"), vec!["a"]);
    Ok(())
}

#[tokio::test]
async fn removing_a_root_from_config_purges_only_its_entries() -> Result<()> {
    let (main, first, second) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    write(main.path(), "a.rs", "fn a() {}\n");
    write(first.path(), "b.rs", "fn b() {}\n");
    write(second.path(), "c.rs", "fn c() {}\n");
    set_roots(main.path(), "additional_paths", &[first.path(), second.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;
    index_all_roots(&root_str(&main), &db).await?;
    assert_eq!(stored(&db).len(), 3);

    set_roots(main.path(), "additional_paths", &[second.path()]);
    index_all_roots(&root_str(&main), &db).await?;
    assert_eq!(stored(&db), sorted(vec!["a.rs".to_string(), abs(second.path(), "c.rs")]));

    std::fs::remove_file(main.path().join(".comp/config.json"))?;
    index_all_roots(&root_str(&main), &db).await?;
    assert_eq!(stored(&db), vec!["a.rs".to_string()]);
    Ok(())
}

/// A root that does not exist must not cost the other roots their entries.
#[tokio::test]
async fn missing_root_is_skipped_and_the_rest_is_indexed() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "a.rs", "fn a() {}\n");
    write(extra.path(), "b.rs", "fn b() {}\n");
    let missing = extra.path().join("no-such-dir");
    set_roots(main.path(), "additional_paths", &[&missing, extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;

    let pass = index_all_roots(&root_str(&main), &db).await?;

    assert_eq!(stored(&db), sorted(vec!["a.rs".to_string(), abs(extra.path(), "b.rs")]));
    assert_eq!(pass.total_files, 2);
    Ok(())
}

/// A single-root index built before this feature keeps working untouched:
/// same stored paths, nothing re-parsed.
#[tokio::test]
async fn single_root_index_is_unchanged() -> Result<()> {
    let main = TempDir::new()?;
    write(main.path(), "src/a.rs", "fn a() {}\n");
    write(main.path(), "README.md", "# Title\n");
    let db = GraphDB::new(&root_str(&main)).await?;
    super::Indexer::new(&root_str(&main)).index_workspace(None, &db).await?;
    let before = stored(&db);
    assert_eq!(before, vec!["README.md".to_string(), "src/a.rs".to_string()]);

    let pass = index_all_roots(&root_str(&main), &db).await?;

    assert_eq!(stored(&db), before);
    assert_eq!((pass.total_files, pass.indexed_files), (2, 0));
    Ok(())
}

/// Cross-root dependency: the reason for sharing one graph.
#[tokio::test]
async fn call_into_additional_root_produces_an_edge() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "main.rs", "fn run() {\n    shared_helper();\n}\n");
    write(extra.path(), "lib.rs", "pub fn shared_helper() {}\n");
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;

    // Baseline: the same two files in a single root.
    let single = TempDir::new()?;
    write(single.path(), "main.rs", "fn run() {\n    shared_helper();\n}\n");
    write(single.path(), "lib.rs", "pub fn shared_helper() {}\n");
    let single_db = GraphDB::new(&root_str(&single)).await?;
    index_all_roots(&root_str(&single), &single_db).await?;
    let (_, _, single_edges) = single_db.get_stats()?;
    assert!(single_edges > 0, "baseline must contain the call edge");

    // The main root is parsed before the callee exists, so this only holds if
    // edges are resolved after every root is in. One pass, no second chance.
    index_all_roots(&root_str(&main), &db).await?;
    let (_, _, edges) = db.get_stats()?;
    assert_eq!(edges, single_edges, "run() -> shared_helper() across roots");
    Ok(())
}

/// Entries that are not spelled the way the current roots store them are
/// dropped by a full pass: an absolute alias of a main-root file, and an
/// entry left under another spelling of an additional root.
#[tokio::test]
async fn entries_under_a_stale_spelling_are_purged() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "a.rs", "fn a() {}\n");
    write(extra.path(), "b.rs", "fn b() {}\n");
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;
    index_all_roots(&root_str(&main), &db).await?;
    let want = sorted(vec!["a.rs".to_string(), abs(extra.path(), "b.rs")]);
    assert_eq!(stored(&db), want);

    let main_alias = abs(main.path(), "a.rs");
    let extra_root = normalize_abs(&extra.path().to_string_lossy()).unwrap();
    // Same directory on a case-insensitive drive path, another one elsewhere;
    // either way it is not the current spelling.
    let swapped: String = extra_root
        .chars()
        .map(|c| if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
        .collect();
    let extra_alias = format!("{}/b.rs", swapped);
    assert_ne!(extra_alias, abs(extra.path(), "b.rs"));
    db.upsert_file(&main_alias, "stale-hash", "rust", 10)?;
    db.upsert_file(&extra_alias, "stale-hash", "rust", 10)?;
    assert_eq!(stored(&db).len(), 4);

    index_all_roots(&root_str(&main), &db).await?;

    assert_eq!(stored(&db), want);
    Ok(())
}

/// Conversation history is indexed for the workspace it was recorded in and
/// never for an additional root: another folder's log must not surface here.
#[tokio::test]
async fn history_of_an_additional_root_is_not_indexed() -> Result<()> {
    let (main, extra) = (TempDir::new()?, TempDir::new()?);
    write(main.path(), "a.rs", "fn a() {}
");
    write(extra.path(), "b.rs", "fn b() {}
");
    let line = r#"{"ts":"2026-11-16T00:00:00Z","request":"r","outcome":"o"}"#;
    write(main.path(), ".comp/history/log-2026-11.jsonl", &format!("{}
", line));
    write(extra.path(), ".comp/history/log-2026-11.jsonl", &format!("{}
", line));
    set_roots(main.path(), "additional_paths", &[extra.path()]);
    let db = GraphDB::new(&root_str(&main)).await?;

    index_all_roots(&root_str(&main), &db).await?;
    index_all_roots(&root_str(&main), &db).await?;

    assert_eq!(
        stored(&db),
        sorted(vec![
            ".comp/history/log-2026-11.jsonl".to_string(),
            "a.rs".to_string(),
            abs(extra.path(), "b.rs"),
        ])
    );
    Ok(())
}
