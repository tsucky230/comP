// Graph module - SQLite-based code graph database
//
// Tables:
// - files: {id, path, hash, last_indexed, language}
// - nodes: {id, file_id, name, kind, line, col, scope}
// - edges: {from_id, to_id, kind}
//
// The graph represents:
// - Nodes: Symbols (functions, classes, variables, types)
// - Edges: Dependencies (function calls, type references, etc.)

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Mutex;

mod schema;

pub use schema::Schema;

/// Cross-file symbol lookup: `name -> [(node_id, file_id, is_exported)]`.
pub type GlobalSymbolIndex = HashMap<String, Vec<(i64, i64, bool)>>;

/// Graph database interface for storing code structure
pub struct GraphDB {
    // WHY: Use Mutex<Connection> to obtain Send+Sync.
    // Required to run indexing concurrently via tokio::spawn.
    conn: Mutex<Connection>,
}

impl GraphDB {
    /// Create/open SQLite database at workspace
    ///
    /// # Process:
    /// 1. Create .comp/ directory if not exists
    /// 2. Create/open index.db SQLite database
    /// 3. Initialize schema (tables, indexes)
    ///
    /// `index.db` is entirely derived data (re-buildable from the workspace by
    /// re-indexing), unlike session-memory/history which have no other copy. So
    /// unlike those stores, a corrupt `index.db` is safe to move aside and
    /// replace automatically rather than merely quarantined-and-reported: if
    /// `open_and_init` fails here (SQLite reports the file as unreadable/not a
    /// database — this used to propagate straight out of `new()` and take the
    /// whole daemon process down before the MCP server ever started, since the
    /// caller chain is `main()`'s `AppState::new(...).await?`), the old files are
    /// quarantined next to a freshly created replacement and `recovery_marker()`
    /// records that this happened so callers (e.g. `get_stats`) can tell an agent
    /// the index is temporarily empty rather than silently returning zero counts
    /// that look like "nothing indexed yet".
    pub async fn new(workspace_root: &str) -> Result<Self> {
        use std::fs;
        use std::path::Path;

        // Create .comp directory
        let comp_dir = Path::new(workspace_root).join(".comp");
        fs::create_dir_all(&comp_dir)?;

        let db_path = comp_dir.join("index.db");

        match Self::open_and_init(&db_path) {
            Ok(conn) => Ok(GraphDB { conn: Mutex::new(conn) }),
            Err(open_err) => {
                log::warn!(
                    "index.db at {} appears corrupt ({}); quarantining and rebuilding from scratch",
                    db_path.display(),
                    open_err
                );
                Self::quarantine_db_files(&db_path);
                let conn = Self::open_and_init(&db_path).map_err(|e| {
                    anyhow::anyhow!("index.db rebuild after quarantine also failed: {}", e)
                })?;
                let db = GraphDB { conn: Mutex::new(conn) };
                db.mark_recovered_from_corruption()?;
                Ok(db)
            }
        }
    }

    /// Open (creating if absent) the SQLite file at `db_path` and apply the
    /// schema, failing loudly if the file exists but is not a usable SQLite
    /// database.
    ///
    /// WHY `PRAGMA quick_check` before touching the schema: SQLite opens a
    /// corrupt file lazily without error — `Connection::open` alone would
    /// succeed on garbage bytes, and the first real read (previously, whatever
    /// `Schema::apply_all`'s DDL happened to touch first) is what actually
    /// surfaced `SQLITE_CORRUPT` / "file is not a database". Running the check
    /// explicitly up front keeps the corrupt-vs-fresh decision in one place
    /// instead of depending on which DDL statement happens to trip first.
    fn open_and_init(db_path: &std::path::Path) -> Result<Connection> {
        let conn = Connection::open(db_path)?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(anyhow::anyhow!("PRAGMA quick_check reported: {}", check));
        }
        Schema::apply_all(&conn)?;
        Ok(conn)
    }

    /// Move `index.db` and its `-wal`/`-shm` siblings (if present) aside to
    /// `<name>.corrupt-<epoch_ms>` rather than deleting them, so a corrupted
    /// file stays available for manual inspection instead of vanishing.
    ///
    /// WHY all three files move together: WAL mode is always on (see
    /// `schema.rs::PRAGMA_INIT`), so a real `index.db` can have `-wal`/`-shm`
    /// siblings holding not-yet-checkpointed pages. Quarantining only the `.db`
    /// file and leaving a stale `-wal` next to the freshly created replacement
    /// would make SQLite try to replay that leftover WAL against the new,
    /// unrelated database on next open — corrupting it again immediately and
    /// turning this into an infinite quarantine-and-recreate loop.
    ///
    /// Best-effort: a failure to rename a given sibling is logged, not
    /// propagated — the caller's subsequent `open_and_init` on a fresh path is
    /// what determines overall success, and refusing to recover at all just
    /// because (for example) a `-shm` file happened to be transiently locked by
    /// another reader would defeat the point of this function.
    fn quarantine_db_files(db_path: &std::path::Path) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        for suffix in ["", "-wal", "-shm"] {
            let src = Self::sibling_path(db_path, suffix);
            if !src.exists() {
                continue;
            }
            let dst = Self::sibling_path(db_path, &format!("{}.corrupt-{}", suffix, ts));
            if let Err(e) = std::fs::rename(&src, &dst) {
                log::warn!("failed to quarantine {}: {}", src.display(), e);
            }
        }
    }

    /// `db_path` with `suffix` appended to its filename (not its extension) —
    /// e.g. `sibling_path(".../index.db", "-wal")` => `.../index.db-wal`, the
    /// real SQLite WAL sidecar naming convention.
    fn sibling_path(db_path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
        let mut s = db_path.as_os_str().to_os_string();
        s.push(suffix);
        std::path::PathBuf::from(s)
    }

    /// Record that this database was just rebuilt from a quarantined file, so
    /// `recovery_marker()` can report it.
    fn mark_recovered_from_corruption(&self) -> Result<()> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('recovered_from_corruption_at', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [ts.to_string()],
        )?;
        Ok(())
    }

    /// Epoch-ms timestamp of the most recent automatic corruption recovery for
    /// this database, or `None` if `index.db` has never been rebuilt from a
    /// quarantined file. Callers (e.g. `get_stats`, `run_pipeline`) surface this
    /// so an agent can tell "just rebuilt, background re-indexing in progress"
    /// apart from "workspace genuinely has nothing indexed yet" — both look
    /// identical as raw zero counts otherwise.
    pub fn recovery_marker(&self) -> Result<Option<u64>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'recovered_from_corruption_at'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(value.and_then(|v| v.parse::<u64>().ok()))
    }

    /// Clear the `recovered_from_corruption_at` marker, if one is set.
    ///
    /// WHY this must be called explicitly rather than `recovery_marker()` ever
    /// expiring on its own: the marker exists so `get_stats`/`run_pipeline` can
    /// tell a genuinely empty workspace apart from an index that just got
    /// rebuilt from a quarantined file and is still catching up. Gating that
    /// purely on `total_files == 0` breaks the moment a partial re-index has
    /// written even one row — an agent could then read "only N files" as "the
    /// rest were deleted" while the re-index is still running (Gemini
    /// design-review finding). Calling this once the *next* `index_workspace`
    /// pass completes (main.rs's background indexing task, which runs on every
    /// startup regardless of whether recovery happened) keeps the marker
    /// present for the entire catch-up window and removes it only once the
    /// index is actually back to a trustworthy state — a no-op, not an error,
    /// on a database that was never recovered in the first place.
    pub fn clear_recovery_marker(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute("DELETE FROM metadata WHERE key = 'recovered_from_corruption_at'", [])?;
        Ok(())
    }

    /// Insert/update a file in the database
    ///
    /// `char_count` is the UTF-8 byte length of the file content, stored as the
    /// real token-baseline for savings calculations in run_pipeline.
    ///
    /// WHY `ON CONFLICT ... DO UPDATE` instead of `INSERT OR REPLACE`: with a
    /// UNIQUE constraint on `path`, REPLACE resolves the conflict by deleting the
    /// existing row and inserting a new one — and since `id` is AUTOINCREMENT, the
    /// new row gets a fresh id rather than reusing the old one. Every re-index of
    /// a changed file would then orphan that file's entire previous `nodes` set
    /// (no FK cascade is enabled, see schema.rs), silently accumulating forever. A
    /// real upsert updates the existing row in place, so `id` stays stable across
    /// re-indexes — which is what lets `clear_file_symbols` below find and remove
    /// exactly this file's stale nodes/edges before new ones are inserted.
    ///
    /// Returns the file ID for use in subsequent operations
    pub fn upsert_file(&self, path: &str, hash: &str, language: &str, char_count: usize) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO files (path, hash, language, last_indexed, char_count)
             VALUES (?, ?, ?, strftime('%s', 'now'), ?)
             ON CONFLICT(path) DO UPDATE SET
                 hash = excluded.hash,
                 language = excluded.language,
                 last_indexed = excluded.last_indexed,
                 char_count = excluded.char_count",
            rusqlite::params![path, hash, language, char_count as i64],
        )?;

        // Get the inserted/updated file ID
        let mut stmt = conn.prepare("SELECT id FROM files WHERE path = ?")?;
        let file_id: i64 = stmt.query_row([path], |row| row.get(0))?;

        Ok(file_id)
    }

    /// Remove a file's own `nodes` (and any edges touching them) without
    /// removing its `files` row.
    ///
    /// WHY this exists separately from `delete_file`: re-indexing a changed file
    /// re-parses it into a fresh set of symbols, but nothing previously deleted
    /// the *old* set first — `insert_node` is a bare INSERT, so old and new nodes
    /// simply accumulated under the same (now-stable, post-upsert-fix) file_id.
    /// Mirrors `delete_file`'s edges-then-nodes cascade (steps 1-3), stopping
    /// short of deleting the `files` row itself since the caller is about to
    /// re-populate it.
    ///
    /// TRADE-OFF (accepted, see plan): this clears edges pointing *into* this
    /// file's old nodes too, including ones inserted by *other* files that
    /// depend on this one. Those other files' outbound edges are only rebuilt
    /// when they themselves get re-parsed (see `resolve_edges_for_file`, which
    /// only recomputes edges for files present in the current change set) — so
    /// a dependency edge into a changed file can go missing until its source
    /// file is re-indexed for some unrelated reason. This is judged better than
    /// the previous behavior of never deleting anything and growing `nodes`
    /// without bound.
    pub fn clear_file_symbols(&self, file_id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;

        // 1. Get all node IDs of this file
        let mut stmt = conn.prepare("SELECT id FROM nodes WHERE file_id = ?")?;
        let node_ids: Vec<i64> = stmt
            .query_map([file_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        // 2. Delete edges referencing the target nodes (both from and to sides)
        for nid in &node_ids {
            conn.execute(
                "DELETE FROM edges WHERE from_id = ? OR to_id = ?",
                rusqlite::params![nid, nid],
            )?;
        }

        // 3. Delete nodes
        conn.execute("DELETE FROM nodes WHERE file_id = ?", [file_id])?;

        Ok(())
    }

    /// Insert a symbol node
    ///
    /// Returns the node ID for use in dependency tracking
    #[allow(clippy::too_many_arguments)] // 引数が多いが、全フィールドが必須のため構造体化による分割は過剰
    pub fn insert_node(
        &self,
        file_id: i64,
        name: &str,
        kind: &str,
        line: i32,
        col: i32,
        scope: Option<&str>,
        is_exported: bool,
        signature: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let is_exported_int = if is_exported { 1 } else { 0 };
        conn.execute(
            "INSERT INTO nodes (file_id, name, kind, line, col, scope, is_exported, signature)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![file_id, name, kind, line, col, scope, is_exported_int, signature],
        )?;

        // Get the inserted node ID
        let last_id = conn.last_insert_rowid();
        Ok(last_id)
    }

    /// Insert a dependency edge between nodes
    pub fn insert_edge(&self, from_id: i64, to_id: i64, kind: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute(
            "INSERT OR IGNORE INTO edges (from_id, to_id, kind)
             VALUES (?, ?, ?)",
            rusqlite::params![from_id, to_id, kind],
        )?;
        Ok(())
    }

    /// Build a global symbol index for cross-file dependency resolution.
    ///
    /// Returns: `name -> [(node_id, file_id, is_exported)]`. Used by
    /// `DependencyAnalyzer::resolve_global` to link a callee name to its
    /// definition in any indexed file.
    pub fn get_global_symbol_index(&self) -> Result<GlobalSymbolIndex> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT name, id, file_id, is_exported FROM nodes")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)? != 0,
            ))
        })?;
        let mut map: GlobalSymbolIndex = HashMap::new();
        for row in rows {
            let (name, id, file_id, is_exported) = row?;
            map.entry(name).or_default().push((id, file_id, is_exported));
        }
        Ok(map)
    }

    /// Delete all edges originating from a file's nodes.
    ///
    /// WHY: On re-index of a changed file, its outbound edges must be rebuilt
    /// from scratch; otherwise stale edges accumulate (FK CASCADE is not enabled).
    pub fn clear_file_edges(&self, file_id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute(
            "DELETE FROM edges WHERE from_id IN (SELECT id FROM nodes WHERE file_id = ?)",
            [file_id],
        )?;
        Ok(())
    }

    /// Get all nodes that depend on a given symbol
    ///
    /// Returns: Vec<(node_id, symbol_name)>
    pub fn get_dependents(&self, node_id: i64) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT edges.to_id, nodes.name FROM edges
             JOIN nodes ON edges.to_id = nodes.id
             WHERE edges.from_id = ?"
        )?;

        let dependents = stmt.query_map([node_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

        let result = dependents.collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }

    /// Files connected to the given files by dependency edges (either direction),
    /// ranked by the number of connecting edges, excluding the input files themselves.
    ///
    /// WHY: run_pipeline returns pivot files by relevance; the blast radius around
    /// them (callers/callees in other files) is what `related_files` reports so an
    /// agent sees which files a change is likely to touch without a second query.
    pub fn get_related_files(&self, file_paths: &[String], limit: usize) -> Result<Vec<(String, usize)>> {
        if file_paths.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let placeholders = vec!["?"; file_paths.len()].join(",");
        let sql = format!(
            "SELECT path, SUM(cnt) AS total FROM (
                 SELECT f2.path AS path, COUNT(*) AS cnt
                 FROM edges e
                 JOIN nodes n1 ON e.from_id = n1.id
                 JOIN nodes n2 ON e.to_id = n2.id
                 JOIN files f1 ON n1.file_id = f1.id
                 JOIN files f2 ON n2.file_id = f2.id
                 WHERE f1.path IN ({ph}) AND f2.path NOT IN ({ph})
                 GROUP BY f2.path
                 UNION ALL
                 SELECT f1.path AS path, COUNT(*) AS cnt
                 FROM edges e
                 JOIN nodes n1 ON e.from_id = n1.id
                 JOIN nodes n2 ON e.to_id = n2.id
                 JOIN files f1 ON n1.file_id = f1.id
                 JOIN files f2 ON n2.file_id = f2.id
                 WHERE f2.path IN ({ph}) AND f1.path NOT IN ({ph})
                 GROUP BY f1.path
             )
             GROUP BY path
             ORDER BY total DESC, path ASC
             LIMIT ?",
            ph = placeholders
        );

        // The IN-lists appear 4 times, then the LIMIT.
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(file_paths.len() * 4 + 1);
        for _ in 0..4 {
            for p in file_paths {
                params.push(Box::new(p.clone()));
            }
        }
        params.push(Box::new(limit as i64));

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize)),
        )?;
        let result = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }

    /// Record a tool call's token consumption in the shared metadata table.
    ///
    /// This is the single write path for all token statistics.  Both the MCP
    /// daemon (Claude Code / Cursor) and the VSCode extension daemon share the
    /// same SQLite file, so writing here makes the numbers visible to both
    /// processes without any in-memory state synchronisation.
    ///
    /// `tokens_saved` is non-zero only for `run_pipeline`, which has a real
    /// full-workspace baseline.  All other tools pass 0 for `tokens_saved`.
    pub fn record_tool_call(&self, tokens_sent: u64, tokens_saved: u64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute(
            "UPDATE metadata SET value = CAST(CAST(value AS INTEGER) + ? AS TEXT) WHERE key = 'tokens_sent'",
            [tokens_sent as i64],
        )?;
        conn.execute(
            "UPDATE metadata SET value = CAST(CAST(value AS INTEGER) + ? AS TEXT) WHERE key = 'tokens_saved'",
            [tokens_saved as i64],
        )?;
        conn.execute(
            "UPDATE metadata SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) WHERE key = 'queries_count'",
            [],
        )?;
        Ok(())
    }

    /// Return the total char count across all indexed files divided by 4
    /// (the standard chars-to-tokens approximation).
    ///
    /// This is the honest baseline for run_pipeline savings: how many tokens
    /// an AI would consume if it read every file in the workspace verbatim.
    pub fn get_full_workspace_tokens(&self) -> Result<u64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let total_chars: i64 = conn.query_row(
            "SELECT COALESCE(SUM(char_count), 0) FROM files",
            [],
            |row| row.get(0),
        )?;
        Ok((total_chars as u64) / 4)
    }

    /// Read token stats from the metadata table
    pub fn get_token_stats(&self) -> Result<(u64, u64, u64)> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let tokens_sent = conn.query_row(
            "SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'tokens_sent'",
            [],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) as u64;
        let tokens_saved = conn.query_row(
            "SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'tokens_saved'",
            [],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) as u64;
        let queries_count = conn.query_row(
            "SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'queries_count'",
            [],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) as u64;
        Ok((tokens_sent, tokens_saved, queries_count))
    }

    /// Get total statistics about the index
    pub fn get_stats(&self) -> Result<(i64, i64, i64)> {
        // Returns: (file_count, node_count, edge_count)
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;

        let file_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM files",
            [],
            |row| row.get(0)
        )?;

        let node_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes",
            [],
            |row| row.get(0)
        )?;

        let edge_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM edges",
            [],
            |row| row.get(0)
        )?;

        Ok((file_count, node_count, edge_count))
    }

    /// List all indexed files with id, path, language
    ///
    /// WHY: Required for handle_list_indexed_files to return actual data.
    pub fn list_files(&self) -> Result<Vec<(i64, String, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, path, language FROM files ORDER BY path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Count symbols (nodes) per file_id
    pub fn count_symbols_per_file(&self) -> Result<HashMap<i64, i64>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT file_id, COUNT(*) FROM nodes GROUP BY file_id"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (file_id, count) = row?;
            map.insert(file_id, count);
        }
        Ok(map)
    }

    /// Per-file char counts, keyed by file id.
    ///
    /// WHY: run_pipeline token estimates use real file sizes (chars/4) instead of
    /// symbol-count heuristics — a Markdown file with 3 headings and a 200-line
    /// function count the same in symbols but differ 10x in tokens.
    pub fn get_file_char_counts(&self) -> Result<HashMap<i64, i64>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT id, char_count FROM files")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (file_id, chars) = row?;
            map.insert(file_id, chars);
        }
        Ok(map)
    }

    /// Search symbols by name (LIKE pattern, case-insensitive)
    ///
    /// WHY: Since SearchEngine's TF-IDF is not yet built, we temporarily return context
    /// by searching symbol names using a LIKE pattern.
    pub fn search_symbols_by_name(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, String, i32)>> {
        // Return: (file_path, symbol_name, kind, line)
        let pattern = format!("%{}%", query);
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT files.path, nodes.name, nodes.kind, nodes.line
             FROM nodes JOIN files ON nodes.file_id = files.id
             WHERE LOWER(nodes.name) LIKE LOWER(?)
             LIMIT ?"
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Build symbol_id -> (name, file_path) map for impact analysis
    pub fn get_symbol_map(&self) -> Result<HashMap<i64, (String, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT nodes.id, nodes.name, files.path
             FROM nodes JOIN files ON nodes.file_id = files.id"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name, path) = row?;
            map.insert(id, (name, path));
        }
        Ok(map)
    }

    /// Build reverse-dependency map: to_id -> [from_id, ...]
    ///
    /// WHY: Finding "who is affected if symbol X is modified" requires
    /// looking up the caller side (from) reversely.
    pub fn get_reverse_deps(&self) -> Result<HashMap<i64, Vec<i64>>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT from_id, to_id FROM edges")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in rows {
            let (from_id, to_id) = row?;
            map.entry(to_id).or_default().push(from_id);
        }
        Ok(map)
    }

    /// Clear all indexed data (for force re-index)
    pub fn clear_index(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        conn.execute("DELETE FROM edges", [])?;
        conn.execute("DELETE FROM nodes", [])?;
        conn.execute("DELETE FROM files", [])?;
        Ok(())
    }

    /// Delete a single file and its associated nodes/edges
    ///
    /// WHY: Leaving old entries on file deletion or renaming makes impact analysis
    /// and stats inaccurate. We delete in the order of edges -> nodes -> files
    /// explicitly without relying on CASCADE (assuming SQLite foreign key constraints are not enabled).
    pub fn delete_file(&self, path: &str) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;

        // Get file_id (no-op with 0 results if it doesn't exist)
        let file_id: Option<i64> = conn
            .query_row("SELECT id FROM files WHERE path = ?", [path], |row| row.get(0))
            .ok();

        let Some(fid) = file_id else { return Ok(0); };

        // 1. Get all node IDs of this file
        let mut stmt = conn.prepare("SELECT id FROM nodes WHERE file_id = ?")?;
        let node_ids: Vec<i64> = stmt
            .query_map([fid], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        // 2. Delete edges referencing the target nodes (both from and to sides)
        for nid in &node_ids {
            conn.execute(
                "DELETE FROM edges WHERE from_id = ? OR to_id = ?",
                rusqlite::params![nid, nid],
            )?;
        }

        // 3. Delete nodes
        conn.execute("DELETE FROM nodes WHERE file_id = ?", [fid])?;

        // 4. Delete file entry
        let removed = conn.execute("DELETE FROM files WHERE id = ?", [fid])?;

        Ok(removed)
    }

    /// Load all (path → hash) entries from the DB for incremental indexing
    ///
    /// WHY: Passing these to index_workspace lets the indexer skip files whose
    /// content hash hasn't changed since the last session, avoiding a full
    /// re-index on every daemon restart.
    pub fn get_all_file_hashes(&self) -> Result<HashMap<String, String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT path, hash FROM files")?;
        let map = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(map)
    }

    /// Get file ID by its relative path
    pub fn get_file_id_by_path(&self, path: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare("SELECT id FROM files WHERE path = ?")?;
        let mut rows = stmt.query_map([path], |row| row.get(0))?;
        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    /// Find nodes matching symbol name and optional file_id
    pub fn get_symbols_by_name(&self, name: &str, file_id: Option<i64>) -> Result<Vec<DbNode>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut result = Vec::new();
        if let Some(fid) = file_id {
            let mut stmt = conn.prepare(
                "SELECT id, file_id, name, kind, line, col, scope, is_exported, signature 
                 FROM nodes WHERE name = ? AND file_id = ?"
            )?;
            let mapped = stmt.query_map(rusqlite::params![name, fid], |row| {
                Ok(DbNode {
                    id: row.get(0)?,
                    file_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: row.get(4)?,
                    col: row.get(5)?,
                    scope: row.get(6)?,
                    is_exported: row.get(7).unwrap_or(0),
                    signature: row.get(8)?,
                })
            })?;
            for item in mapped {
                result.push(item?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, file_id, name, kind, line, col, scope, is_exported, signature 
                 FROM nodes WHERE name = ?"
            )?;
            let mapped = stmt.query_map(rusqlite::params![name], |row| {
                Ok(DbNode {
                    id: row.get(0)?,
                    file_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: row.get(4)?,
                    col: row.get(5)?,
                    scope: row.get(6)?,
                    is_exported: row.get(7).unwrap_or(0),
                    signature: row.get(8)?,
                })
            })?;
            for item in mapped {
                result.push(item?);
            }
        }
        Ok(result)
    }

    /// Get all nodes for a specific file, sorted by line
    pub fn get_file_symbols_sorted(&self, file_id: i64) -> Result<Vec<DbNode>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, file_id, name, kind, line, col, scope, is_exported, signature 
             FROM nodes WHERE file_id = ? ORDER BY line, col"
        )?;
        let mapped = stmt.query_map([file_id], |row| {
            Ok(DbNode {
                id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                line: row.get(4)?,
                col: row.get(5)?,
                scope: row.get(6)?,
                is_exported: row.get(7).unwrap_or(0),
                signature: row.get(8)?,
            })
        })?;
        let mut result = Vec::new();
        for item in mapped {
            result.push(item?);
        }
        Ok(result)
    }

    /// Get nodes that this node depends on (outbound edges)
    pub fn get_node_dependencies_out(&self, node_id: i64) -> Result<Vec<(DbNode, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT n.id, n.file_id, n.name, n.kind, n.line, n.col, n.scope, n.is_exported, n.signature, e.kind
             FROM edges e
             JOIN nodes n ON e.to_id = n.id
             WHERE e.from_id = ?"
        )?;
        let mapped = stmt.query_map([node_id], |row| {
            Ok((
                DbNode {
                    id: row.get(0)?,
                    file_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: row.get(4)?,
                    col: row.get(5)?,
                    scope: row.get(6)?,
                    is_exported: row.get(7).unwrap_or(0),
                    signature: row.get(8)?,
                },
                row.get(9)?,
            ))
        })?;
        let mut result = Vec::new();
        for item in mapped {
            result.push(item?);
        }
        Ok(result)
    }

    /// Get nodes that depend on this node (inbound edges)
    pub fn get_node_dependencies_in(&self, node_id: i64) -> Result<Vec<(DbNode, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT n.id, n.file_id, n.name, n.kind, n.line, n.col, n.scope, n.is_exported, n.signature, e.kind
             FROM edges e
             JOIN nodes n ON e.from_id = n.id
             WHERE e.to_id = ?"
        )?;
        let mapped = stmt.query_map([node_id], |row| {
            Ok((
                DbNode {
                    id: row.get(0)?,
                    file_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line: row.get(4)?,
                    col: row.get(5)?,
                    scope: row.get(6)?,
                    is_exported: row.get(7).unwrap_or(0),
                    signature: row.get(8)?,
                },
                row.get(9)?,
            ))
        })?;
        let mut result = Vec::new();
        for item in mapped {
            result.push(item?);
        }
        Ok(result)
    }

    /// Get file path by its ID
    pub fn get_file_path_by_id(&self, file_id: i64) -> Result<String> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let path: String = conn.query_row(
            "SELECT path FROM files WHERE id = ?",
            [file_id],
            |row| row.get(0)
        )?;
        Ok(path)
    }

    /// Get all symbols for building the TF-IDF search index
    ///
    /// WHY: SearchEngine.build_index requires all (file_path, name, kind, line) tuples.
    /// Called once after indexing completes to populate the in-memory TF-IDF matrix.
    pub fn get_all_symbols_for_search(&self) -> Result<Vec<(String, String, String, u32)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT files.path, nodes.name, nodes.kind, nodes.line
             FROM nodes JOIN files ON nodes.file_id = files.id"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)? as u32,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Get all exported symbols, ordered by file path
    pub fn get_exported_symbols_grouped(&self) -> Result<Vec<(String, DbNode)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT f.path, n.id, n.file_id, n.name, n.kind, n.line, n.col, n.scope, n.is_exported, n.signature
             FROM nodes n
             JOIN files f ON n.file_id = f.id
             WHERE n.is_exported = 1
             ORDER BY f.path, n.name"
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                DbNode {
                    id: row.get(1)?,
                    file_id: row.get(2)?,
                    name: row.get(3)?,
                    kind: row.get(4)?,
                    line: row.get(5)?,
                    col: row.get(6)?,
                    scope: row.get(7)?,
                    is_exported: row.get(8).unwrap_or(0),
                    signature: row.get(9)?,
                },
            ))
        })?;
        let mut result = Vec::new();
        for item in mapped {
            result.push(item?);
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct DbNode {
    pub id: i64,
    pub file_id: i64,
    pub name: String,
    pub kind: String,
    pub line: i32,
    pub col: i32,
    pub scope: Option<String>,
    pub is_exported: i32,
    pub signature: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_graphdb_creation() {
        let temp_dir = std::env::temp_dir().join("comP_test_graphdb_creation");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap()).await.unwrap();
        let (files, nodes, edges) = db.get_stats().unwrap();
        assert_eq!((files, nodes, edges), (0, 0, 0), "fresh DB must be empty");
        assert_eq!(db.recovery_marker().unwrap(), None, "a fresh DB was never recovered from corruption");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_graphdb_recovers_from_corrupt_database_file() {
        // Before this fix, a corrupt index.db propagated its open error all the
        // way out of AppState::new -> main(), killing the daemon process before
        // the MCP server ever started (see docs/ARCHITECTURE_ja.md 4). Recovery
        // must instead quarantine the bad file and hand back a usable, empty DB.
        let temp_dir = std::env::temp_dir().join("comP_test_graphdb_corrupt_recovery");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        let db_path = comp_dir.join("index.db");
        std::fs::write(&db_path, b"this is not a sqlite database").unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap())
            .await
            .expect("GraphDB::new must recover instead of failing");

        let (files, nodes, edges) = db.get_stats().unwrap();
        assert_eq!((files, nodes, edges), (0, 0, 0), "the rebuilt DB starts empty");
        assert!(db.recovery_marker().unwrap().is_some(), "recovery must be recorded");

        // The corrupt original must be quarantined next to the fresh replacement,
        // not deleted and not left in place under the live path.
        assert!(db_path.exists(), "a fresh, working index.db must exist at the live path");
        let quarantined: Vec<_> = std::fs::read_dir(&comp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("index.db.corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "exactly one quarantined copy of the corrupt file must remain");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_clear_recovery_marker_removes_it_and_is_a_noop_when_absent() {
        let temp_dir = std::env::temp_dir().join("comP_test_clear_recovery_marker");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        std::fs::write(comp_dir.join("index.db"), b"garbage, not a sqlite file").unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap()).await.unwrap();
        assert!(db.recovery_marker().unwrap().is_some(), "recovery must be recorded before clearing");

        db.clear_recovery_marker().expect("clearing an existing marker must succeed");
        assert_eq!(db.recovery_marker().unwrap(), None, "marker must be gone after clearing");

        // A second call on an already-clear marker must not error — every normal
        // (non-recovered) daemon startup calls this unconditionally.
        db.clear_recovery_marker().expect("clearing an absent marker must be a no-op, not an error");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_graphdb_recovered_database_is_fully_functional() {
        // Recovering must not merely open without erroring — the rebuilt schema
        // (tables, indexes, metadata defaults from Schema::apply_all) must accept
        // real writes, since the whole point of recovery is that a subsequent
        // background re-index can repopulate it.
        let temp_dir = std::env::temp_dir().join("comP_test_graphdb_recovered_db_usable");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        std::fs::write(comp_dir.join("index.db"), b"garbage, not a sqlite file").unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap())
            .await
            .expect("GraphDB::new must recover instead of failing");

        let file_id = db.upsert_file("a.rs", "hash1", "rust", 10).expect("recovered DB must accept writes");
        db.insert_node(file_id, "alpha", "fn", 1, 0, None, true, None).expect("recovered DB must accept node inserts");

        let (files, nodes, _) = db.get_stats().unwrap();
        assert_eq!((files, nodes), (1, 1), "a re-index against the recovered DB must be reflected in get_stats");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_graphdb_recovery_survives_stale_wal_and_shm_present() {
        // End-to-end sanity check: recovery must succeed even when -wal/-shm
        // siblings exist alongside the corrupt main file (not just when index.db
        // is corrupt in isolation). The precise fate of the sidecars themselves —
        // SQLite's own failed-open handling can consume/reset them before our
        // code ever runs — is covered deterministically by
        // test_quarantine_db_files_moves_db_wal_and_shm_together below, which
        // tests the quarantine mechanism directly instead of through SQLite.
        let temp_dir = std::env::temp_dir().join("comP_test_graphdb_wal_shm_quarantine");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        let db_path = comp_dir.join("index.db");
        let wal_path = comp_dir.join("index.db-wal");
        let shm_path = comp_dir.join("index.db-shm");
        std::fs::write(&db_path, b"corrupt main file").unwrap();
        std::fs::write(&wal_path, b"stale wal contents").unwrap();
        std::fs::write(&shm_path, b"stale shm contents").unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap())
            .await
            .expect("GraphDB::new must recover even with stale -wal/-shm present");
        let (files, _, _) = db.get_stats().unwrap();
        assert_eq!(files, 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_quarantine_db_files_moves_db_wal_and_shm_together() {
        // Direct test of the quarantine mechanism itself (Gemini review finding
        // #2), independent of what SQLite happens to do on a failed open: all
        // three sidecars must move aside together, none deleted, none left
        // behind under their live names. Leaving a stale -wal next to a freshly
        // recreated index.db would make SQLite try to replay it on next open,
        // re-corrupting the brand-new file.
        let temp_dir = std::env::temp_dir().join("comP_test_quarantine_db_files_direct");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("index.db");
        let wal_path = temp_dir.join("index.db-wal");
        let shm_path = temp_dir.join("index.db-shm");
        std::fs::write(&db_path, b"corrupt main file").unwrap();
        std::fs::write(&wal_path, b"stale wal contents").unwrap();
        std::fs::write(&shm_path, b"stale shm contents").unwrap();

        GraphDB::quarantine_db_files(&db_path);

        assert!(!db_path.exists(), "the corrupt main file must not remain at the live path");
        assert!(!wal_path.exists(), "the stale -wal must not remain at the live path");
        assert!(!shm_path.exists(), "the stale -shm must not remain at the live path");

        let entries: Vec<String> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.iter().any(|n| n.starts_with("index.db.corrupt-")), "main file must be quarantined: {:?}", entries);
        assert!(entries.iter().any(|n| n.starts_with("index.db-wal.corrupt-")), "-wal must be quarantined: {:?}", entries);
        assert!(entries.iter().any(|n| n.starts_with("index.db-shm.corrupt-")), "-shm must be quarantined: {:?}", entries);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_get_related_files() {
        // a.rs and b.rs are pivots; c.rs is connected to a.rs by two edges
        // (one in each direction) and d.rs is isolated. Expect only c.rs,
        // with the edge count summed across directions.
        let temp_dir = std::env::temp_dir().join("comP_test_get_related_files");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap()).await.unwrap();
        let fa = db.upsert_file("a.rs", "h1", "rust", 10).unwrap();
        let fb = db.upsert_file("b.rs", "h2", "rust", 10).unwrap();
        let fc = db.upsert_file("c.rs", "h3", "rust", 10).unwrap();
        let fd = db.upsert_file("d.rs", "h4", "rust", 10).unwrap();

        let na = db.insert_node(fa, "alpha", "fn", 1, 0, None, true, None).unwrap();
        let nb = db.insert_node(fb, "beta", "fn", 1, 0, None, true, None).unwrap();
        let nc = db.insert_node(fc, "gamma", "fn", 1, 0, None, true, None).unwrap();
        let _nd = db.insert_node(fd, "delta", "fn", 1, 0, None, true, None).unwrap();

        db.insert_edge(na, nc, "calls").unwrap(); // pivot → related
        db.insert_edge(nc, na, "calls").unwrap(); // related → pivot
        db.insert_edge(na, nb, "calls").unwrap(); // pivot → pivot: must be excluded

        let pivots = vec!["a.rs".to_string(), "b.rs".to_string()];
        let related = db.get_related_files(&pivots, 10).unwrap();
        assert_eq!(related, vec![("c.rs".to_string(), 2)]);

        // Empty input short-circuits.
        assert!(db.get_related_files(&[], 10).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_upsert_file_keeps_the_same_id_across_updates() {
        // WHY this matters: INSERT OR REPLACE would delete-then-reinsert on a
        // path conflict, handing out a fresh AUTOINCREMENT id each time and
        // orphaning every previous nodes/edges row tied to the old id.
        let temp_dir = std::env::temp_dir().join("comP_test_upsert_file_stable_id");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap()).await.unwrap();
        let first_id = db.upsert_file("a.rs", "hash-v1", "rust", 10).unwrap();
        let second_id = db.upsert_file("a.rs", "hash-v2", "rust", 20).unwrap();

        assert_eq!(first_id, second_id, "re-upserting the same path must keep its id");

        let (files, _, _) = db.get_stats().unwrap();
        assert_eq!(files, 1, "re-upserting must update the row in place, not add a second one");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_clear_file_symbols_removes_only_the_target_files_rows() {
        let temp_dir = std::env::temp_dir().join("comP_test_clear_file_symbols");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap()).await.unwrap();
        let fa = db.upsert_file("a.rs", "h1", "rust", 10).unwrap();
        let fb = db.upsert_file("b.rs", "h2", "rust", 10).unwrap();

        let na1 = db.insert_node(fa, "alpha_old", "fn", 1, 0, None, true, None).unwrap();
        let nb = db.insert_node(fb, "beta", "fn", 1, 0, None, true, None).unwrap();
        db.insert_edge(nb, na1, "calls").unwrap(); // b depends on a's old symbol
        db.insert_edge(na1, nb, "calls").unwrap(); // and a depends back on b

        db.clear_file_symbols(fa).unwrap();

        let (_, node_count, edge_count) = db.get_stats().unwrap();
        assert_eq!(node_count, 1, "only b's node should remain");
        assert_eq!(edge_count, 0, "edges touching a's removed node (either direction) must go too");

        // b's own row is untouched — clear_file_symbols must not touch other files.
        let mut stmt = db.conn.lock().unwrap().prepare("SELECT id FROM nodes WHERE file_id = ?").unwrap()
            .query_map([fb], |row| row.get::<_, i64>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(stmt, vec![nb]);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_reparsing_a_file_does_not_duplicate_its_nodes() {
        // End-to-end version of the two tests above: upsert_file + clear_file_symbols
        // together must mean a file's node count reflects only its latest parse.
        let temp_dir = std::env::temp_dir().join("comP_test_reparse_no_duplicate_nodes");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let db = GraphDB::new(temp_dir.to_str().unwrap()).await.unwrap();

        // First "parse": two symbols.
        let file_id = db.upsert_file("a.rs", "hash-v1", "rust", 10).unwrap();
        db.clear_file_symbols(file_id).unwrap();
        db.insert_node(file_id, "old_fn_one", "fn", 1, 0, None, true, None).unwrap();
        db.insert_node(file_id, "old_fn_two", "fn", 2, 0, None, true, None).unwrap();

        // Second "parse" of changed content: one symbol, same file_id (renamed function).
        let file_id_again = db.upsert_file("a.rs", "hash-v2", "rust", 12).unwrap();
        assert_eq!(file_id, file_id_again);
        db.clear_file_symbols(file_id_again).unwrap();
        db.insert_node(file_id_again, "new_fn", "fn", 1, 0, None, true, None).unwrap();

        let (files, nodes, _) = db.get_stats().unwrap();
        assert_eq!(files, 1);
        assert_eq!(nodes, 1, "old symbols must not linger alongside the new one");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
