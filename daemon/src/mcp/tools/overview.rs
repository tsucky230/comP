use super::*;

impl MCPServer {
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

        let changed_files = git_diff_names(&workspace_root, base_ref).map_err(|e| anyhow!(e))?;

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
}
