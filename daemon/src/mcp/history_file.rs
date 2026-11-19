use super::*;

/// Append one already-serialized JSONL line to `hist_path`, holding a short-lived
/// *exclusive* OS advisory lock for the duration of the write.
///
/// WHY exclusive, not shared, even though only compaction strictly needs to
/// exclude other writers: on Windows, `LockFileEx`-backed locks (what
/// `std::fs::File::lock*` uses, stable since Rust 1.89) are not purely advisory
/// like POSIX `flock` — a handle holding only a *shared* lock is denied write
/// access to the locked region with `ERROR_LOCK_VIOLATION` (os error 33), even
/// when the writer is the same handle that holds the lock. Verified empirically
/// on this toolchain: every test appending under `lock_shared()` failed at
/// `write_all`, while the same pattern under `lock()` succeeded. Since every
/// caller of this function writes, every caller needs the exclusive lock — there
/// is no cheaper "shared" tier available for a writer on Windows. Contention is
/// negligible in practice: each critical section is one small write, so
/// concurrent appenders just take turns rather than truly blocking each other.
///
/// WHY this must be the ONLY locked writer of this file: history-record.sh
/// (the Stop hook) used to call `fs.appendFileSync` directly from Node,
/// bypassing this lock entirely — an advisory lock only blocks callers that
/// also try to acquire it, so an unlocked writer defeats the whole scheme.
/// The hook now shells out to `comp-daemon append-history` (see
/// `try_run_cli_subcommand`) whenever the daemon binary is available. When it
/// is not, the hook's fallback no longer writes to this file directly either
/// — see `merge_spill_files`' doc for where that content ends up instead
/// (Phase 3, B4).
///
/// Synchronous / blocking: callers on the async runtime must wrap this in
/// `tokio::task::spawn_blocking`.
pub(in crate::mcp) fn append_history_line(hist_path: &std::path::Path, line: &str) -> Result<()> {
    // WHY no external crate: std::fs::File::lock (stable since Rust 1.89) wraps
    // flock/LockFileEx natively — no locking dependency needed.
    use std::io::Write;

    if let Some(parent) = hist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // WHY .read(true) on a write-only append: on Windows, File::lock (and
    // lock_shared) require the handle to carry read access — an append()-only
    // handle lacks it and lock() fails with ERROR_ACCESS_DENIED (os error 5),
    // verified empirically on this toolchain. Adding .read(true) doesn't change
    // the actual write behavior (writes still go through the append-mode path).
    let mut file = std::fs::OpenOptions::new().create(true).read(true).append(true).open(hist_path)?;
    // WHY block here (not try_lock): compaction and other appenders hold this
    // only briefly, so waiting is preferable to failing the caller's append.
    // Released automatically when `file` drops at the end of this function.
    file.lock()?;
    // WHY merged here, inside the lock, before the caller's own line: any
    // successful locked append — from any agent, any caller — is a safe
    // opportunity to sweep up spill files left by a prior fallback, since it
    // already holds the lock merging needs. This makes self-healing automatic
    // rather than requiring a dedicated "merge spills" trigger, and ordering
    // spills before the new line keeps the file roughly chronological (a
    // spill is always older than "now").
    if let Some(dir) = hist_path.parent() {
        merge_spill_files(dir, &mut file);
    }
    file.write_all(format!("{}\n", line).as_bytes())?;
    Ok(())
}

/// Fold every pending `spill-*.jsonl` file in `hist_dir` into the already-open,
/// already-locked `file`, then delete each one that was merged successfully.
///
/// WHY spill files exist at all: `.claude/hooks/history-record.sh` previously
/// fell back to `fs.appendFileSync` straight into the shared monthly log when
/// the `comp-daemon` binary was unavailable — an unlocked "third writer" that
/// could interleave with a concurrent locked append (see this module's WHY on
/// `append_history_line`). The hook's fallback now writes its one entry to its
/// own uniquely-named `spill-<pid>-<epoch_ms>-<rand>.jsonl` file instead, which
/// needs no lock at all (nothing else ever writes that exact path), and this
/// function is what reintegrates it into the real history the next time
/// anyone appends normally.
///
/// Best-effort per file: a spill file that fails to read is logged and left
/// in place (retried on the next append); one that merges but fails to delete
/// is logged too — leaving it behind risks a harmless duplicate on the next
/// merge, which is preferable to losing the caller's own append by surfacing
/// an error here.
pub(in crate::mcp) fn merge_spill_files(hist_dir: &std::path::Path, file: &mut std::fs::File) {
    use std::io::Write;

    let entries = match std::fs::read_dir(hist_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.starts_with("spill-") && name.ends_with(".jsonl")) {
            continue;
        }
        match std::fs::read(&path) {
            Ok(content) => match file.write_all(&content) {
                Ok(()) => {
                    if let Err(e) = std::fs::remove_file(&path) {
                        log::warn!("merged spill file {} but failed to remove it: {}", path.display(), e);
                    }
                }
                Err(e) => log::warn!("failed to merge spill file {} into history: {}", path.display(), e),
            },
            Err(e) => log::warn!("failed to read spill file {}: {}", path.display(), e),
        }
    }
}

/// Rewrite `hist_path` in place, replacing its content with a compacted form,
/// while holding an *exclusive* OS advisory lock for the entire read-modify-write.
///
/// WHY in-place rewrite (seek-to-0 + write + truncate) instead of writing a new
/// file and atomically renaming it: on Windows, renaming a file over a path
/// while another process holds an open handle to the destination can fail with
/// a sharing violation, and even when it succeeds there is a window between
/// releasing the lock on the old file and renaming the new file into place,
/// where an unlocked append could land in the old file and be silently
/// discarded by the rename. Holding one exclusive lock for the full
/// read+write+truncate closes that window: no appender (which also takes an
/// exclusive lock — see append_history_line's doc for why shared doesn't work
/// on Windows) can write until this function releases the lock.
///
/// WHY a `.bak` snapshot is written before mutating: `compact_fn` is a lossy
/// summarization step; if it produces bad output there must be a manual recovery
/// path back to the pre-compaction content (checklist item: "失敗時に.bakから
/// ロールバック可能").
///
/// `compact_fn` receives the raw pre-compaction file bytes and returns the bytes to
/// write back. The compaction *policy* (dedup? drop entries older than N days? ask
/// an LLM to summarize?) is deliberately not decided here — this function only
/// guarantees the locking/atomicity contract around whatever policy is supplied.
///
/// Synchronous / blocking; manually triggered only (never called from the request
/// handling path), so callers should run it via `spawn_blocking` same as
/// `append_history_line`.
///
/// Triggered manually via the `compact-history` CLI subcommand (Phase 3, B5;
/// see `run_compact_history`), which supplies `dedup_exact_duplicate_lines` as
/// `compact_fn` — the only policy wired up so far. Never called from the
/// request-handling path.
pub(in crate::mcp) fn compact_history_file(
    hist_path: &std::path::Path,
    compact_fn: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(hist_path)?;
    // WHY blocking exclusive lock, held for the entire read+compute+write below:
    // any appender (append_history_line) also takes an exclusive lock before its
    // write, so holding this one for the full critical section guarantees no
    // append can land between "we read the old content" and "we finish writing the
    // new content" — closing the race window from Gemini review finding #2.
    file.lock()?;

    let mut original = Vec::new();
    file.read_to_end(&mut original)?;

    // Compaction *policy* is the caller's problem; abort here (file untouched, no
    // backup written) if it fails — there is nothing to roll back from yet.
    let compacted = compact_fn(&original)?;

    // Snapshot pre-compaction content before mutating anything: compact_fn is a
    // lossy step, and this is the manual recovery path if it summarized badly.
    write_generational_backup(&hist_path.with_extension("jsonl.bak"), &original)?;

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&compacted)?;
    file.set_len(compacted.len() as u64)?;

    Ok(())
}

/// Compaction policy: drop exact-duplicate lines, keeping each line's first
/// occurrence. The only policy wired up to the `compact-history` CLI
/// subcommand so far (Phase 3, B5) — deliberately conservative: it can only
/// shrink a file by removing content that is byte-for-byte already present
/// elsewhere in it, never by judging age or relevance, so it cannot discard
/// information a less conservative policy (see `compact_history_file`'s doc)
/// might.
pub(in crate::mcp) fn dedup_exact_duplicate_lines(original: &[u8]) -> Result<Vec<u8>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in String::from_utf8_lossy(original).lines() {
        if seen.insert(line.to_string()) {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
        }
    }
    Ok(out)
}

/// Run `dedup_exact_duplicate_lines` compaction against every `.comp/history/*.jsonl`
/// file under `workspace_root`. Returns `(path, bytes_before, bytes_after)` for
/// each file actually processed. An absent history directory is not an error
/// — it just means there is nothing to compact yet.
/// Like `run_compact_history`, but after the exact-duplicate pass also folds the
/// text of records older than `days` days (trace::fold_old_lines). `now_ms` is
/// injectable for tests. Same locking and `.bak` guarantees (compact_history_file).
pub(in crate::mcp) fn run_compact_history_fold(
    workspace_root: &str,
    days: u32,
    now_ms: u64,
) -> Result<Vec<(std::path::PathBuf, usize, usize)>> {
    let hist_dir = std::path::Path::new(workspace_root).join(".comp").join("history");
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(&hist_dir) {
        Ok(e) => e,
        Err(_) => return Ok(results),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let before = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
        compact_history_file(&path, |orig| {
            let deduped = dedup_exact_duplicate_lines(orig)?;
            trace::fold_old_lines(&deduped, now_ms, days)
        })?;
        let after = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
        results.push((path, before, after));
    }
    Ok(results)
}

pub(in crate::mcp) fn run_compact_history(workspace_root: &str) -> Result<Vec<(std::path::PathBuf, usize, usize)>> {
    let hist_dir = std::path::Path::new(workspace_root).join(".comp").join("history");
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(&hist_dir) {
        Ok(e) => e,
        Err(_) => return Ok(results),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let before = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
        compact_history_file(&path, dedup_exact_duplicate_lines)?;
        let after = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
        results.push((path, before, after));
    }
    Ok(results)
}

/// Write `content` to `backup_path`, first preserving any file already there
/// under a `.<epoch_ms>` generation suffix instead of silently overwriting it.
///
/// WHY: a rollback snapshot is worthless if the *next* mutating operation can
/// destroy it before anyone restores from it — `compact_history_file` and
/// `repair_history_file` both take exactly this kind of "snapshot before a
/// lossy rewrite" backup, and a second run of either used to clobber the
/// first run's only copy (Phase 2, B6). Propagated as an error rather than
/// best-effort: proceeding to overwrite the one remaining backup on a rename
/// failure would defeat the whole point of taking one.
pub(in crate::mcp) fn write_generational_backup(backup_path: &std::path::Path, content: &[u8]) -> Result<()> {
    if backup_path.exists() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let extension = backup_path.extension().and_then(|e| e.to_str()).unwrap_or("bak");
        let prior = backup_path.with_extension(format!("{}.{}", extension, ts));
        std::fs::rename(backup_path, &prior)?;
    }
    std::fs::write(backup_path, content)?;
    Ok(())
}

/// Outcome of repairing one `.jsonl` history file: how many lines parsed
/// successfully and were kept, and how many were unparseable and moved aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::mcp) struct RepairReport {
    pub(in crate::mcp) good_lines: usize,
    pub(in crate::mcp) bad_lines: usize,
}

/// Rewrite `hist_path` to contain only its successfully-parseable lines,
/// appending every unparseable line to `<hist_path>.quarantine` (created if
/// absent, never truncated — repeated repair runs accumulate quarantined
/// lines rather than losing earlier ones) instead of discarding it.
///
/// Holds the same exclusive lock for the entire read+separate+write as
/// `compact_history_file`, for the same reason: no concurrent appender must be
/// able to land between the read and the write and be silently dropped by the
/// rewrite.
///
/// WHY a dedicated function rather than reusing `compact_history_file`: that
/// function's `compact_fn` contract is "bytes in, replacement bytes out" for a
/// single file — repair has two outputs (the lines that stay, and the ones
/// that get quarantined), which does not fit that shape without capturing a
/// side channel out of a `FnOnce` closure.
///
/// A `.bak` of the pre-repair content is written first (generation-preserving,
/// like `compact_history_file`'s) — a line this function classifies as "bad"
/// might just be a schema this version of `SessionCall` doesn't know about yet,
/// and that should stay recoverable, not merely quarantined-by-value.
pub(in crate::mcp) fn repair_history_file(hist_path: &std::path::Path) -> Result<RepairReport> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(hist_path)?;
    file.lock()?;

    let mut original = Vec::new();
    file.read_to_end(&mut original)?;

    let mut good = Vec::new();
    let mut bad = Vec::new();
    let mut report = RepairReport { good_lines: 0, bad_lines: 0 };
    for line in String::from_utf8_lossy(&original).lines() {
        if line.trim().is_empty() {
            continue;
        }
        if serde_json::from_str::<SessionCall>(line).is_ok() {
            good.extend_from_slice(line.as_bytes());
            good.push(b'\n');
            report.good_lines += 1;
        } else {
            bad.extend_from_slice(line.as_bytes());
            bad.push(b'\n');
            report.bad_lines += 1;
        }
    }

    if report.bad_lines == 0 {
        // Nothing to change; skip the backup/quarantine/rewrite entirely so a
        // clean file's mtime and a never-created .bak stay undisturbed.
        return Ok(report);
    }

    write_generational_backup(&hist_path.with_extension("jsonl.bak"), &original)?;

    let quarantine_path = hist_path.with_extension("jsonl.quarantine");
    let mut quarantine_file = std::fs::OpenOptions::new().create(true).append(true).open(&quarantine_path)?;
    quarantine_file.write_all(&bad)?;

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&good)?;
    file.set_len(good.len() as u64)?;

    Ok(report)
}
