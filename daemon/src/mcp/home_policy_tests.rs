//! MCP path checks refuse files in the home directory's hidden folders when
//! they are reached through an additional root.
//! Owned by the designer role; implementers must not edit.

use super::*;
use crate::indexer::roots::HomePolicy;
use std::path::Path;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
    p
}

fn set_roots(main: &Path, entries: &[&Path]) {
    let list: Vec<String> = entries.iter().map(|p| p.to_string_lossy().to_string()).collect();
    write(main, ".comp/config.json", &json!({ "additional_paths": list }).to_string());
}

/// A fake home with secrets in its hidden folders and ordinary files
/// elsewhere, and a main root next to it (not inside it).
struct Home {
    home: TempDir,
    main: TempDir,
}

impl Home {
    fn new() -> Home {
        let (home, main) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let h = home.path();
        write(h, ".ssh/id_rsa", "PRIVATE KEY\n");
        write(h, "AppData/Roaming/app/token.json", "{\"token\":\"x\"}\n");
        write(h, "Library/Keychains/login.keychain-db", "x\n");
        write(h, "projects/lib/a.rs", "fn lib_fn() {}\n");
        write(h, "AppData/Local/Temp/work/t.rs", "fn temp_fn() {}\n");
        write(main.path(), "src/m.rs", "fn main_fn() {}\n");
        Home { home, main }
    }

    fn policy(&self, allow: bool) -> HomePolicy {
        HomePolicy::from_vars(Some(&self.home.path().to_string_lossy()), None, if allow { Some("1") } else { None })
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.home.path().join(rel)
    }

    fn check(&self, file: &Path, allow: bool) -> Result<std::path::PathBuf> {
        validate_within_workspace(&file.to_string_lossy(), &self.main.path().to_string_lossy(), &self.policy(allow))
    }

    async fn server(&self, allow: bool) -> MCPServer {
        let mut state = crate::AppState::new(self.main.path().to_str().unwrap(), "test-agent").await.expect("AppState");
        state.home_policy = self.policy(allow);
        MCPServer::new(Arc::new(state))
    }
}

const SECRETS: [&str; 3] = [".ssh/id_rsa", "AppData/Roaming/app/token.json", "Library/Keychains/login.keychain-db"];
const ORDINARY: [&str; 2] = ["projects/lib/a.rs", "AppData/Local/Temp/work/t.rs"];

/// `additional_paths: ["~"]` (or `["../.."]`): the root itself is allowed,
/// but its hidden folders are not reachable through it.
#[test]
fn a_root_as_wide_as_the_home_cannot_reach_its_hidden_folders() {
    let h = Home::new();
    set_roots(h.main.path(), &[h.home.path()]);

    for rel in SECRETS {
        let err = h.check(&h.path(rel), false).expect_err(&format!("must be refused: {}", rel));
        assert!(err.to_string().contains("hidden folder"), "{}: {}", rel, err);
    }
    for rel in ORDINARY {
        h.check(&h.path(rel), false).unwrap_or_else(|e| panic!("{} must pass: {}", rel, e));
    }
    for rel in SECRETS.iter().chain(ORDINARY.iter()) {
        h.check(&h.path(rel), true).unwrap_or_else(|e| panic!("allowed, {} must pass: {}", rel, e));
    }
}

/// `additional_paths: ["~/.ssh"]`: the root is dropped, so nothing in it is
/// inside any root.
#[test]
fn a_hidden_folder_named_as_a_root_is_refused() {
    let h = Home::new();
    let ssh = h.path(".ssh");
    let roaming = h.path("AppData/Roaming");
    set_roots(h.main.path(), &[&ssh, &roaming]);

    for rel in [".ssh/id_rsa", "AppData/Roaming/app/token.json"] {
        let err = h.check(&h.path(rel), false).expect_err(&format!("must be refused: {}", rel));
        assert!(err.to_string().contains("outside"), "{}: {}", rel, err);
        h.check(&h.path(rel), true).unwrap_or_else(|e| panic!("allowed, {} must pass: {}", rel, e));
    }
}

#[test]
fn the_main_root_is_never_judged() {
    let home = TempDir::new().unwrap();
    let main = home.path().join(".config").join("app");
    let file = write(&main, "src/x.rs", "fn x() {}\n");
    let policy = HomePolicy::from_vars(Some(&home.path().to_string_lossy()), None, None);

    validate_within_workspace(&file.to_string_lossy(), &main.to_string_lossy(), &policy)
        .expect("main root inside a hidden home folder stays usable");
}

/// A link in an ordinary folder that leads into `~/.ssh` is judged by where it leads.
#[cfg(unix)]
#[test]
fn a_symlink_into_a_hidden_folder_is_refused() {
    let h = Home::new();
    set_roots(h.main.path(), &[h.home.path()]);
    let link = h.path("projects/keys");
    std::os::unix::fs::symlink(h.path(".ssh"), &link).unwrap();

    let err = h.check(&link.join("id_rsa"), false).expect_err("leads into ~/.ssh");
    assert!(err.to_string().contains("hidden folder"), "{}", err);
}

/// The handlers use the policy held in AppState.
#[tokio::test]
async fn compress_file_and_index_file_apply_the_state_policy() {
    let h = Home::new();
    set_roots(h.main.path(), &[h.home.path()]);
    let secret = h.path(".ssh/id_rsa").to_string_lossy().to_string();
    let ordinary = h.path("projects/lib/a.rs").to_string_lossy().to_string();

    let blocked = h.server(false).await;
    let err = blocked
        .handle_compress_file(json!({ "path": secret, "compression_level": 0 }))
        .await
        .expect_err("secret through compress_file");
    assert!(err.to_string().contains("hidden folder"), "{}", err);
    let err = blocked.handle_index_file(json!({ "path": secret })).await.expect_err("secret through indexFile");
    assert!(err.to_string().contains("hidden folder"), "{}", err);
    let out = blocked
        .handle_compress_file(json!({ "path": ordinary, "compression_level": 0 }))
        .await
        .expect("ordinary file");
    assert!(out["compressed_text"].as_str().unwrap().contains("lib_fn"));
    let stored: Vec<String> = blocked.state.graph_db.list_files().unwrap().into_iter().map(|(_, p, _)| p).collect();
    assert!(stored.is_empty(), "nothing may be indexed by the refused call: {:?}", stored);

    let allowed = h.server(true).await;
    let out = allowed
        .handle_compress_file(json!({ "path": secret, "compression_level": 0 }))
        .await
        .expect("allowed by the environment switch");
    assert!(out["compressed_text"].as_str().unwrap().contains("PRIVATE KEY"));
}

#[tokio::test]
async fn app_state_takes_the_policy_from_the_environment() {
    let ws = TempDir::new().unwrap();
    let state = crate::AppState::new(ws.path().to_str().unwrap(), "test-agent").await.expect("AppState");
    assert_eq!(state.home_policy, HomePolicy::from_env());
}
