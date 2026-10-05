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
//! - request: whichever comes last in the transcript of (a) a `type == "last-prompt"`
//!   line with a non-blank `lastPrompt`, or (b) a `type == "user"` line that is not
//!   a sidechain/meta line and whose `message.content` is a string or holds `text`
//!   blocks (tool_result-only lines are skipped). WHY last-wins rather than
//!   last-prompt-always: Claude Code writes the last-prompt line after the user
//!   line, so if the Stop hook ran before the newest last-prompt line landed, a
//!   fixed preference would record the previous turn's prompt. Text blocks that
//!   consist only of a
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

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

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

static SPILL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Exit code for a result: Recorded/NothingToRecord → 0, Failed → 1. Never 2.
pub fn exit_code(result: &RecordResult) -> i32 {
    match result {
        RecordResult::Recorded { .. } | RecordResult::NothingToRecord { .. } => 0,
        RecordResult::Failed { .. } => 1,
    }
}

fn truncate_chars(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect()
    }
}

fn is_sidechain_or_meta(obj: &serde_json::Map<String, Value>) -> bool {
    obj.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
        || obj.get("isMeta").and_then(|v| v.as_bool()) == Some(true)
}

fn is_system_reminder_only(text: &str) -> bool {
    const CLOSE: &str = "</system-reminder>";
    let t = text.trim();
    // The first closing tag must be the end, so "<sr>a</sr> body <sr>b</sr>" keeps its body.
    t.starts_with("<system-reminder>")
        && t.len().checked_sub(CLOSE.len()).is_some_and(|end| t.find(CLOSE) == Some(end))
}

fn extract_text_from_content(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Value::Object(map) = item {
                    if map.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                            if !is_system_reminder_only(text) {
                                parts.push(text);
                            }
                        }
                    }
                }
            }
            if parts.is_empty() {
                return None;
            }
            let joined = parts.join("\n").trim().to_string();
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    }
}

fn extract_user_request(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let message = obj.get("message")?;
    let content = message.get("content")?;
    extract_text_from_content(content)
}

fn extract_assistant_outcome(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let message = obj.get("message")?;
    let content = message.get("content")?;
    extract_text_from_content(content)
}

/// Extract the turn from transcript JSONL text (rules in the module doc).
/// Unparseable lines are skipped. None when no request is found.
pub fn extract_turn(transcript_jsonl: &str, last_assistant_message: Option<&str>) -> Option<TurnEntry> {
    let mut request: Option<String> = None;
    let mut outcome: Option<String> = None;

    for line in transcript_jsonl.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let val: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let obj = match val.as_object() {
            Some(o) => o,
            None => continue,
        };

        if obj.get("type").and_then(|v| v.as_str()) == Some("last-prompt") {
            if let Some(text) = obj.get("lastPrompt").and_then(|v| v.as_str()) {
                let t = text.trim();
                if !t.is_empty() {
                    request = Some(t.to_string());
                }
            }
        }

        if obj.get("type").and_then(|v| v.as_str()) == Some("user") {
            if is_sidechain_or_meta(obj) {
                continue;
            }
            if let Some(text) = extract_user_request(obj) {
                if !text.is_empty() {
                    request = Some(text);
                }
            }
        }

        if obj.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(text) = extract_assistant_outcome(obj) {
                if !text.is_empty() {
                    outcome = Some(text);
                }
            }
        }
    }

    let request = truncate_chars(request?, REQUEST_MAX_CHARS);

    let outcome = if let Some(s) = last_assistant_message {
        let t = s.trim();
        if !t.is_empty() {
            Some(t.to_string())
        } else {
            outcome
        }
    } else {
        outcome
    };
    let outcome = outcome.and_then(|s| {
        let t = truncate_chars(s, OUTCOME_MAX_CHARS);
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });

    Some(TurnEntry { request, outcome })
}

fn resolve_workspace(arg: Option<&str>, env: Option<&str>, cwd: Option<&str>) -> Option<String> {
    [arg, env, cwd]
        .iter()
        .copied()
        .flatten()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn make_spill_path(hist_dir: &Path, now_ms: u64) -> PathBuf {
    let pid = process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = SPILL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let rand = nanos.wrapping_add(counter as u128);
    hist_dir.join(format!("spill-{}-{}-{:x}.jsonl", pid, now_ms, rand))
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
    let stdin: Value = match serde_json::from_str(stdin_json) {
        Ok(v) => v,
        Err(e) => {
            return RecordResult::Failed {
                reason: format!("invalid stdin JSON: {}", e),
                spill: None,
            };
        }
    };
    let stdin_obj = match stdin.as_object() {
        Some(o) => o,
        None => {
            return RecordResult::Failed {
                reason: "stdin is not a JSON object".into(),
                spill: None,
            };
        }
    };

    let cwd = stdin_obj
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let workspace = match resolve_workspace(workspace_arg, env_project_dir, cwd) {
        Some(w) => w,
        None => {
            return RecordResult::Failed {
                reason: "no workspace".into(),
                spill: None,
            };
        }
    };

    let transcript_path = stdin_obj
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let transcript_path = match transcript_path {
        Some(p) => p,
        None => {
            return RecordResult::NothingToRecord {
                reason: "no transcript_path".into(),
            };
        }
    };

    let transcript = match fs::read_to_string(transcript_path) {
        Ok(t) => t,
        Err(e) => {
            return RecordResult::NothingToRecord {
                reason: format!("cannot read transcript: {}", e),
            };
        }
    };

    let last_assistant_message = stdin_obj
        .get("last_assistant_message")
        .and_then(|v| v.as_str());

    let entry = match extract_turn(&transcript, last_assistant_message) {
        Some(e) => e,
        None => {
            return RecordResult::NothingToRecord {
                reason: "no request found".into(),
            };
        }
    };

    let month = &super::format_epoch_ms(now_ms)[0..7];
    let hist_path = PathBuf::from(&workspace)
        .join(".comp")
        .join("history")
        .join(format!("log-{}.jsonl", month));

    let call = super::SessionCall {
        query: entry.request,
        outcome: entry.outcome,
        symbols: Vec::new(),
        files: Vec::new(),
        tokens: 0,
        stale: false,
        timestamp: now_ms,
        agent: AGENT_ID.to_string(),
    };

    let line = match serde_json::to_string(&call) {
        Ok(s) => s,
        Err(e) => {
            return RecordResult::Failed {
                reason: format!("serialization failed: {}", e),
                spill: None,
            };
        }
    };

    if let Err(e) = append(&hist_path, &line) {
        let hist_dir = hist_path.parent().unwrap_or_else(|| Path::new("."));
        let _ = fs::create_dir_all(hist_dir);
        let spill_path = make_spill_path(hist_dir, now_ms);
        match fs::write(&spill_path, format!("{}\n", line)) {
            Ok(_) => {
                return RecordResult::Failed {
                    reason: format!("append failed: {}", e),
                    spill: Some(spill_path),
                };
            }
            Err(_) => {
                return RecordResult::Failed {
                    reason: format!("append failed: {}", e),
                    spill: None,
                };
            }
        }
    }

    RecordResult::Recorded { hist_path }
}

#[cfg(test)]
#[path = "record_turn_tests.rs"]
mod tests;
