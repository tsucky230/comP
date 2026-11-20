# Configuration

All settings are under `comp.*` in VS Code settings (`Ctrl+,`).

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `comp.maxContextTokens` | number | `8000` | Token budget `run_pipeline` uses when the agent passes no `max_tokens`. When you set it, comP writes it to `.comp/config.json` as `default_budget_tokens` (see [Compression Rules](#compression-rules)), which every agent's daemon reads, and keeps it in step when you change it. Clearing the setting removes the value comP wrote; a value you wrote there by hand stays. Nothing is written in a folder without `.comp/`. |
| `comp.enableCodeLens` | boolean | `true` | Show dependency counts as CodeLens above exported symbols. Takes effect as soon as you change it. |
| `comp.autoIndex` | boolean | `true` | Automatically index files on workspace open. Also gates the branch-switch watcher — see [Branch switches](./MCP_SETUP.md#branch-switches) — since disabling auto-indexing means no automatic reindex should fire from either path. |
| `comp.exclude` | string[] | `[]` | Additional directory names to exclude from indexing. Synced to `.comp/config.json` on activation. Changes take effect after Force Re-index. |
| `comp.conversationRecording.enabled` | boolean | `false` | **Beta.** Record each conversation turn to `.comp/history/`. See [Conversation recording (beta)](#conversation-recording-beta). |
| `comp.conversationRecording.claudeCode` | boolean | `true` | **Beta.** Record Claude Code turns (Stop hook). Only takes effect while `comp.conversationRecording.enabled` is on. |
| `comp.indexWorkspaceFolders` | boolean | `true` | In a multi-root workspace, add the other workspace folders to each folder's index. See [Multi-root workspaces](#multi-root-workspaces). |
| `comp.blockHiddenHomeFolders` | boolean | `true` | Refuse additional roots in hidden folders of your home directory (`~/.ssh`, `~/AppData`, …). See [Hidden folders of the home directory](#hidden-folders-of-the-home-directory). |
| `comp.useWorkspaceDaemonBuild` | boolean | `false` | For comP developers. Run the daemon built in this workspace (`daemon/target/release/`, then `daemon/target/debug/`) instead of the bundled one, and write that path into agent MCP settings. Machine-level, so a repository's `.vscode/settings.json` cannot turn it on — but once on, it applies to **every** folder you open, so a cloned repository with a file at that path gets it run. Turn it on only while developing comP. comP warns when the running daemon's version differs from the extension's. |
| `comp.ruleSharing.enabled` | boolean | `false` | **Beta.** Share git-tracked instruction files between agents: `run_pipeline` returns other agents' relevant rules and `check_rule_conflicts` lists possible contradictions. See [Beta Features → Rule sharing](./BETA_FEATURES.md#rule-sharing). |

## Conversation recording (beta)

Off by default. When `comp.conversationRecording.enabled` is on:

1. The setting is synced to `.comp/config.json` (`conversationRecording`) on activation and whenever it changes.
2. **comP: Setup Agents** with Claude Code selected installs a Stop hook into `.claude/settings.local.json`:
   `"<comp-daemon>" record-turn "<workspace>"`. It is never written to the shared `settings.json`.
3. Each Claude Code turn (request up to 600 characters, response up to 400) is appended to `.comp/history/log-YYYY-MM.jsonl`.

Turning the setting off stops recording immediately — `record-turn` checks `.comp/config.json` before reading
anything — but leaves the hook installed. Run **comP: Remove Conversation Recording Hooks**
(`comp.removeHistoryHooks`) to remove it; other hooks and settings in the file are kept and a `.bak` backup is taken.

Records never leave the workspace, but they contain conversation text. Keep `.comp/` out of version control
(add it to `.gitignore`). Other agents are not recorded automatically yet; the instruction files ask them to call
`session_log` after each task. Step-by-step guide and troubleshooting: [Beta Features](./BETA_FEATURES.md#conversation-recording).
Plan and status: [docs/dev/CONVERSATION_RECORDING_ja.md](../dev/CONVERSATION_RECORDING_ja.md).

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

Relative entries are resolved against the workspace root. The change takes
effect at the next daemon start or **comP: Force Re-index**.

How the additional directories appear in results:

- Files in the workspace root keep their relative path (`src/main.rs`).
- Files in an additional directory are listed by absolute path with forward
  slashes (`E:/dev/shared-lib/src/lib.rs`), so two directories can both
  contain a `README.md` without colliding, and an agent can open the path as is.
- Dependencies are resolved across directories: `get_impact_graph` follows a
  call from the workspace root into an additional directory.

Entries that are ignored (with a warning in the daemon log): a path that is not
an existing directory, the workspace root itself or anything inside it, a
parent of the workspace root, and duplicates. Removing an entry removes that
directory's files from the index at the next start or re-index.

Each additional directory uses its own `.comp/ignore` and the `exclude` list
of its own `.comp/config.json`, if it has them. `get_git_diff_context`,
session memory and conversation history cover the workspace root only.

### Hidden folders of the home directory

`.comp/config.json` lives in the repository, so a repository you just cloned
can name any directory as an additional root — `"additional_paths":
["../../.ssh"]`, for example. To keep such a file from exposing your secrets,
comP refuses, by default:

- an additional root that is, or is inside, a protected folder directly under
  your home directory: any folder whose name starts with `.` (`.ssh`, `.aws`,
  `.config`, `.claude`, …), `AppData` (Windows) and `Library` (macOS). A
  symlink that leads into one is judged by where it leads;
- any file in those folders when an additional root is wider, such as the
  home directory itself: it is neither indexed nor readable through
  `compress_file` or file updates.

`AppData\Local\Temp` (the Windows temp directory) is not protected. The
workspace root itself is never judged, so opening `~/.config/nvim` as your
workspace keeps working. Refused entries are reported as warnings in the
daemon log.

Only the process environment can turn this off: set
`COMP_ALLOW_HIDDEN_HOME_ROOTS` to `1` (or `true`). In VS Code, set
`comp.blockHiddenHomeFolders` to `false`; this is a machine-level setting, so a
repository's `.vscode/settings.json` cannot change it, and it applies to the
daemon the extension starts. For a daemon an agent starts (Claude Code, Codex,
…), add `COMP_ALLOW_HIDDEN_HOME_ROOTS=1` to the `env` of that agent's comp MCP
entry and restart the agent.

### Multi-root workspaces

When a VS Code workspace has several folders, each folder that uses comP has
its own `.comp/index.db`, and an agent started in one folder reads that
folder's database. With `comp.indexWorkspaceFolders` on (the default), the
extension lists the *other* folders in each folder's `.comp/config.json`:

```json
{
  "workspace_folders": ["E:\\dev\\other-project"]
}
```

The daemon treats `workspace_folders` exactly like `additional_paths`, so an
agent started in any folder can search every folder.

- The list is rewritten when the extension starts, when folders are added to
  or removed from the workspace, and when the setting changes. Do not edit it
  by hand; put your own entries in `additional_paths`, which the extension
  never touches.
- Only folders that already have a `.comp/` directory get the list. comP does
  not create `.comp/` in a folder that is not using it.
- A workspace folder that is a hidden folder of your home directory (for
  example `~/.config`) is not added to the other folders' index while
  `comp.blockHiddenHomeFolders` is on.
- Each folder indexes all the folders, so indexing takes longer and every
  `.comp/index.db` grows. Turn the setting off to empty the lists.
- An agent that is already running (for example Claude Code in the other
  folder) picks up a changed list the next time its MCP server starts.

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

### Letting comP manage MAGATAMA

[MAGATAMA](https://github.com/tsucky230/MAGATAMA) reads comP's index and is
installed from PyPI, so it does not update with the extension. comP can register
it for you and keep it on the release comP was tested with.

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `comp.magatama.manage` | boolean | `false` | Machine scope. When on, Setup Agents adds a `magatama` entry next to `comp` in each JSON and Codex config: `uvx magatama@<version> serve` with `env.COMP_MANAGED_BY = "comP"`. Needs [uv](https://docs.astral.sh/uv/). |

- **comP: Let comP manage MAGATAMA** turns the setting on and, after asking,
  replaces MAGATAMA entries you wrote by hand with comP's entry.
- comP rewrites only entries that carry `COMP_MANAGED_BY = "comP"`, and only to a
  newer version, so an older comP in another window never moves it back. Entries
  you wrote yourself are never changed.
- After writing or upgrading an entry, comP downloads that MAGATAMA release in the
  background, so the agent's first start does not wait for it.
- Continue and Aider are not covered; register MAGATAMA there by hand.
- If you keep your own entry, comP checks its version at startup (for entries in
  your home directory it runs the entry's own command with `--version`; for
  workspace configs it only runs `magatama --version` from PATH, never a command
  a cloned repository wrote) and warns when it is older than the tested release.

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

You rarely need it. While `comp.autoIndex` is on, the index follows your edits:

- Saving, creating or deleting a source file updates that file within about half a second.
- Deleting, moving or renaming a folder — in the Explorer, or with `mv` / `git mv` in a terminal —
  runs one incremental pass about a second later. It drops the entries of files that are gone, adds
  the files that moved in, and re-links the dependencies of the moved files and of the files that
  call into them. Changes inside hidden or excluded folders, empty folders and — in a git
  repository — gitignored folders are ignored.
- Switching git branches rebuilds the index from scratch, as Force Re-index does.
