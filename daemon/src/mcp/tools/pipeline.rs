use super::*;

impl MCPServer {
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

        // Per-keyword hit count feeds the LIKE relevance score below (see
        // `like_score_for`): a keyword that matches many files (e.g. "add",
        // "new", "test") is generic and should count for less than one that
        // matches only a handful (e.g. "jwt", "lru"). This is what keeps a
        // header-rich Markdown file (many headings = many chances to match a
        // generic word) from outranking a small, precisely on-topic file —
        // see https://github.com/tsucky230/comP/issues/7.
        let mut like_hit_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut like_matched_keywords: std::collections::HashMap<String, std::collections::HashSet<String>> = std::collections::HashMap::new();
        // WHY 50 not 5: this count also feeds `like_score_for`'s specificity
        // weighting (1 / hit_count). Capping too low (an earlier version used
        // 5) makes every keyword that matches more than a handful of files
        // bottom out at the same floor weight, blunting the distinction
        // between "somewhat common" and "extremely generic" keywords — found
        // during Gemini cross-review of this fix (issue #7).
        const LIKE_QUERY_LIMIT: usize = 50;

        let mut all_hits: Vec<(String, String, String, i32)> = if keywords.is_empty() {
            let task_hits = self.state.graph_db.search_symbols_by_name(task, 10)?;
            // WHY: without this, a query with no word >= 3 chars (e.g. "s3",
            // "db") left like_hit_counts/like_matched_keywords empty, so
            // like_score_for returned 0.0 for every file — found during Gemini
            // cross-review. That silently broke the doc/code weight
            // renormalization below too, since it assumes "no BM25 signal"
            // only happens for non-doc files; treat the whole task as one
            // keyword so this branch is scored the same way as the normal one.
            let key = task.to_lowercase();
            like_hit_counts.insert(key.clone(), task_hits.len().max(1));
            for hit in &task_hits {
                like_matched_keywords.entry(hit.0.clone()).or_default().insert(key.clone());
            }
            task_hits
        } else {
            let mut merged = Vec::new();
            for kw in &keywords {
                let kw_hits = self.state.graph_db.search_symbols_by_name(kw, LIKE_QUERY_LIMIT)?;
                like_hit_counts.insert(kw.to_lowercase(), kw_hits.len().max(1));
                for hit in &kw_hits {
                    like_matched_keywords
                        .entry(hit.0.clone())
                        .or_default()
                        .insert(kw.to_lowercase());
                }
                merged.extend(kw_hits);
            }
            merged
        };
        // Augment LIKE hits with TF-IDF semantic results (may find files not matched by exact LIKE)
        let mut tfidf_scores: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        {
            let se = self.state.search_engine.lock().await;
            for hit in se.search(task, 20).unwrap_or_default() {
                tfidf_scores.entry(hit.file_path.clone()).or_insert(hit.score);
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
        let mut bm25_scores: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        // WHY track this explicitly rather than re-deriving it below: when
        // BM25 never ran at all for this query (no doc files in the corpus,
        // or no keyword long enough to search — e.g. task = "s3"), doc-type
        // files must NOT be scored via the BM25-inclusive formula below with
        // an implicit bm25=0.0. That would silently apply a smaller effective
        // weight to every doc file than every code file gets from the
        // renormalized (0.4*like+0.3*tfidf)/0.7 formula, reintroducing a bias
        // in the opposite direction — found during Gemini cross-review.
        let bm25_available = !doc_paths.is_empty() && !keywords.is_empty();
        if bm25_available {
            let workspace_root = self.state.workspace_root.clone();
            let bm25_hits = crate::indexer::doc_parser::Bm25Scorer::search_files(
                &workspace_root,
                &doc_paths,
                &keywords,
                20,
            );
            bm25_hit_count = bm25_hits.len();
            for (path, score) in &bm25_hits {
                bm25_scores.insert(path.clone(), *score);
            }
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

        // Rank all collected candidates by a single normalized relevance score
        // combining LIKE-keyword specificity, TF-IDF cosine similarity, and
        // BM25 full-text relevance (see issue #7: candidates previously kept
        // whatever order they were first inserted in, with no actual scoring).
        //
        // BM25's raw score is unbounded and is saturated into [0, 1) via
        // score/(score+K) rather than min-max normalized against this call's
        // own hits: with min-max, a file that happens to be the *only* BM25
        // hit for a query always normalizes to a perfect 1.0 regardless of how
        // weak its actual match is, which is a common case (most queries only
        // surface one relevant doc file) and would silently re-inflate exactly
        // the doc-file bias this rework is meant to remove.
        const BM25_SATURATION_K: f64 = 2.0;
        let doc_path_set: std::collections::HashSet<&str> =
            doc_paths.iter().map(|s| s.as_str()).collect();
        let like_score_for = |file: &str| -> f32 {
            match like_matched_keywords.get(file) {
                None => 0.0,
                Some(matched) => {
                    let raw: f32 = matched
                        .iter()
                        .map(|kw| 1.0 / *like_hit_counts.get(kw).unwrap_or(&1) as f32)
                        .sum();
                    // WHY length-penalize: a file with many symbols (e.g. every
                    // heading in a header-rich Markdown file) has many more
                    // chances to contain any given keyword than a small,
                    // precisely on-topic file with one or two symbols. Without
                    // this, `raw` alone rewards verbosity exactly like the
                    // un-normalized cosine similarity issue #7 originally
                    // suspected (that theory turned out to point at the wrong
                    // code path, but the missing length normalization was real
                    // — it was just here, in the LIKE tier, not in TF-IDF).
                    let sym_count = path_to_id
                        .get(file)
                        .and_then(|id| symbol_counts.get(id))
                        .copied()
                        .unwrap_or(1)
                        .max(1) as f32;
                    let length_penalty = 1.0 + sym_count.ln();
                    // Best case: every keyword matched this file, each uniquely (hit count 1),
                    // in a single-symbol file (no length penalty).
                    let max_possible = keywords.len().max(1) as f32;
                    (raw / (length_penalty * max_possible)).min(1.0)
                }
            }
        };
        // WHY renormalize per file type: BM25 is only ever computed for
        // Markdown/Office/PDF/jsonl files (`doc_paths`) — a code file's BM25
        // "score" isn't a low-relevance signal, it's a signal that was never
        // computed at all. Giving it a flat 0.3 weight for every file would
        // silently penalize every code file relative to every doc file
        // regardless of true relevance, reintroducing a doc-vs-code bias by a
        // different route. Only files eligible for BM25 include it in the mix;
        // everyone else's weight is redistributed across LIKE + TF-IDF.
        let combined_score_for = |file: &str| -> f32 {
            let like = like_score_for(file);
            let tfidf = tfidf_scores.get(file).copied().unwrap_or(0.0);
            if bm25_available && doc_path_set.contains(file) {
                let raw_bm25 = bm25_scores.get(file).copied().unwrap_or(0.0);
                let bm25 = if raw_bm25 > 0.0 {
                    (raw_bm25 / (raw_bm25 + BM25_SATURATION_K)) as f32
                } else {
                    0.0
                };
                0.4 * like + 0.3 * tfidf + 0.3 * bm25
            } else {
                (0.4 * like + 0.3 * tfidf) / 0.7
            }
        };
        let score_map: std::collections::HashMap<String, f32> = candidates
            .iter()
            .map(|(path, _, _)| (path.clone(), combined_score_for(path)))
            .collect();
        // Stable sort: candidates with equal (e.g. zero) score keep their
        // original relative order rather than being shuffled arbitrarily.
        candidates.sort_by(|(a, _, _), (b, _, _)| {
            score_map.get(b).unwrap_or(&0.0)
                .partial_cmp(score_map.get(a).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

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
            // Exact token count on the same compressed text the agent would actually
            // receive, read once here and reused for `content` below when requested.
            // Falls back to the char-count heuristic if the file went missing between
            // indexing and this read (mirrors the previous include_content-only read).
            let full_path = std::path::Path::new(&ws).join(file);
            let lang = path_to_lang.get(file).map(|s| s.as_str()).unwrap_or("");
            let file_compression = compress::CompressionLevel::from_i64(file_level);
            let compressed = std::fs::read_to_string(&full_path)
                .ok()
                .map(|raw| compress::compress(&raw, lang, file_compression));
            let tokens = match &compressed {
                Some(text) => crate::search::TokenCounter::count_tokens(text)
                    .unwrap_or_else(|_| Self::estimate_tokens(*base, file_level)),
                None => Self::estimate_tokens(*base, file_level),
            };

            let mut entry = json!({
                "path": file,
                "symbols": sym,
                "tokens": tokens
            });
            if git_diff_files.contains(file) {
                entry["git_diff"] = Value::Bool(true);
            }
            if include_content {
                if let Some(text) = compressed {
                    entry["content"] = Value::String(text);
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

        // Rule sharing (beta): sections of other agents' instruction files that
        // matter for this task. None when the switch is off, so the response and
        // the record keep their previous shape.
        let related_rules = related_rules_for(&self.state.workspace_root, task, &self.state.agent_id);
        let rule_refs: Vec<RuleRef> = match &related_rules {
            Some(RelatedRules::Items(items)) => {
                let ws = std::path::Path::new(&self.state.workspace_root);
                items
                    .iter()
                    .filter_map(|section| match rules::snapshot(ws, section) {
                        Ok(_) => Some(RuleRef {
                            file: section.file.clone(),
                            heading: section.heading.clone(),
                            hash: section.hash.clone(),
                        }),
                        // WHY drop the reference rather than fail the call: a record
                        // must never point at a snapshot that does not exist.
                        Err(e) => {
                            log::warn!("rule snapshot failed for {}: {}", section.file, e);
                            None
                        }
                    })
                    .collect()
            }
            _ => Vec::new(),
        };

        // Record this call to session memory
        recorded_symbols.sort();
        recorded_symbols.dedup();
        recorded_files.sort();
        recorded_files.dedup();
        if let Err(e) = record_mcp_call_with_rules(
            &self.state.workspace_root,
            &self.state.agent_id,
            &self.state.session_id,
            task.to_string(),
            recorded_symbols,
            recorded_files,
            total_tokens as u64,
            rule_refs,
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

        let mut response = json!({
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
        });
        // WHY conditional, not always-present: a corruption-triggered rebuild
        // (GraphDB::new quarantining index.db, see graph/mod.rs) starts from zero
        // rows, which otherwise looks identical to "this workspace genuinely has
        // nothing indexed" — an agent seeing empty pivot_files after a real query
        // could wrongly conclude files were deleted and act on that (Gemini review
        // finding #3). The marker clears itself once the next full
        // index_workspace pass completes (main.rs's background indexing task; see
        // graph/mod.rs::clear_recovery_marker's doc for why that — not raw
        // file_count — is the right point to stop surfacing this).
        if let Ok(Some(ts)) = self.state.graph_db.recovery_marker() {
            response["index_recovered_from_corruption_at"] = json!(ts);
        }
        match related_rules {
            None => {}
            Some(RelatedRules::Unavailable(reason)) => {
                response["related_rules"] = json!({ "unavailable": reason });
            }
            Some(RelatedRules::Items(items)) => {
                let tokens: usize = items.iter().map(rule_tokens).sum();
                response["related_rules"] = json!({
                    "note": RELATED_RULES_NOTE,
                    "items": items.iter().map(section_json).collect::<Vec<_>>(),
                    "tokens": tokens,
                });
            }
        }
        Ok(response)
    }

    /// Read `default_budget_tokens` from `.comp/config.json`.
    /// Falls back to 8000 if the file is absent or the key is missing.
    pub(in crate::mcp) fn load_default_budget(root: &str) -> usize {
        let path = std::path::Path::new(root).join(".comp/config.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let json: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
        json["default_budget_tokens"].as_u64().unwrap_or(8000) as usize
    }

    /// Read `compression_rules` from `.comp/config.json`.
    ///
    /// Rules map glob patterns to compression levels (0/1/2).
    /// Returns an empty map when the file is absent, invalid, or has no rules key.
    pub(in crate::mcp) fn load_compression_rules(root: &str) -> std::collections::HashMap<String, i64> {
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
    pub(in crate::mcp) fn match_pattern(filename: &str, pattern: &str) -> bool {
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
    pub(in crate::mcp) fn apply_compression_rule(
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
    pub(in crate::mcp) fn estimate_tokens(base_tokens: usize, level: i64) -> usize {
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
    pub(in crate::mcp) fn choose_compression_level(candidates: &[(String, usize, usize)], budget: usize) -> (i64, bool) {
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
    pub(in crate::mcp) fn pack_within_budget(
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
}
