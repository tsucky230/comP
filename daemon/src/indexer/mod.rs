// mod.rs update - Main indexer orchestrator integrating walker, parser, doc_parser
//
// Updated implementation now includes:
// - FileWalker for filesystem traversal
// - CodeParser for tree-sitter parsing
// - DocumentParser for JSON/XML/Markdown
// - Integration: walk -> parse -> extract -> store

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

pub mod walker;
pub mod parser;
pub mod doc_parser;
pub mod dependency;

pub use walker::{FileWalker, FileEntry, WalkerConfig};
pub use parser::CodeParser;
pub use doc_parser::DocumentParser;
pub use dependency::DependencyAnalyzer;

/// Main indexer orchestrator
pub struct Indexer {
    workspace_root: String,
    walker: FileWalker,
    parser: CodeParser,
}

impl Indexer {
    /// Create a new indexer for the given workspace
    pub fn new(workspace_root: &str) -> Self {
        let comp_ignore = std::path::Path::new(workspace_root).join(".comp/ignore");
        let mut config = WalkerConfig {
            custom_ignore_file: Some(comp_ignore),
            ..WalkerConfig::default()
        };

        // WHY: Extend the built-in skip list with user-defined excludes from
        // .comp/config.json so comp.exclude settings take effect on every indexer
        // creation (initial index and forceReindex alike).
        config.extra_skip_names.extend(Self::load_exclude_patterns(workspace_root));

        let walker = FileWalker::new(workspace_root, config);
        let parser = CodeParser::default();

        Indexer {
            workspace_root: workspace_root.to_string(),
            walker,
            parser,
        }
    }

    /// Read additional workspace paths from `.comp/config.json`.
    ///
    /// WHY: Monorepos or multi-root workspaces need all sub-paths indexed into
    /// a single graph DB so cross-path dependencies can be resolved.
    pub fn read_additional_paths(workspace_root: &str) -> Vec<String> {
        let path = std::path::Path::new(workspace_root).join(".comp/config.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap_or(serde_json::Value::Null);
        json["additional_paths"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load user-defined exclude patterns from `.comp/config.json`.
    ///
    /// WHY: VS Code's `comp.exclude` setting is synced here by the extension.
    /// Daemon reads this at Indexer::new time so forceReindex also picks up changes.
    fn load_exclude_patterns(workspace_root: &str) -> Vec<String> {
        let path = std::path::Path::new(workspace_root).join(".comp/config.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap_or(serde_json::Value::Null);
        json["exclude"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load node limit settings from .comp/config.json
    /// Returns (max_nodes, on_limit_exceeded)
    fn load_comp_config(workspace_root: &str) -> (i64, String) {
        let path = std::path::Path::new(workspace_root).join(".comp/config.json");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap_or(serde_json::Value::Null);
        let max_nodes = json["max_nodes"].as_i64().unwrap_or(200_000);
        let on_limit = json["on_limit_exceeded"].as_str().unwrap_or("warn").to_string();
        (max_nodes, on_limit)
    }

    /// Index the entire workspace
    ///
    /// Process:
    /// 1. Walk filesystem to find all source files
    /// 2. Compare hashes with DB to find changed files
    /// 3. For each changed file:
    ///    - Parse with tree-sitter or document parser
    ///    - Extract symbols
    ///    - Store symbols in graph DB
    ///    - Extract and store dependencies as edges
    /// 4. Delete entries for removed files
    /// 5. Return statistics
    ///
    /// # Arguments
    /// - previous_hashes: Optional map of (path -> hash) from previous index
    /// - db: GraphDB instance for storing results
    ///
    /// # Returns
    /// - (total_files, indexed_files, symbols_extracted)
    pub async fn index_workspace(
        &mut self,
        previous_hashes: Option<&HashMap<String, String>>,
        db: &crate::graph::GraphDB,
    ) -> Result<(usize, usize, usize)> {
        // 1. Walk filesystem
        let walk_result = self.walker.walk(previous_hashes)?;
        let total_files = walk_result.files.len();
        let changed_count = walk_result.changed_files.len();

        // WHY: Warn early when the file count is unexpectedly high so users can
        //      add .comp/ignore entries before the slow indexing loop runs.
        //      2 000 files is a reasonable ceiling for a single-language project.
        const FILE_COUNT_WARN_THRESHOLD: usize = 2_000;
        if total_files > FILE_COUNT_WARN_THRESHOLD {
            let mut dir_counts: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for f in &walk_result.files {
                let top = f.path.split('/').next().unwrap_or(".");
                *dir_counts.entry(top).or_insert(0) += 1;
            }
            let mut sorted: Vec<_> = dir_counts.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1));
            let breakdown: Vec<String> = sorted
                .iter()
                .take(5)
                .map(|(dir, count)| format!("{}: {}", dir, count))
                .collect();
            log::warn!(
                "Large workspace: {} files found (threshold {}). \
                 Indexing may be slow. Add exclusions to .comp/ignore if needed. \
                 Top directories — {}",
                total_files,
                FILE_COUNT_WARN_THRESHOLD,
                breakdown.join(", ")
            );
        }

        // 2. Pass 1 — parse, extract symbols, store nodes. Defer edges so callee
        //    names can be resolved across files once every node exists.
        let mut symbols_count = 0;
        let mut deps_by_file: Vec<(String, Vec<dependency::Dependency>)> = Vec::new();
        for file_entry in walk_result.changed_files {
            match self.parse_and_extract(&file_entry, db).await {
                Ok((count, deps)) => {
                    symbols_count += count;
                    if !deps.is_empty() {
                        deps_by_file.push((file_entry.path.clone(), deps));
                    }
                }
                Err(e) => {
                    eprintln!("Error parsing {}: {}", file_entry.path, e);
                    // Continue with next file on error
                }
            }
        }

        // 2b. Pass 2 — resolve dependencies into edges via the global symbol index.
        if !deps_by_file.is_empty() {
            let global_index = db.get_global_symbol_index()?;
            for (path, deps) in &deps_by_file {
                if let Err(e) = Self::resolve_edges_for_file(db, path, deps, &global_index) {
                    log::warn!("Failed to resolve edges for {}: {}", path, e);
                }
            }
        }

        // 3. Remove deleted file entries from the database
        for path in &walk_result.deleted_files {
            if let Err(e) = db.delete_file(path) {
                // WHY: If deletion fails, entries persist in the database, leading to obsolete search results.
                //      Log a warning to prompt retries during the next indexing run.
                log::warn!("Failed to remove deleted file from index (stale entry may persist): {} — {}", path, e);
            }
        }

        // 4. Check max_nodes limit from .comp/config.json
        let (max_nodes, on_limit) = Self::load_comp_config(&self.workspace_root);
        if let Ok((_, node_count, _)) = db.get_stats() {
            if node_count > max_nodes {
                match on_limit.as_str() {
                    "stop" => {
                        log::warn!(
                            "Node limit reached ({} > {}). Indexing halted. Add entries to .comp/ignore to reduce scope.",
                            node_count, max_nodes
                        );
                        return Ok((total_files, changed_count, symbols_count));
                    }
                    _ => {
                        log::warn!(
                            "Node limit exceeded ({} > {}). Consider adding .comp/ignore entries or increasing max_nodes in .comp/config.json.",
                            node_count, max_nodes
                        );
                    }
                }
            }
        }

        Ok((total_files, changed_count, symbols_count))
    }

    /// Parse a single file and extract symbols, storing them in GraphDB
    ///
    /// # Process:
    /// 1. Read file content from disk
    /// 2. Parse based on language (tree-sitter or document parser)
    /// 3. Store file entry in DB (upsert_file)
    /// 4. For each symbol, store as node in DB (insert_node)
    /// 5. Extract dependencies and store as edges (insert_edge)
    ///
    /// # Arguments
    /// - file_entry: File metadata (path, hash, language)
    /// - db: GraphDB instance for storing results
    ///
    /// # Returns
    /// - Number of symbols extracted and stored
    async fn parse_and_extract(
        &mut self,
        file_entry: &FileEntry,
        db: &crate::graph::GraphDB,
    ) -> Result<(usize, Vec<dependency::Dependency>)> {
        use std::fs;

        // 1. Read file content based on type
        let full_path = Path::new(&self.workspace_root).join(&file_entry.path);
        
        let is_binary = matches!(
            file_entry.language.as_str(),
            "parquet" | "docx" | "pptx" | "xlsx" | "pdf"
        );

        let (symbols, content_str) = if is_binary {
            let syms = match file_entry.language.as_str() {
                "parquet" => DocumentParser::parse_parquet(&full_path)?,
                "docx" => DocumentParser::parse_docx(&full_path)?,
                "pptx" => DocumentParser::parse_pptx(&full_path)?,
                "xlsx" => DocumentParser::parse_xlsx(&full_path)?,
                "pdf" => DocumentParser::parse_pdf(&full_path)?,
                _ => Vec::new(),
            };
            (syms, String::new()) // Binary files don't use string content for extraction later
        } else {
            // WHY: InvalidData usually indicates non-UTF-8 encoding.
            // Skip silently with a debug log and avoid printing to stderr to prevent user noise.
            let content = match fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    log::debug!("Skipping non-UTF-8 file: {} ({})", file_entry.path, e);
                    return Ok((0, Vec::new()));
                }
                Err(e) => return Err(e.into()),
            };
            let syms = match file_entry.language.as_str() {
                "json" => DocumentParser::parse_json(&content)?,
                "jsonl" => DocumentParser::parse_jsonl(&content)?,
                "xml" => DocumentParser::parse_xml(&content)?,
                "markdown" | "md" => DocumentParser::parse_markdown(&content)?,
                _ => {
                    // Tree-sitter parsing for code
                    self.parser.parse_file(&file_entry.language, &content).await?
                }
            };
            (syms, content)
        };

        // 3. Store file in DB (char_count drives the real-token baseline in run_pipeline)
        let char_count = content_str.len();
        let file_id = db.upsert_file(&file_entry.path, &file_entry.hash, &file_entry.language, char_count)?;

        // WHY: this file's previous parse may have produced a different set of
        // symbols (renamed/removed functions, etc.). insert_node below is a bare
        // INSERT, so without clearing the old set first, old and new nodes would
        // simply accumulate under the same file_id forever. See clear_file_symbols'
        // doc comment for the accepted cross-file-edge trade-off.
        db.clear_file_symbols(file_id)?;

        // 4. Store each symbol as a node in DB
        let mut symbol_map: HashMap<String, i64> = HashMap::new();
        for symbol in &symbols {
            let node_id = db.insert_node(
                file_id,
                &symbol.name,
                symbol.kind.as_str(),
                symbol.line as i32,
                symbol.column as i32,
                symbol.scope.as_deref(),
                symbol.is_exported,
                symbol.signature.as_deref(),
            )?;
            symbol_map.insert(symbol.name.clone(), node_id);
        }

        // 5. Extract raw dependencies from source code.
        // Edges are resolved in a second pass (see resolve_edges_for_file), once
        // all nodes exist, so callee names can be linked across files.
        let _ = &symbol_map;
        let raw_deps = if is_binary {
            Vec::new()
        } else {
            DependencyAnalyzer::extract_dependencies(
                &file_entry.language,
                &content_str,
                &file_entry.path,
            ).unwrap_or_default()
        };

        Ok((symbols.len(), raw_deps))
    }

    /// Resolve and store edges for a single file using the global symbol index.
    ///
    /// WHY: A separate pass over already-inserted nodes lets us link a callee
    /// name to a definition in any file, not just the current one.
    fn resolve_edges_for_file(
        db: &crate::graph::GraphDB,
        path: &str,
        deps: &[dependency::Dependency],
        global_index: &crate::graph::GlobalSymbolIndex,
    ) -> Result<()> {
        let Some(file_id) = db.get_file_id_by_path(path)? else {
            return Ok(());
        };

        // Rebuild this file's outbound edges from scratch to avoid stale entries.
        db.clear_file_edges(file_id)?;

        let local_nodes: Vec<(i64, String, i32)> = db
            .get_file_symbols_sorted(file_id)?
            .into_iter()
            .map(|n| (n.id, n.name, n.line))
            .collect();

        let edges = DependencyAnalyzer::resolve_global(deps, &local_nodes, global_index, file_id);
        for (from_id, to_id, edge_kind) in edges {
            if let Err(e) = db.insert_edge(from_id, to_id, &edge_kind) {
                log::warn!("Failed to insert edge {}->{}: {}", from_id, to_id, e);
            }
        }
        Ok(())
    }

    /// Index a single file (incremental update)
    ///
    /// WHY: Called upon file changes. Previous implementation was a TODO that did not save to DB.
    /// We reuse parse_and_extract to parse, extract symbols, resolve dependencies, and save to DB in one go.
    pub async fn index_file(&mut self, path: &Path, db: &crate::graph::GraphDB) -> Result<()> {
        use sha2::{Sha256, Digest};

        // Convert to workspace-relative path (using absolute paths breaks DB uniqueness constraint)
        // WHY: Windows path normalizer returns backslashes (\), but the database must use forward slashes (/) consistently.
        //      The TypeScript extension sends forward slashes, so mismatch breaks path lookups.
        let relative_path = path
            .strip_prefix(&self.workspace_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

        // WHY: FileSystemWatcher fires for any matching extension, including paths inside
        //      excluded directories (.venv, node_modules, __pycache__, etc.).
        //      Guard here so we never index library code from excluded subtrees.
        if self.walker.should_skip_relative_path(&relative_path) {
            return Ok(());
        }

        let language = self.walker_detect_language(&relative_path);

        // Calculate file hash (required for FileEntry)
        // Read as bytes to handle binary files (e.g., Parquet) gracefully
        let content_bytes = std::fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&content_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let file_entry = FileEntry {
            path: relative_path,
            hash,
            language,
            modified_time: 0,
        };

        // parse_and_extract writes nodes; resolve this file's edges against the
        // global index so incremental updates also populate the dependency graph.
        let (_count, deps) = self.parse_and_extract(&file_entry, db).await?;
        if !deps.is_empty() {
            let global_index = db.get_global_symbol_index()?;
            Self::resolve_edges_for_file(db, &file_entry.path, &deps, &global_index)?;
        }

        Ok(())
    }

    // Helper: same language detection as walker
    fn walker_detect_language(&self, path: &str) -> String {
        if let Some(ext) = Path::new(path).extension() {
            match ext.to_string_lossy().as_ref() {
                "rs" => "rust",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" => "javascript",
                "py" => "python",
                "go" => "go",
                "java" => "java",
                "c" | "h" => "c",
                "cpp" | "cc" | "cxx" | "hpp" => "cpp",
                "cs" => "csharp",
                "rb" => "ruby",
                "php" => "php",
                "sh" | "bash" => "bash",
                "sql" => "sql",
                "html" | "htm" => "html",
                "css" | "scss" | "less" => "css",
                "json" => "json",
                "jsonl" => "jsonl",
                "yaml" | "yml" => "yaml",
                "xml" => "xml",
                "md" => "markdown",
                "parquet" => "parquet",
                "docx" => "docx",
                "pptx" => "pptx",
                "xlsx" => "xlsx",
                "pdf" => "pdf",
                _ => "unknown",
            }
        } else {
            "unknown"
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::dependency::{Dependency, EdgeKind};

    #[test]
    fn test_indexer_creation() {
        let indexer = Indexer::new("/tmp/test");
        assert_eq!(indexer.workspace_root, "/tmp/test");
    }

    #[test]
    fn test_language_detection() {
        let indexer = Indexer::new(".");

        assert_eq!(indexer.walker_detect_language("main.rs"), "rust");
        assert_eq!(indexer.walker_detect_language("app.ts"), "typescript");
        assert_eq!(indexer.walker_detect_language("index.json"), "json");
        assert_eq!(indexer.walker_detect_language("README.md"), "markdown");
        assert_eq!(indexer.walker_detect_language("report.docx"), "docx");
        assert_eq!(indexer.walker_detect_language("slides.pptx"), "pptx");
        assert_eq!(indexer.walker_detect_language("data.xlsx"), "xlsx");
        assert_eq!(indexer.walker_detect_language("data.parquet"), "parquet");
    }

    #[tokio::test]
    async fn test_parse_and_extract_stores_symbols() -> Result<()> {
        use tempfile::TempDir;
        use std::fs::File;
        use std::io::Write;

        // Setup: Create temp directory with a Rust file
        let temp_dir = TempDir::new()?;
        let workspace_root = temp_dir.path().to_string_lossy().to_string();

        // Create a simple Rust file with symbols
        let rust_file = temp_dir.path().join("test.rs");
        let mut file = File::create(&rust_file)?;
        writeln!(file, "fn hello() {{}}\nstruct Point {{}}\nfn main() {{}}")?;

        // Calculate hash for the file
        use sha2::{Sha256, Digest};
        let content = std::fs::read_to_string(&rust_file)?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let hash = format!("{:x}", hasher.finalize());

        // Create GraphDB in temp directory
        let db = crate::graph::GraphDB::new(&workspace_root).await?;

        // Create indexer and parse the file
        let mut indexer = Indexer::new(&workspace_root);
        let file_entry = FileEntry {
            path: "test.rs".to_string(),
            hash: hash.clone(),
            language: "rust".to_string(),
            modified_time: 0,
        };

        let (count, _deps) = indexer.parse_and_extract(&file_entry, &db).await?;

        // Verify: Should extract multiple symbols (hello, Point, main)
        assert!(count > 0, "Should extract at least one symbol");

        // Verify: Database should contain file entry
        let (file_count, node_count, _edge_count) = db.get_stats()?;
        assert_eq!(file_count, 1, "Should have exactly 1 file");
        assert!(node_count > 0, "Should have extracted symbols as nodes");

        Ok(())
    }

    #[tokio::test]
    async fn test_index_workspace_integration() -> Result<()> {
        use tempfile::TempDir;
        use std::fs::File;
        use std::io::Write;

        // Setup: Create temp directory with multiple Rust files
        let temp_dir = TempDir::new()?;
        let workspace_root = temp_dir.path().to_string_lossy().to_string();

        // Create test files
        let file1 = temp_dir.path().join("file1.rs");
        let mut f1 = File::create(&file1)?;
        writeln!(f1, "fn func1() {{}}")?;

        let file2 = temp_dir.path().join("file2.rs");
        let mut f2 = File::create(&file2)?;
        writeln!(f2, "fn func2() {{}}\nstruct Data {{}}")?;

        // Create GraphDB
        let db = crate::graph::GraphDB::new(&workspace_root).await?;

        // Index workspace (without previous hashes, all files are "new")
        let mut indexer = Indexer::new(&workspace_root);
        let (total_files, indexed_files, symbols) = indexer
            .index_workspace(None, &db)
            .await?;

        // Verify: Should find and index files
        // Note: total_files may include non-Rust files, indexed_files should include our 2 Rust files
        assert!(total_files > 0, "Should find files");
        assert_eq!(indexed_files, 2, "Should index 2 Rust files");
        assert!(symbols > 0, "Should extract symbols from indexed files");

        // Verify: Database stats
        let (file_count, node_count, _edge_count) = db.get_stats()?;
        assert_eq!(file_count, 2, "Should have 2 files in DB");
        assert!(node_count > 0, "Should have nodes for extracted symbols");

        Ok(())
    }

    #[tokio::test]
    async fn test_index_workspace_empty() -> Result<()> {
        use tempfile::TempDir;

        let temp_dir = TempDir::new()?;
        let workspace_root = temp_dir.path().to_string_lossy().to_string();

        // Create GraphDB in empty directory
        let db = crate::graph::GraphDB::new(&workspace_root).await?;

        let mut indexer = Indexer::new(&workspace_root);
        let (total_files, indexed_files, symbols) = indexer
            .index_workspace(None, &db)
            .await?;

        // Empty directory should have no indexable files
        assert_eq!(indexed_files, 0, "Empty directory should have 0 indexed files");
        assert_eq!(symbols, 0, "Empty directory should have 0 symbols");

        Ok(())
    }

    #[test]
    fn test_dependency_analyzer_resolve() {
        // Test dependency resolution with symbol_map
        let deps = vec![
            Dependency {
                from: "main".to_string(),
                to: "helper".to_string(),
                kind: EdgeKind::FunctionCall,
                line: 5,
            },
        ];

        let mut symbol_map = HashMap::new();
        symbol_map.insert("main".to_string(), 1);
        symbol_map.insert("helper".to_string(), 2);

        let edges = DependencyAnalyzer::resolve_dependencies(&deps, &symbol_map, &HashMap::new());

        // Verify edge is created with correct node IDs
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].0, 1); // from_id
        assert_eq!(edges[0].1, 2); // to_id
        assert_eq!(edges[0].2, "function_call");
    }

    /// 4-5: load_exclude_patterns returns empty Vec when config.json has no exclude key
    #[test]
    fn test_load_exclude_patterns_missing() {
        let patterns = Indexer::load_exclude_patterns("/nonexistent/path");
        assert!(patterns.is_empty(), "missing config.json must yield empty exclude list");
    }

    /// 4-5: load_exclude_patterns reads the exclude array from config.json
    #[test]
    fn test_load_exclude_patterns_present() {
        use tempfile::TempDir;
        let temp_dir = TempDir::new().unwrap();
        let comp_dir = temp_dir.path().join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        std::fs::write(
            comp_dir.join("config.json"),
            r#"{"exclude": ["env", "data", "build"]}"#,
        ).unwrap();

        let patterns = Indexer::load_exclude_patterns(temp_dir.path().to_str().unwrap());
        assert_eq!(patterns, vec!["env", "data", "build"]);
    }

    /// 4-5: Indexer::new with exclude in config.json skips the excluded directory
    #[tokio::test]
    async fn test_indexer_excludes_from_config() -> Result<()> {
        use tempfile::TempDir;
        use std::fs::File;
        use std::io::Write;

        let temp_dir = TempDir::new()?;
        let workspace_root = temp_dir.path().to_str().unwrap();

        // Write config.json with "env" as an excluded directory name
        let comp_dir = temp_dir.path().join(".comp");
        std::fs::create_dir_all(&comp_dir)?;
        std::fs::write(comp_dir.join("config.json"), r#"{"exclude": ["env"]}"#)?;

        // Create an excluded directory with a file
        let env_dir = temp_dir.path().join("env").join("lib");
        std::fs::create_dir_all(&env_dir)?;
        File::create(env_dir.join("six.py"))?.write_all(b"# excluded")?;

        // Create a normal source file that must be indexed
        File::create(temp_dir.path().join("app.py"))?.write_all(b"x = 1")?;

        let db = crate::graph::GraphDB::new(workspace_root).await?;
        let mut indexer = Indexer::new(workspace_root);
        let (total, _indexed, _symbols) = indexer.index_workspace(None, &db).await?;

        // Verify: env/ subtree is excluded; only app.py is found
        assert_eq!(total, 1, "only app.py should be found, env/ must be excluded");

        Ok(())
    }

    #[test]
    fn test_dependency_analyzer_unresolved() {
        // Test that unresolved dependencies are skipped
        let deps = vec![
            Dependency {
                from: "main".to_string(),
                to: "unknown".to_string(),
                kind: EdgeKind::FunctionCall,
                line: 5,
            },
        ];

        let mut symbol_map = HashMap::new();
        symbol_map.insert("main".to_string(), 1);
        // "unknown" is not in symbol_map

        let edges = DependencyAnalyzer::resolve_dependencies(&deps, &symbol_map, &HashMap::new());

        // Unresolved dependencies should not create edges
        assert_eq!(edges.len(), 0);
    }

    #[tokio::test]
    async fn test_parse_and_extract_with_dependencies() -> Result<()> {
        use tempfile::TempDir;
        use std::fs::File;
        use std::io::Write;

        // Setup: Create temp directory with a Rust file containing function calls
        let temp_dir = TempDir::new()?;
        let workspace_root = temp_dir.path().to_string_lossy().to_string();

        // Create a Rust file with a function call
        let rust_file = temp_dir.path().join("test.rs");
        let mut file = File::create(&rust_file)?;
        writeln!(file, "fn main() {{}}\nfn helper() {{}}\n")?;

        // Calculate hash
        use sha2::{Sha256, Digest};
        let content = std::fs::read_to_string(&rust_file)?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let hash = format!("{:x}", hasher.finalize());

        // Create GraphDB and indexer
        let db = crate::graph::GraphDB::new(&workspace_root).await?;
        let mut indexer = Indexer::new(&workspace_root);

        let file_entry = FileEntry {
            path: "test.rs".to_string(),
            hash,
            language: "rust".to_string(),
            modified_time: 0,
        };

        // Parse and extract
        let (symbol_count, _deps) = indexer.parse_and_extract(&file_entry, &db).await?;

        // Verify symbols were extracted
        assert!(symbol_count > 0);

        // Verify database state
        let (file_count, node_count, _edge_count) = db.get_stats()?;
        assert_eq!(file_count, 1);
        assert!(node_count > 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_index_workspace_builds_cross_file_edges() -> Result<()> {
        use tempfile::TempDir;
        use std::fs::File;
        use std::io::Write;

        let temp_dir = TempDir::new()?;
        let workspace_root = temp_dir.path().to_string_lossy().to_string();

        // lib.rs exports `helper`; main.rs calls it → one cross-file edge expected.
        let mut lib = File::create(temp_dir.path().join("lib.rs"))?;
        writeln!(lib, "pub fn helper() {{}}")?;

        let mut main = File::create(temp_dir.path().join("main.rs"))?;
        writeln!(main, "fn run() {{\n    helper();\n}}")?;

        let db = crate::graph::GraphDB::new(&workspace_root).await?;
        let mut indexer = Indexer::new(&workspace_root);
        indexer.index_workspace(None, &db).await?;

        let (_files, _nodes, edges) = db.get_stats()?;
        assert!(edges > 0, "cross-file call should produce at least one edge");

        Ok(())
    }
}
