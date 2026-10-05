//! `comp-daemon record-turn [workspace_root]` — Claude Code の Stop フックから呼ばれ、
//! 会話の1往復（最後の依頼と応答）を `.comp/history/` へ記録する。
//!
//! WHY the binary itself, not a bash/node hook script: comP's setup writes this
//! command into every project's `.claude/settings.local.json`, so it must work
//! on machines without bash or node (plain Windows). The extracting logic used
//! to live in `.claude/hooks/history-record.sh`, which only ever ran inside the
//! comP repository and read the transcript's top-level `content` (the real
//! format nests it under `message.content`), so nothing was recorded anywhere.
//!
//! # Contract
//! - stdin: the Stop hook JSON (`transcript_path`, `cwd`, `last_assistant_message`, ...).
//! - workspace: the CLI argument, else env `CLAUDE_PROJECT_DIR`, else stdin `cwd`.
//! - request: `lastPrompt` of the last `type == "last-prompt"` line; if none, the
//!   text of the last `type == "user"` line that is not a sidechain/meta line and
//!   whose `message.content` is a string or holds `text` blocks (tool_result-only
//!   lines are skipped). Text blocks that consist only of a
//!   `<system-reminder>...</system-reminder>` element are dropped; the rest are
//!   joined with "\n" and trimmed.
//! - outcome: stdin `last_assistant_message` when non-empty after trim, else the
//!   joined text blocks of the last `type == "assistant"` line, else None.
//! - request is cut to 600 chars, outcome to 400 chars (chars, not bytes).
//! - agent is always "claude-code"; nothing in stdin or the transcript can set it.
//! - exit code (see `exit_code`): 0 = recorded, or nothing to record (no
//!   transcript_path, transcript file missing, no request found). 1 = failure
//!   (no workspace, stdin not JSON, append failed). Never 2: Claude Code treats
//!   exit 2 from a Stop hook as "block the stop", which would trap the session.
//! - on append failure the entry is written to
//!   `<workspace>/.comp/history/spill-<pid>-<epoch_ms>-<rand>.jsonl` (one
//!   SessionCall JSON line) before returning Failed, so the next locked append
//!   merges it (see `merge_spill_files`).

use std::path::{Path, PathBuf};

/// Agent id written for every record; matches AgentSetup.agentIdFor("Claude Code").
pub const AGENT_ID: &str = "claude-code";
pub const REQUEST_MAX_CHARS: usize = 600;
pub const OUTCOME_MAX_CHARS: usize = 400;

/// The request/outcome pair extracted from one turn, already truncated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEntry {
    pub request: String,
    pub outcome: Option<String>,
}

/// What one `record-turn` run ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordResult {
    /// One line was appended to `hist_path`.
    Recorded { hist_path: PathBuf },
    /// Nothing to record; the reason is for stderr/logging only.
    NothingToRecord { reason: String },
    /// Something went wrong. `spill` is the spill file written, if any.
    Failed { reason: String, spill: Option<PathBuf> },
}

/// Exit code for a result: Recorded/NothingToRecord → 0, Failed → 1. Never 2.
pub fn exit_code(result: &RecordResult) -> i32 {
    let _ = result;
    todo!()
}

/// Extract the turn from transcript JSONL text (rules in the module doc).
/// Unparseable lines are skipped. None when no request is found.
pub fn extract_turn(transcript_jsonl: &str, last_assistant_message: Option<&str>) -> Option<TurnEntry> {
    let _ = (transcript_jsonl, last_assistant_message);
    todo!()
}

/// Run one record-turn invocation.
///
/// `append(hist_path, line)` performs the locked append; production passes
/// `append_history_line`, tests inject failures. `now_ms` is the timestamp
/// written into the record and used for the monthly file name
/// (`log-YYYY-MM.jsonl`, UTC) and the spill name.
pub fn run_record_turn(
    workspace_arg: Option<&str>,
    env_project_dir: Option<&str>,
    stdin_json: &str,
    now_ms: u64,
    append: &mut dyn FnMut(&Path, &str) -> anyhow::Result<()>,
) -> RecordResult {
    let _ = (workspace_arg, env_project_dir, stdin_json, now_ms, append);
    todo!()
}

#[cfg(test)]
#[path = "record_turn_tests.rs"]
mod tests;
