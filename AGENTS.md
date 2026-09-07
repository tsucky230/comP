# comP — AI Agent Instructions

## MANDATORY: use comP MCP pipeline — do NOT grep or glob the codebase

For every task — bug fixes, features, refactors, debugging:
**call `run_pipeline` FIRST**. It searches the indexed codebase and returns
the most relevant files and symbols for your task.

Do NOT use grep, glob, Bash find, or cat to search/explore the codebase.
comP returns pre-indexed, graph-ranked context that is more relevant and
uses fewer tokens than manual searching.
Only use Read when you need exact raw content to edit a specific line.

## Primary Tool

- `run_pipeline` — **USE THIS FOR EVERYTHING**. Splits your task into keywords,
  searches the symbol graph, and returns ranked pivot files.

  Examples:
  - `run_pipeline({ "task": "fix JWT validation bug" })`
  - `run_pipeline({ "task": "add user authentication", "max_tokens": 12000 })`
  - `run_pipeline({ "task": "sidebar panel webview", "include_tests": true })`

## Other MCP tools (use only when run_pipeline is insufficient)

- `get_context` — search symbols by query string, returns ranked results
- `get_impact_graph` — show files affected by a symbol change (blast radius)
- `list_indexed_files` — list all indexed files with symbol counts and language
- `get_stats` — show total file/node/edge counts (health check)

## Workflow

1. `run_pipeline({ "task": "..." })` — ALWAYS FIRST
2. Need to see what's indexed? Use `list_indexed_files`
3. Editing a specific file? Use Read only for exact line content
4. Need blast radius before refactor? Use `get_impact_graph`

## Parameters

- `max_tokens` — increase result budget (default: 8000)
- `include_tests: true` — include test files in results

---

## comP MCP Tool Usage

At the start of every coding or documentation task, ALWAYS call `run_pipeline` FIRST.
Do NOT read files, run grep/find/Bash searches, or explore the codebase manually before calling run_pipeline.
run_pipeline indexes the entire codebase and returns exactly the relevant files — use it every time.

---

## Session Continuity (デーモン再起動・セッション切れ対応)

セッションはデーモン再起動をまたいで永続化されます。
作業を再開したり、エージェントを再起動した場合は、まず `session_recall` を呼んで過去の文脈を復帰させてください。

**作業を再開する場合**:

1. `session_recall()` を呼んで過去のやり取りを確認する
   - `session_recall({ "query": "keyword" })` — タスクのキーワードで絞り込み
   - `session_recall({ "limit": 5 })` — 直近N件を表示
2. 過去に何をしたかを確認し、その文脈のまま作業を続ける

**補足**: フック機構もプロンプトごとに直近の履歴を自動的に注入します（`<system-reminder>`）が、
過去の作業を手動で確認したり特定のタスクを検索したりする場合は、明示的に `session_recall` を呼ぶと便利です。
