//! A full incremental pass keeps the edges of unchanged callers into files
//! that changed or moved.
//! Owned by the designer role; implementers must not edit.

use super::*;
use std::path::Path;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Names (and files) of the symbols that depend on `name`.
fn dependents(db: &crate::graph::GraphDB, name: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for sym in db.get_symbols_by_name(name, None).unwrap() {
        for (dep, _) in db.get_node_dependencies_in(sym.id).unwrap() {
            out.push((dep.name.clone(), db.get_file_path_by_id(dep.file_id).unwrap()));
        }
    }
    out.sort();
    out
}

async fn indexed_workspace() -> (TempDir, crate::graph::GraphDB) {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/lib/callee.rs", "pub fn callee_fn() {}\n");
    write(dir.path(), "src/app.rs", "fn app_fn() {\n    callee_fn();\n}\n");
    let db = crate::graph::GraphDB::new(dir.path().to_str().unwrap()).await.unwrap();
    index_all_roots(dir.path().to_str().unwrap(), &db).await.unwrap();
    assert_eq!(dependents(&db, "callee_fn"), vec![("app_fn".to_string(), "src/app.rs".to_string())]);
    (dir, db)
}

#[tokio::test]
async fn an_unchanged_caller_keeps_its_edge_into_a_moved_file() {
    let (dir, db) = indexed_workspace().await;

    std::fs::rename(dir.path().join("src/lib"), dir.path().join("src/core")).unwrap();
    index_all_roots(dir.path().to_str().unwrap(), &db).await.unwrap();

    assert_eq!(dependents(&db, "callee_fn"), vec![("app_fn".to_string(), "src/app.rs".to_string())]);
}

#[tokio::test]
async fn an_unchanged_caller_keeps_its_edge_into_a_changed_file() {
    let (dir, db) = indexed_workspace().await;

    write(dir.path(), "src/lib/callee.rs", "pub fn callee_fn() {}\npub fn added_fn() {}\n");
    index_all_roots(dir.path().to_str().unwrap(), &db).await.unwrap();

    assert_eq!(dependents(&db, "callee_fn"), vec![("app_fn".to_string(), "src/app.rs".to_string())]);
}

#[tokio::test]
async fn a_caller_of_a_deleted_function_ends_up_with_no_edge_and_no_error() {
    let (dir, db) = indexed_workspace().await;

    std::fs::remove_dir_all(dir.path().join("src/lib")).unwrap();
    index_all_roots(dir.path().to_str().unwrap(), &db).await.unwrap();

    assert_eq!(dependents(&db, "callee_fn"), Vec::<(String, String)>::new());
    let (files, _, edges) = db.get_stats().unwrap();
    assert_eq!((files, edges), (1, 0));
}
