//! Rule sharing (beta): read the instruction files that agents keep in a
//! repository, hand an agent the sections written for *other* agents that
//! matter for its task, flag pairs of sections that may contradict, and
//! snapshot what was handed out so later records show the exact text.
//!
//! Part of the multi-agent trace plan (docs/dev/MULTI_AGENT_TRACE_ja.md, T2).
//!
//! # Prompt-injection boundaries (the contract every function here keeps)
//! - Only files inside the workspace, only the fixed list in [`owner_of`], and
//!   only those `git ls-files` reports (git-tracked). Nothing under the home
//!   directory is ever read: paths come from git, relative to the workspace.
//! - A path with an absolute form or a `..` component is ignored. At read time
//!   the file is canonicalized and must still lie under the canonical
//!   workspace (a symlink swapped in after listing is refused). Files larger
//!   than [`MAX_FILE_BYTES`] are skipped.
//! - Returned text has control characters removed (except `\n` and `\t`).
//! - Off unless `.comp/config.json` has `ruleSharing.enabled == true` (the
//!   JSON boolean); see [`rule_sharing_enabled`].

use std::path::{Path, PathBuf};

/// Files larger than this are skipped (not truncated).
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Sections shorter than this (characters, after trimming) never become conflict candidates.
pub const MIN_CONFLICT_CHARS: usize = 40;
/// Minimum TF-IDF cosine similarity for a conflict candidate.
pub const CONFLICT_SIMILARITY: f64 = 0.3;
/// Minimum number of distinct shared terms for a conflict candidate.
pub const MIN_SHARED_TERMS: usize = 2;

/// English words ignored when building terms.
pub const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "into", "is", "it",
    "of", "on", "or", "the", "to", "with", "this", "that", "these", "those",
];

// Terms (used by relevant_rules and conflict_candidates):
// - ASCII: maximal runs of [a-z0-9] after lowercasing, 2+ characters, not in STOPWORDS.
// - CJK: maximal runs of Hiragana (U+3040–309F), Katakana (U+30A0–30FF, incl. U+30FC),
//   and CJK Unified Ideographs (U+4E00–9FFF); each run yields its character bigrams
//   (a single-character run yields that character).
// - IDF(t) = ln(1 + N / df(t)), N = number of sections given, df = sections containing t
//   (in heading or text). Never zero, so a term present everywhere still counts.

/// One heading-delimited section of an instruction file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSection {
    /// Workspace-relative path with `/` separators, e.g. `.cursor/rules/style.mdc`.
    pub file: String,
    /// Agent id that reads this file natively (see [`owner_of`]).
    pub owner: String,
    /// Heading text without the leading `#`s; empty for text before the first heading.
    pub heading: String,
    /// Body text (trimmed, control characters removed).
    pub text: String,
    /// Lowercase hex SHA-256 of `heading + "\n" + text`.
    pub hash: String,
}

/// Two sections from files with different owners that may contradict.
#[derive(Debug, Clone, PartialEq)]
pub struct ConflictPair {
    /// The pair is ordered so that `(a.file, a.heading) < (b.file, b.heading)`.
    pub a: RuleSection,
    pub b: RuleSection,
    pub similarity: f64,
}

/// The agent id that natively reads `rel_path`, or None when it is not an instruction file.
///
/// | Path (workspace-relative, `/` separators, case-sensitive) | Owner |
/// |---|---|
/// | `CLAUDE.md`, `.claude/CLAUDE.md` | `claude-code` |
/// | `AGENTS.md` | `codex` |
/// | `GEMINI.md` | `gemini-cli` |
/// | `.github/copilot-instructions.md` | `github-copilot` |
/// | `CONVENTIONS.md` | `aider` |
/// | `.clinerules`, `.clinerules/<anything>.md` | `cline` |
/// | `.windsurfrules` | `windsurf` |
/// | `.cursor/rules/<anything>.md` or `.mdc` (any depth) | `cursor` |
///
/// Backslashes are treated as `/`. Absolute paths and paths with a `..`
/// component return None.
pub fn owner_of(rel_path: &str) -> Option<&'static str> {
    let _ = rel_path;
    todo!()
}

/// Run `git ls-files -z` in `workspace` and return the tracked paths.
/// Err(reason) when git is missing or `workspace` is not inside a repository.
pub fn git_tracked_files(workspace: &Path) -> Result<Vec<String>, String> {
    let _ = workspace;
    todo!()
}

/// Read one instruction file. Ok(None) when it must be skipped: missing,
/// larger than [`MAX_FILE_BYTES`], not valid UTF-8, or resolving (after
/// canonicalization) outside the canonical workspace.
pub fn read_rule_file(workspace: &Path, rel_path: &str) -> std::io::Result<Option<String>> {
    let _ = (workspace, rel_path);
    todo!()
}

/// Split one file into sections.
///
/// - A heading is a line of 1–3 `#`, a space, then text (trailing `#`s and
///   spaces removed), outside fenced code. `####` and deeper stay in the body.
/// - A fence opens on a line starting (after up to 3 spaces) with ``` or `~~~`
///   and closes on the next line starting (after up to 3 spaces) with the same
///   three characters. An unclosed fence makes the rest of the file body text.
/// - A leading YAML front matter block (`---` on the first line … next `---`)
///   is dropped.
/// - Text before the first heading becomes a section with an empty heading.
/// - Sections whose body is empty after trimming are dropped.
/// - Heading and body have control characters removed (see [`sanitize`]).
pub fn parse_sections(file: &str, owner: &str, text: &str) -> Vec<RuleSection> {
    let _ = (file, owner, text);
    todo!()
}

/// Remove control characters except `\n` and `\t`.
pub fn sanitize(text: &str) -> String {
    let _ = text;
    todo!()
}

/// All sections of every git-tracked instruction file in `workspace`.
/// `tracked` supplies the git file list (production: [`git_tracked_files`]);
/// its Err is returned unchanged. Files are read with [`read_rule_file`].
pub fn load_sections(
    workspace: &Path,
    tracked: &dyn Fn(&Path) -> Result<Vec<String>, String>,
) -> Result<Vec<RuleSection>, String> {
    let _ = (workspace, tracked);
    todo!()
}

/// Sections relevant to `task`, for the agent `agent_id`, within `max_tokens`.
///
/// - Sections owned by `agent_id` are excluded (it already reads its own file).
/// - Score = sum of IDF (over all given sections) of the distinct task terms
///   found in the section's heading or text. Terms: lowercase ASCII words of 2+
///   characters, plus character bigrams of CJK runs; common English stopwords
///   are ignored. Score 0 is excluded.
/// - Sections with the same hash are kept once (the first in input order).
/// - Highest score first (ties: input order). Sections are added while the
///   running total of `count_tokens(heading + "\n" + text)` stays within
///   `max_tokens`; a section that would exceed it is skipped and later, smaller
///   ones may still be added.
pub fn relevant_rules(
    sections: &[RuleSection],
    task: &str,
    agent_id: &str,
    max_tokens: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Vec<RuleSection> {
    let _ = (sections, task, agent_id, max_tokens, count_tokens);
    todo!()
}

/// Pairs of sections that may contradict, for an LLM to judge.
///
/// Vectors: raw term counts (heading + text) times IDF, over the given sections.
///
/// Only pairs whose owners differ, whose hashes differ, where both texts are
/// at least [`MIN_CONFLICT_CHARS`] characters, that share at least
/// [`MIN_SHARED_TERMS`] distinct terms, and whose TF-IDF cosine similarity is
/// at least [`CONFLICT_SIMILARITY`]. Highest similarity first (ties: by
/// `(a.file, a.heading, b.file, b.heading)`), at most `max_pairs`.
pub fn conflict_candidates(sections: &[RuleSection], max_pairs: usize) -> Vec<ConflictPair> {
    let _ = (sections, max_pairs);
    todo!()
}

/// Save `section` as `<workspace>/.comp/rules/<hash>.md` unless it exists.
/// Content: `<!-- file: <file> -->`, `<!-- heading: <heading> -->`, a blank
/// line, then the text and a trailing newline. Written through a temporary
/// file and rename. Returns the path.
pub fn snapshot(workspace: &Path, section: &RuleSection) -> std::io::Result<PathBuf> {
    let _ = (workspace, section);
    todo!()
}

/// True only when `<workspace>/.comp/config.json` parses and has
/// `ruleSharing.enabled` equal to the JSON boolean `true`.
pub fn rule_sharing_enabled(workspace: &Path) -> bool {
    let _ = workspace;
    todo!()
}

#[cfg(test)]
mod tests;
