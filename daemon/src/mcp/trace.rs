//! Trace helpers for multi-agent tracing (YASAKANI plan B, stages 2-3): pure
//! functions behind record-turn, append-history, session_log, session_recall and
//! compact-history. Kept free of I/O so each rule is unit-testable.
//!
//! WHY a separate module: mod.rs is already ~6k lines; these rules (redaction,
//! ranking, turn/delegation linking, folding) are independent of the MCP plumbing.
//!
//! # Contract
//!
//! `redact_secrets(text)` replaces secrets with `[REDACTED]` and leaves everything
//! else byte-for-byte unchanged. Idempotent.
//!   1. Known key shapes, anywhere in the text (the whole match is replaced):
//!      `sk-[A-Za-z0-9_-]{20,}` (covers sk-proj-, sk-ant-), `AIza[0-9A-Za-z_-]{35}`,
//!      `gh[pousr]_[A-Za-z0-9]{36,}`, `github_pat_[A-Za-z0-9_]{22,}`,
//!      `xox[abposr]-[A-Za-z0-9-]{10,}`, `AKIA[0-9A-Z]{16}`.
//!   2. Assignments: a name matching (case-insensitive, as a whole word)
//!      `api_key|api-key|apikey|secret|token|password|passwd|access_key|access-key`,
//!      then optional spaces, `:` or `=`, optional spaces, an optional `"` or `'`,
//!      then a value of 16+ chars from `[A-Za-z0-9_\-./+]`. Only the value is
//!      replaced, and only when it contains at least one ASCII digit and the char
//!      right after it is not `(` (a function call, not a secret). WHY these
//!      limits: plain code such as `api_key = config.get("key")` or
//!      `token = read_token_from_env()` must survive (Gemini review G5).
//!
//! `tokenize(text)` lowercases, then splits into tokens in order of appearance:
//!   - a maximal run of ASCII letters/digits/`_` of length >= 2 is one token
//!     (shorter runs are dropped);
//!   - a maximal run of CJK chars (Hiragana U+3040-309F, Katakana U+30A0-30FF,
//!     CJK Ext A U+3400-4DBF, CJK Unified U+4E00-9FFF, halfwidth Katakana
//!     U+FF66-FF9F) yields, for each char c_i: c_i, then c_i c_{i+1} when a next
//!     char exists (unigrams so a one-char query still matches; bigrams for
//!     precision — Japanese has no spaces to split on);
//!   - everything else separates tokens. Duplicates are kept (they are term counts).
//!
//! `bm25_scores(docs, query)` returns one Okapi BM25 score per doc (k1 = 1.2,
//! b = 0.75, idf = ln(1 + (N - df + 0.5) / (df + 0.5))), summing over the
//! *distinct* query tokens. Empty docs or a query with no tokens score 0.0.
//!
//! `recall_text(call)` = query, outcome (if any) and files joined with "\n" — the
//! text a recall query is matched against.
//!
//! `rank_by_query(calls, query, now_ms)` returns indices into `calls`:
//!   - blank query (after trim): every index, newest first;
//!   - otherwise group A = calls whose query or outcome contains the trimmed query
//!     (case-insensitive substring, the previous behaviour), newest first; then
//!     group B = the other calls whose BM25 score over `recall_text` is > 0,
//!     ordered by score x 0.5^(age_days / 90) descending (age_days =
//!     max(0, now_ms - timestamp) / 86_400_000). WHY substring hits first: every
//!     call the old filter found stays found, in the same order.
//!   - ties (same timestamp in A, same weighted score in B) keep the smaller
//!     index first; "newest first" means larger timestamp first.
//!
//! `delegations_for_turn(calls, turn_idx)` lists the delegation records that a
//! turn produced, oldest first (ties: smaller index first). Empty unless
//! `calls[turn_idx].kind == Some("turn")`. A call d (with `kind ==
//! Some("delegation")`) belongs to turn t when either
//!   - explicit: `d.parent_turn_id` is Some and equals `t.turn_id` (Some); or
//!   - implicit: `d.parent_turn_id` is None, `d.session_id` is Some and equals
//!     `t.session_id`, and prev < d.timestamp <= t.timestamp, where prev is the
//!     largest timestamp of another `kind == Some("turn")` call with the same
//!     session_id and a timestamp < t.timestamp (0 when there is none). WHY: the
//!     orchestrator cannot know its own turn id mid-turn (Gemini review G1); the
//!     Stop hook records the turn after the delegations it contains.
//!
//! `parent_turn_of(calls, idx)` is the inverse: the index of the turn whose
//! `delegations_for_turn` contains idx (the first such turn by index), or None.
//!
//! `fold_old_lines(original, now_ms, days)` is a compact-history policy. `days ==
//! 0` is an error. Blank lines are dropped. A line that is not a JSON object is
//! kept verbatim. An object whose numeric `timestamp` < now_ms - days * 86_400_000
//! has each of its string fields `query`, `request`, `outcome` that is longer than
//! FOLD_CHARS chars cut to its first FOLD_CHARS - 1 chars followed by "…" (so the
//! result is exactly FOLD_CHARS chars and folding again changes nothing); other
//! fields keep their values. A line that needs no change is kept byte-for-byte.
//! Every output line ends with "\n". Folded text is no longer searchable; the
//! caller keeps a .bak (Gemini review G4, accepted trade-off: capacity).
//!
//! `extract_turn_meta(transcript_jsonl)` returns
//!   - `request_uuid`: the `uuid` string of the last `type == "user"` line that is
//!     not sidechain/meta and whose `message.content` is a non-blank string or has
//!     a `text` block that is not only a `<system-reminder>` (the same lines
//!     record_turn treats as requests); None when there is none or it has no uuid;
//!   - `files`: from `type == "assistant"` lines after that user line (the whole
//!     transcript when there is none), excluding sidechain lines, each
//!     `tool_use` block named Edit, Write or MultiEdit with a string
//!     `input.file_path`, or NotebookEdit with a string `input.notebook_path`;
//!     deduplicated keeping first occurrence, at most MAX_TOUCHED_FILES.
//!
//! `extract_turn_meta` skips unparseable lines.

use super::SessionCall;

pub const FOLD_CHARS: usize = 120;
pub const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;
pub const BM25_K1: f64 = 1.2;
pub const BM25_B: f64 = 0.75;
pub const MAX_TOUCHED_FILES: usize = 50;
pub const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnMeta {
    pub request_uuid: Option<String>,
    pub files: Vec<String>,
}

pub fn redact_secrets(text: &str) -> String {
    // Compiled once: redaction runs on every recorded turn.
    static KEYS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ASSIGNMENTS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let keys = KEYS.get_or_init(|| {
        regex::Regex::new(
            r"sk-[A-Za-z0-9_-]{20,}|AIza[0-9A-Za-z_-]{35}|gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,}|xox[abposr]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}",
        )
        .expect("constant key regex is valid")
    });
    let redacted = keys.replace_all(text, REDACTED);
    let assignments = ASSIGNMENTS.get_or_init(|| {
        regex::Regex::new(
            r#"(?i:\b(?:api_key|api-key|apikey|secret|token|password|passwd|access_key|access-key)\b)[ \t]*[:=][ \t]*["']?([A-Za-z0-9_./+\-]{16,})"#,
        )
        .expect("constant assignment regex is valid")
    });
    // Replace only the captured value; code-like identifiers and calls survive.
    let mut output = String::new();
    let mut end = 0;
    for captures in assignments.captures_iter(&redacted) {
        let value = captures.get(1).expect("value capture is present");
        if value.as_str().bytes().any(|b| b.is_ascii_digit())
            && redacted.as_bytes().get(value.end()) != Some(&b'(')
        {
            output.push_str(&redacted[end..value.start()]);
            output.push_str(REDACTED);
            end = value.end();
        }
    }
    output.push_str(&redacted[end..]);
    output
}

pub fn tokenize(text: &str) -> Vec<String> {
    fn is_cjk(c: char) -> bool {
        matches!(c as u32, 0x3040..=0x309f | 0x30a0..=0x30ff |
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xff66..=0xff9f)
    }
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        if chars[i].is_ascii_alphanumeric() || chars[i] == '_' {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i - start >= 2 {
                tokens.push(chars[start..i].iter().collect());
            }
        } else if is_cjk(chars[i]) {
            while i < chars.len() && is_cjk(chars[i]) {
                i += 1;
            }
            for j in start..i {
                tokens.push(chars[j].to_string());
                if j + 1 < i {
                    tokens.push(chars[j..j + 2].iter().collect());
                }
            }
        } else {
            i += 1;
        }
    }
    tokens
}

pub fn bm25_scores(docs: &[String], query: &str) -> Vec<f64> {
    use std::collections::{BTreeSet, HashMap};
    let query_tokens: BTreeSet<String> = tokenize(query).into_iter().collect();
    let mut scores = vec![0.0; docs.len()];
    if docs.is_empty() || query_tokens.is_empty() {
        return scores;
    }
    let counts: Vec<(usize, HashMap<String, usize>)> = docs
        .iter()
        .map(|doc| {
            let tokens = tokenize(doc);
            let len = tokens.len();
            let mut counts = HashMap::new();
            for token in tokens {
                *counts.entry(token).or_insert(0) += 1;
            }
            (len, counts)
        })
        .collect();
    let average = counts.iter().map(|(len, _)| *len as f64).sum::<f64>() / docs.len() as f64;
    if average == 0.0 {
        return scores;
    }
    for token in query_tokens {
        let df = counts
            .iter()
            .filter(|(_, terms)| terms.contains_key(&token))
            .count() as f64;
        let idf = (1.0 + (docs.len() as f64 - df + 0.5) / (df + 0.5)).ln();
        for (i, (len, terms)) in counts.iter().enumerate() {
            let tf = *terms.get(&token).unwrap_or(&0) as f64;
            if tf > 0.0 {
                scores[i] += idf * tf * (BM25_K1 + 1.0)
                    / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * *len as f64 / average));
            }
        }
    }
    scores
}

pub fn recall_text(call: &SessionCall) -> String {
    let mut parts = vec![call.query.as_str()];
    if let Some(outcome) = &call.outcome {
        parts.push(outcome);
    }
    parts.extend(call.files.iter().map(String::as_str));
    parts.join("\n")
}

pub fn rank_by_query(calls: &[SessionCall], query: &str, now_ms: u64) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let newest_first = |a: &usize, b: &usize| {
        calls[*b]
            .timestamp
            .cmp(&calls[*a].timestamp)
            .then_with(|| a.cmp(b))
    };
    if query.is_empty() {
        let mut indices: Vec<usize> = (0..calls.len()).collect();
        indices.sort_by(newest_first);
        return indices;
    }
    let docs: Vec<String> = calls.iter().map(recall_text).collect();
    let scores = bm25_scores(&docs, &query);
    let mut substring_hits = Vec::new();
    let mut ranked_hits = Vec::new();
    for (i, call) in calls.iter().enumerate() {
        if call.query.to_lowercase().contains(&query)
            || call
                .outcome
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&query)
        {
            substring_hits.push(i);
        } else if scores[i] > 0.0 {
            let age_days = now_ms.saturating_sub(call.timestamp) as f64 / 86_400_000.0;
            ranked_hits.push((
                i,
                scores[i] * 0.5_f64.powf(age_days / RECENCY_HALF_LIFE_DAYS),
            ));
        }
    }
    substring_hits.sort_by(newest_first);
    ranked_hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    substring_hits.extend(ranked_hits.into_iter().map(|(i, _)| i));
    substring_hits
}

pub fn delegations_for_turn(calls: &[SessionCall], turn_idx: usize) -> Vec<usize> {
    let Some(turn) = calls
        .get(turn_idx)
        .filter(|c| c.kind.as_deref() == Some("turn"))
    else {
        return Vec::new();
    };
    let prev = calls
        .iter()
        .filter(|c| {
            c.kind.as_deref() == Some("turn")
                && c.session_id == turn.session_id
                && c.timestamp < turn.timestamp
        })
        .map(|c| c.timestamp)
        .max()
        .unwrap_or(0);
    let mut indices: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            if d.kind.as_deref() != Some("delegation") {
                return None;
            }
            let belongs = if let Some(parent) = &d.parent_turn_id {
                turn.turn_id.as_ref() == Some(parent)
            } else {
                d.session_id.is_some()
                    && d.session_id == turn.session_id
                    && prev < d.timestamp
                    && d.timestamp <= turn.timestamp
            };
            belongs.then_some(i)
        })
        .collect();
    indices.sort_by(|a, b| {
        calls[*a]
            .timestamp
            .cmp(&calls[*b].timestamp)
            .then_with(|| a.cmp(b))
    });
    indices
}

pub fn parent_turn_of(calls: &[SessionCall], idx: usize) -> Option<usize> {
    if idx >= calls.len() || calls[idx].kind.as_deref() != Some("delegation") {
        return None;
    }
    // O(n): session_recall calls this for every delegation it shows, so scanning
    // delegations_for_turn for each turn (O(n^2)) would not scale with history size.
    let d = &calls[idx];
    let is_turn = |c: &SessionCall| c.kind.as_deref() == Some("turn");
    if let Some(parent) = &d.parent_turn_id {
        return calls.iter().position(|c| is_turn(c) && c.turn_id.as_ref() == Some(parent));
    }
    d.session_id.as_ref()?;
    // The turn whose window (prev, t] holds d is the earliest same-session turn at or
    // after d; ties go to the smaller index, as delegations_for_turn's caller order would.
    calls
        .iter()
        .enumerate()
        .filter(|(_, c)| is_turn(c) && c.session_id == d.session_id && c.timestamp >= d.timestamp)
        .min_by(|(ia, a), (ib, b)| a.timestamp.cmp(&b.timestamp).then_with(|| ia.cmp(ib)))
        .map(|(i, _)| i)
}

pub fn fold_old_lines(original: &[u8], now_ms: u64, days: u32) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(days != 0, "days must be greater than zero");
    let cutoff = now_ms.saturating_sub(u64::from(days) * 86_400_000);
    let mut output = Vec::new();
    for line in original.split(|b| *b == b'\n') {
        if std::str::from_utf8(line).is_ok_and(|s| s.trim().is_empty()) {
            continue;
        }
        let mut changed = false;
        let mut value = serde_json::from_slice::<serde_json::Value>(line).ok();
        if let Some(obj) = value.as_mut().and_then(serde_json::Value::as_object_mut) {
            let old = obj.get("timestamp").is_some_and(|ts| {
                ts.as_u64()
                    .map(|n| n < cutoff)
                    .unwrap_or_else(|| ts.as_f64().is_some_and(|n| n < cutoff as f64))
            });
            if old {
                for key in ["query", "request", "outcome"] {
                    if let Some(field) = obj.get_mut(key) {
                        if let Some(text) = field.as_str() {
                            if text.chars().count() > FOLD_CHARS {
                                let mut folded: String =
                                    text.chars().take(FOLD_CHARS - 1).collect();
                                folded.push('…');
                                *field = serde_json::Value::String(folded);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if changed {
            output.extend(serde_json::to_vec(&value.expect("changed object exists"))?);
        } else {
            output.extend_from_slice(line);
        }
        output.push(b'\n');
    }
    Ok(output)
}

pub fn extract_turn_meta(transcript_jsonl: &str) -> TurnMeta {
    use serde_json::Value;
    fn reminder_only(text: &str) -> bool {
        const CLOSE: &str = "</system-reminder>";
        let t = text.trim();
        t.starts_with("<system-reminder>")
            && t.len()
                .checked_sub(CLOSE.len())
                .is_some_and(|end| t.find(CLOSE) == Some(end))
    }
    // Mirror record_turn's content extraction, including joining text blocks.
    fn request_text(content: &Value) -> Option<String> {
        let text = match content {
            Value::String(s) => s.trim().to_string(),
            Value::Array(items) => items
                .iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    if obj.get("type").and_then(Value::as_str) != Some("text") {
                        return None;
                    }
                    obj.get("text")
                        .and_then(Value::as_str)
                        .filter(|s| !reminder_only(s))
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string(),
            _ => return None,
        };
        (!text.is_empty() && !reminder_only(&text)).then_some(text)
    }
    let mut meta = TurnMeta::default();
    for line in transcript_jsonl.lines() {
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let sidechain = obj.get("isSidechain").and_then(Value::as_bool) == Some(true);
        let is_meta = obj.get("isMeta").and_then(Value::as_bool) == Some(true);
        let kind = obj.get("type").and_then(Value::as_str);
        let content = obj.get("message").and_then(|m| m.get("content"));
        if kind == Some("user")
            && !sidechain
            && !is_meta
            && content.and_then(request_text).is_some()
        {
            meta.request_uuid = obj.get("uuid").and_then(Value::as_str).map(String::from);
            // Even a request without a UUID starts a new turn's file collection.
            meta.files.clear();
        } else if kind == Some("assistant") && !sidechain {
            if let Some(items) = content.and_then(Value::as_array) {
                for item in items {
                    if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let field = match item.get("name").and_then(Value::as_str) {
                        Some("Edit" | "Write" | "MultiEdit") => "file_path",
                        Some("NotebookEdit") => "notebook_path",
                        _ => continue,
                    };
                    if let Some(path) = item
                        .get("input")
                        .and_then(|i| i.get(field))
                        .and_then(Value::as_str)
                    {
                        if meta.files.len() < MAX_TOUCHED_FILES
                            && !meta.files.iter().any(|p| p == path)
                        {
                            meta.files.push(path.to_string());
                        }
                    }
                }
            }
        }
    }
    meta
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
