use super::*;

impl MCPServer {
    /// Tool: mark_stale
    ///
    /// Mark every recorded call across every agent's session-memory file as
    /// `stale` if it mentions `path` (absolute, or relative to the workspace
    /// root; either slash style). Called by the VSCode extension's file-system watcher
    /// whenever a source file changes or is deleted — see `extension.ts`'s
    /// `setupFileWatchers`, which used to do this by instantiating
    /// `SessionMemoryManager` and writing `.comp/session-memory.json`
    /// directly (the single file the per-agent split made dead; see
    /// `mark_stale_in_file`'s doc). Not part of the original 5-tool set, so it
    /// is deliberately absent from `tools/list` — only the VSCode extension's
    /// own JSON-RPC client calls this method, never an AI agent.
    pub async fn handle_mark_stale(&self, params: Value) -> Result<Value> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'path' parameter"))?;
        // WHY absolute paths are converted: the file watcher sends the path VS Code
        // reports, and session memory records the stored form (relative for the
        // main root, absolute for an additional root). A file in no root was never
        // recorded, so there is nothing to mark.
        let relative_path = if crate::indexer::roots::looks_absolute(path) {
            match crate::indexer::roots::Roots::load(&self.state.workspace_root).locate(path) {
                Some(located) => located.stored_path,
                None => return Ok(json!({ "marked": 0 })),
            }
        } else {
            path.replace('\\', "/")
        };

        let memory_dir = std::path::Path::new(&self.state.workspace_root)
            .join(".comp")
            .join("session-memory");
        let mut marked = 0usize;
        if let Ok(entries) = std::fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match mark_stale_in_file(&path, &relative_path) {
                    Ok(n) => marked += n,
                    Err(e) => log::warn!("mark_stale: failed to update {}: {}", path.display(), e),
                }
            }
        }

        Ok(json!({ "marked": marked }))
    }

    /// Tool 6: session_recall
    ///
    /// Recall past MCP tool invocations for the current session.
    pub async fn handle_session_recall(&self, params: Value) -> Result<Value> {
        let query_filter = params["query"].as_str().map(|q| q.to_lowercase());
        let limit = params["limit"].as_u64().unwrap_or(20) as usize;

        let no_result_msg = if query_filter.is_some() {
            "No matching past invocations found for the query."
        } else {
            "No past invocations recorded."
        };

        // WHY: recall must survive daemon restarts / session breaks, AND merge the work of
        // every agent that has touched this workspace. Gather from BOTH stores:
        //   1. .comp/session-memory/*.json — one file per agent (see get_session_memory_path);
        //      each is single-writer, so this is a read-only fan-in across files, not a shared
        //      mutable store. Auto-recorded run_pipeline/get_context query records.
        //   2. .comp/history/*.jsonl — explicit interaction logs (request + outcome) written by
        //      session_log / the Stop hook (any agent may append), which also feed BM25 recall
        //      via run_pipeline.
        // Flatten everything across ALL agents/sessions and show newest-first.
        let mut calls: Vec<SessionCall> = Vec::new();

        // WHY tracked instead of silently skipped: a file/line that fails to read
        // or parse here is evidence of the exact corruption this recovery work is
        // about — dropping it with no trace would make comP's own diagnostic tool
        // blind to the condition it exists to help investigate (Phase 2, A8).
        let mut unreadable_memory_files = 0usize;
        let mut unreadable_history_lines = 0usize;

        let memory_dir = std::path::Path::new(&self.state.workspace_root)
            .join(".comp")
            .join("session-memory");
        if let Ok(entries) = std::fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match std::fs::File::open(&p) {
                    Ok(file) => {
                        let reader = std::io::BufReader::new(file);
                        match serde_json::from_reader::<_, SessionMemory>(reader) {
                            Ok(memory) => {
                                for session in memory.sessions {
                                    calls.extend(session.calls);
                                }
                            }
                            Err(_) => unreadable_memory_files += 1,
                        }
                    }
                    Err(_) => unreadable_memory_files += 1,
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
                        match serde_json::from_str::<SessionCall>(line) {
                            Ok(c) => calls.push(c),
                            Err(_) => unreadable_history_lines += 1,
                        }
                    }
                }
            }
        }

        calls.sort_by_key(|c| std::cmp::Reverse(c.timestamp));

        let mut markdown = String::new();
        markdown.push_str("### Session Recall\n\n");
        if unreadable_memory_files > 0 || unreadable_history_lines > 0 {
            markdown.push_str(&format!(
                "⚠ {} session-memory file(s) and {} history line(s) could not be read (corrupted or malformed) and are excluded below.\n\n",
                unreadable_memory_files, unreadable_history_lines
            ));
        }

        // WHY ranked rather than filtered: a substring filter misses paraphrases and
        // multi-word queries (Japanese has no spaces to split on). Substring hits keep
        // their old place at the top; char-bigram BM25 hits follow (trace::rank_by_query).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let order: Vec<usize> = match query_filter {
            Some(ref q) => trace::rank_by_query(&calls, q, now_ms),
            None => (0..calls.len()).collect(),
        };

        let mut shown = 0;
        for &idx in &order {
            let call = &calls[idx];
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
            if !call.rules.is_empty() {
                let refs: Vec<String> = call
                    .rules
                    .iter()
                    .map(|r| format!("{}#{} ({})", r.file, r.heading, &r.hash[..r.hash.len().min(8)]))
                    .collect();
                markdown.push_str(&format!("  - **Rules**: {}\n", format_capped_list(&refs, RECALL_LIST_CAP)));
            }
            // Multi-agent trace: which delegations a turn ran, and which turn ran a delegation.
            match call.kind.as_deref() {
                Some("turn") => {
                    if let Some(ref c) = call.commit {
                        markdown.push_str(&format!("  - **Commit**: {}\n", &c[..c.len().min(8)]));
                    }
                    let children: Vec<String> = trace::delegations_for_turn(&calls, idx)
                        .iter()
                        .map(|&d| match calls[d].test_exit {
                            Some(code) => format!("{} (test_exit {})", calls[d].agent, code),
                            None => calls[d].agent.clone(),
                        })
                        .collect();
                    if !children.is_empty() {
                        markdown.push_str(&format!(
                            "  - **Delegations**: {}\n",
                            format_capped_list(&children, RECALL_LIST_CAP)
                        ));
                    }
                }
                Some("delegation") => {
                    if let Some(p) = trace::parent_turn_of(&calls, idx) {
                        let parent: String = calls[p].query.chars().take(60).collect();
                        markdown.push_str(&format!("  - **Delegated by**: \"{}\"\n", parent));
                    }
                }
                _ => {}
            }
        }

        if shown == 0 {
            markdown.push_str(no_result_msg);
        }

        Ok(Value::String(markdown))
    }

    /// Tool: check_rule_conflicts (rule sharing, beta)
    ///
    /// Candidate pairs of possibly contradicting sections across agents'
    /// instruction files. Off → `{"disabled": ...}` (a result, not an error, so
    /// an agent learns how to turn it on); not a git repository →
    /// `{"unavailable": ...}`. An out-of-range `max_pairs` is an error.
    pub async fn handle_check_rule_conflicts(&self, params: Value) -> Result<Value> {
        let max_pairs = match params.get("max_pairs") {
            None | Some(Value::Null) => 20,
            Some(v) => match v.as_u64() {
                Some(n) if (1..=100).contains(&n) => n as usize,
                _ => return Err(anyhow!("'max_pairs' must be an integer between 1 and 100")),
            },
        };
        let ws = std::path::Path::new(&self.state.workspace_root);
        if !rules::rule_sharing_enabled(ws) {
            return Ok(json!({ "disabled": "rule sharing (beta) is off — enable comp.ruleSharing.enabled in VS Code settings" }));
        }
        let sections = match rules::load_sections(ws, &rules::git_tracked_files) {
            Ok(s) => s,
            Err(reason) => return Ok(json!({ "unavailable": reason })),
        };
        let mut files: Vec<String> = sections.iter().map(|s| s.file.clone()).collect();
        files.sort();
        files.dedup();
        let pairs: Vec<Value> = rules::conflict_candidates(&sections, max_pairs)
            .iter()
            .map(|p| json!({ "a": section_json(&p.a), "b": section_json(&p.b), "similarity": p.similarity }))
            .collect();
        Ok(json!({ "note": CONFLICT_NOTE, "pairs": pairs, "files_scanned": files }))
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
            .ok_or_else(|| anyhow!("Missing 'request' parameter"))?;
        // Redact at write time: the history file is plain text that outlives the session.
        let request = trace::redact_secrets(request);
        let outcome = params["outcome"].as_str().map(trace::redact_secrets);
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
            agent: self.state.agent_id.clone(),
            rules: Vec::new(),
            ..Default::default()
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
        // WHY spawn_blocking: append_history_line takes an OS advisory lock (fs4,
        // synchronous API) around a single write_all — a blocking call that must not
        // run directly on the async runtime's worker thread.
        let lock_path = hist_path.clone();
        tokio::task::spawn_blocking(move || append_history_line(&lock_path, &line)).await??;

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
}
