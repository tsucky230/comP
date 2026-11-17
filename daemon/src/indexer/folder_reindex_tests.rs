//! folder_changes_need_reindex_with: a recreated folder and the home policy.
//! Owned by the designer role; implementers must not edit.

use super::*;
use std::path::Path;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[tokio::test]
async fn a_folder_deleted_and_recreated_empty_still_needs_a_pass() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/gen/out.rs", "pub fn generated() {}\n");
    let db = crate::graph::GraphDB::new(&s(dir.path())).await.unwrap();
    let policy = roots::HomePolicy::new(&[], false);
    index_all_roots_with(&s(dir.path()), &db, &policy).await.unwrap();

    // `rm -rf src/gen && mkdir src/gen`: the event batch only sees an existing, empty folder.
    std::fs::remove_dir_all(dir.path().join("src/gen")).unwrap();
    std::fs::create_dir(dir.path().join("src/gen")).unwrap();

    let paths = vec![s(&dir.path().join("src/gen"))];
    assert!(folder_changes_need_reindex_with(&s(dir.path()), &paths, &db, &policy).unwrap());
}

#[tokio::test]
async fn folders_the_home_policy_blocks_in_an_additional_root_need_no_pass() {
    let main = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write(main.path(), "src/a.rs", "pub fn a() {}\n");
    write(home.path(), "proj/p.rs", "pub fn p() {}\n");
    write(main.path(), ".comp/config.json", &serde_json::json!({ "additional_paths": [s(home.path())] }).to_string());
    let db = crate::graph::GraphDB::new(&s(main.path())).await.unwrap();
    let policy = roots::HomePolicy::new(&[s(home.path())], false);
    index_all_roots_with(&s(main.path()), &db, &policy).await.unwrap();

    write(home.path(), "AppData/Local/cache/c.rs", "pub fn c() {}\n");
    write(home.path(), "newproj/n.rs", "pub fn n() {}\n");

    let blocked = vec![s(&home.path().join("AppData/Local/cache"))];
    assert!(!folder_changes_need_reindex_with(&s(main.path()), &blocked, &db, &policy).unwrap());
    let allowed = vec![s(&home.path().join("newproj"))];
    assert!(folder_changes_need_reindex_with(&s(main.path()), &allowed, &db, &policy).unwrap());
}
