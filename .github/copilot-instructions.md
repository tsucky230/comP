## Session Continuity (デーモン再起動・セッション切れ対応)

セッションはデーモン再起動をまたいで永続化されます。
作業を再開したり、エージェントを再起動した場合は、まず `session_recall` を呼んで過去の文脈を復帰させてください。

**When resuming work**:

1. Call `session_recall()` to view past interactions
   - `session_recall({ "query": "keyword" })` — filter by task keywords
   - `session_recall({ "limit": 5 })` — show last N interactions
2. Review what was done previously and continue in that context

**Note**: The hook system also auto-injects recent history into each prompt (`<system-reminder>`),
but explicit `session_recall` is useful to manually review past work or search specific tasks.

---

## comP MCP Tool Usage

At the start of every coding or documentation task, ALWAYS call `run_pipeline` FIRST.
Do NOT read files, run grep/find/Bash searches, or explore the codebase manually before calling run_pipeline.
run_pipeline indexes the entire codebase and returns exactly the relevant files — use it every time.

---

## comP Session Logging (作業の記録)

タスクを1つ終えたら、`session_log` を呼んで記録してください。
`request` にユーザーの依頼、`outcome` に行ったことと結果を短く書きます（関係したファイルがあれば `files` にも）。
記録は `.comp/history/` に残り、次のセッションで `session_recall` から読めます。

Claude Code で、`.claude/settings.local.json` に `record-turn` の Stop フックがある場合は、各往復が自動で記録されるので `session_log` を呼ばないでください（二重に記録されます）。
