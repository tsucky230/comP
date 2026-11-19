use super::*;

impl MCPServer {
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
        let hits = self.state.graph_db.search_symbols_with_ids(query, limit * 2)?;

        let results: Vec<Value> = hits
            .into_iter()
            .filter(|hit| kind_filter.is_none_or(|f| f == hit.kind))
            .take(limit)
            .map(|hit| {
                json!({
                    // The symbol_id get_impact_graph takes.
                    "id": hit.id,
                    "file": hit.path,
                    "symbol": hit.name,
                    "kind": hit.kind,
                    "line": hit.line,
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
            &self.state.agent_id,
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

    /// Compress a single file using AST compression
    pub async fn handle_compress_file(&self, params: Value) -> Result<Value> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'path' parameter"))?;
        let compression_level = params["compression_level"].as_i64().unwrap_or(1);

        let workspace_root = self.state.workspace_root.clone();

        let safe_path = validate_within_workspace(path_str, &workspace_root, &self.state.home_policy)?;

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
