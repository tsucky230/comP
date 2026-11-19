use super::*;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct SessionCall {
    /// The Stop hook (history-record.sh) writes this field as "request";
    /// session_log writes "query". Accept both so hook-written JSONL lines
    /// are not silently dropped during recall.
    #[serde(alias = "request")]
    pub query: String,
    /// What was done in response (set by session_log / Stop-hook records).
    /// Defaulted for backward compatibility with older session-memory.json files
    /// and auto-recorded run_pipeline/get_context calls that have no outcome.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Hook-written records carry only timestamp/request/outcome —
    /// everything else must default rather than fail deserialization.
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub stale: bool,
    pub timestamp: u64,
    /// Which MCP client/LLM produced this record (e.g. "claude-code", "codex").
    /// WHY default rather than Option: pre-migration records and any client that
    /// doesn't set COMP_AGENT_ID have no way to know their origin — "unknown" keeps
    /// session_recall's formatting code from needing to unwrap a missing value.
    #[serde(default = "default_agent")]
    pub agent: String,
    /// Rule-sharing (beta) sections handed out with this call, by reference; the
    /// text is in `.comp/rules/<hash>.md`. Omitted from the JSON when empty so
    /// records without rules keep their previous shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleRef>,
    /// Multi-agent trace (YASAKANI plan B): the agent session this record came
    /// from (Claude Code's session_id), so delegations can be linked to the turn
    /// that ran them without the orchestrator knowing its own turn id. See
    /// trace::delegations_for_turn. All trace fields are omitted when None so
    /// older records keep their shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Stable id of a conversation turn (transcript uuid of the request line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Explicit parent turn of a delegation; None means "resolve by session and time".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
    /// "turn" (record-turn) or "delegation" (append-history from an orchestrator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// git HEAD when the record was written: the outcome as a fact, not a summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Exit code of the delegation's test command (0 = accepted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_exit: Option<i32>,
}

/// Reference to one rule section returned by run_pipeline (see crate::rules).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RuleRef {
    pub file: String,
    pub heading: String,
    pub hash: String,
}

/// Token budget for run_pipeline's `related_rules`, separate from `max_tokens`.
pub const RELATED_RULES_MAX_TOKENS: usize = 1000;

pub(in crate::mcp) fn default_agent() -> String {
    "unknown".to_string()
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Session {
    pub id: String,
    pub timestamp: u64,
    pub calls: Vec<SessionCall>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SessionMemory {
    pub sessions: Vec<Session>,
}

/// Keep only ASCII alphanumerics, `-`, and `_` from `agent_id`; replace everything
/// else with `_`. WHY: agent_id ultimately comes from the COMP_AGENT_ID env var
/// (set by each MCP client's launch config) and is used as a filename component —
/// this prevents a malformed value (path separators, "..") from escaping the
/// session-memory directory. Agent ids are drawn from a small, known set defined
/// by AgentSetup.ts, so Windows-reserved names (CON, NUL, ...) are out of scope.
pub(in crate::mcp) fn sanitize_agent_id(agent_id: &str) -> String {
    agent_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Path to this agent's own session-memory cache file.
///
/// WHY per-agent rather than one shared file: comP is a stdio MCP server, so each
/// MCP client (Claude Code, Cursor, a future Codex integration, ...) spawns its own
/// comp-daemon subprocess against the same workspace (see AgentSetup.ts). A single
/// shared session-memory.json would be read-modify-written by multiple independent
/// OS processes with no coordination — lost updates, or a full reset to empty on a
/// torn-write parse failure (see docs/ARCHITECTURE_ja.md's session-memory ADR).
/// Splitting by agent id makes each file single-writer by construction, so no
/// locking is needed for this layer at all.
pub(in crate::mcp) fn get_session_memory_path(root: &str, agent_id: &str) -> std::path::PathBuf {
    std::path::Path::new(root)
        .join(".comp")
        .join("session-memory")
        .join(format!("{}.json", sanitize_agent_id(agent_id)))
}

/// Render a backtick-quoted, comma-separated list capped at `cap` items,
/// with an overflow marker ("… (+N more)") instead of the full enumeration.
///
/// WHY: recall entries auto-recorded by run_pipeline can carry 30-50 symbols
/// and files each; enumerating them all makes session_recall output so long
/// it defeats its own purpose (fast context reconstruction, low tokens).
pub(in crate::mcp) fn format_capped_list(items: &[String], cap: usize) -> String {
    let shown: Vec<String> = items.iter().take(cap).map(|s| format!("`{}`", s)).collect();
    let mut out = shown.join(", ");
    if items.len() > cap {
        out.push_str(&format!(" … (+{} more)", items.len() - cap));
    }
    out
}

/// Path to the sidecar lock file guarding read-modify-write access to a
/// session-memory file.
///
/// WHY a sidecar rather than locking `path` itself: the write step
/// (`write_session_memory_atomically`) ends with a same-directory rename onto
/// `path`, and on Windows a `LockFileEx`-based lock (what `File::lock()` uses)
/// held on the destination can block replacing it — the rename must run with
/// `path` completely unlocked. Locking a separate `.lock` file for the whole
/// critical section still serializes concurrent writers (see
/// `record_mcp_call`) without ever touching `path`'s own lock state.
pub(in crate::mcp) fn session_memory_lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".lock");
    std::path::PathBuf::from(s)
}

/// Load `path` as a `SessionMemory`, or quarantine it and start fresh if it
/// exists but is not valid JSON.
///
/// WHY quarantine instead of the previous `unwrap_or(empty)`: session-memory
/// has no other copy anywhere — silently resetting a file that failed to parse
/// (e.g. after a crash mid-write, before this function's caller made writes
/// atomic) permanently destroyed every past record for that agent with no
/// warning to anyone. Renaming the unreadable file aside keeps the bytes
/// available for manual recovery while still letting the caller proceed.
///
/// WHY a plain I/O error from `std::fs::read` (permission denied, the path
/// transiently locked by another process, etc.) is propagated as `Err` rather
/// than also treated as "empty": unlike a parse failure, an I/O error says
/// nothing about whether the file's *content* is bad — it may well be a
/// perfectly intact document that just couldn't be read this instant. Treating
/// that as empty would let the caller's atomic write overwrite a healthy file
/// with a near-empty one once the transient condition clears, recreating the
/// exact data-loss bug this function exists to close via a different trigger
/// (Gemini acceptance-check finding, Phase 1).
pub(in crate::mcp) fn load_session_memory_or_quarantine(path: &std::path::Path) -> Result<SessionMemory> {
    if !path.exists() {
        return Ok(SessionMemory { sessions: Vec::new() });
    }
    let raw = std::fs::read(path)?;
    match serde_json::from_slice(&raw) {
        Ok(memory) => Ok(memory),
        Err(e) => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let quarantine = path.with_extension(format!("corrupt-{}.json", ts));
            log::warn!(
                "session-memory file {} is not valid JSON ({}); quarantining to {} and starting empty",
                path.display(), e, quarantine.display()
            );
            if let Err(rename_err) = std::fs::rename(path, &quarantine) {
                log::warn!(
                    "failed to quarantine corrupt session-memory file {}: {}",
                    path.display(), rename_err
                );
            }
            Ok(SessionMemory { sessions: Vec::new() })
        }
    }
}

/// Persist `memory` to `path` atomically: write to a temp file in the same
/// directory, flush+fsync it, then rename it into place.
///
/// WHY: the previous implementation called `File::create(&path)` directly,
/// which truncates immediately — a crash or panic partway through
/// `to_writer_pretty` left `path` holding a truncated, unparseable JSON
/// document indistinguishable from real corruption on the next read. A
/// same-directory rename is atomic on both POSIX and Windows (same volume), so
/// `path` only ever contains either the previous complete document or the new
/// complete one, never a partial write.
pub(in crate::mcp) fn write_session_memory_atomically(path: &std::path::Path, memory: &SessionMemory) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("session-memory path {} has no parent directory", path.display()))?;
    let tmp_path = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name().and_then(|f| f.to_str()).unwrap_or("session-memory"),
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    ));

    // WHY cleanup on Err here too, not just on the rename failure below: a
    // failure partway through writing the temp file itself (serialize error,
    // or an I/O error from flush/fsync) previously left the half-written tmp
    // file behind forever, since only the rename step's failure path removed
    // it (Gemini acceptance-check finding, Phase 1).
    if let Err(e) = write_tmp_session_memory(&tmp_path, memory) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // WHY retry a few times: a small, non-zero chance exists that some other
    // process (e.g. session_recall's plain, unlocked read) has `path` open at
    // the exact instant of rename. Rust opens files on Windows with
    // FILE_SHARE_DELETE, so this is a transient condition, not a lock conflict
    // (this function never takes a lock on `path` itself — only its `.lock`
    // sidecar, held by the caller — precisely so the rename target is never
    // locked by us).
    let mut attempt = 0u32;
    loop {
        match std::fs::rename(&tmp_path, path) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 4 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 * attempt as u64));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(anyhow!("failed to persist session-memory to {}: {}", path.display(), e));
            }
        }
    }
}

/// Write `memory` as pretty JSON to `tmp_path`, fully flushed and fsynced.
/// Isolated from the rename step so `write_session_memory_atomically` can
/// clean up `tmp_path` on any failure here without duplicating the writer
/// setup at each call site.
pub(in crate::mcp) fn write_tmp_session_memory(tmp_path: &std::path::Path, memory: &SessionMemory) -> Result<()> {
    use std::io::Write;
    let file = std::fs::File::create(tmp_path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, memory)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

pub(in crate::mcp) fn record_mcp_call(
    workspace_root: &str,
    agent_id: &str,
    session_id: &str,
    query: String,
    symbols: Vec<String>,
    files: Vec<String>,
    tokens: u64,
) -> Result<()> {
    record_mcp_call_with_rules(workspace_root, agent_id, session_id, query, symbols, files, tokens, Vec::new())
}

/// record_mcp_call plus the rule-sharing sections handed out with the call.
#[allow(clippy::too_many_arguments)]
pub(in crate::mcp) fn record_mcp_call_with_rules(
    workspace_root: &str,
    agent_id: &str,
    session_id: &str,
    query: String,
    symbols: Vec<String>,
    files: Vec<String>,
    tokens: u64,
    rules: Vec<RuleRef>,
) -> Result<()> {
    let path = get_session_memory_path(workspace_root, agent_id);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Exclusive lock over the whole read-modify-write below: without it, two
    // independent processes sharing the same agent_id (e.g. two Claude Code
    // windows on this workspace) can both read the same starting state and
    // each write back a version missing the other's call — a lost update. The
    // per-agent split this file lives under only makes the *common* case
    // single-writer; this lock covers the case where that assumption doesn't
    // hold.
    let lock_path = session_memory_lock_path(&path);
    // WHY .append(true) rather than .write(true): this handle's only purpose is
    // to hold the OS lock below — nothing is ever written through it — so
    // .append avoids clippy::suspicious_open_options' ambiguity over whether a
    // plain .write(true) without .truncate(...) is intentional. Matches the
    // same .read(true) requirement documented on append_history_line's lock
    // handle (Windows needs read access on a handle to call .lock()).
    let lock_file = std::fs::OpenOptions::new().create(true).read(true).append(true).open(&lock_path)?;
    lock_file.lock()?;

    let mut memory = load_session_memory_or_quarantine(&path)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut found = false;
    for session in &mut memory.sessions {
        if session.id == session_id {
            session.calls.push(SessionCall {
                query: query.clone(),
                outcome: None,
                symbols: symbols.clone(),
                files: files.clone(),
                tokens,
                stale: false,
                timestamp: now,
                agent: agent_id.to_string(),
                rules: rules.clone(),
                ..Default::default()
            });
            found = true;
            break;
        }
    }

    if !found {
        memory.sessions.push(Session {
            id: session_id.to_string(),
            timestamp: now,
            calls: vec![SessionCall {
                query,
                outcome: None,
                symbols,
                files,
                tokens,
                stale: false,
                timestamp: now,
                agent: agent_id.to_string(),
                rules,
                ..Default::default()
            }],
        });
    }

    write_session_memory_atomically(&path, &memory)
    // `lock_file` drops here, releasing the sidecar lock.
}

/// Mark `stale: true` on every call in the session-memory file at `path`
/// whose `files` list contains `relative_path` (both sides normalized to `/`
/// separators). Returns how many calls were newly marked; a result of 0 never
/// touches the file (no lock contention, no write, for agents whose history
/// doesn't mention this path at all).
///
/// WHY this exists in the daemon now rather than in `SessionMemoryManager.ts`:
/// that TS class read and wrote `.comp/session-memory.json` directly — the
/// single file the per-agent split (v0.11.1) replaced, so every call it made
/// was writing to a file `session_recall` never reads. Routing the mutation
/// through the daemon via the `mark_stale` RPC means it goes through the same
/// sidecar-lock + atomic-write discipline as `record_mcp_call`, instead of a
/// second, divergent implementation in another language touching the same
/// class of file (Phase 3, A7/A9).
pub(in crate::mcp) fn mark_stale_in_file(path: &std::path::Path, relative_path: &str) -> Result<usize> {
    let lock_path = session_memory_lock_path(path);
    let lock_file = std::fs::OpenOptions::new().create(true).read(true).append(true).open(&lock_path)?;
    lock_file.lock()?;

    let mut memory = load_session_memory_or_quarantine(path)?;
    let mut marked = 0usize;
    for session in &mut memory.sessions {
        for call in &mut session.calls {
            if !call.stale && call.files.iter().any(|f| f.replace('\\', "/") == relative_path) {
                call.stale = true;
                marked += 1;
            }
        }
    }
    if marked > 0 {
        write_session_memory_atomically(path, &memory)?;
    }
    Ok(marked)
    // `lock_file` drops here, releasing the sidecar lock.
}

/// Latest `timestamp` across every call recorded in any per-agent
/// session-memory file or history jsonl line under `workspace_root`, or
/// `None` if nothing has ever been recorded.
///
/// WHY this exists: `SidebarPanel.ts` used to show "last agent connection
/// time" by instantiating `SessionMemoryManager` and reading
/// `.comp/session-memory.json` directly — again, the single pre-v0.11.1 file
/// nothing writes to anymore, so the UI silently showed "Waiting..." forever
/// (Phase 3, A7). Exposing the real answer via `get_stats` lets the UI read it
/// from a response it already fetches, with no file access of its own.
///
/// Read-only and deliberately tolerant of unreadable files/lines (same
/// best-effort posture as `handle_session_recall`) — this is a UI
/// nice-to-have, not a correctness-critical path.
pub(in crate::mcp) fn latest_activity_timestamp(workspace_root: &str) -> Option<u64> {
    let mut latest: Option<u64> = None;

    let memory_dir = std::path::Path::new(workspace_root).join(".comp").join("session-memory");
    if let Ok(entries) = std::fs::read_dir(&memory_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = std::fs::read(&p) {
                if let Ok(memory) = serde_json::from_slice::<SessionMemory>(&raw) {
                    for session in &memory.sessions {
                        for call in &session.calls {
                            latest = Some(latest.map_or(call.timestamp, |cur| cur.max(call.timestamp)));
                        }
                    }
                }
            }
        }
    }

    let hist_dir = std::path::Path::new(workspace_root).join(".comp").join("history");
    if let Ok(entries) = std::fs::read_dir(&hist_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                for line in content.lines() {
                    if let Ok(c) = serde_json::from_str::<SessionCall>(line) {
                        latest = Some(latest.map_or(c.timestamp, |cur| cur.max(c.timestamp)));
                    }
                }
            }
        }
    }

    latest
}
