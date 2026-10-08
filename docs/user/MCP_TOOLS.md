# MCP Tools Reference

comP exposes tools via the Model Context Protocol (JSON-RPC 2.0 over stdio).

## Setup

Run `comP: Setup Agent MCP` from the VS Code Command Palette to auto-configure
Claude Code, Cursor, Cline, Windsurf, or Continue.

## Tool annotations

Every tool carries MCP annotations, which clients such as Codex use to decide
whether a call needs approval. The daemon runs locally and never sends anything
over the network, and no tool deletes or overwrites your files:

| Tool | `readOnlyHint` | `destructiveHint` | `openWorldHint` |
| --- | --- | --- | --- |
| `session_log` | `false` | `false` | `false` |
| All other tools | `true` | `false` | `false` |

`session_log` exists to append your request and its outcome to `.comp/history/`,
so it is the one tool that does not claim to be read-only. `run_pipeline` and
`get_context` also append to comP's own `.comp/session-memory/`, but leave your
files untouched, so they count as read-only. For what this changes in Codex, see
[MCP Setup → Codex's auto-review denies comP's tools](./MCP_SETUP.md#codexs-auto-review-denies-comps-tools).

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

Response fields (unreleased):

- `index_recovered_from_corruption_at` (number, epoch ms) — present while `.comp/index.db` is in the catch-up window after an automatic rebuild from a corrupted file (see `get_stats` below for the full explanation). Absent under normal operation, and again once the catch-up re-index finishes.

Response fields (rule sharing, beta — only when `comp.ruleSharing.enabled` is on; see [Beta Features](./BETA_FEATURES.md#rule-sharing)):

- `related_rules` (object) — sections of *other* agents' git-tracked instruction files that match the task. Absent entirely while rule sharing is off.

  ```json
  {
    "note": "Rule sharing (beta): excerpts from instruction files written for other agents ...",
    "items": [{ "file": "AGENTS.md", "heading": "Testing", "hash": "47727fe4…", "text": "..." }],
    "tokens": 162
  }
  ```

  - The caller's own instruction file (e.g. `CLAUDE.md` for Claude Code) is never included.
  - `tokens` is the tiktoken count of the items; the budget is 1000 tokens, separate from `max_tokens`.
  - `items` is `[]` when nothing matches. When the workspace is not a git repository (or git is missing), the object is `{ "unavailable": "<reason>" }` instead.
  - Each returned section is saved once as `.comp/rules/<hash>.md`, and the call's session record keeps `rules: [{file, heading, hash}]`.

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

Response fields (unreleased):

- `index_recovered_from_corruption_at` (number, epoch ms) — present while `.comp/index.db` is in the catch-up window after being found unreadable at daemon startup and automatically rebuilt from scratch (the corrupted file is moved aside to `index.db.corrupt-<epoch_ms>`, never deleted). A rebuilt index starts at zero files/nodes/edges, which otherwise looks identical to "this workspace has nothing indexed" — check this field before assuming an empty or partial result means files were deleted. The field disappears once the next full re-index pass finishes (automatically on startup, or via `comP: Force Re-index`) — not simply once file counts become non-zero, since a re-index in progress can have written only some of the workspace's files at the moment of a given call. Absent entirely for a daemon that has never had to recover.

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

### `check_rule_conflicts`

**Beta (rule sharing).** Lists pairs of sections from different agents' instruction files that may
contradict. comP only finds candidates — the calling agent judges each pair and reports real
contradictions to the user.

```json
{ "max_pairs": 20 }
```

Parameters:

- `max_pairs` (integer, optional, default 20) — 1 to 100; anything else is an error

Response:

```json
{
  "note": "Rule sharing (beta): each pair comes from instruction files of different agents ... Do not edit any instruction file unless the user asks you to.",
  "pairs": [{ "a": { "file": "AGENTS.md", "heading": "Testing", "hash": "…", "text": "…" },
              "b": { "file": "CLAUDE.md", "heading": "Testing", "hash": "…", "text": "…" },
              "similarity": 0.62 }],
  "files_scanned": ["AGENTS.md", "CLAUDE.md", "GEMINI.md"]
}
```

A pair qualifies when the two files belong to different agents, both texts are at least 40
characters, they share at least 2 terms, and their TF-IDF cosine similarity is at least 0.3. Pairs
with the same content words (similarity 0.999 or more) are agreement and left out.

- Rule sharing off: `{ "disabled": "rule sharing (beta) is off — enable comp.ruleSharing.enabled in VS Code settings" }` (a result, not an error)
- Not a git repository: `{ "unavailable": "<reason>" }`

The tool is always listed in `tools/list`, also while rule sharing is off.

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

- `query` (string, optional) — entries whose request or outcome contains `query` (case-insensitive)
  come first, newest first, as before; other entries that share terms with it follow, ranked by BM25
  over words and character 1-/2-grams (so Japanese and multi-word queries match) times a recency
  weight (half-life 90 days)
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

Multi-agent trace: a conversation turn recorded by `record-turn` also shows **Commit** (git HEAD at
the end of the turn, 8 characters) and **Delegations** (the delegations that turn ran, as
`agent (test_exit N)`); a delegation recorded through `append-history` with `kind: "delegation"`
shows **Delegated by** with the request of its turn. The parent is `parent_turn_id` when given,
otherwise the turn of the same `session_id` that ended next after the delegation.

With rule sharing (beta), a run_pipeline entry that handed out rules also shows a **Rules** line
listing each rule as `file#heading (hash8)` — for example `AGENTS.md#Testing (47727fe4)` — where
`hash8` is the first 8 characters of the snapshot hash (`.comp/rules/<hash>.md` holds the exact text
that was given).

**v0.9.2+**: Symbols and Files are capped at **the first 5 entries** each, with the
remainder collapsed into `… (+N more)` — an auto-recorded run_pipeline entry can
carry dozens of symbols, and listing all of them would waste the very tokens
recall is meant to save.

**Recommended**: call `session_recall` at the start of a new session, or when
resuming work, to check the previous request and how it was handled.

## Errors

A failed tool call returns a JSON-RPC error with `code` `-32603`. Since **v0.11.6** the
cause is part of `message` (for example `Internal error: Missing 'task' parameter`);
`data` carries the same text. Before v0.11.6 `message` was always a bare
`Internal error`, which most MCP clients display as-is, so a wrong argument name
looked like a broken daemon.

The most common cause is a wrong or missing argument name. Check it first:

| Tool | Required arguments |
| --- | --- |
| `run_pipeline` | `task` |
| `get_context` | `query` |
| `get_impact_graph` | `symbol_id` (numeric) |
| `get_dependencies` | `name`, `direction` (`in` or `out`) |
| `get_symbol` | `name` |
| `get_file_summary` | `file_path` |
