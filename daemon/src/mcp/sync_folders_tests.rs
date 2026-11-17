//! `syncFolders`: folder-level deletes, renames and creations reach the index.
//! Owned by the designer role; implementers must not edit.

use super::*;
use crate::indexer::roots::normalize_abs;
use std::path::Path;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The spelling VS Code sends on Windows: lower-case drive letter.
fn vscode_spelling(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    match s.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => format!("{}{}", (*d as char).to_ascii_lowercase(), &s[1..]),
        _ => s,
    }
}

/// An indexed git workspace with one additional root.
struct Ws {
    main: TempDir,
    extra: TempDir,
    outside: TempDir,
    server: MCPServer,
}

impl Ws {
    async fn new() -> Ws {
        let (main, extra, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap(), TempDir::new().unwrap());
        let m = main.path();
        write(m, "src/a/x.rs", "pub fn x_fn() {}\n");
        write(m, "src/ab/y.rs", "pub fn y_fn() {}\n");
        write(m, "src/keep.rs", "pub fn keep_fn() {}\n");
        write(m, "src/pkg/callee.rs", "pub fn callee_fn() {}\n");
        write(m, "src/pkg/caller.rs", "fn caller_fn() {\n    callee_fn();\n}\n");
        write(m, ".gitignore", "build/\n");
        write(extra.path(), "lib/z.rs", "pub fn z_fn() {}\n");
        write(extra.path(), "top.rs", "pub fn top_fn() {}\n");
        write(outside.path(), "o/o.rs", "pub fn o_fn() {}\n");
        let out = std::process::Command::new("git").arg("init").current_dir(m).output().unwrap();
        assert!(out.status.success());
        let config = json!({ "additional_paths": [extra.path().to_string_lossy()] });
        write(m, ".comp/config.json", &config.to_string());
        let state = Arc::new(crate::AppState::new(m.to_str().unwrap(), "test-agent").await.expect("AppState"));
        let server = MCPServer::new(state);
        crate::indexer::index_all_roots(m.to_str().unwrap(), &server.state.graph_db).await.unwrap();
        Ws { main, extra, outside, server }
    }

    fn stored(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.server.state.graph_db.list_files().unwrap().into_iter().map(|(_, p, _)| p).collect();
        v.sort();
        v
    }

    fn extra_stored(&self, rel: &str) -> String {
        format!("{}/{}", normalize_abs(&self.extra.path().to_string_lossy()).unwrap(), rel)
    }

    async fn sync(&self, paths: &[String]) -> Value {
        self.server.handle_sync_folders(json!({ "paths": paths })).await.expect("syncFolders")
    }

    fn main_path(&self, rel: &str) -> String {
        vscode_spelling(&self.main.path().join(rel))
    }
}

#[tokio::test]
async fn a_deleted_folder_takes_only_its_own_entries_with_it() {
    let ws = Ws::new().await;
    assert!(ws.stored().contains(&"src/a/x.rs".to_string()));

    std::fs::remove_dir_all(ws.main.path().join("src/a")).unwrap();
    let out = ws.sync(&[ws.main_path("src/a")]).await;

    assert_eq!(out["reindexed"], true);
    let stored = ws.stored();
    assert!(!stored.contains(&"src/a/x.rs".to_string()), "{:?}", stored);
    // `src/ab` shares the `src/a` prefix but is another folder.
    assert!(stored.contains(&"src/ab/y.rs".to_string()), "{:?}", stored);
    assert!(stored.contains(&"src/keep.rs".to_string()), "{:?}", stored);
    assert!(stored.contains(&ws.extra_stored("lib/z.rs")), "{:?}", stored);
}

#[tokio::test]
async fn a_renamed_folder_moves_its_entries_and_keeps_the_edges_between_them() {
    let ws = Ws::new().await;

    std::fs::rename(ws.main.path().join("src/pkg"), ws.main.path().join("src/moved")).unwrap();
    let out = ws.sync(&[ws.main_path("src/pkg"), ws.main_path("src/moved")]).await;

    assert_eq!(out["reindexed"], true);
    let stored = ws.stored();
    assert!(stored.contains(&"src/moved/callee.rs".to_string()), "{:?}", stored);
    assert!(stored.contains(&"src/moved/caller.rs".to_string()), "{:?}", stored);
    assert!(!stored.iter().any(|p| p.starts_with("src/pkg/")), "{:?}", stored);
    let deps = ws
        .server
        .handle_get_dependencies(json!({ "name": "callee_fn", "direction": "in" }))
        .await
        .unwrap();
    let deps = deps.as_str().unwrap();
    assert!(deps.contains("caller_fn") && deps.contains("src/moved/caller.rs"), "{}", deps);
}

#[tokio::test]
async fn run_pipeline_forgets_the_old_paths_of_a_renamed_folder() {
    let ws = Ws::new().await;
    ws.server.rebuild_search_index().await;
    let pivots = |out: Value| -> Vec<String> {
        out["pivot_files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect()
    };
    let task = json!({ "task": "callee_fn caller_fn" });
    let before = pivots(ws.server.handle_run_pipeline(task.clone()).await.unwrap());
    assert!(before.iter().any(|p| p.starts_with("src/pkg/")), "{:?}", before);

    std::fs::rename(ws.main.path().join("src/pkg"), ws.main.path().join("src/moved")).unwrap();
    ws.sync(&[ws.main_path("src/pkg"), ws.main_path("src/moved")]).await;

    let after = pivots(ws.server.handle_run_pipeline(task).await.unwrap());
    assert!(!after.iter().any(|p| p.starts_with("src/pkg/")), "old paths returned: {:?}", after);
    assert!(after.iter().any(|p| p.starts_with("src/moved/")), "{:?}", after);
}

#[tokio::test]
async fn a_created_folder_with_source_files_is_indexed() {
    let ws = Ws::new().await;

    write(ws.main.path(), "src/fresh/f.rs", "pub fn f_fn() {}\n");
    let out = ws.sync(&[ws.main_path("src/fresh")]).await;

    assert_eq!(out["reindexed"], true);
    assert!(ws.stored().contains(&"src/fresh/f.rs".to_string()));
    assert!(out["total_files"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn folder_changes_in_an_additional_root_reach_the_index() {
    let ws = Ws::new().await;
    assert!(ws.stored().contains(&ws.extra_stored("lib/z.rs")));

    std::fs::remove_dir_all(ws.extra.path().join("lib")).unwrap();
    let out = ws.sync(&[vscode_spelling(&ws.extra.path().join("lib"))]).await;

    assert_eq!(out["reindexed"], true);
    let stored = ws.stored();
    assert!(!stored.contains(&ws.extra_stored("lib/z.rs")), "{:?}", stored);
    assert!(stored.contains(&ws.extra_stored("top.rs")), "{:?}", stored);
}

#[tokio::test]
async fn a_deleted_additional_root_takes_its_entries_with_it() {
    let ws = Ws::new().await;
    let extra_path = vscode_spelling(ws.extra.path());

    std::fs::remove_dir_all(ws.extra.path()).unwrap();
    let out = ws.sync(&[extra_path]).await;

    assert_eq!(out["reindexed"], true);
    let stored = ws.stored();
    assert!(!stored.iter().any(|p| p.starts_with(&ws.extra_stored(""))), "{:?}", stored);
    assert!(stored.contains(&"src/keep.rs".to_string()), "{:?}", stored);
}

#[tokio::test]
async fn changes_that_cannot_affect_the_index_do_not_reindex() {
    let ws = Ws::new().await;
    // On disk only: a re-index would pick this file up.
    write(ws.main.path(), "src/unindexed.rs", "pub fn u_fn() {}\n");
    write(ws.main.path(), ".cache/h.rs", "pub fn h_fn() {}\n");
    write(ws.main.path(), "node_modules/pkg/n.rs", "pub fn n_fn() {}\n");
    write(ws.main.path(), "build/gen.rs", "pub fn gen_fn() {}\n");
    std::fs::create_dir_all(ws.main.path().join("src/empty")).unwrap();
    let before = ws.stored();

    let cases: Vec<(&str, String)> = vec![
        ("deleted path with nothing indexed under it", ws.main_path("docs/gone")),
        // `src/keep.rs` starts with `src/k` but is not under a folder `src/k`.
        ("deleted path that is only a name prefix of an indexed file", ws.main_path("src/k")),
        ("existing file", ws.main_path("src/keep.rs")),
        ("folder outside every root", vscode_spelling(&ws.outside.path().join("o"))),
        ("hidden folder", ws.main_path(".cache")),
        ("excluded folder name", ws.main_path("node_modules/pkg")),
        ("gitignored folder", ws.main_path("build")),
        ("empty folder", ws.main_path("src/empty")),
        ("the workspace root itself", vscode_spelling(ws.main.path())),
    ];
    for (what, path) in cases {
        let out = ws.sync(&[path.clone()]).await;
        assert_eq!(out["reindexed"], false, "{}: {}", what, path);
        assert_eq!(ws.stored(), before, "{}: the index must not change", what);
    }
}

#[tokio::test]
async fn paths_must_be_an_array_of_strings() {
    let ws = Ws::new().await;
    assert!(ws.server.handle_sync_folders(json!({})).await.is_err());
    assert!(ws.server.handle_sync_folders(json!({ "paths": "src/a" })).await.is_err());
}

#[tokio::test]
async fn sync_folders_is_reachable_over_json_rpc_but_not_listed_as_a_tool() {
    let defs = MCPServer::tool_definitions();
    assert!(!defs["tools"].as_array().unwrap().iter().any(|t| t["name"] == "syncFolders"));
    let ws = Ws::new().await;
    let out = ws.server.handle_rpc_method("syncFolders", json!({ "paths": [] })).await.expect("dispatched");
    assert_eq!(out["reindexed"], false);
}
