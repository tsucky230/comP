use super::*;

impl MCPServer {
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
        let _pass = self.state.full_pass_lock.lock().await;
        info!("handle_force_reindex: clearing index");
        self.state.graph_db.clear_index()?;

        let workspace_root = self.state.workspace_root.clone();

        info!("handle_force_reindex: rebuilding index for {}", workspace_root);
        let summary = crate::indexer::index_all_roots(&workspace_root, &self.state.graph_db).await?;
        let (total, indexed, symbols) = (summary.total_files, summary.indexed_files, summary.symbols);

        // A full index_all_roots pass just completed — see
        // graph/mod.rs::clear_recovery_marker's doc for why this is the contract
        // for when index_recovered_from_corruption_at should stop appearing.
        // No-op if the marker was never set.
        if let Err(e) = self.state.graph_db.clear_recovery_marker() {
            log::warn!("failed to clear index recovery marker after force reindex: {}", e);
        }

        // WHY: main()'s startup task rebuilds the TF-IDF search_engine after its
        // own indexing pass, but handle_force_reindex previously only rebuilt the
        // graph DB. Without this, TF-IDF results (and therefore run_pipeline's
        // combined ranking score) would keep scoring every file 0.0 and go stale
        // after any force re-index that runs later than process startup.
        self.rebuild_search_index().await;

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

        let resolved = resolve_within_roots(path_str, &workspace_root, &self.state.home_policy)?;

        crate::indexer::index_one_file(&workspace_root, &resolved.in_root, &self.state.graph_db).await?;

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

        let (stored_path, removed) =
            crate::indexer::remove_one_file(&workspace_root, path_str, &self.state.graph_db)?
                .unwrap_or_else(|| (path_str.replace('\\', "/"), 0));
        info!("handle_remove_file: deleted {} ({} rows)", stored_path, removed);

        Ok(json!({
            "status": "ok",
            "path": stored_path,
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

        let mut response = json!({
            // Lets clients detect a stale running binary after an upgrade
            // (Windows locks the exe, so rebuilds don't take effect until restart).
            "daemon_version": env!("CARGO_PKG_VERSION"),
            // A workspace build older than its sources: see build_staleness.rs.
            "daemon_build_stale": self.state.build_staleness,
            "total_files": file_count,
            "total_nodes": node_count,
            "total_edges": edge_count,
            "tokens_sent": sent,
            "tokens_saved": saved,
            "queries_count": queries,
            "efficiency": efficiency,
            "avg_tokens_per_query": avg_tokens_per_query
        });
        // See handle_run_pipeline's identical check: a corruption-triggered
        // rebuild starts from zero rows, indistinguishable from "nothing indexed
        // yet" unless this is surfaced explicitly. The marker clears itself once
        // the next full index_workspace pass completes (see main.rs's background
        // indexing task and graph/mod.rs::clear_recovery_marker's doc) — so
        // unlike gating on file_count == 0 here, this stays accurate throughout
        // a partial re-index instead of disappearing the moment the first file
        // lands (Gemini design-review finding).
        if let Ok(Some(ts)) = self.state.graph_db.recovery_marker() {
            response["index_recovered_from_corruption_at"] = json!(ts);
        }
        // See latest_activity_timestamp's doc: replaces SidebarPanel.ts reading
        // the pre-v0.11.1 single session-memory.json file directly (Phase 3, A7).
        if let Some(ts) = latest_activity_timestamp(&self.state.workspace_root) {
            response["last_activity_at"] = json!(ts);
        }
        Ok(response)
    }

    /// Bring the index in step after folder-level changes the per-file watcher
    /// cannot see: VS Code reports a deleted, moved or renamed folder as one
    /// event for the folder, not one per file inside it.
    ///
    /// `params.paths`: absolute paths as the editor reports them (required,
    /// array of strings). One full incremental pass (`index_all_roots`, no
    /// `clear_index`) runs when at least one path
    /// - is an existing directory inside a root, under which that root's walker
    ///   (ignore files, hidden and excluded names) finds a file to index; or
    /// - no longer exists, and the index holds an entry under it — including an
    ///   additional root that was deleted as a whole.
    ///
    /// WHY a full pass rather than deleting the entries under the folder: the
    /// pass removes only entries whose file is really gone, so a prefix mistake
    /// cannot drop an unrelated file; it re-parses moved files and re-links
    /// both their own calls and the calls of unchanged files into them.
    ///
    /// Returns `{ reindexed, total_files, total_nodes, total_edges }`.
    pub async fn handle_sync_folders(&self, params: Value) -> Result<Value> {
        let paths: Vec<String> = params["paths"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing 'paths' parameter (an array of absolute paths)"))?
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| anyhow!("'paths' must contain only strings")))
            .collect::<Result<_>>()?;

        let _pass = self.state.full_pass_lock.lock().await;
        let root = self.state.workspace_root.clone();
        let reindexed = crate::indexer::folder_changes_need_reindex(&root, &paths, &self.state.graph_db)?;
        if reindexed {
            info!("handle_sync_folders: folder changes at {:?}, running an incremental pass", paths);
            crate::indexer::index_all_roots(&root, &self.state.graph_db).await?;
            // A full pass completed: see graph/mod.rs::clear_recovery_marker's doc.
            if let Err(e) = self.state.graph_db.clear_recovery_marker() {
                log::warn!("failed to clear index recovery marker after syncFolders: {}", e);
            }
            self.rebuild_search_index().await;
        }

        let (files, nodes, edges) = self.state.graph_db.get_stats()?;
        Ok(json!({
            "reindexed": reindexed,
            "total_files": files,
            "total_nodes": nodes,
            "total_edges": edges
        }))
    }

    /// Rebuild the TF-IDF index from the graph DB after a full pass.
    pub(crate) async fn rebuild_search_index(&self) {
        if let Ok(all_symbols) = self.state.graph_db.get_all_symbols_for_search() {
            let mut se = self.state.search_engine.lock().await;
            if let Err(e) = se.build_index(&all_symbols) {
                log::warn!("TF-IDF index build failed: {}", e);
            }
        }
    }
}
