//! `mark_stale` with the absolute paths the VS Code file watcher sends.
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

/// The spelling VS Code sends on Windows (`e:\dev\…`): lower-case drive
/// letter. Unchanged for paths without a drive letter.
fn vscode_spelling(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    match s.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => format!("{}{}", (*d as char).to_ascii_lowercase(), &s[1..]),
        _ => s,
    }
}

/// Main root with one additional root. Each recorded call is kept under its
/// own agent id so a test can tell exactly which one was marked.
struct Ws {
    main: TempDir,
    extra: TempDir,
    outside: TempDir,
    server: MCPServer,
}

impl Ws {
    async fn new() -> Ws {
        let (main, extra, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap(), TempDir::new().unwrap());
        write(main.path(), "src/a.rs", "fn a() {}\n");
        write(main.path(), "lib.rs", "fn main_lib() {}\n");
        write(extra.path(), "lib.rs", "fn extra_lib() {}\n");
        write(outside.path(), "src/a.rs", "fn other() {}\n");
        let config = json!({ "additional_paths": [extra.path().to_string_lossy()] });
        write(main.path(), ".comp/config.json", &config.to_string());

        let root = main.path().to_str().unwrap();
        let extra_stored = format!("{}/lib.rs", normalize_abs(&extra.path().to_string_lossy()).unwrap());
        record_mcp_call(root, "main-a", "s", "q".into(), vec![], vec!["src/a.rs".into()], 1).unwrap();
        record_mcp_call(root, "main-lib", "s", "q".into(), vec![], vec!["lib.rs".into()], 1).unwrap();
        record_mcp_call(root, "extra-lib", "s", "q".into(), vec![], vec![extra_stored], 1).unwrap();

        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("AppState"));
        Ws { main, extra, outside, server: MCPServer::new(state) }
    }

    fn stale(&self, agent: &str) -> bool {
        let path = get_session_memory_path(self.main.path().to_str().unwrap(), agent);
        let memory = load_session_memory_or_quarantine(&path).unwrap();
        memory.sessions.iter().flat_map(|s| s.calls.iter()).any(|c| c.stale)
    }

    async fn mark(&self, path: &str) -> u64 {
        let out = self.server.handle_mark_stale(json!({ "path": path })).await.expect("mark_stale");
        out["marked"].as_u64().unwrap()
    }
}

#[tokio::test]
async fn absolute_main_root_path_marks_the_call_recorded_under_its_relative_path() {
    let ws = Ws::new().await;

    let marked = ws.mark(&vscode_spelling(&ws.main.path().join("src").join("a.rs"))).await;

    assert_eq!(marked, 1);
    assert!(ws.stale("main-a"));
    assert!(!ws.stale("main-lib"));
    assert!(!ws.stale("extra-lib"));
}

#[tokio::test]
async fn absolute_additional_root_path_marks_only_that_roots_file() {
    let ws = Ws::new().await;

    let marked = ws.mark(&vscode_spelling(&ws.extra.path().join("lib.rs"))).await;

    assert_eq!(marked, 1);
    assert!(ws.stale("extra-lib"));
    assert!(!ws.stale("main-lib"), "the main root's lib.rs is a different file");
    assert!(!ws.stale("main-a"));
}

#[tokio::test]
async fn absolute_path_outside_every_root_marks_nothing() {
    let ws = Ws::new().await;

    let marked = ws.mark(&vscode_spelling(&ws.outside.path().join("src").join("a.rs"))).await;

    assert_eq!(marked, 0);
    assert!(!ws.stale("main-a"), "same relative name, but not a file of any root");
}

#[tokio::test]
async fn relative_path_still_marks_a_main_root_call() {
    let ws = Ws::new().await;

    let marked = ws.mark("src\\a.rs").await;

    assert_eq!(marked, 1);
    assert!(ws.stale("main-a"));
}
