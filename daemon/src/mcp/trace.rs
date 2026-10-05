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
//!   Unparseable lines are skipped.

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
    let _ = text;
    unimplemented!()
}

pub fn tokenize(text: &str) -> Vec<String> {
    let _ = text;
    unimplemented!()
}

pub fn bm25_scores(docs: &[String], query: &str) -> Vec<f64> {
    let _ = (docs, query);
    unimplemented!()
}

pub fn recall_text(call: &SessionCall) -> String {
    let _ = call;
    unimplemented!()
}

pub fn rank_by_query(calls: &[SessionCall], query: &str, now_ms: u64) -> Vec<usize> {
    let _ = (calls, query, now_ms);
    unimplemented!()
}

pub fn delegations_for_turn(calls: &[SessionCall], turn_idx: usize) -> Vec<usize> {
    let _ = (calls, turn_idx);
    unimplemented!()
}

pub fn parent_turn_of(calls: &[SessionCall], idx: usize) -> Option<usize> {
    let _ = (calls, idx);
    unimplemented!()
}

pub fn fold_old_lines(original: &[u8], now_ms: u64, days: u32) -> anyhow::Result<Vec<u8>> {
    let _ = (original, now_ms, days);
    unimplemented!()
}

pub fn extract_turn_meta(transcript_jsonl: &str) -> TurnMeta {
    let _ = transcript_jsonl;
    unimplemented!()
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
