//! `git diff` paths must match the stored paths of the workspace root's files:
//! also when the workspace is a subfolder of the repository, and for names git
//! would quote (non-ASCII).
//! Owned by the designer role; implementers must not edit.

use super::*;
use std::path::Path;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A repository with the workspace at `packages/app`, one file inside it, one
/// outside it, and one non-ASCII name inside it — all committed, then modified.
fn repo_with_workspace_in_a_subfolder() -> (TempDir, std::path::PathBuf) {
    let repo = TempDir::new().unwrap();
    let ws = repo.path().join("packages").join("app");
    write(&ws, "src/a.rs", "fn a() {}\n");
    write(&ws, "docs/日本語.md", "# 見出し\n");
    write(repo.path(), "other.rs", "fn other() {}\n");
    git(repo.path(), &["init"]);
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "init"]);
    write(&ws, "src/a.rs", "fn a() { let x = 1; }\n");
    write(&ws, "docs/日本語.md", "# 見出し\n本文\n");
    write(repo.path(), "other.rs", "fn other() { let y = 2; }\n");
    (repo, ws)
}

#[test]
fn diff_paths_are_relative_to_a_workspace_in_a_subfolder_and_unquoted() {
    let (_repo, ws) = repo_with_workspace_in_a_subfolder();

    let mut got: Vec<String> = get_git_diff_files(ws.to_str().unwrap()).into_iter().collect();
    got.sort();

    assert_eq!(got, vec!["docs/日本語.md".to_string(), "src/a.rs".to_string()]);
}

#[test]
fn diff_paths_at_the_repository_root_are_unchanged() {
    let (repo, _ws) = repo_with_workspace_in_a_subfolder();

    let mut got: Vec<String> = get_git_diff_files(repo.path().to_str().unwrap()).into_iter().collect();
    got.sort();

    assert_eq!(
        got,
        vec!["other.rs".to_string(), "packages/app/docs/日本語.md".to_string(), "packages/app/src/a.rs".to_string()]
    );
}

#[tokio::test]
async fn get_git_diff_context_matches_indexed_files_of_a_workspace_in_a_subfolder() {
    let (_repo, ws) = repo_with_workspace_in_a_subfolder();
    let state = Arc::new(crate::AppState::new(ws.to_str().unwrap(), "test-agent").await.expect("AppState"));
    state.graph_db.upsert_file("src/a.rs", "h", "rust", 0).unwrap();
    state.graph_db.upsert_file("docs/日本語.md", "h", "markdown", 0).unwrap();
    let server = MCPServer::new(state);

    let out = server.handle_get_git_diff_context(json!({ "base_ref": "HEAD" })).await.unwrap();

    assert_eq!(out["total_changed"], 2, "other.rs is outside the workspace: {}", out["changed_files"]);
    let mut paths: Vec<(String, bool)> = out["changed_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap().to_string(), f["indexed"].as_bool().unwrap()))
        .collect();
    paths.sort();
    assert_eq!(paths, vec![("docs/日本語.md".to_string(), true), ("src/a.rs".to_string(), true)]);
}
