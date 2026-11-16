//! Multi-root behaviour of the MCP handlers that take a file path.
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

/// Main root, one configured additional root, and a directory that is in no root.
struct Ws {
    main: TempDir,
    extra: TempDir,
    outside: TempDir,
    server: MCPServer,
}

impl Ws {
    async fn new() -> Ws {
        let (main, extra, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap(), TempDir::new().unwrap());
        write(main.path(), "src/a.rs", "fn main_fn() {}\n");
        write(extra.path(), "lib.rs", "fn extra_fn() {}\n");
        write(outside.path(), "secret.rs", "fn secret_fn() {}\n");
        let config = json!({ "additional_paths": [extra.path().to_string_lossy()] });
        write(main.path(), ".comp/config.json", &config.to_string());
        let state = Arc::new(crate::AppState::new(main.path().to_str().unwrap(), "test-agent").await.expect("AppState"));
        Ws { main, extra, outside, server: MCPServer::new(state) }
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

    async fn index(&self, path: &Path) -> Result<Value> {
        self.server.handle_index_file(json!({ "path": path.to_string_lossy() })).await
    }

    async fn remove(&self, path: &str) -> Result<Value> {
        self.server.handle_remove_file(json!({ "path": path })).await
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// The spelling VS Code sends on Windows (`e:\dev\…`): lower-case drive
/// letter. Unchanged for paths without a drive letter.
fn vscode_spelling(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    match s.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => format!("{}{}", (*d as char).to_ascii_lowercase(), &s[1..]),
        _ => s,
    }
}

#[tokio::test]
async fn index_file_stores_a_main_root_file_under_its_relative_path() {
    let ws = Ws::new().await;

    ws.index(&ws.main.path().join("src/a.rs")).await.expect("indexFile");
    // The second spelling must land on the same entry, not beside it.
    ws.server
        .handle_index_file(json!({ "path": vscode_spelling(&ws.main.path().join("src/a.rs")) }))
        .await
        .expect("indexFile, VS Code spelling");

    assert_eq!(ws.stored(), vec!["src/a.rs".to_string()]);
}

#[tokio::test]
async fn index_file_accepts_an_additional_root_and_stores_it_absolute() {
    let ws = Ws::new().await;

    ws.index(&ws.extra.path().join("lib.rs")).await.expect("indexFile in additional root");
    ws.server
        .handle_index_file(json!({ "path": vscode_spelling(&ws.extra.path().join("lib.rs")) }))
        .await
        .expect("indexFile, VS Code spelling");

    assert_eq!(ws.stored(), vec![ws.extra_stored("lib.rs")]);
}

/// A save-triggered update and a full pass must agree on the stored path.
#[tokio::test]
async fn index_file_and_a_full_pass_never_list_a_file_twice() {
    let ws = Ws::new().await;
    let want = sorted(vec!["src/a.rs".to_string(), ws.extra_stored("lib.rs")]);
    let root = ws.main.path().to_string_lossy().to_string();

    crate::indexer::index_all_roots(&root, &ws.server.state.graph_db).await.unwrap();
    assert_eq!(ws.stored(), want);

    ws.index(&ws.main.path().join("src/a.rs")).await.unwrap();
    ws.index(&ws.extra.path().join("lib.rs")).await.unwrap();
    assert_eq!(ws.stored(), want);

    let pass = crate::indexer::index_all_roots(&root, &ws.server.state.graph_db).await.unwrap();
    assert_eq!(ws.stored(), want);
    assert_eq!(pass.indexed_files, 0, "entries written by indexFile are reused, not re-parsed");
}

#[tokio::test]
async fn index_file_picks_up_new_content_in_an_additional_root() {
    let ws = Ws::new().await;
    ws.index(&ws.extra.path().join("lib.rs")).await.unwrap();

    write(ws.extra.path(), "lib.rs", "fn renamed_fn() {}\n");
    ws.index(&ws.extra.path().join("lib.rs")).await.unwrap();

    let db = &ws.server.state.graph_db;
    let id = db.get_file_id_by_path(&ws.extra_stored("lib.rs")).unwrap().expect("indexed");
    let names: Vec<String> = db.get_file_symbols_sorted(id).unwrap().into_iter().map(|n| n.name).collect();
    assert_eq!(names, vec!["renamed_fn"]);
}

#[tokio::test]
async fn index_file_rejects_paths_outside_every_root() {
    let ws = Ws::new().await;
    // A sibling whose name starts with the additional root's name.
    let sibling = std::path::PathBuf::from(format!("{}2", ws.extra.path().to_string_lossy()));
    write(&sibling, "x.rs", "fn sibling_fn() {}\n");
    let outside_name = ws.outside.path().file_name().unwrap().to_string_lossy().to_string();

    let rejected = [
        ws.outside.path().join("secret.rs"),
        sibling.join("x.rs"),
        // `..` out of the additional root, and out of the main root
        ws.extra.path().join("..").join(&outside_name).join("secret.rs"),
        ws.main.path().join("src").join("..").join("..").join(&outside_name).join("secret.rs"),
    ];
    for path in &rejected {
        let err = ws.index(path).await.expect_err(&format!("must be rejected: {}", path.display()));
        assert!(err.to_string().contains("outside"), "unexpected error for {}: {}", path.display(), err);
    }
    assert_eq!(ws.stored(), Vec::<String>::new());

    let _ = std::fs::remove_dir_all(&sibling);
}

/// A link inside a root must not let a file from outside into the index.
#[cfg(unix)]
#[tokio::test]
async fn index_file_rejects_a_symlink_that_leaves_the_roots() {
    let ws = Ws::new().await;
    for root in [ws.main.path(), ws.extra.path()] {
        let link = root.join("link.rs");
        std::os::unix::fs::symlink(ws.outside.path().join("secret.rs"), &link).unwrap();
        let err = ws.index(&link).await.expect_err("symlink target is outside");
        assert!(err.to_string().contains("outside"), "unexpected error: {}", err);
    }
    assert_eq!(ws.stored(), Vec::<String>::new());
}

#[tokio::test]
async fn remove_file_drops_exactly_the_named_entry() {
    let ws = Ws::new().await;
    write(ws.main.path(), "lib.rs", "fn main_lib() {}\n");
    let root = ws.main.path().to_string_lossy().to_string();
    crate::indexer::index_all_roots(&root, &ws.server.state.graph_db).await.unwrap();
    assert_eq!(ws.stored(), sorted(vec!["lib.rs".to_string(), "src/a.rs".to_string(), ws.extra_stored("lib.rs")]));

    // The file is already gone from disk when the event arrives.
    std::fs::remove_file(ws.extra.path().join("lib.rs")).unwrap();
    let out = ws.remove(&vscode_spelling(&ws.extra.path().join("lib.rs"))).await.unwrap();
    assert_eq!(out["removed"], 1);
    assert_eq!(out["path"], ws.extra_stored("lib.rs"));
    assert_eq!(ws.stored(), vec!["lib.rs".to_string(), "src/a.rs".to_string()]);

    std::fs::remove_file(ws.main.path().join("src/a.rs")).unwrap();
    let out = ws.remove(&vscode_spelling(&ws.main.path().join("src").join("a.rs"))).await.unwrap();
    assert_eq!(out["removed"], 1);
    assert_eq!(out["path"], "src/a.rs");
    assert_eq!(ws.stored(), vec!["lib.rs".to_string()]);
}

#[tokio::test]
async fn remove_file_outside_every_root_changes_nothing() {
    let ws = Ws::new().await;
    let root = ws.main.path().to_string_lossy().to_string();
    crate::indexer::index_all_roots(&root, &ws.server.state.graph_db).await.unwrap();
    let before = ws.stored();
    assert_eq!(before.len(), 2);

    // Same trailing segments as indexed entries: a match on the tail alone
    // would delete `src/a.rs` or the additional root's `lib.rs`.
    write(ws.outside.path(), "src/a.rs", "fn decoy() {}
");
    for path in [ws.outside.path().join("src").join("a.rs"), ws.outside.path().join("lib.rs")] {
        let out = ws.remove(&path.to_string_lossy()).await.unwrap();
        assert_eq!(out["status"], "ok");
        assert_eq!(out["removed"], 0);
    }
    assert_eq!(ws.stored(), before);
}

#[tokio::test]
async fn compress_file_reads_additional_roots_but_nothing_outside() {
    let ws = Ws::new().await;

    let out = ws
        .server
        .handle_compress_file(json!({ "path": ws.extra.path().join("lib.rs").to_string_lossy(), "compression_level": 0 }))
        .await
        .expect("additional root is readable");
    assert!(out["compressed_text"].as_str().unwrap().contains("extra_fn"));

    let err = ws
        .server
        .handle_compress_file(json!({ "path": ws.outside.path().join("secret.rs").to_string_lossy(), "compression_level": 0 }))
        .await
        .expect_err("outside every root");
    assert!(err.to_string().contains("outside"), "unexpected error: {}", err);
}

/// get_symbol resolves a stored path back to a file on disk.
#[tokio::test]
async fn get_symbol_returns_source_from_an_additional_root() {
    let ws = Ws::new().await;
    let root = ws.main.path().to_string_lossy().to_string();
    crate::indexer::index_all_roots(&root, &ws.server.state.graph_db).await.unwrap();

    let out = ws.server.handle_get_symbol(json!({ "name": "extra_fn" })).await.unwrap();
    let text = out.as_str().expect("markdown string");

    assert!(text.contains(&ws.extra_stored("lib.rs")), "stored path missing: {}", text);
    assert!(text.contains("fn extra_fn() {}"), "source missing: {}", text);
}

/// A root dropped from the config stops being readable at once, not at the
/// next restart.
#[tokio::test]
async fn a_root_removed_from_config_is_rejected_immediately() {
    let ws = Ws::new().await;
    ws.index(&ws.extra.path().join("lib.rs")).await.expect("configured root");

    write(ws.main.path(), ".comp/config.json", "{}");

    let err = ws.index(&ws.extra.path().join("lib.rs")).await.expect_err("no longer a root");
    assert!(err.to_string().contains("outside"), "unexpected error: {}", err);
}

/// compress_file has no second check behind the path validation, so these
/// cases pin the validation itself.
#[tokio::test]
async fn compress_file_rejects_a_sibling_and_a_removed_root() {
    let ws = Ws::new().await;
    let read = |path: std::path::PathBuf| {
        let server = &ws.server;
        async move {
            server.handle_compress_file(json!({ "path": path.to_string_lossy(), "compression_level": 0 })).await
        }
    };

    let sibling = std::path::PathBuf::from(format!("{}2", ws.extra.path().to_string_lossy()));
    write(&sibling, "x.rs", "fn sibling_fn() {}
");
    let err = read(sibling.join("x.rs")).await.expect_err("sibling of the additional root");
    assert!(err.to_string().contains("outside"), "unexpected error: {}", err);
    let _ = std::fs::remove_dir_all(&sibling);

    read(ws.extra.path().join("lib.rs")).await.expect("still configured");
    write(ws.main.path(), ".comp/config.json", "{}");
    let err = read(ws.extra.path().join("lib.rs")).await.expect_err("root removed from config");
    assert!(err.to_string().contains("outside"), "unexpected error: {}", err);
}

/// `..` inside a root is fine and names the file it really reaches.
#[tokio::test]
async fn index_file_resolves_dot_dot_to_the_real_file() {
    let ws = Ws::new().await;
    write(ws.extra.path(), "pkg/mod.rs", "fn pkg_fn() {}
");

    ws.index(&ws.main.path().join("src").join("..").join("src").join("a.rs")).await.unwrap();
    ws.index(&ws.extra.path().join("pkg").join("..").join("lib.rs")).await.unwrap();

    assert_eq!(ws.stored(), sorted(vec!["src/a.rs".to_string(), ws.extra_stored("lib.rs")]));
}

/// The indexed file must be the one that was checked. With `s -> d/e`, the
/// path `s/../evil/notes.rs` really is `d/evil/notes.rs`; collapsing `..`
/// lexically would instead name `evil/notes.rs`, here a link out of the roots.
#[cfg(unix)]
#[tokio::test]
async fn index_file_stores_the_checked_file_not_the_lexical_one() {
    let ws = Ws::new().await;
    let main = ws.main.path();
    write(main, "d/e/keep.rs", "fn keep() {}
");
    write(main, "d/evil/notes.rs", "fn inside_fn() {}
");
    write(ws.outside.path(), "notes.rs", "fn outside_fn() {}
");
    std::os::unix::fs::symlink(main.join("d").join("e"), main.join("s")).unwrap();
    std::os::unix::fs::symlink(ws.outside.path(), main.join("evil")).unwrap();

    ws.index(&main.join("s").join("..").join("evil").join("notes.rs")).await.unwrap();

    assert_eq!(ws.stored(), vec!["d/evil/notes.rs".to_string()]);
    let db = &ws.server.state.graph_db;
    let id = db.get_file_id_by_path("d/evil/notes.rs").unwrap().unwrap();
    let names: Vec<String> = db.get_file_symbols_sorted(id).unwrap().into_iter().map(|n| n.name).collect();
    assert_eq!(names, vec!["inside_fn"]);
}
