//! Whether this daemon is a workspace's own cargo build that is older than the
//! sources next to it.
//!
//! WHY: a comP checkout registers `daemon/target/release/comp-daemon(.exe)` with
//! agents. Nothing rebuilds it on `git pull`, and on Windows a running daemon
//! keeps the file locked, so an old build kept serving under a newer extension
//! (0.11.11 under 0.12.0) without anyone noticing. The daemon is the one place
//! that can tell, so it reports it — in the MCP `initialize` instructions, which
//! agents show the model, and in `getStats`, which the extension shows the user.

use std::path::Path;
use std::time::SystemTime;

/// A warning when `exe` lies under `<workspace_root>/daemon/target/` and a
/// source it was built from (`daemon/Cargo.toml`, `daemon/Cargo.lock`, any
/// `daemon/src/**/*.rs`) was modified after it; otherwise None.
///
/// Anything unreadable counts as "not stale": a false alarm on every start
/// would teach people to ignore the warning.
pub fn stale_workspace_build(exe: &Path, workspace_root: &Path) -> Option<String> {
    let daemon_dir = workspace_root.canonicalize().ok()?.join("daemon");
    if !exe.canonicalize().ok()?.starts_with(daemon_dir.join("target")) {
        return None;
    }
    let built = modified(exe)?;

    let mut sources = vec![daemon_dir.join("Cargo.toml"), daemon_dir.join("Cargo.lock")];
    collect_rust_files(&daemon_dir.join("src"), &mut sources);
    let (newest_at, newest) = sources
        .into_iter()
        .filter_map(|path| modified(&path).map(|at| (at, path)))
        .max_by_key(|(at, _)| *at)?;
    if newest_at <= built {
        return None;
    }
    let shown = newest.strip_prefix(&daemon_dir).unwrap_or(&newest).display().to_string();
    Some(format!(
        "This comP daemon ({}) is older than its sources: daemon/{} changed after it was built. \
         Features added since then may fail. Rebuild it (cd daemon && cargo build --release; on \
         Windows rename the running comp-daemon.exe first) and restart the agent.",
        exe.display(),
        shown.replace('\\', "/")
    ))
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn collect_rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect_rust_files(&path, out),
            Ok(kind) if kind.is_file() && path.extension().is_some_and(|ext| ext == "rs") => out.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::Duration;

    fn touch(path: &Path, at: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        File::create(path).unwrap().set_modified(at).unwrap();
    }

    struct Ws {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        exe: std::path::PathBuf,
    }

    /// A checkout whose sources date from `src_at` and whose release build from `exe_at`.
    fn workspace(src_at: SystemTime, exe_at: SystemTime) -> Ws {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        touch(&root.join("daemon").join("Cargo.toml"), src_at);
        touch(&root.join("daemon").join("Cargo.lock"), src_at);
        touch(&root.join("daemon").join("src").join("main.rs"), src_at);
        touch(&root.join("daemon").join("src").join("mcp").join("mod.rs"), src_at);
        let exe = root.join("daemon").join("target").join("release").join("comp-daemon.exe");
        touch(&exe, exe_at);
        Ws { _dir: dir, root, exe }
    }

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    #[test]
    fn a_build_newer_than_every_source_is_not_stale() {
        let ws = workspace(t(0), t(100));
        assert_eq!(stale_workspace_build(&ws.exe, &ws.root), None);
    }

    #[test]
    fn a_source_edited_after_the_build_is_named_in_the_warning() {
        let ws = workspace(t(0), t(100));
        touch(&ws.root.join("daemon").join("src").join("mcp").join("mod.rs"), t(200));
        let warning = stale_workspace_build(&ws.exe, &ws.root).expect("stale");
        assert!(warning.contains("mod.rs"), "{warning}");
        assert!(warning.contains("cargo build --release"), "{warning}");
    }

    #[test]
    fn a_newer_cargo_lock_alone_makes_it_stale() {
        let ws = workspace(t(0), t(100));
        touch(&ws.root.join("daemon").join("Cargo.lock"), t(200));
        assert!(stale_workspace_build(&ws.exe, &ws.root).is_some());
    }

    #[test]
    fn non_rust_files_under_src_are_ignored() {
        let ws = workspace(t(0), t(100));
        touch(&ws.root.join("daemon").join("src").join("notes.md"), t(200));
        assert_eq!(stale_workspace_build(&ws.exe, &ws.root), None);
    }

    #[test]
    fn a_binary_outside_the_workspace_target_is_never_judged() {
        // The bundled binary of the extension, or a build in another checkout.
        let ws = workspace(t(200), t(100));
        let elsewhere = tempfile::tempdir().unwrap();
        let exe = elsewhere.path().join("comp-daemon-win.exe");
        touch(&exe, t(0));
        assert_eq!(stale_workspace_build(&exe, &ws.root), None);
    }

    #[test]
    fn a_workspace_without_daemon_sources_is_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("daemon").join("target").join("release").join("comp-daemon");
        touch(&exe, t(0));
        assert_eq!(stale_workspace_build(&exe, dir.path()), None);
    }
}
