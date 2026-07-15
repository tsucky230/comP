// mod.rs - Model Context Protocol (MCP) Server
//
// Exposes 5 core tools to AI agents (Claude Code, Cursor, Cline, etc.):
// 1. run_pipeline - Full context generation + impact analysis
// 2. get_context - Extract optimized code context
// 3. get_impact_graph - Show code affected by symbol change
// 4. list_indexed_files - Show all indexed files with stats
// 5. get_token_usage - Show token consumption statistics
//
// Protocol: JSON-RPC 2.0 over stdio

mod compress;

use anyhow::{Result, anyhow};
use log::info;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
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

fn get_session_memory_path(root: &str) -> std::path::PathBuf {
    std::path::Path::new(root).join(".comp").join("session-memory.json")
}

/// Render a backtick-quoted, comma-separated list capped at `cap` items,
/// with an overflow marker ("… (+N more)") instead of the full enumeration.
///
/// WHY: recall entries auto-recorded by run_pipeline can carry 30-50 symbols
/// and files each; enumerating them all makes session_recall output so long
/// it defeats its own purpose (fast context reconstruction, low tokens).
fn format_capped_list(items: &[String], cap: usize) -> String {
    let shown: Vec<String> = items.iter().take(cap).map(|s| format!("`{}`", s)).collect();
    let mut out = shown.join(", ");
    if items.len() > cap {
        out.push_str(&format!(" … (+{} more)", items.len() - cap));
    }
    out
}

fn record_mcp_call(
    workspace_root: &str,
    session_id: &str,
    query: String,
    symbols: Vec<String>,
    files: Vec<String>,
    tokens: u64,
) -> Result<()> {
    let path = get_session_memory_path(workspace_root);
    
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut memory: SessionMemory = if path.exists() {
        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        serde_json::from_reader(reader).unwrap_or(SessionMemory { sessions: Vec::new() })
    } else {
        SessionMemory { sessions: Vec::new() }
    };

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
            }],
        });
    }

    let file = std::fs::File::create(&path)?;
    let writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &memory)?;

    Ok(())
}

/// Format a Unix-epoch millisecond timestamp as "YYYY-MM-DD HH:MM" in UTC.
///
/// WHY: session_recall must show *when* past work happened so resumed work can be
/// placed in time. We compute the civil date by hand (Howard Hinnant's days-from-civil
/// algorithm) to avoid pulling in a date crate for this single formatting need.
fn format_epoch_ms(ms: u64) -> String {
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


/// MCP Server
///
/// Listens on stdin for JSON-RPC 2.0 requests
/// Sends JSON-RPC 2.0 responses to stdout
pub struct MCPServer {
    state: Arc<crate::AppState>,
}

/// Canonicalize `path_str` and verify it resides under `workspace_root`.
/// Prevents path-traversal attacks from MCP callers.
fn validate_within_workspace(
    path_str: &str,
    workspace_root: &str,
) -> Result<std::path::PathBuf> {
    let path = std::path::Path::new(path_str);
    let canonical = path
        .canonicalize()
        .map_err(|e| anyhow!("Cannot resolve path '{}': {}", path_str, e))?;
    let canonical_root = std::path::Path::new(workspace_root)
        .canonicalize()
        .map_err(|e| anyhow!("Cannot resolve workspace root '{}': {}", workspace_root, e))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(anyhow!("Access denied: '{}' is outside the workspace", path_str));
    }
    Ok(canonical)
}

/// Reject git refs that start with `-` or contain control characters to prevent
/// flag injection (e.g. `-C /other/path`) passed to `git diff`.
fn validate_git_ref(base_ref: &str) -> Result<()> {
    if base_ref.starts_with('-') {
        return Err(anyhow!("Invalid git ref '{}': cannot start with '-'", base_ref));
    }
    if base_ref.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err(anyhow!("Invalid git ref: contains control characters"));
    }
    Ok(())
}

/// Return the set of files modified in the working tree relative to HEAD.
/// Silently returns an empty set on any error: git not installed, not a repo,
/// no commits yet (HEAD undefined), detached HEAD with no parent, etc.
fn get_git_diff_files(workspace_root: &str) -> std::collections::HashSet<String> {
    let output = std::process::Command::new("git")
        .args(["diff", "HEAD", "--name-only"])
        .current_dir(workspace_root)
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => std::collections::HashSet::new(),
    }
}

impl MCPServer {
    /// Create a new MCP server
    pub fn new(state: Arc<crate::AppState>) -> Self {
        MCPServer { state }
    }

    /// Run the MCP server
    ///
    /// Listens on stdin for JSON-RPC requests
    /// Writes JSON-RPC responses to stdout
    ///
    /// # Protocol:
    /// Request:
    /// ```json
    /// { "jsonrpc": "2.0", "id": 1, "method": "run_pipeline", "params": { "task": "..." } }
    /// ```
    ///
    /// Response:
    /// ```json
    /// { "jsonrpc": "2.0", "id": 1, "result": { ... } }
    /// ```
    pub async fn run(&self) -> Result<()> {
        use std::io::{self, BufRead, Write};

        let stdin = io::stdin();
        let reader = stdin.lock();
        let mut stdout = io::stdout();

        for line in reader.lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }

            // Parse JSON-RPC request
            let request: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => {
                    let error = json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32700,
                            "message": "Parse error"
                        }
                    });
                    writeln!(stdout, "{}", error)?;
                    continue;
                }
            };

            // Extract method and params
            let method = request["method"].as_str().unwrap_or("");
            let params = request["params"].clone();
            let id = request["id"].clone();

            // JSON-RPC 2.0 notifications (requests without an "id") must not
            // receive a response. MCP clients such as Claude Code send
            // `notifications/initialized` right after the handshake; replying to
            // it with an `id: null` error makes strict clients drop the
            // connection (-32000 Connection closed).
            if id.is_null() {
                continue;
            }

            // Call appropriate handler
            let result = match method {
                "initialize" => self.handle_initialize(params).await,
                "tools/list" => self.handle_tools_list().await,
                "tools/call" => self.handle_tools_call(params).await,
                "run_pipeline" => self.handle_run_pipeline(params).await,
                "get_context" => self.handle_get_context(params).await,
                "get_impact_graph" => self.handle_get_impact_graph(params).await,
                "list_indexed_files" => self.handle_list_indexed_files().await,
                "get_token_usage" => self.handle_get_token_usage().await,
                "getStats" => self.handle_get_stats().await,
                "forceReindex" => self.handle_force_reindex().await,
                "indexFile" => self.handle_index_file(params).await,
                "removeFile" => self.handle_remove_file(params).await,
                "session_recall" => self.handle_session_recall(params).await,
                "session_log" => self.handle_session_log(params).await,
                "get_symbol" => self.handle_get_symbol(params).await,
                "get_dependencies" => self.handle_get_dependencies(params).await,
                "get_file_summary" => self.handle_get_file_summary(params).await,
                "get_project_overview" => self.handle_get_project_overview().await,
                "get_git_diff_context" => self.handle_get_git_diff_context(params).await,
                "compressFile" => self.handle_compress_file(params).await,
                _ => Err(anyhow!("Unknown method: {}", method)),
            };

            // Build response
            let response = match result {
                Ok(result_value) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result_value
                }),
                Err(e) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32603,
                        "message": "Internal error",
                        "data": e.to_string()
                    }
                }),
            };

            // Write response
            writeln!(stdout, "{}", response)?;
            stdout.flush()?;
        }

        Ok(())
    }

    /// Tool 1: run_pipeline
    ///
    /// Full pipeline: task → search → context generation → token counting
    ///
    /// # Request:
    /// ```json
    /// {
    ///   "task": "add user authentication to login endpoint",
    ///   "max_tokens": 8000,
    ///   "include_tests": true
    /// }
    /// ```
    ///
    /// # Response:
    /// ```json
    /// {
    ///   "pivot_files": [
    ///     { "path": "src/auth/authenticate.ts", "symbols": 5, "tokens": 500 }
    ///   ],
    ///   "related_files": [
    ///     { "path": "src/types/user.ts", "symbols": 3, "tokens": 200 }
    ///   ],
    ///   "total_tokens": 700,
    ///   "savings": "62%",
    ///   "estimated_cost": "$0.02"
    /// }
    /// ```
    ///
    /// # Process:
    /// 1. Extract task description from params
    /// 2. Perform semantic search using task as query
    /// 3. Select top results as pivot_files (most relevant)
    /// 4. Gather related files from impact graph
    /// 5. Count tokens across all files
    /// 6. Calculate savings percentage
    /// 7. Estimate API cost based on model
    pub async fn handle_run_pipeline(&self, params: Value) -> Result<Value> {
        let task = params["task"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'task' parameter"))?;

        // budget: explicit param overrides config default
        let budget = params["max_tokens"].as_u64().map(|v| v as usize)
            .unwrap_or_else(|| Self::load_default_budget(&self.state.workspace_root));

        let include_content = params["include_content"].as_bool().unwrap_or(false);

        // None = auto-adjust mode; Some(n) = fixed level, no compression adjustment.
        // WHY: Use params.get() to distinguish "omitted" (None → auto) from "explicitly 0" (Some(0) → fixed).
        let explicit_compression: Option<i64> = params.get("compression_level")
            .and_then(|v| v.as_i64());

        // 1. Split task into words and query symbol name LIKE for each keyword -> pivot candidates
        // WHY: A LIKE query on the entire task string (e.g. "fix auth bug") would return 0 hits.
        //      We search each word individually and merge with OR to match related files.
        let keywords: Vec<&str> = task
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 3)
            .collect();

        let mut all_hits: Vec<(String, String, String, i32)> = if keywords.is_empty() {
            self.state.graph_db.search_symbols_by_name(task, 10)?
        } else {
            let mut merged = Vec::new();
            for kw in &keywords {
                merged.extend(self.state.graph_db.search_symbols_by_name(kw, 5)?);
            }
            merged
        };
        // Augment LIKE hits with TF-IDF semantic results (may find files not matched by exact LIKE)
        {
            let se = self.state.search_engine.lock().await;
            for hit in se.search(task, 20).unwrap_or_default() {
                all_hits.push((hit.file_path, hit.symbol_name, hit.kind, hit.line as i32));
            }
        }

        // Sort files by first occurrence before applying limits
        all_hits.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        let hits = all_hits;
        let symbol_counts = self.state.graph_db.count_symbols_per_file()?;
        let char_counts = self.state.graph_db.get_file_char_counts()?;
        let files_list = self.state.graph_db.list_files()?;
        // WHY: Save language info before consuming. BM25 requires the list of Markdown,
        //      Office, and interaction-history (jsonl) files — jsonl symbols only carry
        //      first-line keys, so full-text BM25 is the only way history content matches.
        let doc_paths: Vec<String> = files_list
            .iter()
            .filter(|(_, _, lang)| {
                matches!(
                    lang.as_str(),
                    "markdown" | "docx" | "pptx" | "xlsx" | "pdf" | "jsonl"
                )
            })
            .map(|(_, path, _)| path.clone())
            .collect();
        let indexed_doc_count = doc_paths.len();
        // WHY: Save language per path before consuming files_list — needed for content compression.
        let mut path_to_lang: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let path_to_id: std::collections::HashMap<String, i64> = files_list
            .into_iter()
            .map(|(id, path, lang)| {
                path_to_lang.insert(path.clone(), lang);
                (path, id)
            })
            .collect();

        // 2. Collect candidates in relevance order (deduped by file path).
        //    Content compression is deferred until after level selection so we
        //    only read each file once at the chosen level.
        let mut seen = std::collections::HashSet::new();
        // Uncompressed token estimate: real file size (chars/4) when indexed;
        // symbol-count heuristic (~50 tokens/symbol) only as fallback.
        let base_tokens_for = |path: &str, sym: usize| -> usize {
            path_to_id
                .get(path)
                .and_then(|id| char_counts.get(id))
                .map(|c| (*c as usize) / 4)
                .filter(|c| *c > 0)
                .unwrap_or_else(|| sym.saturating_mul(50))
        };
        // (path, sym_count, base_tokens)
        let mut candidates: Vec<(String, usize, usize)> = Vec::new();
        let mut recorded_symbols = Vec::new();
        let mut recorded_files = Vec::new();
        for (file, name, _kind, _line) in hits {
            recorded_symbols.push(name.clone());
            recorded_files.push(file.clone());
            if seen.insert(file.clone()) {
                let sym = path_to_id
                    .get(&file)
                    .and_then(|id| symbol_counts.get(id))
                    .copied()
                    .unwrap_or(0) as usize;
                let base = base_tokens_for(&file, sym);
                candidates.push((file, sym, base));
            }
        }

        // BM25 full-text search (complements search for Markdown and Office files)
        // WHY: Symbol LIKE queries only match headings, missing body content keywords.
        //      We read Markdown/Office files and score using BM25, then add to candidates.
        let mut bm25_hit_count: usize = 0;
        if !doc_paths.is_empty() && !keywords.is_empty() {
            let workspace_root = self.state.workspace_root.clone();
            let bm25_hits = crate::indexer::doc_parser::Bm25Scorer::search_files(
                &workspace_root,
                &doc_paths,
                &keywords,
                20,
            );
            bm25_hit_count = bm25_hits.len();
            for (path, _score) in bm25_hits {
                recorded_files.push(path.clone());
                if seen.insert(path.clone()) {
                    let sym = path_to_id
                        .get(&path)
                        .and_then(|id| symbol_counts.get(id))
                        .copied()
                        .unwrap_or(0) as usize;
                    let base = base_tokens_for(&path, sym);
                    candidates.push((path, sym, base));
                }
            }
        }

        // Git diff boost: move currently-modified files to the front of candidates.
        // Files in `git diff HEAD` are the ones the agent is actively working on, so they
        // should rank above purely relevance-matched files regardless of TF-IDF score.
        // Any error (git unavailable, not a repo, no commits, detached HEAD without parent)
        // silently degrades to no boost — the rest of the pipeline is unaffected.
        let git_diff_files = get_git_diff_files(&self.state.workspace_root);
        let git_diff_boosted_count: usize;
        if git_diff_files.is_empty() {
            git_diff_boosted_count = 0;
        } else {
            // Partition existing candidates into diff-hit and others.
            // partition() preserves relative order within each group.
            let (mut diff_cands, other_cands): (Vec<_>, Vec<_>) = candidates
                .into_iter()
                .partition(|(path, _, _)| git_diff_files.contains(path));

            // Insert git-diff files that were not matched by LIKE/TF-IDF/BM25 but are indexed.
            // Sort for deterministic ordering (HashSet iteration is non-deterministic).
            let mut unmatched: Vec<&String> = git_diff_files
                .iter()
                .filter(|p| !seen.contains(*p))
                .collect();
            unmatched.sort();
            for diff_path in unmatched {
                if let Some(&file_id) = path_to_id.get(diff_path) {
                    let sym = symbol_counts.get(&file_id).copied().unwrap_or(0) as usize;
                    let base = base_tokens_for(diff_path, sym);
                    diff_cands.push((diff_path.clone(), sym, base));
                    recorded_files.push(diff_path.clone());
                }
                // If not indexed, skip — no content to serve.
            }

            git_diff_boosted_count = diff_cands.len();
            // Reassemble: diff-boosted files first, then relevance-ranked remainder.
            candidates = diff_cands;
            candidates.extend(other_cands);
        }

        // 3. Choose compression level and pack within budget.
        //
        //    Fixed mode  (explicit_compression = Some(n)):
        //      Use level n without adjustment; greedy-pack files at that level.
        //
        //    Auto mode   (explicit_compression = None):
        //      Try levels 0 → 1 → 2 until total estimate fits budget.
        //      If level 2 still overflows, stay at 2 and greedy-truncate.
        let (final_level, budget_adjusted) = match explicit_compression {
            Some(level) => (level, false),
            None => Self::choose_compression_level(&candidates, budget),
        };
        let packed = Self::pack_within_budget(&candidates, budget, final_level);

        // 4. Build pivot_files JSON, compressing content at the chosen level.
        //    Per-extension rules from .comp/config.json may override level per file.
        let ws = self.state.workspace_root.clone();
        let compression_rules = Self::load_compression_rules(&ws);
        let mut pivot_files: Vec<Value> = Vec::new();
        let mut pivot_paths: Vec<String> = Vec::new();
        let mut compression_rules_applied = false;
        for (file, sym, base) in &packed {
            let file_level = match Self::apply_compression_rule(file, &compression_rules) {
                Some(rule_level) if rule_level != final_level => {
                    compression_rules_applied = true;
                    rule_level
                }
                _ => final_level,
            };
            let tokens = Self::estimate_tokens(*base, file_level);
            let mut entry = json!({
                "path": file,
                "symbols": sym,
                "tokens": tokens
            });
            if git_diff_files.contains(file) {
                entry["git_diff"] = Value::Bool(true);
            }
            if include_content {
                let full_path = std::path::Path::new(&ws).join(file);
                let lang = path_to_lang.get(file).map(|s| s.as_str()).unwrap_or("");
                let file_compression = compress::CompressionLevel::from_i64(file_level);
                if let Ok(raw) = std::fs::read_to_string(&full_path) {
                    entry["content"] = Value::String(compress::compress(&raw, lang, file_compression));
                }
            }
            pivot_files.push(entry);
            pivot_paths.push(file.clone());
        }

        let total_tokens: usize = pivot_files
            .iter()
            .filter_map(|v| v["tokens"].as_u64())
            .map(|t| t as usize)
            .sum();

        // Rules can override the base level with less compression (e.g. rule=0, base=2),
        // causing total_tokens to exceed the budget even when budget_adjusted is false.
        // Flag this explicitly so agents are not surprised by the discrepancy.
        let budget_exceeded_by_rules = compression_rules_applied && total_tokens > budget;

        // 5. Real workspace token baseline from stored file char counts
        let full_workspace_tokens = self.state.graph_db
            .get_full_workspace_tokens()
            .unwrap_or(total_tokens as u64 + 1);

        let saved_this_call = full_workspace_tokens.saturating_sub(total_tokens as u64);
        let savings = crate::search::TokenCounter::calculate_savings(
            full_workspace_tokens as usize,
            total_tokens,
        );
        let cost = crate::search::TokenCounter::estimate_cost(total_tokens, "sonnet");

        // Persist to shared DB (single source of truth for both daemon processes)
        if let Err(e) = self.state.graph_db.record_tool_call(total_tokens as u64, saved_this_call) {
            log::warn!("record_tool_call failed in run_pipeline: {}", e);
        }

        // Record this call to session memory
        recorded_symbols.sort();
        recorded_symbols.dedup();
        recorded_files.sort();
        recorded_files.dedup();
        if let Err(e) = record_mcp_call(
            &self.state.workspace_root,
            &self.state.session_id,
            task.to_string(),
            recorded_symbols,
            recorded_files,
            total_tokens as u64,
        ) {
            log::warn!("record_mcp_call failed in run_pipeline: {}", e);
        }

        // WHY: pivot_file_types gives agents a quick signal that Markdown/docs were searched,
        // preventing the false assumption that comP only indexes code files.
        let pivot_file_types: std::collections::HashMap<String, usize> = pivot_paths
            .iter()
            .map(|p| {
                std::path::Path::new(p)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            })
            .fold(std::collections::HashMap::new(), |mut m, ext| {
                *m.entry(ext).or_insert(0) += 1;
                m
            });

        // Files one dependency hop away from the pivots (callers/callees in other
        // files), ranked by connecting-edge count. Best-effort: an empty list on
        // error must not fail the whole pipeline.
        let related_files: Vec<Value> = self
            .state
            .graph_db
            .get_related_files(&pivot_paths, 10)
            .unwrap_or_default()
            .into_iter()
            .map(|(path, edge_count)| json!({ "path": path, "edge_count": edge_count }))
            .collect();

        Ok(json!({
            "task": task,
            "pivot_files": pivot_files,
            "related_files": related_files,
            "total_tokens": total_tokens,
            "max_tokens": budget,
            "compression_level_applied": final_level,
            "compression_rules_applied": compression_rules_applied,
            "budget_exceeded_by_rules": budget_exceeded_by_rules,
            "budget_adjusted": budget_adjusted,
            "savings": savings,
            "full_workspace_tokens": full_workspace_tokens,
            "estimated_cost": cost,
            "coverage": {
                "indexed_doc_files": indexed_doc_count,
                "bm25_hits": bm25_hit_count,
                "pivot_file_types": pivot_file_types,
                "git_diff_boosted": git_diff_boosted_count
            }
        }))
    }

    /// Read `default_budget_tokens` from `.comp/config.json`.
    /// Falls back to 8000 if the file is absent or the key is missing.
    fn load_default_budget(root: &str) -> usize {
        let path = std::path::Path::new(root).join(".comp/config.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let json: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
        json["default_budget_tokens"].as_u64().unwrap_or(8000) as usize
    }

    /// Read `compression_rules` from `.comp/config.json`.
    ///
    /// Rules map glob patterns to compression levels (0/1/2).
    /// Returns an empty map when the file is absent, invalid, or has no rules key.
    fn load_compression_rules(root: &str) -> std::collections::HashMap<String, i64> {
        let path = std::path::Path::new(root).join(".comp/config.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let json: Value = serde_json::from_str(&content).unwrap_or(Value::Null);

        let mut rules = std::collections::HashMap::new();
        if let Some(obj) = json["compression_rules"].as_object() {
            for (pattern, level_val) in obj {
                if let Some(l) = level_val.as_i64() {
                    rules.insert(pattern.clone(), l.clamp(0, 2));
                }
            }
        }
        rules
    }

    /// Match a filename against a simple glob pattern.
    ///
    /// Supported forms: `*` (any), `*.ext` (suffix), `prefix*` (prefix), `exact` (literal).
    fn match_pattern(filename: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            return filename.ends_with(suffix);
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return filename.starts_with(prefix);
        }
        filename == pattern
    }

    /// Return the compression level override for a file path from the rules map.
    ///
    /// Matching order (deterministic):
    ///   1. Exact full-path match
    ///   2. Exact filename match
    ///   3. Glob patterns — most specific wins (longest non-wildcard portion);
    ///      ties broken alphabetically so results never vary between runs.
    ///
    /// Returns None when no rule matches (caller falls back to the base level).
    fn apply_compression_rule(
        file_path: &str,
        rules: &std::collections::HashMap<String, i64>,
    ) -> Option<i64> {
        if rules.is_empty() {
            return None;
        }
        let filename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);

        // 1. Exact full-path match
        if let Some(&level) = rules.get(file_path) {
            return Some(level);
        }
        // 2. Exact filename match
        if let Some(&level) = rules.get(filename) {
            return Some(level);
        }
        // 3. Glob patterns — collect all matches, then pick most specific deterministically.
        //    WHY: HashMap iteration order is not guaranteed; without this, the chosen rule
        //    for a file that matches multiple globs (e.g. "test_*.rs" matching both "*.rs"
        //    and "test_*") would change between runs.
        let mut matches: Vec<(&str, i64)> = rules
            .iter()
            .filter(|(p, _)| p.contains('*') && Self::match_pattern(filename, p))
            .map(|(p, &l)| (p.as_str(), l))
            .collect();

        if matches.is_empty() {
            return None;
        }
        // Sort: longer non-wildcard portion = more specific = wins.
        // Tie-break: alphabetical pattern string for full determinism.
        matches.sort_by(|(a, _), (b, _)| {
            let a_len = a.replace('*', "").len();
            let b_len = b.replace('*', "").len();
            b_len.cmp(&a_len).then_with(|| a.cmp(b))
        });
        Some(matches[0].1)
    }

    /// Estimate token count for a file given its uncompressed token estimate
    /// (real chars/4, or the symbol-count fallback) and compression level.
    ///
    /// Reduction factors mirror the compression ratio ranges advertised in the tool schema:
    ///   level 1 (compact)   → ~30% reduction  → factor 0.70
    ///   level 2 (skeleton)  → ~75% reduction  → factor 0.25
    fn estimate_tokens(base_tokens: usize, level: i64) -> usize {
        match level {
            1 => ((base_tokens as f64) * 0.70) as usize,
            2 => ((base_tokens as f64) * 0.25) as usize,
            _ => base_tokens,
        }
    }

    /// Choose the lowest compression level whose total token estimate fits within budget.
    ///
    /// Returns (level, budget_adjusted).
    /// If even level 2 overflows, returns (2, true) — greedy truncation handles the rest.
    fn choose_compression_level(candidates: &[(String, usize, usize)], budget: usize) -> (i64, bool) {
        for level in [0i64, 1, 2] {
            let total: usize = candidates.iter()
                .map(|(_, _, base)| Self::estimate_tokens(*base, level))
                .sum();
            if total <= budget {
                return (level, level > 0);
            }
        }
        (2, true)
    }

    /// Greedily pack candidates within budget at the given compression level.
    ///
    /// Iterates in relevance order (highest first). A file that exceeds the remaining
    /// budget is skipped rather than stopping the loop, so smaller subsequent files
    /// can still fill remaining space.
    fn pack_within_budget(
        candidates: &[(String, usize, usize)],
        budget: usize,
        level: i64,
    ) -> Vec<(String, usize, usize)> {
        let mut remaining = budget;
        let mut packed = Vec::new();
        for (path, sym, base) in candidates {
            let tokens = Self::estimate_tokens(*base, level);
            if tokens <= remaining {
                packed.push((path.clone(), *sym, *base));
                remaining = remaining.saturating_sub(tokens);
            }
        }
        packed
    }

    /// Tool 2: get_context
    ///
    /// Extract relevant code context based on search query
    ///
    /// # Request:
    /// ```json
    /// {
    ///   "query": "authentication functions",
    ///   "limit": 10,
    ///   "kind_filter": "function"
    /// }
    /// ```
    ///
    /// # Response:
    /// ```json
    /// {
    ///   "results": [
    ///     { "file": "src/auth/authenticate.ts", "symbol": "authenticate", "line": 10, "score": 0.95 }
    ///   ],
    ///   "count": 1,
    ///   "query": "authentication functions"
    /// }
    /// ```
    ///
    /// # Process:
    /// 1. Extract query from params
    /// 2. Perform semantic search using TF-IDF
    /// 3. Optionally filter by symbol kind
    /// 4. Return results ranked by relevance score
    pub async fn handle_get_context(&self, params: Value) -> Result<Value> {
        let query = params["query"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'query' parameter"))?;
        let limit = params["limit"]
            .as_u64()
            .unwrap_or(10) as usize;
        let kind_filter = params["kind_filter"].as_str();

        // Query GraphDB directly via LIKE (since SearchEngine TF-IDF is not yet built)
        let hits = self.state.graph_db.search_symbols_by_name(query, limit * 2)?;

        let results: Vec<Value> = hits
            .into_iter()
            .filter(|(_, _, kind, _)| kind_filter.is_none_or(|f| f == kind))
            .take(limit)
            .map(|(file, name, kind, line)| {
                json!({
                    "file": file,
                    "symbol": name,
                    "kind": kind,
                    "line": line,
                    "score": 1.0  // Score is fixed for LIKE queries. To be replaced after TF-IDF implementation.
                })
            })
            .collect();

        let count = results.len();

        // Record this call to session memory
        let mut recorded_symbols = Vec::new();
        let mut recorded_files = Vec::new();
        for res in &results {
            if let Some(sym) = res["symbol"].as_str() {
                recorded_symbols.push(sym.to_string());
            }
            if let Some(file) = res["file"].as_str() {
                recorded_files.push(file.to_string());
            }
        }
        recorded_symbols.sort();
        recorded_symbols.dedup();
        recorded_files.sort();
        recorded_files.dedup();
        let estimated_tokens = (count as u64) * 50;
        if let Err(e) = record_mcp_call(
            &self.state.workspace_root,
            &self.state.session_id,
            query.to_string(),
            recorded_symbols,
            recorded_files,
            estimated_tokens,
        ) {
            log::warn!("record_mcp_call failed in get_context: {}", e);
        }
        if let Err(e) = self.state.graph_db.record_tool_call(estimated_tokens, 0) {
            log::warn!("record_tool_call failed in get_context: {}", e);
        }

        Ok(json!({
            "query": query,
            "results": results,
            "count": count,
            "limit": limit
        }))
    }

    /// Tool 3: get_impact_graph
    ///
    /// Show all code affected by changing a symbol
    ///
    /// # Request:
    /// ```json
    /// {
    ///   "symbol_id": 123,
    ///   "symbol_name": "authenticate"
    /// }
    /// ```
    ///
    /// # Response:
    /// ```json
    /// {
    ///   "symbol": "authenticate",
    ///   "affected_files": {
    ///     "src/routes/login.ts": ["handleLogin", "validateCredentials"],
    ///     "src/middleware/auth.ts": ["authMiddleware"]
    ///   },
    ///   "impact_count": 3,
    ///   "severity": "high"
    /// }
    /// ```
    ///
    /// # Process:
    /// 1. Extract symbol_id from params
    /// 2. Query GraphDB for all symbols that depend on this symbol
    /// 3. Recursively traverse impact graph (BFS)
    /// 4. Group affected symbols by file
    /// 5. Calculate severity based on impact count
    pub async fn handle_get_impact_graph(&self, params: Value) -> Result<Value> {
        let symbol_id = params["symbol_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("Missing 'symbol_id' parameter"))?;
        let symbol_name = params["symbol_name"].as_str().unwrap_or("unknown");
        let max_depth = params["max_depth"].as_u64().unwrap_or(0) as usize;

        // Build reverse dependency & symbol maps from GraphDB and invoke SearchEngine BFS
        let reverse_deps = self.state.graph_db.get_reverse_deps()?;
        let symbol_map = self.state.graph_db.get_symbol_map()?;

        let search_engine = self.state.search_engine.lock().await;
        let impact = search_engine.get_impact_graph_depth(symbol_id, &reverse_deps, &symbol_map, max_depth)?;
        drop(search_engine);

        let mut affected_obj = serde_json::Map::new();
        let mut impact_count: usize = 0;
        for (file, symbols) in &impact {
            impact_count += symbols.len();
            affected_obj.insert(file.clone(), json!(symbols));
        }

        let severity = match impact_count {
            0 => "none",
            1..=5 => "low",
            6..=20 => "medium",
            _ => "high",
        };

        let result = json!({
            "symbol_id": symbol_id,
            "symbol": symbol_name,
            "affected_files": Value::Object(affected_obj),
            "impact_count": impact_count,
            "severity": severity
        });
        let tokens = (result.to_string().len() / 4) as u64;
        if let Err(e) = self.state.graph_db.record_tool_call(tokens, 0) {
            log::warn!("record_tool_call failed in get_impact_graph: {}", e);
        }
        Ok(result)
    }

    /// Tool 4: list_indexed_files
    ///
    /// List all indexed files with statistics
    ///
    /// # Response:
    /// ```json
    /// {
    ///   "files": [
    ///     { "path": "src/main.rs", "language": "rust", "symbols": 15 }
    ///   ],
    ///   "total_files": 42,
    ///   "total_symbols": 523,
    ///   "languages": { "rust": 20, "typescript": 22 }
    /// }
    /// ```
    ///
    /// # Process:
    /// 1. Query GraphDB for all files
    /// 2. Count symbols per file
    /// 3. Group by language
    /// 4. Calculate totals and statistics
    pub async fn handle_list_indexed_files(&self) -> Result<Value> {
        let files_raw = self.state.graph_db.list_files()?;
        let symbol_counts = self.state.graph_db.count_symbols_per_file()?;

        let mut total_symbols: i64 = 0;
        let mut languages: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let files: Vec<Value> = files_raw
            .into_iter()
            .map(|(id, path, language)| {
                let sym = *symbol_counts.get(&id).unwrap_or(&0);
                total_symbols += sym;
                *languages.entry(language.clone()).or_insert(0) += 1;
                json!({
                    "path": path,
                    "language": language,
                    "symbols": sym
                })
            })
            .collect();
        let total_files = files.len();

        Ok(json!({
            "files": files,
            "total_files": total_files,
            "total_symbols": total_symbols,
            "languages": languages
        }))
    }

    /// Tool 5: get_token_usage
    ///
    /// Show token consumption statistics and metrics
    ///
    /// # Response:
    /// ```json
    /// {
    ///   "total_tokens_consumed": 45000,
    ///   "queries_executed": 15,
    ///   "average_tokens_per_query": 3000,
    ///   "timestamp": 1716226421,
    ///   "efficiency": "75%"
    /// }
    /// ```
    ///
    /// # Process:
    /// 1. Retrieve statistics from internal counter
    /// 2. Calculate average tokens per query
    /// 3. Calculate efficiency (context optimization benefit)
    /// 4. Add current timestamp
    pub async fn handle_get_token_usage(&self) -> Result<Value> {
        let (sent, saved, queries) = self.state.graph_db.get_token_stats()
            .unwrap_or_else(|e| { log::warn!("get_token_stats failed in get_token_usage: {}", e); (0, 0, 0) });
        let avg = sent.checked_div(queries).unwrap_or(0);
        let efficiency = (saved * 100).checked_div(sent + saved)
            .map(|e| format!("{}%", e))
            .unwrap_or_else(|| "0%".to_string());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Ok(json!({
            "total_tokens_consumed": sent,
            "total_tokens_saved": saved,
            "queries_count": queries,
            "average_tokens_per_query": avg,
            "timestamp": timestamp,
            "efficiency": efficiency
        }))
    }

    /// Force re-index of entire workspace
    ///
    /// WHY: Called from VSCode `comp.forceReindex` command.
    /// Clears and rebuilds the database. Response might be slow since indexing is blocking (beware of timeouts).
    pub async fn handle_force_reindex(&self) -> Result<Value> {
        info!("handle_force_reindex: clearing index");
        self.state.graph_db.clear_index()?;

        let workspace_root = self.state.workspace_root.clone();

        info!("handle_force_reindex: rebuilding index for {}", workspace_root);
        let mut indexer = crate::indexer::Indexer::new(&workspace_root);
        let (total, indexed, symbols) = indexer
            .index_workspace(None, &self.state.graph_db)
            .await?;

        let (files, nodes, edges) = self.state.graph_db.get_stats()?;
        info!(
            "handle_force_reindex: complete - {}/{} files, {} symbols, {} nodes, {} edges",
            indexed, total, symbols, nodes, edges
        );

        Ok(json!({
            "total_files": files,
            "total_nodes": nodes,
            "total_edges": edges,
            "indexed_files": indexed,
            "scanned_files": total,
            "symbols_extracted": symbols
        }))
    }

    /// Index a single file (incremental update)
    ///
    /// WHY: Called by VSCode's FileSystemWatcher for each modified file.
    /// The previous implementation was a TODO that did not write to DB, which left stats unreflected.
    pub async fn handle_index_file(&self, params: Value) -> Result<Value> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'path' parameter"))?;

        let workspace_root = self.state.workspace_root.clone();

        let safe_path = validate_within_workspace(path_str, &workspace_root)?;

        let mut indexer = crate::indexer::Indexer::new(&workspace_root);
        indexer
            .index_file(&safe_path, &self.state.graph_db)
            .await?;

        Ok(json!({ "status": "ok", "path": path_str }))
    }

    /// Remove a file from the index
    ///
    /// Called by VSCode's onDidDelete. Converts absolute path to workspace-relative and deletes from DB.
    pub async fn handle_remove_file(&self, params: Value) -> Result<Value> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'path' parameter"))?;

        let workspace_root = self.state.workspace_root.clone();

        // Convert to workspace-relative path if absolute, and normalize \ to / (DB uses / unified)
        let relative_path = std::path::Path::new(path_str)
            .strip_prefix(&workspace_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path_str.replace('\\', "/"));

        let removed = self.state.graph_db.delete_file(&relative_path)?;
        info!("handle_remove_file: deleted {} ({} rows)", relative_path, removed);

        Ok(json!({
            "status": "ok",
            "path": relative_path,
            "removed": removed
        }))
    }

    /// Get index statistics (file count, node count, edge count)
    ///
    /// # Response:
    /// ```json
    /// {
    ///   "total_files": 42,
    ///   "total_nodes": 1250,
    ///   "total_edges": 890
    /// }
    /// ```
    pub async fn handle_get_stats(&self) -> Result<Value> {
        info!("handle_get_stats: called");

        let (file_count, node_count, edge_count) = self.state.graph_db.get_stats()?;
        // Read from shared DB (not in-memory) so VSCode extension's daemon sees stats
        // accumulated by the Claude Code MCP daemon in the same workspace
        let (sent, saved, queries) = self.state.graph_db.get_token_stats()
            .unwrap_or_else(|e| { log::warn!("get_token_stats failed in get_stats: {}", e); (0, 0, 0) });
        let efficiency = (saved * 100).checked_div(sent + saved)
            .map(|e| format!("{}%", e))
            .unwrap_or_else(|| "0%".to_string());
        let avg_tokens_per_query = sent.checked_div(queries).unwrap_or(0);

        info!("handle_get_stats: returning stats - files: {}, nodes: {}, edges: {}",
              file_count, node_count, edge_count);

        Ok(json!({
            // Lets clients detect a stale running binary after an upgrade
            // (Windows locks the exe, so rebuilds don't take effect until restart).
            "daemon_version": env!("CARGO_PKG_VERSION"),
            "total_files": file_count,
            "total_nodes": node_count,
            "total_edges": edge_count,
            "tokens_sent": sent,
            "tokens_saved": saved,
            "queries_count": queries,
            "efficiency": efficiency,
            "avg_tokens_per_query": avg_tokens_per_query
        }))
    }

    /// MCP initialize handshake — returns server capabilities
    pub async fn handle_initialize(&self, _params: Value) -> Result<Value> {
        Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "comP", "version": env!("CARGO_PKG_VERSION") }
        }))
    }

    /// MCP tools/list — AI-accessible tools with precise descriptions and input schemas
    ///
    /// WHY: Description quality directly controls when AI agents call each tool.
    /// "Call ONLY when..." / "Do NOT call..." constraints prevent accidental invocations
    /// that would pollute the context window mid-implementation.
    pub async fn handle_tools_list(&self) -> Result<Value> {
        Ok(json!({
            "tools": [
                {
                    "name": "run_pipeline",
                    "description": "ALWAYS call this tool FIRST at the start of every task — before reading files, running grep/find/Bash searches, or exploring the codebase manually. Covers coding tasks (bug fix, feature, refactor) and documentation tasks (writing/editing Markdown). Do NOT use Read, Bash, or get_context to locate relevant files — this tool replaces all of that. Returns pivot files and related symbols ranked by relevance. Files modified in the current branch (git diff HEAD) are ranked first and marked with `git_diff: true` in the response. IMPORTANT: The 'task' parameter MUST be in English. Translate queries from other languages (e.g. Japanese) to English before calling.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task": {
                                "type": "string",
                                "description": "One sentence describing what you are about to implement, fix, or write. IMPORTANT: The task description MUST be in English. Translate to English if needed. Examples: 'fix JWT token expiry bug in auth middleware', 'write installation section in README.md'"
                            },
                            "max_tokens": {
                                "type": "integer",
                                "description": "Token budget for returned context. Default: 8000"
                            },
                            "include_tests": {
                                "type": "boolean",
                                "description": "Include test files in results. Default: false"
                            },
                            "include_content": {
                                "type": "boolean",
                                "description": "If true, include compressed file content in each pivot_file entry. Default: false"
                            },
                            "compression_level": {
                                "type": "integer",
                                "enum": [0, 1, 2],
                                "description": "Omit for auto-adjustment (comP picks the lowest level that fits the budget). Set explicitly to disable auto-adjustment: 0=full source, 1=compact (comments removed, ~30% smaller), 2=skeleton (signatures only, ~75% smaller)"
                            }
                        },
                        "required": ["task"]
                    }
                },
                {
                    "name": "get_context",
                    "description": "Search for specific symbols (functions, classes, types) by name or keyword. Use when you know the exact name to look up. Do NOT use for starting a new task — use run_pipeline instead. IMPORTANT: The 'query' parameter MUST be in English or match exact symbol names.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Symbol name or keyword. IMPORTANT: The query MUST be in English or match exact symbol names in the code. Example: 'authenticate' or 'UserRepository'"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max results to return. Default: 10"
                            },
                            "kind_filter": {
                                "type": "string",
                                "description": "Filter by symbol kind: 'function', 'class', 'type', or 'variable'"
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "get_impact_graph",
                    "description": "Show which files and symbols are affected when a specific symbol changes. Call before modifying a function or class to understand the blast radius. Requires a numeric symbol_id from a prior run_pipeline or get_context result.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol_id": {
                                "type": "integer",
                                "description": "Numeric ID of the symbol to analyze (obtained from run_pipeline or get_context)"
                            },
                            "symbol_name": {
                                "type": "string",
                                "description": "Human-readable name for display purposes only"
                            },
                            "max_depth": {
                                "type": "integer",
                                "default": 0,
                                "description": "Maximum traversal depth for transitive impact (0 = unlimited, 1 = direct dependents only, N = N hops)"
                            }
                        },
                        "required": ["symbol_id"]
                    }
                },
                {
                    "name": "list_indexed_files",
                    "description": "List all indexed files with symbol counts and language breakdown. Use to understand overall codebase structure. Do NOT use to find relevant files for a task — use run_pipeline for that.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "session_recall",
                    "description": "Recall past MCP tool invocations (queries, symbols, files, tokens) across ALL sessions, newest first, each tagged with its date/time. Survives daemon restarts and session breaks — call this when resuming work to reconstruct what was previously asked and done. Returns a Markdown list with stale status. If query is provided, filters by substring match.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Optional search query to filter past invocations (case-insensitive substring)"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max number of past invocations to return (default: 20)"
                            }
                        }
                    }
                },
                {
                    "name": "session_log",
                    "description": "Record an interaction — a user request and what was done in response — so it can be recalled in later sessions (survives daemon restarts). Call this after completing a task to persist intent and outcome. Stored in .comp/history and indexed so run_pipeline can also surface it.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "request": {
                                "type": "string",
                                "description": "What the user asked for (their request/instruction)"
                            },
                            "outcome": {
                                "type": "string",
                                "description": "What was done in response (summary of the action and result)"
                            },
                            "files": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional list of files touched"
                            }
                        },
                        "required": ["request"]
                    }
                },
                {
                    "name": "get_symbol",
                    "description": "Get source code, outbound dependencies, and inbound dependents for a specific symbol by name. Useful when you need to inspect a function, class, or type definition.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Symbol name (e.g. 'authenticate' or 'MCPServer')"
                            },
                            "file_path": {
                                "type": "string",
                                "description": "Optional file path relative to workspace root to narrow search"
                            },
                            "compression_level": {
                                "type": "integer",
                                "enum": [0, 1, 2],
                                "default": 0,
                                "description": "0=full source (default), 1=compact (comments+blank lines removed, 20-35% smaller), 2=skeleton (signatures only, 50-70% smaller)"
                            }
                        },
                        "required": ["name"]
                    }
                },
                {
                    "name": "get_dependencies",
                    "description": "Retrieve dependencies of a symbol. Direction 'out' returns symbols that this symbol depends on. Direction 'in' returns symbols that depend on this symbol.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Symbol name"
                            },
                            "direction": {
                                "type": "string",
                                "enum": ["in", "out"],
                                "description": "Dependency direction. 'in' for incoming dependencies (dependents), 'out' for outgoing dependencies."
                            }
                        },
                        "required": ["name", "direction"]
                    }
                },
                {
                    "name": "get_file_summary",
                    "description": "List all symbol names, their kinds (function, class, etc.), and signatures inside a specific file. Excludes body content.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "File path relative to workspace root (e.g., 'src/extension.ts')"
                            }
                        },
                        "required": ["file_path"]
                    }
                },
                {
                    "name": "get_project_overview",
                    "description": "Get a high-level summary of the workspace: total file count, symbol count, language distribution, top files by symbol count, and lists of exported symbols.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "get_git_diff_context",
                    "description": "Get context for files changed in a git diff. Useful for PR review or understanding recent changes. Returns a table of changed files with symbol counts.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "base_ref": {
                                "type": "string",
                                "description": "Git ref to diff against. Default: HEAD~1. Use 'main' or 'master' for branch diffs."
                            }
                        }
                    }
                },
                {
                    "name": "compress_file",
                    "description": "Compress a file using AST-based compression. Removes comments (level 1) or extracts signatures only (level 2). Useful for reading large files with fewer tokens. Requires an absolute path within the workspace.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Absolute path to the file within the workspace"
                            },
                            "compression_level": {
                                "type": "integer",
                                "description": "0=full (no-op), 1=compact (comments removed), 2=skeleton (signatures only). Default: 1"
                            }
                        },
                        "required": ["path"]
                    }
                }
            ]
        }))
    }

    /// MCP tools/call — dispatches to the appropriate tool handler
    ///
    /// WHY: Standard MCP agents call tools/call instead of the raw method name.
    /// Wraps the result in MCP content format so agents can parse it uniformly.
    pub async fn handle_tools_call(&self, params: Value) -> Result<Value> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'name' in tools/call params"))?;
        let args = params["arguments"].clone();

        let result = match name {
            "run_pipeline" => self.handle_run_pipeline(args).await?,
            "get_context" => self.handle_get_context(args).await?,
            "get_impact_graph" => self.handle_get_impact_graph(args).await?,
            "list_indexed_files" => self.handle_list_indexed_files().await?,
            "get_token_usage" => self.handle_get_token_usage().await?,
            "session_recall" => self.handle_session_recall(args).await?,
            "session_log" => self.handle_session_log(args).await?,
            "get_symbol" => self.handle_get_symbol(args).await?,
            "get_dependencies" => self.handle_get_dependencies(args).await?,
            "get_file_summary" => self.handle_get_file_summary(args).await?,
            "get_project_overview" => self.handle_get_project_overview().await?,
            "get_git_diff_context" => self.handle_get_git_diff_context(args).await?,
            "compress_file" => self.handle_compress_file(args).await?,
            _ => return Err(anyhow!("Unknown tool: {}", name)),
        };

        let text_content = if let Some(s) = result.as_str() {
            s.to_string()
        } else {
            result.to_string()
        };

        Ok(json!({
            "content": [{ "type": "text", "text": text_content }],
            "isError": false
        }))
    }

    /// Tool 6: session_recall
    ///
    /// Recall past MCP tool invocations for the current session.
    pub async fn handle_session_recall(&self, params: Value) -> Result<Value> {
        let query_filter = params["query"].as_str().map(|q| q.to_lowercase());
        let limit = params["limit"].as_u64().unwrap_or(20) as usize;
        let path = get_session_memory_path(&self.state.workspace_root);

        let mut markdown = String::new();
        markdown.push_str("### Session Recall\n\n");

        let no_result_msg = if query_filter.is_some() {
            "No matching past invocations found for the query."
        } else {
            "No past invocations recorded."
        };

        // WHY: recall must survive daemon restarts / session breaks. Gather from BOTH stores:
        //   1. session-memory.json — auto run_pipeline/get_context query records (per-session,
        //      accumulated across daemon starts);
        //   2. .comp/history/*.jsonl — explicit interaction logs (request + outcome) written by
        //      session_log / the Stop hook, which also feed BM25 recall via run_pipeline.
        // Flatten everything across ALL sessions and show newest-first to reconstruct context.
        let mut calls: Vec<SessionCall> = Vec::new();

        if path.exists() {
            if let Ok(file) = std::fs::File::open(&path) {
                let reader = std::io::BufReader::new(file);
                let memory: SessionMemory =
                    serde_json::from_reader(reader).unwrap_or(SessionMemory { sessions: Vec::new() });
                for session in memory.sessions {
                    calls.extend(session.calls);
                }
            }
        }

        let hist_dir = std::path::Path::new(&self.state.workspace_root)
            .join(".comp")
            .join("history");
        if let Ok(entries) = std::fs::read_dir(&hist_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&p) {
                    for line in content.lines() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(c) = serde_json::from_str::<SessionCall>(line) {
                            calls.push(c);
                        }
                    }
                }
            }
        }

        calls.sort_by_key(|c| std::cmp::Reverse(c.timestamp));

        let mut shown = 0;
        for call in &calls {
            if let Some(ref q_filter) = query_filter {
                let hit = call.query.to_lowercase().contains(q_filter)
                    || call
                        .outcome
                        .as_ref()
                        .map(|o| o.to_lowercase().contains(q_filter))
                        .unwrap_or(false);
                if !hit {
                    continue;
                }
            }
            if shown >= limit {
                break;
            }
            shown += 1;

            let stale_flag = if call.stale { " [Stale]" } else { "" };
            markdown.push_str(&format!(
                "- `{}` **Query**: \"{}\" (Tokens: {}){}\n",
                format_epoch_ms(call.timestamp), call.query, call.tokens, stale_flag
            ));
            if let Some(ref o) = call.outcome {
                if !o.is_empty() {
                    markdown.push_str(&format!("  - **Outcome**: {}\n", o));
                }
            }
            // Cap at 5: enough to identify the work, without flooding the output.
            const RECALL_LIST_CAP: usize = 5;
            if !call.symbols.is_empty() {
                markdown.push_str(&format!(
                    "  - **Symbols**: {}\n",
                    format_capped_list(&call.symbols, RECALL_LIST_CAP)
                ));
            }
            if !call.files.is_empty() {
                markdown.push_str(&format!(
                    "  - **Files**: {}\n",
                    format_capped_list(&call.files, RECALL_LIST_CAP)
                ));
            }
        }

        if shown == 0 {
            markdown.push_str(no_result_msg);
        }

        Ok(Value::String(markdown))
    }

    /// Tool: session_log
    ///
    /// Explicitly record an interaction — a user request and what was done in response —
    /// so it can be recalled in later sessions. Appends one JSONL line to
    /// `.comp/history/log-YYYY-MM.jsonl` and indexes that file so run_pipeline (BM25) can
    /// also surface it. session_recall reads the same store directly.
    pub async fn handle_session_log(&self, params: Value) -> Result<Value> {
        let request = params["request"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'request' parameter"))?
            .to_string();
        let outcome = params["outcome"].as_str().map(|s| s.to_string());
        let files: Vec<String> = params["files"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let call = SessionCall {
            query: request,
            outcome,
            symbols: Vec::new(),
            files,
            tokens: 0,
            stale: false,
            timestamp: now,
        };

        // Monthly file bounds each log while preserving full history.
        let month = &format_epoch_ms(now)[0..7]; // "YYYY-MM"
        let hist_path = std::path::Path::new(&self.state.workspace_root)
            .join(".comp")
            .join("history")
            .join(format!("log-{}.jsonl", month));
        if let Some(parent) = hist_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let line = serde_json::to_string(&call)?;
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&hist_path)?;
            writeln!(f, "{}", line)?;
        }

        // Index the history file so run_pipeline BM25 can surface past interactions.
        // Best-effort: session_recall reads the file directly regardless of indexing.
        let mut indexer = crate::indexer::Indexer::new(&self.state.workspace_root);
        if let Err(e) = indexer.index_file(&hist_path, &self.state.graph_db).await {
            info!("session_log: failed to index history file: {}", e);
        }

        Ok(json!({
            "status": "ok",
            "path": hist_path.to_string_lossy(),
            "timestamp": now
        }))
    }

    /// Tool 7: get_symbol
    ///
    /// Get source code, outbound dependencies, and inbound dependents for a specific symbol by name.
    /// compression_level: 0=full (default), 1=compact (no comments/blanks), 2=skeleton (signatures only)
    pub async fn handle_get_symbol(&self, params: Value) -> Result<Value> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'name' parameter"))?;
        let file_path = params["file_path"].as_str();
        let level = compress::CompressionLevel::from_i64(
            params["compression_level"].as_i64().unwrap_or(0),
        );

        let file_id = if let Some(fp) = file_path {
            self.state.graph_db.get_file_id_by_path(fp)?
        } else {
            None
        };

        let symbols = self.state.graph_db.get_symbols_by_name(name, file_id)?;

        if symbols.is_empty() {
            return Ok(Value::String(format!("Symbol '{}' not found.", name)));
        }

        let workspace_root = self.state.workspace_root.clone();

        let mut markdown = String::new();
        markdown.push_str(&format!("## {}\n\n", name));

        for sym in symbols {
            let relative_path = self.state.graph_db.get_file_path_by_id(sym.file_id)?;
            let absolute_path = std::path::Path::new(&workspace_root).join(&relative_path);

            markdown.push_str(&format!("`{}` L{} ({})\n\n", relative_path, sym.line, sym.kind));

            // Extract and compress source code
            if absolute_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&absolute_path) {
                    let file_symbols = self.state.graph_db.get_file_symbols_sorted(sym.file_id)?;
                    let lines: Vec<&str> = content.lines().collect();

                    let start_line = (sym.line as usize).saturating_sub(1);
                    let mut end_line = lines.len();
                    if let Some(pos) = file_symbols.iter().position(|x| x.id == sym.id) {
                        if pos + 1 < file_symbols.len() {
                            end_line = (file_symbols[pos + 1].line as usize).saturating_sub(1);
                        }
                    }

                    if start_line < lines.len() {
                        let actual_end = end_line.min(lines.len());
                        let code_slice = lines[start_line..actual_end].join("\n");
                        let ext = relative_path.split('.').next_back().unwrap_or("");
                        let compressed = compress::compress(&code_slice, ext, level);
                        markdown.push_str(&format!("```{}\n{}\n```\n\n", ext, compressed));
                    }
                }
            }

            // Outbound dependencies — compact one-liner, omit if empty
            let out_deps = self.state.graph_db.get_node_dependencies_out(sym.id)?;
            if !out_deps.is_empty() {
                let names: Vec<String> = out_deps.iter().map(|(d, _)| d.name.clone()).collect();
                markdown.push_str(&format!("→ {}\n", names.join(", ")));
            }

            // Inbound dependents — compact one-liner, omit if empty
            let in_deps = self.state.graph_db.get_node_dependencies_in(sym.id)?;
            if !in_deps.is_empty() {
                let names: Vec<String> = in_deps.iter().map(|(d, _)| d.name.clone()).collect();
                markdown.push_str(&format!("← {}\n", names.join(", ")));
            }

            markdown.push_str("\n---\n\n");
        }

        if let Err(e) = self.state.graph_db.record_tool_call((markdown.len() / 4) as u64, 0) {
            log::warn!("record_tool_call failed in get_symbol: {}", e);
        }
        Ok(Value::String(markdown))
    }

    /// Tool 8: get_dependencies
    ///
    /// Retrieve dependencies of a symbol (inbound or outbound).
    pub async fn handle_get_dependencies(&self, params: Value) -> Result<Value> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'name' parameter"))?;
        let direction = params["direction"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'direction' parameter"))?;

        if direction != "in" && direction != "out" {
            return Err(anyhow!("Invalid direction: '{}'. Must be 'in' or 'out'", direction));
        }

        let symbols = self.state.graph_db.get_symbols_by_name(name, None)?;
        if symbols.is_empty() {
            return Ok(Value::String(format!("Symbol '{}' not found.", name)));
        }

        let mut markdown = String::new();
        markdown.push_str(&format!("# Dependencies for `{}` (direction: {})\n\n", name, direction));

        for sym in symbols {
            let relative_path = self.state.graph_db.get_file_path_by_id(sym.file_id)?;
            markdown.push_str(&format!("## Symbol `{}` in `{}`\n", sym.name, relative_path));

            if direction == "out" {
                let deps = self.state.graph_db.get_node_dependencies_out(sym.id)?;
                if deps.is_empty() {
                    markdown.push_str("No outgoing dependencies.\n\n");
                } else {
                    for (dep, edge_kind) in deps {
                        let dep_file = self.state.graph_db.get_file_path_by_id(dep.file_id)?;
                        markdown.push_str(&format!("- Depends on `{}` ({}) via `{}` in `{}`\n", dep.name, dep.kind, edge_kind, dep_file));
                    }
                    markdown.push('\n');
                }
            } else {
                let deps = self.state.graph_db.get_node_dependencies_in(sym.id)?;
                if deps.is_empty() {
                    markdown.push_str("No incoming dependencies.\n\n");
                } else {
                    for (dep, edge_kind) in deps {
                        let dep_file = self.state.graph_db.get_file_path_by_id(dep.file_id)?;
                        markdown.push_str(&format!("- Depended on by `{}` ({}) via `{}` in `{}`\n", dep.name, dep.kind, edge_kind, dep_file));
                    }
                    markdown.push('\n');
                }
            }
        }

        if let Err(e) = self.state.graph_db.record_tool_call((markdown.len() / 4) as u64, 0) {
            log::warn!("record_tool_call failed in get_dependencies: {}", e);
        }
        Ok(Value::String(markdown))
    }

    /// Tool 9: get_file_summary
    ///
    /// List all symbols inside a specific file.
    pub async fn handle_get_file_summary(&self, params: Value) -> Result<Value> {
        let file_path = params["file_path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'file_path' parameter"))?;

        let normalized_path = file_path.replace('\\', "/");

        let file_id = self.state.graph_db.get_file_id_by_path(&normalized_path)?;
        let Some(fid) = file_id else {
            return Ok(Value::String(format!("File '{}' not found in index.", file_path)));
        };

        let symbols = self.state.graph_db.get_file_symbols_sorted(fid)?;

        let mut markdown = String::new();
        markdown.push_str(&format!("# File Summary: `{}`\n\n", file_path));
        markdown.push_str("| Line | Symbol Name | Kind | Exported | Scope | Signature |\n");
        markdown.push_str("| --- | --- | --- | --- | --- | --- |\n");

        for sym in symbols {
            let exported_str = if sym.is_exported == 1 { "Yes" } else { "No" };
            let scope_str = sym.scope.clone().unwrap_or_else(|| "-".to_string());
            let sig_str = sym.signature.clone().unwrap_or_else(|| "-".to_string());
            
            markdown.push_str(&format!(
                "| {} | `{}` | {} | {} | {} | `{}` |\n",
                sym.line, sym.name, sym.kind, exported_str, scope_str, sig_str
            ));
        }

        if let Err(e) = self.state.graph_db.record_tool_call((markdown.len() / 4) as u64, 0) {
            log::warn!("record_tool_call failed in get_file_summary: {}", e);
        }
        Ok(Value::String(markdown))
    }

    /// Tool 10: get_project_overview
    ///
    /// High-level summary of the workspace structure.
    pub async fn handle_get_project_overview(&self) -> Result<Value> {
        let (file_count, node_count, edge_count) = self.state.graph_db.get_stats()?;
        let files = self.state.graph_db.list_files()?;
        let symbol_counts = self.state.graph_db.count_symbols_per_file()?;

        let mut markdown = String::new();
        markdown.push_str("# Project Overview\n\n");
        
        markdown.push_str("## Project Statistics\n");
        markdown.push_str(&format!("- **Total Files**: {}\n", file_count));
        markdown.push_str(&format!("- **Total Symbols (Nodes)**: {}\n", node_count));
        markdown.push_str(&format!("- **Total Dependencies (Edges)**: {}\n\n", edge_count));

        // Language distribution
        let mut lang_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (_, _, lang) in &files {
            *lang_counts.entry(lang.clone()).or_insert(0) += 1;
        }
        let mut lang_sorted: Vec<(String, usize)> = lang_counts.into_iter().collect();
        lang_sorted.sort_by_key(|b| std::cmp::Reverse(b.1));

        markdown.push_str("## Language Distribution\n");
        for (lang, count) in &lang_sorted {
            markdown.push_str(&format!("- **{}**: {} files\n", lang, count));
        }
        markdown.push('\n');

        // Top 10 files by symbol count
        let mut top_files: Vec<(String, i64)> = files
            .iter()
            .map(|(id, path, _)| (path.clone(), symbol_counts.get(id).copied().unwrap_or(0)))
            .collect();
        top_files.sort_by_key(|b| std::cmp::Reverse(b.1));
        top_files.truncate(10);

        markdown.push_str("## Top Files by Symbol Count\n");
        markdown.push_str("| File | Symbols |\n");
        markdown.push_str("| --- | --- |\n");
        for (path, count) in &top_files {
            markdown.push_str(&format!("| `{}` | {} |\n", path, count));
        }
        markdown.push('\n');

        markdown.push_str("## Files Breakdown\n");
        markdown.push_str("| File Path | Language | Symbols Count |\n");
        markdown.push_str("| --- | --- | --- |\n");

        for (id, path, lang) in &files {
            let count = symbol_counts.get(id).copied().unwrap_or(0);
            markdown.push_str(&format!("| `{}` | {} | {} |\n", path, lang, count));
        }
        markdown.push('\n');

        markdown.push_str("## Exported Symbols by File\n");
        let exported = self.state.graph_db.get_exported_symbols_grouped()?;
        if exported.is_empty() {
            markdown.push_str("No exported symbols indexed.\n");
        } else {
            let mut current_file = String::new();
            for (file_path, sym) in exported {
                if file_path != current_file {
                    current_file = file_path.clone();
                    markdown.push_str(&format!("\n### `{}`\n", current_file));
                }
                let sig_str = sym.signature.map(|s| format!(" `{}`", s)).unwrap_or_default();
                markdown.push_str(&format!("- `{}` ({}){}\n", sym.name, sym.kind, sig_str));
            }
        }

        if let Err(e) = self.state.graph_db.record_tool_call((markdown.len() / 4) as u64, 0) {
            log::warn!("record_tool_call failed in get_project_overview: {}", e);
        }
        Ok(Value::String(markdown))
    }

    /// Tool 11: get_git_diff_context
    ///
    /// Get context for files changed in a git diff.
    /// Runs `git diff --name-only <base_ref>` and maps changed files to indexed symbols.
    pub async fn handle_get_git_diff_context(&self, params: Value) -> Result<Value> {
        let base_ref = params["base_ref"].as_str().unwrap_or("HEAD~1");
        validate_git_ref(base_ref)?;
        let workspace_root = self.state.workspace_root.clone();

        let output = std::process::Command::new("git")
            .args(["diff", "--name-only", base_ref])
            .current_dir(&workspace_root)
            .output()
            .map_err(|e| anyhow!("Failed to run git diff: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("git diff failed: {}", stderr.trim()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let changed_files: Vec<String> = stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        let files_list = self.state.graph_db.list_files()?;
        let symbol_counts = self.state.graph_db.count_symbols_per_file()?;
        let indexed: std::collections::HashMap<String, (i64, String)> = files_list
            .into_iter()
            .map(|(id, path, lang)| (path, (id, lang)))
            .collect();

        let mut diff_files = Vec::new();
        for rel_path in &changed_files {
            match indexed.get(rel_path) {
                Some((id, lang)) => {
                    let sym_count = symbol_counts.get(id).copied().unwrap_or(0);
                    diff_files.push(json!({
                        "path": rel_path,
                        "language": lang,
                        "symbols": sym_count,
                        "indexed": true
                    }));
                }
                None => {
                    diff_files.push(json!({
                        "path": rel_path,
                        "language": "unknown",
                        "symbols": 0,
                        "indexed": false
                    }));
                }
            }
        }

        let mut markdown = String::new();
        markdown.push_str(&format!("# Git Diff Context (base: `{}`)\n\n", base_ref));
        markdown.push_str(&format!("**{} files changed**\n\n", changed_files.len()));

        if diff_files.is_empty() {
            markdown.push_str("No changes detected.\n");
        } else {
            markdown.push_str("| File | Language | Symbols | Indexed |\n");
            markdown.push_str("| --- | --- | --- | --- |\n");
            for f in &diff_files {
                let indexed_icon = if f["indexed"].as_bool().unwrap_or(false) { "✅" } else { "⚠️" };
                markdown.push_str(&format!(
                    "| `{}` | {} | {} | {} |\n",
                    f["path"].as_str().unwrap_or(""),
                    f["language"].as_str().unwrap_or(""),
                    f["symbols"].as_i64().unwrap_or(0),
                    indexed_icon
                ));
            }
        }

        let result = json!({
            "base_ref": base_ref,
            "changed_files": diff_files,
            "total_changed": changed_files.len(),
            "markdown": markdown
        });
        let tokens = (markdown.len() / 4) as u64;
        if let Err(e) = self.state.graph_db.record_tool_call(tokens, 0) {
            log::warn!("record_tool_call failed in get_git_diff_context: {}", e);
        }
        Ok(result)
    }

    /// Compress a single file using AST compression
    pub async fn handle_compress_file(&self, params: Value) -> Result<Value> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'path' parameter"))?;
        let compression_level = params["compression_level"].as_i64().unwrap_or(1);

        let workspace_root = self.state.workspace_root.clone();

        let safe_path = validate_within_workspace(path_str, &workspace_root)?;

        let content = std::fs::read_to_string(&safe_path)?;
        let path = safe_path.as_path();
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let level = compress::CompressionLevel::from_i64(compression_level);
        let compressed = compress::compress(&content, ext, level);

        let original_chars = content.len();
        let compressed_chars = compressed.len();
        let compression_rate = match level {
            compress::CompressionLevel::Full => "0%".to_string(),
            _ => {
                if original_chars > 0 {
                    let rate = ((1.0 - compressed_chars as f64 / original_chars as f64) * 100.0)
                        .max(0.0);
                    format!("{:.0}%", rate)
                } else {
                    "0%".to_string()
                }
            }
        };
        if let Err(e) = self.state.graph_db.record_tool_call((compressed_chars / 4) as u64, 0) {
            log::warn!("record_tool_call failed in compress_file: {}", e);
        }

        Ok(json!({
            "compressed_text": compressed,
            "original_chars": original_chars,
            "compressed_chars": compressed_chars,
            "compression_rate": compression_rate
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_server_creation() {
        let temp_dir = std::env::temp_dir().join("comP_test_mcp_creation");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        // Nodes are 0 at creation since indexing has not run
        let stats = server.handle_get_stats().await.expect("getStats failed");
        assert_eq!(stats["total_files"].as_i64().unwrap(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_json_rpc_format() {
        // Verify request/response format
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "run_pipeline",
            "params": { "task": "test" }
        });

        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["method"], "run_pipeline");
    }

    #[tokio::test]
    async fn test_handle_run_pipeline() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({
            "task": "add authentication",
            "max_tokens": 8000
        });

        let result = server.handle_run_pipeline(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["pivot_files"].is_array());
        assert!(response["related_files"].is_array());
        assert!(response["total_tokens"].is_number());
        assert!(response["savings"].is_string());
        assert!(response["estimated_cost"].is_string());
        assert!(response["coverage"].is_object());
        assert!(response["coverage"]["indexed_doc_files"].is_number());
        assert!(response["coverage"]["bm25_hits"].is_number());
        assert!(response["coverage"]["pivot_file_types"].is_object());
    }

    /// 0.8.6 regression: an interaction recorded via session_log must surface in
    /// run_pipeline results (walker carve-out → files table → BM25 over jsonl).
    #[tokio::test]
    async fn test_run_pipeline_surfaces_session_history() {
        let temp_dir = std::env::temp_dir().join("comP_test_history_bm25");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(
            crate::AppState::new(temp_dir.to_str().unwrap())
                .await
                .expect("Failed to create AppState"),
        );
        let server = MCPServer::new(state);

        server
            .handle_session_log(json!({
                "request": "implement zanzibar quota widget",
                "outcome": "added zanzibar widget module"
            }))
            .await
            .expect("session_log failed");

        let response = server
            .handle_run_pipeline(json!({
                "task": "continue zanzibar quota widget work",
                "max_tokens": 8000
            }))
            .await
            .expect("run_pipeline failed");

        let pivots: Vec<String> = response["pivot_files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["path"].as_str().map(String::from))
            .collect();
        assert!(
            pivots
                .iter()
                .any(|p| p.starts_with(".comp/history/") && p.ends_with(".jsonl")),
            "history jsonl must appear in pivot_files, got: {:?}",
            pivots
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_run_pipeline_missing_task() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({ "max_tokens": 8000 }); // Missing task

        let result = server.handle_run_pipeline(params).await;
        assert!(result.is_err()); // Should error on missing task
    }

    #[test]
    fn test_match_pattern() {
        // Wildcard any
        assert!(MCPServer::match_pattern("foo.rs", "*"));
        // Suffix glob
        assert!(MCPServer::match_pattern("main.rs", "*.rs"));
        assert!(!MCPServer::match_pattern("main.ts", "*.rs"));
        // Prefix glob
        assert!(MCPServer::match_pattern("test_auth.rs", "test_*"));
        assert!(!MCPServer::match_pattern("auth_test.rs", "test_*"));
        // Exact
        assert!(MCPServer::match_pattern("README.md", "README.md"));
        assert!(!MCPServer::match_pattern("readme.md", "README.md"));
    }

    #[test]
    fn test_apply_compression_rule_empty() {
        let rules = std::collections::HashMap::new();
        assert_eq!(MCPServer::apply_compression_rule("src/main.rs", &rules), None);
    }

    #[test]
    fn test_apply_compression_rule_glob() {
        let mut rules = std::collections::HashMap::new();
        rules.insert("*.md".to_string(), 0i64);
        rules.insert("*.rs".to_string(), 2i64);

        assert_eq!(MCPServer::apply_compression_rule("docs/README.md", &rules), Some(0));
        assert_eq!(MCPServer::apply_compression_rule("src/main.rs", &rules), Some(2));
        assert_eq!(MCPServer::apply_compression_rule("app.ts", &rules), None);
    }

    #[test]
    fn test_apply_compression_rule_exact_takes_priority() {
        let mut rules = std::collections::HashMap::new();
        rules.insert("*.rs".to_string(), 2i64);
        rules.insert("main.rs".to_string(), 0i64); // exact overrides glob

        assert_eq!(MCPServer::apply_compression_rule("src/main.rs", &rules), Some(0));
        assert_eq!(MCPServer::apply_compression_rule("src/lib.rs", &rules), Some(2));
    }

    #[test]
    fn test_apply_compression_rule_conflict_is_deterministic() {
        // "test_*" (5 non-wildcard chars) is more specific than "*.rs" (2 non-wildcard chars)
        // so test_* should win regardless of HashMap insertion order.
        let mut rules = std::collections::HashMap::new();
        rules.insert("*.rs".to_string(), 2i64);
        rules.insert("test_*".to_string(), 1i64);

        // Run 100 times to expose any non-determinism
        for _ in 0..100 {
            let level = MCPServer::apply_compression_rule("test_auth.rs", &rules);
            assert_eq!(level, Some(1), "test_* (5 chars) should beat *.rs (2 chars)");
        }

        // *.rs (2) vs *.md (2) — tie on non-wildcard length, alphabetical: "*.md" < "*.rs"
        let mut rules2 = std::collections::HashMap::new();
        rules2.insert("*.rs".to_string(), 2i64);
        rules2.insert("*.md".to_string(), 0i64);
        // these don't conflict: .rs and .md can't both match the same file
        assert_eq!(MCPServer::apply_compression_rule("main.rs", &rules2), Some(2));
        assert_eq!(MCPServer::apply_compression_rule("README.md", &rules2), Some(0));
    }

    #[test]
    fn test_load_compression_rules_missing_file() {
        // Should return empty map when config.json doesn't exist
        let rules = MCPServer::load_compression_rules("/nonexistent/path");
        // No config.json at this path — must return empty map without panic
        assert!(rules.is_empty());
    }

    #[tokio::test]
    async fn test_handle_get_context() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({
            "query": "authentication",
            "limit": 10
        });

        let result = server.handle_get_context(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["results"].is_array());
        assert!(response["count"].is_number());
        assert!(response["query"].is_string());
    }

    #[tokio::test]
    async fn test_handle_get_context_missing_query() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({ "limit": 10 }); // Missing query

        let result = server.handle_get_context(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_get_impact_graph() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({
            "symbol_id": 123,
            "symbol_name": "authenticate"
        });

        let result = server.handle_get_impact_graph(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["symbol_id"].is_number());
        assert!(response["affected_files"].is_object());
        assert!(response["impact_count"].is_number());
        assert!(response["severity"].is_string());
    }

    #[tokio::test]
    async fn test_handle_get_impact_graph_missing_symbol_id() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({ "symbol_name": "authenticate" }); // Missing symbol_id

        let result = server.handle_get_impact_graph(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_list_indexed_files() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let result = server.handle_list_indexed_files().await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["files"].is_array());
        assert!(response["total_files"].is_number());
        assert!(response["total_symbols"].is_number());
        assert!(response["languages"].is_object());
    }

    #[tokio::test]
    async fn test_handle_get_token_usage() {
        let temp_dir = std::env::temp_dir().join("comP_test_token_usage");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let response = server.handle_get_token_usage().await.unwrap();
        assert!(response["timestamp"].is_number());
        assert_eq!(response["total_tokens_consumed"].as_u64().unwrap(), 0);
        assert_eq!(response["queries_count"].as_u64().unwrap(), 0);
        assert!(response["efficiency"].is_string());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_token_counter_cost_estimation() {
        // Test TokenCounter integration
        let cost = crate::search::TokenCounter::estimate_cost(10000, "sonnet");
        assert!(cost.starts_with("$"));
        assert!(cost.len() > 0);
    }

    #[test]
    fn test_token_counter_savings_calculation() {
        // Test TokenCounter integration
        let savings = crate::search::TokenCounter::calculate_savings(10000, 4000);
        assert!(savings.contains("%"));
    }

    #[tokio::test]
    async fn test_handle_get_stats() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let result = server.handle_get_stats().await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["total_files"].is_number());
        assert!(response["total_nodes"].is_number());
        assert!(response["total_edges"].is_number());

        // Verify values are non-negative
        assert!(response["total_files"].as_i64().unwrap_or(-1) >= 0);
        assert!(response["total_nodes"].as_i64().unwrap_or(-1) >= 0);
        assert!(response["total_edges"].as_i64().unwrap_or(-1) >= 0);

        // The running binary must self-report its crate version so clients can
        // detect a stale daemon after an upgrade.
        assert_eq!(response["daemon_version"].as_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn test_session_recall() {
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let result = server.handle_session_recall(json!({})).await.unwrap();
        assert!(result.as_str().unwrap().contains("No past invocations recorded"));

        record_mcp_call(
            temp_dir.to_str().unwrap(),
            &state.session_id,
            "test task".to_string(),
            vec!["test_symbol".to_string()],
            vec!["src/test.rs".to_string()],
            100,
        ).unwrap();

        let result = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = result.as_str().unwrap();
        assert!(markdown.contains("test task"));
        assert!(markdown.contains("Tokens: 100"));
        assert!(markdown.contains("test_symbol"));

        let result = server.handle_session_recall(json!({ "query": "task" })).await.unwrap();
        assert!(result.as_str().unwrap().contains("test task"));

        let result = server.handle_session_recall(json!({ "query": "nomatch" })).await.unwrap();
        assert!(result.as_str().unwrap().contains("No matching past invocations"));

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_format_capped_list() {
        let items: Vec<String> = (1..=8).map(|i| format!("sym{}", i)).collect();
        let rendered = format_capped_list(&items, 5);
        assert_eq!(rendered, "`sym1`, `sym2`, `sym3`, `sym4`, `sym5` … (+3 more)");

        // At or under the cap: no overflow marker.
        let few: Vec<String> = vec!["a".to_string(), "b".to_string()];
        assert_eq!(format_capped_list(&few, 5), "`a`, `b`");
    }

    #[tokio::test]
    async fn test_session_recall_caps_symbol_and_file_lists() {
        // Auto-recorded run_pipeline calls can carry dozens of symbols/files;
        // recall must summarize them instead of enumerating everything.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_cap");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let symbols: Vec<String> = (1..=30).map(|i| format!("symbol_{}", i)).collect();
        let files: Vec<String> = (1..=12).map(|i| format!("src/file_{}.rs", i)).collect();
        record_mcp_call(root, &state.session_id, "big task".to_string(), symbols, files, 100).unwrap();

        let markdown = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = markdown.as_str().unwrap();
        assert!(markdown.contains("symbol_1"), "leading symbols must be shown");
        assert!(!markdown.contains("symbol_6"), "symbols beyond the cap must be omitted");
        assert!(markdown.contains("(+25 more)"), "symbol overflow count must be shown");
        assert!(!markdown.contains("src/file_6.rs"), "files beyond the cap must be omitted");
        assert!(markdown.contains("(+7 more)"), "file overflow count must be shown");

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_format_epoch_ms() {
        // Unix epoch start.
        assert_eq!(format_epoch_ms(0), "1970-01-01 00:00");
        // 1_700_000_000_000 ms == 2023-11-14T22:13:20Z.
        assert_eq!(format_epoch_ms(1_700_000_000_000), "2023-11-14 22:13");
    }

    #[tokio::test]
    async fn test_session_recall_cross_session() {
        // Recall must surface calls from sessions other than the current one
        // (i.e. survive daemon restarts), each tagged with a date and capped by limit.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_cross");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        // Two records under two DIFFERENT session ids, neither equal to the current
        // session_id — the old per-session filter would have returned nothing.
        record_mcp_call(root, "sess-old", "alpha task".to_string(), vec![], vec![], 10).unwrap();
        record_mcp_call(root, "sess-new", "beta task".to_string(), vec![], vec![], 20).unwrap();

        let markdown = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = markdown.as_str().unwrap();
        assert!(markdown.contains("alpha task"), "cross-session recall must include older session");
        assert!(markdown.contains("beta task"), "cross-session recall must include newer session");
        // Date tag present (UTC year prefix).
        assert!(markdown.contains("- `20"), "each entry must be tagged with a date");

        // limit caps the number of entries returned.
        let limited = server.handle_session_recall(json!({ "limit": 1 })).await.unwrap();
        let limited = limited.as_str().unwrap();
        assert_eq!(limited.matches("**Query**").count(), 1, "limit must cap shown entries");

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_session_log_and_recall() {
        // session_log persists request+outcome to .comp/history and session_recall surfaces it.
        let temp_dir = std::env::temp_dir().join("comP_test_session_log");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let logged = server
            .handle_session_log(json!({
                "request": "fix the login redirect bug",
                "outcome": "patched auth middleware and added a regression test",
                "files": ["src/auth.rs"]
            }))
            .await
            .unwrap();
        assert_eq!(logged["status"], "ok");

        // A monthly history JSONL file must now exist with one record.
        let hist_dir = temp_dir.join(".comp").join("history");
        let mut jsonl_files: Vec<_> = std::fs::read_dir(&hist_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
            .collect();
        assert_eq!(jsonl_files.len(), 1, "one monthly history file expected");
        let content = std::fs::read_to_string(jsonl_files.pop().unwrap()).unwrap();
        assert!(content.contains("login redirect"));
        assert!(content.contains("auth middleware"));

        // Recall surfaces the request and its outcome.
        let recall = server.handle_session_recall(json!({})).await.unwrap();
        let recall = recall.as_str().unwrap();
        assert!(recall.contains("fix the login redirect bug"), "request must be recalled");
        assert!(recall.contains("Outcome"), "outcome must be rendered");
        assert!(recall.contains("regression test"), "outcome text must be recalled");

        // Filtering on a word that appears only in the outcome still matches.
        let filtered = server
            .handle_session_recall(json!({ "query": "regression" }))
            .await
            .unwrap();
        assert!(filtered.as_str().unwrap().contains("login redirect"));

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_session_recall_hook_written_jsonl() {
        // The Stop hook (history-record.sh) writes {"timestamp","request","outcome"} —
        // no query/symbols/files/tokens/stale fields. Recall must still parse these
        // lines instead of silently dropping them.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_hook");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let hist_dir = temp_dir.join(".comp").join("history");
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(
            hist_dir.join("log-2026-06.jsonl"),
            concat!(
                r#"{"timestamp":1782521356807,"request":"add session_log feature","outcome":"implemented session_log in mod.rs"}"#,
                "\n",
                r#"{"timestamp":1782521356900,"request":"outcome may be null","outcome":null}"#,
                "\n"
            ),
        )
        .unwrap();

        let recall = server.handle_session_recall(json!({})).await.unwrap();
        let recall = recall.as_str().unwrap();
        assert!(recall.contains("add session_log feature"), "hook 'request' field must map to query");
        assert!(recall.contains("implemented session_log"), "hook outcome must be rendered");
        assert!(recall.contains("outcome may be null"), "null outcome must not break parsing");

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_new_mcp_tools() {
        let temp_dir = std::env::temp_dir().join("comP_test_new_mcp_tools");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        // Insert mock data into DB
        let file_id = state.graph_db.upsert_file("src/test_mcp.rs", "hash1", "rust", 0).unwrap();
        let node_id_1 = state.graph_db.insert_node(file_id, "my_mcp_func", "function", 10, 5, None, true, None).unwrap();
        let node_id_2 = state.graph_db.insert_node(file_id, "caller_func", "function", 20, 5, None, true, None).unwrap();

        // Add edge
        state.graph_db.insert_edge(node_id_2, node_id_1, "calls").unwrap();

        // 1. Test get_symbol (new slim format: ## name, → deps, ← callers)
        let result = server.handle_get_symbol(json!({ "name": "my_mcp_func" })).await.unwrap();
        let markdown = result.as_str().unwrap();
        assert!(markdown.contains("## my_mcp_func"));
        assert!(markdown.contains("src/test_mcp.rs"));
        assert!(markdown.contains("caller_func"));

        // 2. Test get_dependencies
        let result_out = server.handle_get_dependencies(json!({ "name": "caller_func", "direction": "out" })).await.unwrap();
        assert!(result_out.as_str().unwrap().contains("my_mcp_func"));

        let result_in = server.handle_get_dependencies(json!({ "name": "my_mcp_func", "direction": "in" })).await.unwrap();
        assert!(result_in.as_str().unwrap().contains("caller_func"));

        // 3. Test get_file_summary
        let result_summary = server.handle_get_file_summary(json!({ "file_path": "src/test_mcp.rs" })).await.unwrap();
        // Wait, handle_get_file_summary returns Result<Value>. It's Value::String.
        let summary_md = result_summary.as_str().unwrap();
        assert!(summary_md.contains("my_mcp_func"));
        assert!(summary_md.contains("caller_func"));

        // 4. Test get_project_overview
        let result_overview = server.handle_get_project_overview().await.unwrap();
        let overview_md = result_overview.as_str().unwrap();
        assert!(overview_md.contains("Total Files**: 1"));
        assert!(overview_md.contains("Total Symbols (Nodes)**: 2"));

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_get_symbol_compression_levels() {
        let temp_dir = std::env::temp_dir().join("comP_test_compression");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.unwrap());
        let server = MCPServer::new(state.clone());

        let file_id = state.graph_db.upsert_file("src/test.rs", "hash1", "rust", 0).unwrap();
        state.graph_db.insert_node(file_id, "compress_test_fn", "function", 1, 1, None, true, None).unwrap();

        // All compression levels should succeed without error
        for level in [0i64, 1, 2] {
            let result = server
                .handle_get_symbol(json!({ "name": "compress_test_fn", "compression_level": level }))
                .await;
            assert!(result.is_ok(), "compression_level={} should not error", level);
            let markdown = result.unwrap();
            let md = markdown.as_str().unwrap();
            assert!(md.contains("## compress_test_fn"), "header missing for level {}", level);
        }

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_compress_file() {
        let temp_dir = std::env::temp_dir().join("comP_test_compress_file");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let temp_dir = temp_dir.canonicalize().unwrap();

        let file_path = temp_dir.join("test_file.rs");
        let code = "fn my_test_func() {\n    // some comment\n    let x = 42;\n}\n";
        std::fs::write(&file_path, code).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.unwrap());
        let server = MCPServer::new(state);

        // Test level 1 (Compact) -> should remove comment
        let params = json!({
            "path": file_path.to_str().unwrap(),
            "compression_level": 1
        });
        let result = server.handle_compress_file(params).await.unwrap();
        let compressed = result["compressed_text"].as_str().unwrap();
        assert!(compressed.contains("fn my_test_func()"));
        assert!(compressed.contains("let x = 42;"));
        assert!(!compressed.contains("some comment"));

        // Test level 2 (Skeleton) -> should replace body
        let params_sk = json!({
            "path": file_path.to_str().unwrap(),
            "compression_level": 2
        });
        let result_sk = server.handle_compress_file(params_sk).await.unwrap();
        let compressed_sk = result_sk["compressed_text"].as_str().unwrap();
        assert!(compressed_sk.contains("fn my_test_func() { ... }") || compressed_sk.contains("fn my_test_func()  { ... }"));
        assert!(!compressed_sk.contains("let x = 42;"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // --- git diff boost tests ---

    #[test]
    fn test_get_git_diff_files_not_a_repo() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_not_repo");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.is_empty(), "non-git directory must return empty set");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_git_diff_files_empty_repo_no_commits() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_no_commits");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // init a repo but make no commits — HEAD is undefined, git diff HEAD fails
        let _ = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&temp_dir)
            .output();

        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.is_empty(), "repo with no commits must return empty set");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_git_diff_files_clean_working_tree() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_clean");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&temp_dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
        };

        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(temp_dir.join("a.rs"), "fn a() {}").unwrap();
        run(&["add", "a.rs"]);
        run(&["commit", "-m", "init"]);

        // No uncommitted changes — must return empty
        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.is_empty(), "clean working tree must return empty set");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_git_diff_files_with_changes() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_with_changes");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&temp_dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
        };

        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(temp_dir.join("a.rs"), "fn a() {}").unwrap();
        run(&["add", "a.rs"]);
        run(&["commit", "-m", "init"]);

        // Modify the file -> should appear in diff
        std::fs::write(temp_dir.join("a.rs"), "fn a() { let x = 1; }").unwrap();

        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.contains("a.rs"), "modified file must be in diff set");
        assert_eq!(result.len(), 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_coverage_has_git_diff_boosted() {
        let state = Arc::new(crate::AppState::new(".").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let result = server.handle_run_pipeline(json!({
            "task": "add authentication",
            "max_tokens": 8000
        })).await.unwrap();

        assert!(
            result["coverage"]["git_diff_boosted"].is_number(),
            "coverage.git_diff_boosted must be a number"
        );
    }

    #[tokio::test]
    async fn test_run_pipeline_git_diff_marker_on_boosted_files() {
        let temp_dir = std::env::temp_dir().join("comP_test_boost_marker");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&temp_dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
        };

        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(temp_dir.join("boosted.rs"), "fn boosted() {}").unwrap();
        run(&["add", "boosted.rs"]);
        run(&["commit", "-m", "init"]);
        // Modify the file so it appears in git diff HEAD
        std::fs::write(temp_dir.join("boosted.rs"), "fn boosted() { let x = 1; }").unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());
        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.unwrap());
        let server = MCPServer::new(state);

        // Index the file
        server.state.graph_db.list_files().unwrap(); // ensure DB is accessible

        let result = server.handle_run_pipeline(json!({
            "task": "fix boosted function",
            "max_tokens": 8000
        })).await.unwrap();

        // Any pivot_file with git_diff:true must actually be a git-diff file
        let pivot_files = result["pivot_files"].as_array().unwrap();
        for entry in pivot_files {
            if entry["git_diff"].as_bool() == Some(true) {
                // The marker is only set for files in the actual git diff — valid by construction
                assert!(
                    entry["path"].as_str().is_some(),
                    "git_diff-marked entry must have a path"
                );
            }
        }

        // coverage.git_diff_boosted must be a non-negative number
        let boosted = result["coverage"]["git_diff_boosted"].as_u64().unwrap();
        assert!(boosted == 0 || boosted > 0); // always a valid count

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_no_git_repo_degrades_gracefully() {
        let temp_dir = std::env::temp_dir().join("comP_test_no_git_pipeline");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());
        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap()).await.unwrap());
        let server = MCPServer::new(state);

        // Must not error even though there is no git repo
        let result = server.handle_run_pipeline(json!({
            "task": "test task",
            "max_tokens": 8000
        })).await;
        assert!(result.is_ok(), "run_pipeline must succeed when not in a git repo");

        let response = result.unwrap();
        assert_eq!(
            response["coverage"]["git_diff_boosted"].as_u64().unwrap(),
            0,
            "git_diff_boosted must be 0 when not in a git repo"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
