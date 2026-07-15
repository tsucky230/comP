// walker.rs — Filesystem walker using the `ignore` crate for gitignore support
//
// Responsibilities:
// - Scan workspace directory recursively
// - Respect .gitignore (and nested .gitignore files) via the `ignore` crate
// - Respect .comp/ignore for project-specific exclusions
// - Prune subtrees for extra skip names (venv, __pycache__, etc.) via filter_entry
// - Calculate file hashes for incremental updates
// - Detect which files have changed since last index
//
// Key data structures:
// - FileEntry: path, hash, language, last_modified
// - WalkerResult: total files, indexed files, skipped files

use anyhow::Result;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A file entry with metadata
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Relative path from workspace root
    pub path: String,
    /// SHA256 hash of file content
    pub hash: String,
    /// Detected language (e.g., "rust", "typescript", "python")
    pub language: String,
    /// File modification time (Unix timestamp)
    #[allow(dead_code)]
    pub modified_time: i64,
}

/// Result of walking the filesystem
#[derive(Debug)]
pub struct WalkerResult {
    /// All files found (including unchanged)
    pub files: Vec<FileEntry>,
    /// Files that need re-indexing (changed or new)
    pub changed_files: Vec<FileEntry>,
    /// Files that were deleted (tracked before but not found now)
    pub deleted_files: Vec<String>,
}

/// Walker configuration
pub struct WalkerConfig {
    /// Directory names to always skip, regardless of gitignore (segment-complete match).
    /// These prune the entire subtree via filter_entry — no descent into the directory.
    pub extra_skip_names: Vec<String>,
    /// Path to a supplementary ignore file (e.g., .comp/ignore).
    /// Gitignore syntax; read by the `ignore` crate alongside .gitignore.
    pub custom_ignore_file: Option<PathBuf>,
    /// Skip files larger than this size in bytes. Default: 5 MiB.
    /// WHY: Unbounded reads of huge binaries/generated files waste I/O and token budget.
    pub max_file_bytes: u64,
}

impl Default for WalkerConfig {
    fn default() -> Self {
        WalkerConfig {
            // Non-hidden dirs that are never source code.
            // Hidden dirs (starting with '.') are skipped automatically by standard_filters.
            extra_skip_names: vec![
                "node_modules".to_string(),
                "venv".to_string(),         // Python venv without leading dot
                "__pycache__".to_string(),  // Python bytecode cache
                "coverage".to_string(),
                "vendor".to_string(),
                "out".to_string(),
            ],
            custom_ignore_file: None,
            max_file_bytes: 5 * 1024 * 1024, // 5 MiB
        }
    }
}

pub struct FileWalker {
    workspace_root: PathBuf,
    config: WalkerConfig,
}

impl FileWalker {
    /// Create a new file walker
    pub fn new(workspace_root: &str, config: WalkerConfig) -> Self {
        FileWalker {
            workspace_root: PathBuf::from(workspace_root),
            config,
        }
    }

    /// Walk the workspace and return all files.
    ///
    /// Uses `ignore::WalkBuilder` which provides:
    /// - .gitignore / nested .gitignore support
    /// - Hidden-file filtering (files/dirs starting with '.')
    /// - .comp/ignore via `add_ignore`
    /// - filter_entry for hard-coded extra_skip_names (prunes entire subtrees)
    pub fn walk(&self, previous_hashes: Option<&HashMap<String, String>>) -> Result<WalkerResult> {
        let mut builder = WalkBuilder::new(&self.workspace_root);
        // Enables: gitignore, .ignore, global gitignore, hidden-file filtering
        builder.standard_filters(true);

        // Supplementary ignore file (e.g., .comp/ignore)
        if let Some(ref p) = self.config.custom_ignore_file {
            if p.exists() {
                builder.add_ignore(p);
            }
        }

        // Prune extra non-hidden directories by name.
        // filter_entry returns false for a directory → the entire subtree is skipped,
        // unlike a plain .filter_map which only skips the directory node itself.
        let extra_skip = self.config.extra_skip_names.clone();
        builder.filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            !extra_skip.iter().any(|s| s == name.as_ref())
        });

        let mut all_files = Vec::new();
        let mut changed_files = Vec::new();
        let mut found_paths = HashSet::new();

        for entry in builder.build().filter_map(|e| e.ok()) {
            if !entry.path().is_file() {
                continue;
            }

            let relative_path = match self.get_relative_path(entry.path()) {
                Ok(p) => p,
                Err(_) => continue,
            };

            // Skip oversized files before the expensive SHA-256 read.
            if let Ok(meta) = entry.metadata() {
                if meta.len() > self.config.max_file_bytes {
                    log::debug!(
                        "Skipping oversized file ({} bytes > {} limit): {}",
                        meta.len(), self.config.max_file_bytes, relative_path
                    );
                    continue;
                }
            }

            let hash = match self.calculate_file_hash(entry.path()) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let modified_time = self.get_modified_time(entry.path()).unwrap_or(0);
            let language = self.detect_language(&relative_path);

            found_paths.insert(relative_path.clone());

            let file_entry = FileEntry {
                path: relative_path.clone(),
                hash: hash.clone(),
                language,
                modified_time,
            };

            let file_changed = previous_hashes
                .map(|h| h.get(&relative_path) != Some(&hash))
                .unwrap_or(true);

            if file_changed {
                changed_files.push(file_entry.clone());
            }
            all_files.push(file_entry);
        }

        // Supplemental scan for interaction history — must run before the
        // deleted-files diff so history rows registered by session_log's
        // index_file are not misclassified as deleted and purged.
        self.scan_history_files(
            previous_hashes,
            &mut all_files,
            &mut changed_files,
            &mut found_paths,
        );

        let deleted_files = previous_hashes
            .map(|h| {
                h.keys()
                    .filter(|p| !found_paths.contains(*p))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        Ok(WalkerResult {
            files: all_files,
            changed_files,
            deleted_files,
        })
    }

    /// Scan `.comp/history/*.jsonl` and append them to the walk result.
    ///
    /// WHY: standard_filters(true) prunes every hidden directory, so the main
    /// walk never descends into .comp/. Interaction logs must still be indexed
    /// so BM25 recall (run_pipeline) can surface past requests/outcomes — the
    /// same carve-out should_skip_relative_path applies to single-file updates.
    fn scan_history_files(
        &self,
        previous_hashes: Option<&HashMap<String, String>>,
        all_files: &mut Vec<FileEntry>,
        changed_files: &mut Vec<FileEntry>,
        found_paths: &mut HashSet<String>,
    ) {
        let hist_dir = self.workspace_root.join(".comp").join("history");
        let Ok(entries) = fs::read_dir(&hist_dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file()
                || path.extension().map(|e| e != "jsonl").unwrap_or(true)
            {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if meta.len() > self.config.max_file_bytes {
                    continue;
                }
            }
            let Ok(relative_path) = self.get_relative_path(&path) else {
                continue;
            };
            if !found_paths.insert(relative_path.clone()) {
                continue;
            }
            let Ok(hash) = self.calculate_file_hash(&path) else {
                continue;
            };
            let file_entry = FileEntry {
                path: relative_path.clone(),
                hash: hash.clone(),
                language: self.detect_language(&relative_path),
                modified_time: self.get_modified_time(&path).unwrap_or(0),
            };
            let file_changed = previous_hashes
                .map(|h| h.get(&relative_path) != Some(&hash))
                .unwrap_or(true);
            if file_changed {
                changed_files.push(file_entry.clone());
            }
            all_files.push(file_entry);
        }
    }

    /// Check if a workspace-relative path should be excluded from indexing.
    ///
    /// Used by Indexer::index_file() to guard single-file update requests.
    /// Checks hidden segments and extra_skip_names; full gitignore checking
    /// is handled by the ignore crate during batch walk.
    pub fn should_skip_relative_path(&self, relative_path: &str) -> bool {
        let normalized = relative_path.replace('\\', "/");
        // WHY: interaction history under .comp/history/ must be indexable so BM25 recall
        // (run_pipeline) can surface past requests/outcomes, even though .comp/ is an
        // otherwise hidden, non-source directory that is skipped wholesale.
        if normalized.starts_with(".comp/history/") {
            return false;
        }
        for segment in normalized.split('/') {
            if segment.starts_with('.') {
                return true;
            }
            if self.config.extra_skip_names.iter().any(|s| s == segment) {
                return true;
            }
        }
        false
    }

    /// Get relative path from workspace root, normalized to forward slashes
    fn get_relative_path(&self, path: &Path) -> Result<String> {
        let relative = path.strip_prefix(&self.workspace_root)?;
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }

    /// Calculate SHA256 hash of file content
    fn calculate_file_hash(&self, path: &Path) -> Result<String> {
        let content = fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Get file modification time (Unix timestamp)
    fn get_modified_time(&self, path: &Path) -> Result<i64> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?;
        let duration = modified.duration_since(std::time::UNIX_EPOCH)?;
        Ok(duration.as_secs() as i64)
    }

    /// Detect programming language from file extension
    pub fn detect_language(&self, path: &str) -> String {
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
                "docx" => "docx",
                "pptx" => "pptx",
                "xlsx" => "xlsx",
                "pdf" => "pdf",
                "parquet" => "parquet",
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
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_walker_creation() {
        let walker = FileWalker::new("/tmp", WalkerConfig::default());
        assert_eq!(walker.workspace_root, PathBuf::from("/tmp"));
    }

    #[test]
    fn test_language_detection() {
        let walker = FileWalker::new(".", WalkerConfig::default());

        assert_eq!(walker.detect_language("main.rs"), "rust");
        assert_eq!(walker.detect_language("app.ts"), "typescript");
        assert_eq!(walker.detect_language("script.py"), "python");
        assert_eq!(walker.detect_language("main.go"), "go");
        assert_eq!(walker.detect_language("unknown.xyz"), "unknown");
        assert_eq!(walker.detect_language("doc.docx"), "docx");
        assert_eq!(walker.detect_language("data.parquet"), "parquet");
        assert_eq!(walker.detect_language("slides.pptx"), "pptx");
        assert_eq!(walker.detect_language("sheet.xlsx"), "xlsx");
        assert_eq!(walker.detect_language("data.jsonl"), "jsonl");
    }

    #[test]
    fn test_file_hash_calculation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("test.txt");
        let mut file = File::create(&file_path)?;
        file.write_all(b"test content")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());
        let hash1 = walker.calculate_file_hash(&file_path)?;
        let hash2 = walker.calculate_file_hash(&file_path)?;

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 hex
        Ok(())
    }

    #[tokio::test]
    async fn test_walk_empty_directory() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());

        let result = walker.walk(None)?;
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.changed_files.len(), 0);
        assert_eq!(result.deleted_files.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_walk_with_files() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let file1_path = temp_dir.path().join("test1.rs");
        File::create(&file1_path)?.write_all(b"fn main() {}")?;

        let file2_path = temp_dir.path().join("test2.py");
        File::create(&file2_path)?.write_all(b"print('hello')")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());
        let result = walker.walk(None)?;

        assert_eq!(result.files.len(), 2);
        assert_eq!(result.changed_files.len(), 2);
        assert_eq!(result.deleted_files.len(), 0);

        let langs: Vec<_> = result.files.iter().map(|f| f.language.as_str()).collect();
        assert!(langs.contains(&"rust"));
        assert!(langs.contains(&"python"));
        Ok(())
    }

    #[tokio::test]
    async fn test_incremental_detection() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("test.rs");
        File::create(&file_path)?.write_all(b"fn main() {}")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());

        let result1 = walker.walk(None)?;
        assert_eq!(result1.changed_files.len(), 1);

        let mut previous_hashes = HashMap::new();
        for fe in &result1.files {
            previous_hashes.insert(fe.path.clone(), fe.hash.clone());
        }

        let result2 = walker.walk(Some(&previous_hashes))?;
        assert_eq!(result2.changed_files.len(), 0);

        File::create(&file_path)?.write_all(b"fn main() { println!(\"hello\"); }")?;

        let result3 = walker.walk(Some(&previous_hashes))?;
        assert_eq!(result3.changed_files.len(), 1);
        Ok(())
    }

    /// Fix 1: filter_entry prunes .hidden dirs — subtree never descended
    #[tokio::test]
    async fn test_walk_prunes_hidden_dirs() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Simulate .venv with a Python package inside
        let venv_dir = temp_dir.path().join(".venv").join("Lib").join("site-packages");
        std::fs::create_dir_all(&venv_dir)?;
        File::create(venv_dir.join("six.py"))?.write_all(b"# six")?;

        // A real source file at root
        File::create(temp_dir.path().join("main.py"))?.write_all(b"print('hello')")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());
        let result = walker.walk(None)?;

        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"main.py"), "main.py should be indexed");
        assert!(
            !paths.iter().any(|p| p.contains(".venv")),
            ".venv subtree must not be indexed"
        );
        Ok(())
    }

    /// Fix 2: extra_skip_names prunes non-hidden dirs like venv, __pycache__
    #[tokio::test]
    async fn test_walk_prunes_extra_skip_names() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // venv (no leading dot)
        let venv = temp_dir.path().join("venv").join("lib");
        std::fs::create_dir_all(&venv)?;
        File::create(venv.join("six.py"))?.write_all(b"# six")?;

        // __pycache__
        let cache = temp_dir.path().join("__pycache__");
        std::fs::create_dir_all(&cache)?;
        File::create(cache.join("main.cpython-311.pyc"))?.write_all(b"")?;

        // Real source file
        File::create(temp_dir.path().join("app.py"))?.write_all(b"x = 1")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());
        let result = walker.walk(None)?;

        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"app.py"));
        assert!(
            !paths.iter().any(|p| p.contains("venv")),
            "venv subtree must be excluded"
        );
        assert!(
            !paths.iter().any(|p| p.contains("__pycache__")),
            "__pycache__ must be excluded"
        );
        Ok(())
    }

    /// Fix 2 regression: segment-complete match must not exclude src/builder.rs or targets.rs
    #[tokio::test]
    async fn test_no_substring_false_positive() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // These names contain "build", "target", "node" as substrings but must NOT be excluded
        let src = temp_dir.path().join("src");
        std::fs::create_dir_all(&src)?;
        File::create(src.join("builder.rs"))?.write_all(b"fn build() {}")?;
        File::create(src.join("targets.rs"))?.write_all(b"fn targets() {}")?;
        File::create(temp_dir.path().join("retargeting.ts"))?.write_all(b"export {}")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());
        let result = walker.walk(None)?;

        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src/builder.rs"), "src/builder.rs must NOT be excluded");
        assert!(paths.contains(&"src/targets.rs"), "src/targets.rs must NOT be excluded");
        assert!(paths.contains(&"retargeting.ts"), "retargeting.ts must NOT be excluded");
        Ok(())
    }

    /// Fix 4: should_skip_relative_path guards index_file
    #[test]
    fn test_should_skip_relative_path() {
        let walker = FileWalker::new(".", WalkerConfig::default());

        // Hidden dir segments
        assert!(walker.should_skip_relative_path(".venv/Lib/six.py"));
        assert!(walker.should_skip_relative_path(".git/config"));

        // extra_skip_names
        assert!(walker.should_skip_relative_path("venv/lib/six.py"));
        assert!(walker.should_skip_relative_path("src/__pycache__/main.pyc"));
        assert!(walker.should_skip_relative_path("node_modules/react/index.js"));

        // Normal source paths must NOT be skipped
        assert!(!walker.should_skip_relative_path("src/main.rs"));
        assert!(!walker.should_skip_relative_path("src/builder.rs"));
        assert!(!walker.should_skip_relative_path("targets.rs"));
        assert!(!walker.should_skip_relative_path("retargeting.ts"));

        // .comp/history/ is carved out so interaction logs are indexable for BM25 recall,
        // while the rest of .comp/ (index.db, config) stays excluded.
        assert!(!walker.should_skip_relative_path(".comp/history/log-2026-06.jsonl"));
        assert!(walker.should_skip_relative_path(".comp/index.db"));
        assert!(walker.should_skip_relative_path(".comp/config.json"));
    }

    /// 0.8.6 regression: the batch walk must include .comp/history/*.jsonl so
    /// interaction logs land in the files table at initial/full index time,
    /// while the rest of .comp/ stays excluded.
    #[tokio::test]
    async fn test_walk_includes_comp_history_jsonl() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let hist_dir = temp_dir.path().join(".comp").join("history");
        std::fs::create_dir_all(&hist_dir)?;
        File::create(hist_dir.join("log-2026-07.jsonl"))?
            .write_all(br#"{"query":"fix jwt bug","outcome":"patched middleware"}"#)?;
        // Non-jsonl files in history and other .comp files must stay excluded
        File::create(hist_dir.join("notes.txt"))?.write_all(b"scratch")?;
        File::create(temp_dir.path().join(".comp").join("config.json"))?.write_all(b"{}")?;

        File::create(temp_dir.path().join("main.rs"))?.write_all(b"fn main() {}")?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());
        let result = walker.walk(None)?;

        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&".comp/history/log-2026-07.jsonl"),
            "history jsonl must be walked, got: {:?}", paths
        );
        assert!(paths.contains(&"main.rs"));
        assert!(!paths.contains(&".comp/config.json"), ".comp root files must stay excluded");
        assert!(!paths.contains(&".comp/history/notes.txt"), "non-jsonl history files must stay excluded");

        let history_entry = result.files.iter()
            .find(|f| f.path == ".comp/history/log-2026-07.jsonl")
            .unwrap();
        assert_eq!(history_entry.language, "jsonl");
        Ok(())
    }

    /// 0.8.6 regression: a history file registered by session_log's index_file
    /// must not be flagged deleted by the next full walk (which previously
    /// could not see inside .comp/ and purged the row).
    #[tokio::test]
    async fn test_history_file_not_flagged_deleted() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let hist_dir = temp_dir.path().join(".comp").join("history");
        std::fs::create_dir_all(&hist_dir)?;
        File::create(hist_dir.join("log-2026-07.jsonl"))?
            .write_all(br#"{"query":"add feature","outcome":"done"}"#)?;

        let walker = FileWalker::new(temp_dir.path().to_str().unwrap(), WalkerConfig::default());

        // Simulate DB state after session_log + index_file
        let first = walker.walk(None)?;
        let mut previous_hashes = HashMap::new();
        for fe in &first.files {
            previous_hashes.insert(fe.path.clone(), fe.hash.clone());
        }

        // Unchanged file: not deleted, not re-indexed
        let second = walker.walk(Some(&previous_hashes))?;
        assert!(
            second.deleted_files.is_empty(),
            "history file must not be flagged deleted: {:?}", second.deleted_files
        );
        assert_eq!(second.changed_files.len(), 0);

        // Appended line (Stop hook behavior): detected as changed on next walk
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(hist_dir.join("log-2026-07.jsonl"))?;
        writeln!(f, r#"{{"query":"second task","outcome":"done"}}"#)?;

        let third = walker.walk(Some(&previous_hashes))?;
        assert_eq!(third.changed_files.len(), 1, "appended history must be re-indexed");
        assert!(third.deleted_files.is_empty());
        Ok(())
    }

    /// §4-2: files larger than max_file_bytes are skipped before hash calculation
    #[tokio::test]
    async fn test_walk_skips_oversized_files() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Small file — must be indexed
        File::create(temp_dir.path().join("small.rs"))?.write_all(b"fn main(){}")?;

        // Large file — must be skipped (threshold set to 100 bytes for speed)
        let big_content = vec![b'x'; 200];
        File::create(temp_dir.path().join("big.bin"))?.write_all(&big_content)?;

        let walker = FileWalker::new(
            temp_dir.path().to_str().unwrap(),
            WalkerConfig {
                max_file_bytes: 100,
                ..WalkerConfig::default()
            },
        );
        let result = walker.walk(None)?;

        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"small.rs"), "small.rs must be indexed");
        assert!(!paths.contains(&"big.bin"), "big.bin must be skipped (oversized)");
        Ok(())
    }

    /// Fix 3 / gitignore: .comp/ignore is respected when provided as custom_ignore_file
    #[tokio::test]
    async fn test_custom_ignore_file_respected() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Write .comp/ignore
        let comp_dir = temp_dir.path().join(".comp");
        std::fs::create_dir_all(&comp_dir)?;
        let mut ignore_file = File::create(comp_dir.join("ignore"))?;
        writeln!(ignore_file, "legacy_data/")?;
        writeln!(ignore_file, "*.log")?;

        // Create matching files
        let legacy = temp_dir.path().join("legacy_data");
        std::fs::create_dir_all(&legacy)?;
        File::create(legacy.join("dump.py"))?.write_all(b"# old")?;
        File::create(temp_dir.path().join("error.log"))?.write_all(b"err")?;

        // Create a normal source file
        File::create(temp_dir.path().join("app.rs"))?.write_all(b"fn main(){}")?;

        let walker = FileWalker::new(
            temp_dir.path().to_str().unwrap(),
            WalkerConfig {
                custom_ignore_file: Some(comp_dir.join("ignore")),
                ..WalkerConfig::default()
            },
        );
        let result = walker.walk(None)?;

        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"app.rs"), "app.rs should be indexed");
        assert!(
            !paths.iter().any(|p| p.contains("legacy_data")),
            "legacy_data/ must be excluded by .comp/ignore"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with(".log")),
            "*.log must be excluded by .comp/ignore"
        );
        Ok(())
    }

    /// 4-5: extra_skip_names added at runtime excludes the directory in both walk and should_skip_relative_path
    #[tokio::test]
    async fn test_extra_skip_names_runtime_extension() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Simulate user-added "env" directory
        let env_lib = temp_dir.path().join("env").join("lib");
        std::fs::create_dir_all(&env_lib)?;
        File::create(env_lib.join("six.py"))?.write_all(b"# env lib")?;

        // Normal source file
        File::create(temp_dir.path().join("app.py"))?.write_all(b"x = 1")?;

        let walker = FileWalker::new(
            temp_dir.path().to_str().unwrap(),
            WalkerConfig {
                extra_skip_names: {
                    let mut names = WalkerConfig::default().extra_skip_names;
                    names.push("env".to_string());
                    names
                },
                ..WalkerConfig::default()
            },
        );

        // walk: env/ subtree must be excluded
        let result = walker.walk(None)?;
        let paths: Vec<_> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"app.py"), "app.py must be indexed");
        assert!(
            !paths.iter().any(|p| p.contains("env")),
            "env/ subtree must be excluded via extra_skip_names"
        );

        // should_skip_relative_path: guard path inside env/
        assert!(
            walker.should_skip_relative_path("env/lib/six.py"),
            "env/lib/six.py must be skipped by should_skip_relative_path"
        );
        assert!(
            !walker.should_skip_relative_path("app.py"),
            "app.py must not be skipped"
        );

        Ok(())
    }
}
