# Configuration

All settings are under `comp.*` in VS Code settings (`Ctrl+,`).

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `comp.maxTokens` | number | `8000` | Maximum tokens for `run_pipeline` context capsule |
| `comp.enableCodeLens` | boolean | `true` | Show dependency counts as CodeLens above symbols |
| `comp.autoIndex` | boolean | `true` | Automatically index files on workspace open. Also gates the branch-switch watcher — see [Branch switches](./MCP_SETUP.md#branch-switches) — since disabling auto-indexing means no automatic reindex should fire from either path. |
| `comp.exclude` | string[] | `[]` | Additional directory names to exclude from indexing. Synced to `.comp/config.json` on activation. Changes take effect after Force Re-index. |

## Workspace vs User settings

Settings can be applied at user level (`~/.config/Code/User/settings.json`) or
per-workspace (`.vscode/settings.json`). Workspace settings take precedence.

## Multi-Agent Configuration

comP works with multiple AI agents simultaneously. Each agent gets its own configuration:

### VS Code Integrated Agents

For agents running inside VS Code (Copilot, Cline), configure MCP servers in `.vscode/mcp.json`:

```json
{
  "servers": {
    "comp": {
      "command": "comp-daemon",
      "args": [],
      "env": {
        "COMP_WORKSPACE_ROOT": "."
      }
    }
  }
}
```

### External Agents (Claude Code, Codex, Cursor, Antigravity)

External agents use their own MCP configuration files. Run:

```text
Ctrl+Shift+P → "comP: Setup Agents"
```

comP writes itself into each agent's real config file and merges with any MCP
servers already listed there. Nothing needs to be copied by hand. See
[MCP_SETUP.md](./MCP_SETUP.md) for the full path table, the backup behaviour, and
the restart step each agent needs afterwards.

### Using Multiple Agents in One Workspace

You can use Claude Code + Cursor + Copilot simultaneously: select all of them in
**comP: Setup Agents**, then restart each one. They share the same
`.comp/index.db`, so the workspace is indexed once regardless of how many agents
read it.

---

## Multi-path indexing (monorepo / multi-root)

Create `.comp/config.json` in the workspace root to index additional directories
into the same graph database:

```json
{
  "additional_paths": [
    "../shared-lib",
    "/absolute/path/to/another-project"
  ]
}
```

---

## Compression Rules

Control compression level per file extension in `.comp/config.json`:

```json
{
  "default_budget_tokens": 8000,
  "compression_rules": {
    "*.md": 0,
    "*.rs": 2,
    "*.ts": 1
  }
}
```

| Option | Values | Description |
| --- | --- | --- |
| `default_budget_tokens` | integer | Token budget for `run_pipeline`. When set, compression level is auto-selected (0→1→2) to fit within budget. |
| `compression_rules` | object | Glob pattern → compression level (0/1/2). Overrides auto-budget selection per file. |

Compression levels:

- `0` — full source (no change)
- `1` — compact: comments and blank lines removed (~20-35% smaller)
- `2` — skeleton: function/class bodies replaced with `{ ... }` (~50-70% smaller)

All paths are indexed into the primary workspace's `.comp/index.db`.
Relative paths are resolved from the workspace root.

---

## Agent Setup Configuration

### Auto-generation of LLM Constitution Files

When you run `comP: Setup Agents`, comP automatically creates or updates local constitution files
(`.claude/CLAUDE.md`, `CLAUDE.md`, and agent-specific files like `.github/copilot-instructions.md`)
with the **comP MCP Tool Usage** rules (call `run_pipeline` first) and the
**Session Continuity** instructions that prompt the LLM to call `session_recall()`
when resuming work. Each section is tracked by its own marker, so a repeat run
never appends a second copy and never overwrites your own edits.

To **disable** this auto-generation (if you prefer to manage constitution files manually),
add this to `.comp/config.json`:

```json
{
  "autoGenerateConstitution": false
}
```

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `autoGenerateConstitution` | boolean | `true` | When `true`, Setup Agents auto-creates/updates CLAUDE.md files with session_recall instructions. When `false`, skips auto-generation (manual control). |

**Note**: Setting this to `false` does NOT prevent the MCP configuration files
(`.mcp.json`, `.cursor/mcp.json`, and the rest) from being written — only the LLM
instruction files are affected.

---

## Excluding files from indexing

comP respects `.gitignore` (and nested `.gitignore` files throughout the workspace).
Files and directories matching gitignore patterns are never indexed or re-indexed.

Hidden directories (names starting with `.`) are also excluded automatically,
so `.venv`, `.pytest_cache`, `.mypy_cache`, etc. require no additional configuration.

### Excluding directories via VS Code settings

Use the `comp.exclude` setting to specify directory names that should never be indexed:

```json
// .vscode/settings.json
{
  "comp.exclude": ["env", "data", "dist"]
}
```

The extension syncs this list to `.comp/config.json` on activation. The daemon reads it each time
an indexer is created, so both initial indexing and **Force Re-index** pick up the changes.

> **Note**: Values in `comp.exclude` are matched against directory **name segments** (not paths),
> so `"env"` excludes any directory named `env` at any depth.

### Excluding via config.json directly

You can also write the `exclude` array directly to `.comp/config.json`:

```json
{
  "exclude": ["env", "data"]
}
```

Changes to `.comp/config.json` take effect on the next **Force Re-index** (`Ctrl+Shift+P` →
`comP: Force Re-index Workspace`).

### Excluding via .comp/ignore

To exclude additional paths that are **not** already covered by `.gitignore`, create
`.comp/ignore` in the workspace root using standard gitignore syntax:

```gitignore
# .comp/ignore
venv/
__pycache__/
legacy_data/
*.log
```

Common patterns for Python projects:

```gitignore
venv/
__pycache__/
*.pyc
.pytest_cache/
.mypy_cache/
```

> **Note**: `files.exclude` in VS Code settings has no effect on comP's daemon-side
> indexing. Use `.gitignore` or `.comp/ignore` to control what the daemon indexes.

## Automatic limits

The following limits are applied automatically and require no configuration:

| Limit | Value | Behavior |
| --- | --- | --- |
| Max file size | 5 MiB | Files larger than 5 MiB are silently skipped during indexing. Useful for large generated files, binary assets, or data files inadvertently left in the workspace. |
| Large-workspace warning | 2 000 files | When more than 2 000 files are found after exclusions, a warning is logged listing the top directories by file count. Use this as a hint for which directories to add to `.comp/ignore`. |

## Manual re-indexing

Run `comP: Force Re-index Workspace` from the Command Palette (`Ctrl+Shift+P`)
to rebuild the index from scratch.
