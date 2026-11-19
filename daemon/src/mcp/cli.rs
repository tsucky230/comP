use super::*;


/// Parsed form of a recognized CLI subcommand (`args[1..]`, i.e. excluding the
/// binary path). Kept separate from execution (`run_append_history`) so parsing
/// alone is unit-testable without touching stdin or the filesystem.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::mcp) enum CliSubcommand {
    AppendHistory { workspace_root: String, agent_id: String },
    Doctor { workspace_root: String, repair: bool },
    CompactHistory { workspace_root: String },
    /// `compact-history <ws> --fold-after-days N` (N >= 1): dedup, then trace::fold_old_lines.
    CompactHistoryFold { workspace_root: String, days: u32 },
    RecordTurn { workspace_root: Option<String> },
}

/// Parse `args` (as given to `main`, so `args[0]` is the binary path) into a
/// recognized subcommand, or `None` if `args` should fall through to normal MCP
/// server startup — either no subcommand was given, or it wasn't recognized.
///
/// # Subcommands
/// - `append-history <workspace_root> <agent_id>`
/// - `doctor <workspace_root> [--repair]`
/// - `compact-history <workspace_root>`
/// - `record-turn [workspace_root]` (Claude Code Stop hook; see record_turn.rs)
pub(in crate::mcp) fn parse_cli_subcommand(args: &[String]) -> Option<CliSubcommand> {
    if args.len() == 4 && args[1] == "append-history" {
        return Some(CliSubcommand::AppendHistory {
            workspace_root: args[2].clone(),
            agent_id: args[3].clone(),
        });
    }
    if args.len() >= 3 && args[1] == "doctor" {
        let repair = args.len() == 4 && args[3] == "--repair";
        if args.len() > 4 || (args.len() == 4 && !repair) {
            return None;
        }
        return Some(CliSubcommand::Doctor {
            workspace_root: args[2].clone(),
            repair,
        });
    }
    if args.len() == 5 && args[1] == "compact-history" && args[3] == "--fold-after-days" {
        let days = args[4].parse::<u32>().ok().filter(|d| *d >= 1)?;
        return Some(CliSubcommand::CompactHistoryFold { workspace_root: args[2].clone(), days });
    }
    if args.len() == 3 && args[1] == "compact-history" {
        return Some(CliSubcommand::CompactHistory { workspace_root: args[2].clone() });
    }
    if (args.len() == 2 || args.len() == 3) && args[1] == "record-turn" {
        return Some(CliSubcommand::RecordTurn { workspace_root: args.get(2).cloned() });
    }
    None
}

/// Read one JSON object `{ "request": string, "outcome": string | null }` from
/// `input`, build the corresponding `SessionCall` — attaching `agent_id` and the
/// current timestamp itself, never trusting the payload for either, so attribution
/// can't be spoofed by a confused or malicious caller — and append it to this
/// month's `.comp/history/log-YYYY-MM.jsonl` under `workspace_root` via
/// `append_history_line` (locked-append protocol).
///
/// `input` is injectable (any `Read`) so this is unit-testable without real stdin;
/// the CLI entry point below passes `std::io::stdin()`.
pub(in crate::mcp) fn run_append_history(
    workspace_root: &str,
    agent_id: &str,
    mut input: impl std::io::Read,
) -> Result<()> {
    // WHY a dedicated payload struct instead of deserializing straight into
    // SessionCall: SessionCall has an `agent` field, and if the stdin payload
    // could set it directly, a confused or malicious hook script could spoof
    // attribution. This struct has no `agent` field at all, so serde silently
    // ignores one if present in the input — agent_id (the CLI arg) always wins.
    //
    // The optional trace fields (YASAKANI plan B) let an orchestrator record a
    // delegation: `kind` must be "turn" or "delegation" when given, so a typo
    // cannot create records session_recall would never link.
    #[derive(serde::Deserialize)]
    struct AppendHistoryPayload {
        request: String,
        #[serde(default)]
        outcome: Option<String>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        turn_id: Option<String>,
        #[serde(default)]
        parent_turn_id: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        commit: Option<String>,
        #[serde(default)]
        test_exit: Option<i32>,
    }

    let mut buf = String::new();
    input.read_to_string(&mut buf)?;
    let payload: AppendHistoryPayload = serde_json::from_str(&buf)?;
    if let Some(kind) = payload.kind.as_deref() {
        if kind != "turn" && kind != "delegation" {
            return Err(anyhow!("'kind' must be \"turn\" or \"delegation\", got {:?}", kind));
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let call = SessionCall {
        query: trace::redact_secrets(&payload.request),
        outcome: payload.outcome.as_deref().map(trace::redact_secrets),
        symbols: Vec::new(),
        files: payload.files,
        tokens: 0,
        stale: false,
        timestamp: now,
        agent: agent_id.to_string(),
        rules: Vec::new(),
        session_id: payload.session_id,
        turn_id: payload.turn_id,
        parent_turn_id: payload.parent_turn_id,
        kind: payload.kind,
        commit: payload.commit,
        test_exit: payload.test_exit,
    };

    let month = &format_epoch_ms(now)[0..7];
    let hist_path = std::path::Path::new(workspace_root)
        .join(".comp")
        .join("history")
        .join(format!("log-{}.jsonl", month));

    let line = serde_json::to_string(&call)?;
    append_history_line(&hist_path, &line)
}

/// Handle a lightweight CLI invocation of the comp-daemon binary, as opposed to
/// starting the long-running MCP stdio server.
///
/// Returns `Some(exit_code)` if `args` matched a known subcommand — the caller
/// (main.rs) must exit immediately with that code rather than falling through to
/// normal daemon startup. Returns `None` if `args` did not match any subcommand,
/// meaning the caller should proceed with normal MCP server startup.
///
/// WHY `append-history` exists as a CLI subcommand rather than requiring
/// callers to speak JSON-RPC over stdio: .claude/hooks/history-record.sh fires
/// once per Claude Code turn and needs a fast, one-shot write — spinning up
/// the full MCP server (GraphDB open + background indexing, ~48s) for that
/// would be wrong. See append_history_line's doc for why this path must be the
/// only JSONL writer. `doctor` is a CLI subcommand for the same reason: a
/// manual health check/repair should not require (or trigger) a full startup.
///
/// Thin by design: real parsing lives in `parse_cli_subcommand`, real logic in
/// `run_append_history` / `run_doctor` — all independently unit-tested. This
/// function only wires them to real stdin/stdout and an exit code, which is
/// why it isn't itself covered by a dedicated unit test.
pub fn try_run_cli_subcommand(args: &[String]) -> Option<i32> {
    match parse_cli_subcommand(args)? {
        CliSubcommand::AppendHistory { workspace_root, agent_id } => {
            match run_append_history(&workspace_root, &agent_id, std::io::stdin()) {
                Ok(()) => Some(0),
                Err(e) => {
                    eprintln!("append-history failed: {}", e);
                    Some(1)
                }
            }
        }
        CliSubcommand::Doctor { workspace_root, repair } => {
            match run_doctor(&workspace_root, repair) {
                Ok(report) => {
                    println!("{}", report.render(&workspace_root, repair));
                    Some(if report.is_healthy(repair) { 0 } else { 1 })
                }
                Err(e) => {
                    eprintln!("doctor failed: {}", e);
                    Some(1)
                }
            }
        }
        CliSubcommand::RecordTurn { workspace_root } => {
            // WHY reading stdin leniently: a Stop hook must never block or crash the
            // session; unreadable stdin becomes an empty string, which
            // run_record_turn reports as Failed (exit 1, non-blocking).
            let mut stdin_json = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin_json);
            let env_project_dir = std::env::var("CLAUDE_PROJECT_DIR").ok();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let result = record_turn::run_record_turn(
                workspace_root.as_deref(),
                env_project_dir.as_deref(),
                &stdin_json,
                now_ms,
                &mut |path, line| append_history_line(path, line),
            );
            match &result {
                record_turn::RecordResult::Recorded { .. } => {}
                record_turn::RecordResult::NothingToRecord { reason } => {
                    eprintln!("record-turn: nothing to record ({})", reason);
                }
                record_turn::RecordResult::Failed { reason, spill } => match spill {
                    Some(p) => eprintln!("record-turn failed: {} (saved to {})", reason, p.display()),
                    None => eprintln!("record-turn failed: {}", reason),
                },
            }
            Some(record_turn::exit_code(&result))
        }
        CliSubcommand::CompactHistoryFold { workspace_root, days } => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            match run_compact_history_fold(&workspace_root, days, now_ms) {
                Ok(results) => {
                    for (path, before, after) in &results {
                        println!("compact-history: {} — {} -> {} bytes (folded > {} days)", path.display(), before, after, days);
                    }
                    Some(0)
                }
                Err(e) => {
                    eprintln!("compact-history failed: {}", e);
                    Some(1)
                }
            }
        }
        CliSubcommand::CompactHistory { workspace_root } => {
            match run_compact_history(&workspace_root) {
                Ok(results) => {
                    if results.is_empty() {
                        println!("compact-history: no .jsonl files found under {}/.comp/history", workspace_root);
                    }
                    for (path, before, after) in &results {
                        println!("compact-history: {} — {} -> {} bytes", path.display(), before, after);
                    }
                    Some(0)
                }
                Err(e) => {
                    eprintln!("compact-history failed: {}", e);
                    Some(1)
                }
            }
        }
    }
}

/// Result of one `doctor` inspection pass across the three persistent stores
/// under `.comp/` (session-memory, history, index.db).
///
/// WHY this is the lightweight path (not `GraphDB::new`): `GraphDB::new` runs
/// schema migrations and main.rs's startup spawns a full background re-index
/// after it (~48s on this repo). `doctor` must stay a fast, non-intrusive
/// diagnostic — it only ever reads `index.db` via a bare `PRAGMA quick_check`,
/// and only quarantines it (never reopens/recreates it) when `--repair` is
/// given, leaving recreation to the next real daemon startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::mcp) struct DoctorReport {
    pub(in crate::mcp) session_memory_files_ok: usize,
    pub(in crate::mcp) session_memory_files_corrupt: usize,
    pub(in crate::mcp) history_lines_ok: usize,
    pub(in crate::mcp) history_lines_corrupt: usize,
    /// "ok" | "missing" | "corrupt" | "corrupt_and_quarantined" | "check_failed"
    /// ("check_failed": the database is busy/locked by another process right
    /// now — inconclusive, never treated as corruption, never quarantined).
    pub(in crate::mcp) index_db_status: &'static str,
}

impl DoctorReport {
    /// Human-readable report for the CLI. Not JSON: `doctor` is a terminal
    /// diagnostic tool, not an MCP tool response.
    pub(in crate::mcp) fn render(&self, workspace_root: &str, repair: bool) -> String {
        let mode = if repair { "repair" } else { "report-only" };
        format!(
            "comP doctor ({mode}) — {root}\n  session-memory: {mem_ok} ok, {mem_bad} corrupt\n  history: {hist_ok} line(s) ok, {hist_bad} line(s) corrupt\n  index.db: {db}",
            mode = mode,
            root = workspace_root,
            mem_ok = self.session_memory_files_ok,
            mem_bad = self.session_memory_files_corrupt,
            hist_ok = self.history_lines_ok,
            hist_bad = self.history_lines_corrupt,
            db = self.index_db_status,
        )
    }

    /// Whether this report represents a state with no outstanding, unresolved
    /// corruption. With `repair` set, anything found was already quarantined,
    /// so only a hard failure to even inspect `index.db` counts as unhealthy —
    /// and "check_failed" (busy/locked, not confirmed corrupt) counts as
    /// unhealthy too: it means doctor could not actually determine the
    /// database's health, which must not be reported as "fine".
    pub(in crate::mcp) fn is_healthy(&self, repair: bool) -> bool {
        if self.index_db_status == "corrupt" || self.index_db_status == "check_failed" {
            return false;
        }
        repair || (self.session_memory_files_corrupt == 0 && self.history_lines_corrupt == 0)
    }
}

/// Classify the outcome of running `PRAGMA quick_check` against `index.db`
/// into `"ok"`, `"check_failed"`, or `"corrupt"` (never `"missing"` or
/// `"corrupt_and_quarantined"` — those are decided by the caller).
///
/// WHY this is its own function rather than inlined into `run_doctor`: the
/// busy/locked-vs-corrupt distinction is a real correctness requirement (a
/// database merely busy from routine concurrent use by another comp-daemon
/// process must never be mistaken for corrupt and quarantined out from under
/// whoever is using it), but provoking a real `SQLITE_BUSY` deterministically
/// in a test means actually holding a lock across the 5-second
/// `busy_timeout`. Testing this classification against a synthetic
/// `rusqlite::Error` is instant and exercises the same decision.
pub(in crate::mcp) fn classify_quick_check_result(check: &std::result::Result<String, rusqlite::Error>) -> &'static str {
    match check {
        Ok(s) if s == "ok" => "ok",
        Err(rusqlite::Error::SqliteFailure(e, _))
            if matches!(e.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) =>
        {
            "check_failed"
        }
        _ => "corrupt",
    }
}

/// Inspect (and, if `repair` is true, quarantine/fix) the three `.comp/`
/// stores. See `DoctorReport`'s doc for why this never opens `index.db` via
/// the full `GraphDB`.
pub(in crate::mcp) fn run_doctor(workspace_root: &str, repair: bool) -> Result<DoctorReport> {
    let comp_dir = std::path::Path::new(workspace_root).join(".comp");

    let mut session_memory_files_ok = 0usize;
    let mut session_memory_files_corrupt = 0usize;
    let memory_dir = comp_dir.join("session-memory");
    if let Ok(entries) = std::fs::read_dir(&memory_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let parses = std::fs::read(&p)
                .ok()
                .map(|raw| serde_json::from_slice::<SessionMemory>(&raw).is_ok())
                .unwrap_or(false);
            if parses {
                session_memory_files_ok += 1;
            } else {
                session_memory_files_corrupt += 1;
                if repair {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    let quarantine = p.with_extension(format!("corrupt-{}.json", ts));
                    if let Err(e) = std::fs::rename(&p, &quarantine) {
                        log::warn!("doctor: failed to quarantine {}: {}", p.display(), e);
                    }
                }
            }
        }
    }

    let mut history_lines_ok = 0usize;
    let mut history_lines_corrupt = 0usize;
    let hist_dir = comp_dir.join("history");
    if let Ok(entries) = std::fs::read_dir(&hist_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if repair {
                match repair_history_file(&p) {
                    Ok(r) => {
                        history_lines_ok += r.good_lines;
                        history_lines_corrupt += r.bad_lines;
                    }
                    Err(e) => log::warn!("doctor: failed to repair {}: {}", p.display(), e),
                }
            } else if let Ok(content) = std::fs::read_to_string(&p) {
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if serde_json::from_str::<SessionCall>(line).is_ok() {
                        history_lines_ok += 1;
                    } else {
                        history_lines_corrupt += 1;
                    }
                }
            }
        }
    }

    let db_path = comp_dir.join("index.db");
    let index_db_status: &'static str = if !db_path.exists() {
        "missing"
    } else {
        use rusqlite::Connection;
        // WHY set busy_timeout before the check: a bare Connection::open has no
        // timeout (SQLite fails immediately with SQLITE_BUSY by default), and
        // index.db is routinely open concurrently by another comp-daemon
        // process (the VSCode extension's daemon, or another MCP client's).
        // Without this, a database that is merely busy at this exact instant
        // would look identical to a corrupt one below.
        let check = Connection::open(&db_path).and_then(|conn| {
            conn.execute_batch("PRAGMA busy_timeout=5000;")?;
            conn.query_row::<String, _, _>("PRAGMA quick_check", [], |row| row.get(0))
        });
        match classify_quick_check_result(&check) {
            "corrupt" if repair => {
                crate::graph::GraphDB::quarantine_db_files(&db_path);
                "corrupt_and_quarantined"
            }
            other => other,
        }
    };

    Ok(DoctorReport {
        session_memory_files_ok,
        session_memory_files_corrupt,
        history_lines_ok,
        history_lines_corrupt,
        index_db_status,
    })
}

/// Format a Unix-epoch millisecond timestamp as "YYYY-MM-DD HH:MM" in UTC.
///
/// WHY: session_recall must show *when* past work happened so resumed work can be
/// placed in time. We compute the civil date by hand (Howard Hinnant's days-from-civil
/// algorithm) to avoid pulling in a date crate for this single formatting need.
pub(in crate::mcp) fn format_epoch_ms(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, m, d, hour, minute)
}
