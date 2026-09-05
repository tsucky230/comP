# MCP Tools Reference

comP exposes tools via the Model Context Protocol (JSON-RPC 2.0 over stdio).

## Setup

Run `comP: Setup Agent MCP` from the VS Code Command Palette to auto-configure
Claude Code, Cursor, Cline, Windsurf, or Continue.

## Tools

### `run_pipeline`

Primary tool. Splits a task description into keywords, searches the indexed symbol
graph, and returns ranked context files.

```json
{ "task": "fix JWT validation bug", "max_tokens": 8000 }
```

Parameters:

- `task` (string, required) — natural language description of the task
- `max_tokens` (number, optional, default 8000) — result budget
- `include_tests` (boolean, optional) — include test files in results
- `include_content` (boolean, optional) — if true, each pivot_file entry includes a `content` field with the file contents
- `compression_level` (0/1/2, optional, default 0) — content compression applied when `include_content` is true:
  - `0` — full source (no change)
  - `1` — compact: comments and blank lines removed (~20-35% smaller)
  - `2` — skeleton: function/class bodies replaced with `{ ... }` (~50-70% smaller)

Response fields (v0.6+):

- `compression_level_applied` (number) — actual compression level used after auto-budget selection
- `budget_adjusted` (boolean) — `true` if compression level was raised to fit within `default_budget_tokens`
- `compression_rules_applied` (boolean) — `true` if any per-extension rules from `compression_rules` were applied

Response fields (v0.9.2+):

- `related_files` (array) — files one dependency hop away from the pivot files (callers/callees in other files), ranked by connecting-edge count, up to 10 entries:

  ```json
  [{ "path": "src/auth/middleware.rs", "edge_count": 4 }]
  ```

- Token estimates per pivot file are based on the real indexed file size (`chars / 4`), no longer on symbol-count heuristics

---

### `get_context`

Search symbols by query string. Returns ranked matches with file paths and line numbers.

```json
{ "query": "DaemonManager", "limit": 10 }
```

---

### `get_impact_graph`

Show all files affected by changes to a symbol (blast radius analysis).

```json
{ "symbol": "request", "file": "src/daemon/DaemonManager.ts", "max_depth": 3 }
```

Parameters:

- `symbol` (string, required) — symbol name to analyze
- `file` (string, optional) — narrow to a specific file when the symbol appears in multiple files
- `max_depth` (number, optional, default 0) — BFS hop limit; 0 means unlimited transitive traversal

---

### `list_indexed_files`

List all indexed files with symbol counts and detected language.

```json
{}
```

---

### `get_symbol`

Return full source of a specific symbol with optional compression.

```json
{ "symbol": "authenticate", "file": "src/auth.rs", "compression_level": 1 }
```

Parameters:

- `symbol` (string, required) — exact symbol name
- `file` (string, optional) — narrow to a specific file
- `compression_level` (number, optional, default 0):
  - `0` — full source (no change)
  - `1` — compact: comments and blank lines removed
  - `2` — skeleton: function/class bodies replaced with `{ ... }`

---

### `get_stats`

Return total file, node, and edge counts (index health check).

```json
{}
```

Response fields (v0.9.2+):

- `daemon_version` (string) — version of the running daemon binary. Compare against the installed release to detect a stale daemon that kept running across an upgrade (on Windows the running exe stays locked, so rebuilds do not take effect until the daemon restarts).

---

### `get_git_diff_context`

Get context for files changed in a git diff. Runs `git diff --name-only <base_ref>` and maps each changed file to its indexed symbols.

```json
{ "base_ref": "main" }
```

Parameters:

- `base_ref` (string, optional, default `HEAD~1`) — git ref to diff against. Use `main` or `master` for branch comparisons.

Returns a Markdown table of changed files with language, symbol count, and whether each file is indexed.

---

### `session_log`

Persists the user's request and its outcome to `.comp/history/log-YYYY-MM.jsonl`.
The entry is reflected into the BM25 index immediately after the write, so later
`run_pipeline` searches naturally surface past exchanges.

Call it when a significant task finishes — it's the "work log" that survives a
session ending or the daemon restarting.

```json
{
  "request": "add a session_log MCP tool",
  "outcome": "implemented handle_session_log in daemon/src/mcp/mod.rs; appends JSONL and indexes immediately",
  "files": ["daemon/src/mcp/mod.rs", "daemon/src/indexer/walker.rs"]
}
```

Parameters:

- `request` (string, required) — the user's request, as text (max 600 characters)
- `outcome` (string, optional) — a summary of the outcome (max 400 characters)
- `files` (string[], optional) — paths of the files that were changed

Example response:

```json
{ "status": "ok", "path": ".comp/history/log-2026-06.jsonl", "timestamp": 1751023456789 }
```

---

### `session_recall`

Searches and returns past exchanges across sessions. Covers **every session**,
including ones from before the daemon last restarted.

Merges `.comp/session-memory.json` (auto-recorded by run_pipeline / get_context)
with `.comp/history/*.jsonl` (explicit session_log entries, plus Stop-hook
auto-records), and returns them newest first.

```json
{ "query": "session_log", "limit": 10 }
```

Parameters:

- `query` (string, optional) — partial-match filter against both the request and outcome fields
- `limit` (number, optional, default 20) — maximum number of results to return

Response format (Markdown text):

```
### Session Recall

- `2026-06-27 01:30` **Query**: "add a session_log MCP tool" (Tokens: 4200)
  - **Outcome**: implemented handle_session_log in daemon/src/mcp/mod.rs; appends JSONL and indexes immediately
  - **Symbols**: `SessionCall`, `format_epoch_ms` (when present)
  - **Files**: `daemon/src/mcp/mod.rs`, `daemon/src/indexer/walker.rs` (when present)
```

Each field (Outcome, Symbols, Files) is shown only when the entry actually has data.

**v0.9.2+**: Symbols and Files are capped at **the first 5 entries** each, with the
remainder collapsed into `… (+N more)` — an auto-recorded run_pipeline entry can
carry dozens of symbols, and listing all of them would waste the very tokens
recall is meant to save.

**Recommended**: call `session_recall` at the start of a new session, or when
resuming work, to check the previous request and how it was handled.
