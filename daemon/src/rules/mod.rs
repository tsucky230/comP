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
use std::collections::{BTreeMap, BTreeSet};
use sha2::{Digest, Sha256};

/// Files larger than this are skipped (not truncated).
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Sections shorter than this (characters, after trimming) never become conflict candidates.
pub const MIN_CONFLICT_CHARS: usize = 40;
/// Minimum TF-IDF cosine similarity for a conflict candidate.
pub const CONFLICT_SIMILARITY: f64 = 0.3;
/// Minimum number of distinct shared terms for a conflict candidate.
pub const MIN_SHARED_TERMS: usize = 2;
/// At or above this similarity two sections say the same thing (see conflict_candidates).
pub const DUPLICATE_SIMILARITY: f64 = 0.999;

/// English words ignored when building terms.
pub const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "into", "is", "it",
    "of", "on", "or", "the", "to", "with", "this", "that", "these", "those",
];

// Terms (used by relevant_rules and conflict_candidates):
// - ASCII: maximal runs of [a-z0-9_] after lowercasing, with leading/trailing `_` removed,
//   2+ characters, not in STOPWORDS. `_` is kept so identifiers such as `run_pipeline`
//   stay one term instead of matching every text that says "run".
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
    let path = rel_path.replace('\\', "/");
    if path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
        || path.split('/').any(|part| part == "..")
    {
        return None;
    }
    match path.as_str() {
        "CLAUDE.md" | ".claude/CLAUDE.md" => Some("claude-code"),
        "AGENTS.md" => Some("codex"),
        "GEMINI.md" => Some("gemini-cli"),
        ".github/copilot-instructions.md" => Some("github-copilot"),
        "CONVENTIONS.md" => Some("aider"),
        ".clinerules" => Some("cline"),
        ".windsurfrules" => Some("windsurf"),
        _ if path.starts_with(".clinerules/") && path.ends_with(".md") => Some("cline"),
        _ if path.starts_with(".cursor/rules/")
            && (path.ends_with(".md") || path.ends_with(".mdc")) => Some("cursor"),
        _ => None,
    }
}

/// Run `git ls-files -z` in `workspace` and return the tracked paths.
/// Err(reason) when git is missing or `workspace` is not inside a repository.
pub fn git_tracked_files(workspace: &Path) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("git ls-files: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout.split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).replace('\\', "/"))
        .collect())
}

/// Read one instruction file. Ok(None) when it must be skipped: missing,
/// larger than [`MAX_FILE_BYTES`], not valid UTF-8, or resolving (after
/// canonicalization) outside the canonical workspace.
pub fn read_rule_file(workspace: &Path, rel_path: &str) -> std::io::Result<Option<String>> {
    if owner_of(rel_path).is_none() {
        return Ok(None);
    }
    let root = workspace.canonicalize()?;
    let path = match workspace.join(rel_path.replace('\\', "/")).canonicalize() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !path.starts_with(&root) {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    // Bound the read as well as checking metadata, in case the file grows.
    use std::io::Read;
    let mut bytes = Vec::new();
    (&mut file).take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok().map(|text| sanitize(&text)))
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
/// - Thematic-break lines (only `-`, `*` or `_`, 3 or more, optionally spaced) at the
///   start or end of a body are removed, so a `---` separator does not change a hash.
/// - Sections whose body is empty after trimming are dropped.
/// - Heading and body have control characters removed (see [`sanitize`]).
pub fn parse_sections(file: &str, owner: &str, text: &str) -> Vec<RuleSection> {
    let lines: Vec<&str> = text.lines().collect();
    let start = if lines.first() == Some(&"---") {
        lines.iter().skip(1).position(|line| *line == "---")
            .map(|index| index + 2).unwrap_or(0)
    } else {
        0
    };
    let mut sections = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    let mut fence: Option<&str> = None;
    for line in &lines[start..] {
        let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
        let marker = if indentation <= 3 {
            let trimmed = &line[indentation..];
            if trimmed.starts_with("```") { Some("```") }
            else if trimmed.starts_with("~~~") { Some("~~~") }
            else { None }
        } else {
            None
        };
        if let Some(open) = fence {
            if marker == Some(open) {
                fence = None;
            }
        } else if let Some(marker) = marker {
            fence = Some(marker);
        } else {
            let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
            if (1..=3).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
                push_section(&mut sections, file, owner, &heading, &body);
                heading = sanitize(line[hashes + 1..].trim_end_matches(['#', ' ']).trim());
                body.clear();
                continue;
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    push_section(&mut sections, file, owner, &heading, &body);
    sections
}

/// Remove control characters except `\n` and `\t`.
pub fn sanitize(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t').collect()
}

/// All sections of every git-tracked instruction file in `workspace`.
/// `tracked` supplies the git file list (production: [`git_tracked_files`]);
/// its Err is returned unchanged. Files are read with [`read_rule_file`].
pub fn load_sections(
    workspace: &Path,
    tracked: &dyn Fn(&Path) -> Result<Vec<String>, String>,
) -> Result<Vec<RuleSection>, String> {
    let mut sections = Vec::new();
    for file in tracked(workspace)? {
        let file = file.replace('\\', "/");
        if let Some(owner) = owner_of(&file) {
            match read_rule_file(workspace, &file) {
                Ok(Some(text)) => sections.extend(parse_sections(&file, owner, &text)),
                Ok(None) => {},
                Err(error) => return Err(format!("{file}: {error}")),
            }
        }
    }
    Ok(sections)
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
    let (counts, idf) = term_corpus(sections);
    let task_terms = term_counts(task);
    let mut seen = BTreeSet::new();
    let mut ranked = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        if !seen.insert(&section.hash) || section.owner == agent_id {
            continue;
        }
        let score: f64 = task_terms.keys()
            .filter(|term| counts[index].contains_key(*term))
            .map(|term| idf[term]).sum();
        if score > 0.0 {
            ranked.push((index, score));
        }
    }
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut remaining = max_tokens;
    let mut result = Vec::new();
    for (index, _) in ranked {
        let section = &sections[index];
        let tokens = count_tokens(&format!("{}\n{}", section.heading, section.text));
        if tokens <= remaining {
            remaining -= tokens;
            result.push(section.clone());
        }
    }
    result
}

/// Pairs of sections that may contradict, for an LLM to judge.
///
/// Vectors: raw term counts (heading + text) times IDF, over the given sections.
///
/// Pairs whose similarity is at least [`DUPLICATE_SIMILARITY`] use the same
/// content words (they differ only in punctuation or stopwords) and are
/// agreement, not conflict, so they are left out.
///
/// Only pairs whose owners differ, whose hashes differ, where both texts are
/// at least [`MIN_CONFLICT_CHARS`] characters, that share at least
/// [`MIN_SHARED_TERMS`] distinct terms, and whose TF-IDF cosine similarity is
/// at least [`CONFLICT_SIMILARITY`]. Highest similarity first (ties: by
/// `(a.file, a.heading, b.file, b.heading)`), at most `max_pairs`.
pub fn conflict_candidates(sections: &[RuleSection], max_pairs: usize) -> Vec<ConflictPair> {
    if max_pairs == 0 {
        return Vec::new();
    }
    let (counts, idf) = term_corpus(sections);
    let vectors: Vec<BTreeMap<String, f64>> = counts.iter().map(|terms| {
        terms.iter().map(|(term, count)| (term.clone(), *count as f64 * idf[term])).collect()
    }).collect();
    let norms: Vec<f64> = vectors.iter()
        .map(|vector| vector.values().map(|value| value * value).sum::<f64>().sqrt())
        .collect();
    let mut pairs = Vec::new();
    for (i, a) in sections.iter().enumerate() {
        if a.text.trim().chars().count() < MIN_CONFLICT_CHARS {
            continue;
        }
        for (j, b) in sections.iter().enumerate().skip(i + 1) {
            if a.owner == b.owner || a.hash == b.hash
                || b.text.trim().chars().count() < MIN_CONFLICT_CHARS {
                continue;
            }
            let mut shared = 0;
            let mut dot = 0.0;
            for (term, value) in &vectors[i] {
                if let Some(other) = vectors[j].get(term) {
                    shared += 1;
                    dot += value * other;
                }
            }
            let similarity = if norms[i] == 0.0 || norms[j] == 0.0 {
                0.0
            } else {
                (dot / (norms[i] * norms[j])).clamp(0.0, 1.0)
            };
            if shared >= MIN_SHARED_TERMS
                && (CONFLICT_SIMILARITY..DUPLICATE_SIMILARITY).contains(&similarity)
            {
                let (a, b) = if (&a.file, &a.heading) < (&b.file, &b.heading) {
                    (a, b)
                } else if (&b.file, &b.heading) < (&a.file, &a.heading) {
                    (b, a)
                } else {
                    // Equal keys cannot meet ConflictPair's strict ordering contract.
                    continue;
                };
                pairs.push(ConflictPair { a: a.clone(), b: b.clone(), similarity });
            }
        }
    }
    pairs.sort_by(|a, b| b.similarity.total_cmp(&a.similarity).then_with(|| {
        (&a.a.file, &a.a.heading, &a.b.file, &a.b.heading)
            .cmp(&(&b.a.file, &b.a.heading, &b.b.file, &b.b.heading))
    }));
    pairs.truncate(max_pairs);
    pairs
}

/// Save `section` as `<workspace>/.comp/rules/<hash>.md` unless it exists.
/// Content: `<!-- file: <file> -->`, `<!-- heading: <heading> -->`, a blank
/// line, then the text and a trailing newline. Written through a temporary
/// file and rename. Returns the path.
pub fn snapshot(workspace: &Path, section: &RuleSection) -> std::io::Result<PathBuf> {
    let directory = workspace.join(".comp").join("rules");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.md", section.hash));
    if path.exists() {
        return Ok(path);
    }
    let temporary = directory.join(format!("{}.md.tmp-{}", section.hash, std::process::id()));
    let content = format!("<!-- file: {} -->\n<!-- heading: {} -->\n\n{}\n",
        section.file, section.heading, section.text);
    std::fs::write(&temporary, content)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

/// True only when `<workspace>/.comp/config.json` parses and has
/// `ruleSharing.enabled` equal to the JSON boolean `true`.
pub fn rule_sharing_enabled(workspace: &Path) -> bool {
    let Ok(bytes) = std::fs::read(workspace.join(".comp").join("config.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value.get("ruleSharing").and_then(|rules| rules.get("enabled"))
        .and_then(serde_json::Value::as_bool) == Some(true)
}

// Commit only nonempty, sanitized sections, hashing exactly the returned content.
fn push_section(sections: &mut Vec<RuleSection>, file: &str, owner: &str, heading: &str, body: &str) {
    let heading = sanitize(heading);
    let text = trim_thematic_breaks(sanitize(body).trim());
    if text.is_empty() {
        return;
    }
    let hash = format!("{:x}", Sha256::digest(format!("{heading}\n{text}").as_bytes()));
    sections.push(RuleSection {
        file: file.replace('\\', "/"), owner: owner.to_owned(), heading, text, hash,
    });
}

// Drop thematic-break lines (---, ***, ___, spaced variants) from both ends of a body.
fn trim_thematic_breaks(body: &str) -> String {
    fn is_break(line: &str) -> bool {
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        compact.len() >= 3
            && (compact.chars().all(|c| c == '-') || compact.chars().all(|c| c == '*')
                || compact.chars().all(|c| c == '_'))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut start = 0;
    let mut end = lines.len();
    while start < end && (lines[start].trim().is_empty() || is_break(lines[start])) {
        start += 1;
    }
    while end > start && (lines[end - 1].trim().is_empty() || is_break(lines[end - 1])) {
        end -= 1;
    }
    lines[start..end].join("\n")
}

// Tokenize maximal ASCII and CJK runs independently; delimiters flush each run.
fn term_counts(text: &str) -> BTreeMap<String, usize> {
    fn cjk(ch: char) -> bool {
        matches!(ch, '\u{3040}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}')
    }
    let mut counts = BTreeMap::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            let mut word = String::from(ch.to_ascii_lowercase());
            while chars.peek().is_some_and(|next| next.is_ascii_alphanumeric() || *next == '_') {
                word.push(chars.next().unwrap().to_ascii_lowercase());
            }
            let word = word.trim_matches('_').to_owned();
            if word.len() >= 2 && !STOPWORDS.contains(&word.as_str()) {
                *counts.entry(word).or_insert(0) += 1;
            }
        } else if cjk(ch) {
            let mut run = vec![ch];
            while chars.peek().is_some_and(|next| cjk(*next)) {
                run.push(chars.next().unwrap());
            }
            if run.len() == 1 {
                *counts.entry(ch.to_string()).or_insert(0) += 1;
            } else {
                for pair in run.windows(2) {
                    *counts.entry(pair.iter().collect::<String>()).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

// Compute document frequency over the full input, before filtering or deduplication.
fn term_corpus(sections: &[RuleSection]) -> (Vec<BTreeMap<String, usize>>, BTreeMap<String, f64>) {
    let counts: Vec<_> = sections.iter()
        .map(|section| term_counts(&format!("{}\n{}", section.heading, section.text)))
        .collect();
    let mut frequencies = BTreeMap::new();
    for terms in &counts {
        for term in terms.keys() {
            *frequencies.entry(term.clone()).or_insert(0usize) += 1;
        }
    }
    let idf = frequencies.into_iter().map(|(term, frequency)| {
        (term, (1.0 + sections.len() as f64 / frequency as f64).ln())
    }).collect();
    (counts, idf)
}

#[cfg(test)]
mod tests;
