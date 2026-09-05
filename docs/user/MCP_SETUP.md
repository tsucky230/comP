# MCP Server Setup for Multiple Agents

comP runs as an MCP server, so any MCP-capable AI agent can use it.

Setup is automatic. **comP: Setup Agents** writes comP into the config files each
agent actually reads — merging with the servers you already have — and appends the
comP usage rules to your instruction files. Nothing has to be copied by hand.

The one manual step left is restarting the agent, because none of these tools
re-read their MCP configuration while running. See
[Applying the configuration](#applying-the-configuration).

---

## Setup

1. Install comP from the VS Code Marketplace, or build it locally
2. Open the Command Palette and run:

   ```text
   Ctrl+Shift+P → "comP: Setup Agents"
   ```

3. Pick the agents you want. Agents already present on your machine are
   preselected — you can add or remove any of them
4. Read the report in the output panel (**View → Output → "comP Setup"**), then
   restart each agent you configured

If you selected Claude Code, comP also offers to register itself for **every**
project by running `claude mcp add --scope user`. Answering "今はしない" leaves
the project-level registration in place.

---

## What gets written where

| Agent | Project config | Machine-wide config |
| --- | --- | --- |
| Claude Code | `.mcp.json` | `claude mcp add --scope user` (optional) |
| Codex | `.codex/config.toml` | `~/.codex/config.toml` (or `$CODEX_HOME`) |
| Cursor | `.cursor/mcp.json` | `~/.cursor/mcp.json` |
| Cline | — | VS Code `globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` |
| Windsurf | — | `~/.codeium/windsurf/mcp_config.json` |
| Continue | `.continue/mcpServers/comp.yaml` | `~/.continue/mcpServers/comp.yaml` |
| Antigravity | — | `~/.gemini/antigravity-ide/mcp_config.json` |
| GitHub Copilot | `.vscode/mcp.json` | — |
| Aider | `.aider.conf.yml` | — |

Two rules govern these writes:

- **Existing files are backed up.** Before any file is rewritten, comP copies it
  to `<file>.bak`. If the backup cannot be taken, the file is left alone.
- **Machine-wide configs omit `COMP_WORKSPACE_ROOT`.** That variable names one
  project, so writing it into a config shared by every project would make your
  other projects index this one. Without it the daemon falls back to its working
  directory, which the MCP client sets per project.

Other MCP servers already listed in these files are preserved. If a file cannot
be parsed, comP does not overwrite it — it reports the failure and opens a
document with the entry for you to merge by hand.

---

## Applying the configuration

None of these agents reload MCP configuration while they are running. After
setup, do the following for each agent you configured.

### Claude Code

- **CLI**: quit the running session (`/exit` or Ctrl+C), then start `claude`
  again from the project directory
- **VS Code / JetBrains extension**: Command Palette →
  **Developer: Reload Window**

On first start after setup, Claude Code asks whether to trust this project's MCP
servers. Approve it — this prompt is a security check and comP cannot skip it.

Verify with:

```bash
claude mcp list
```

`comp` should be listed as `✔ Connected`.

### Codex

- **CLI**: leave the session and start `codex` again
- **VS Code extension**: Command Palette → **Developer: Reload Window**

Both read the same `config.toml`, so one setup run covers them.

Verify by running `/mcp` inside a Codex session — `comp` should appear with its
tools.

**The project-level file only counts for a trusted project.** Codex skips
`.codex/config.toml` entirely in a project you have not marked trusted. Until you
trust it, only the machine-wide `~/.codex/config.toml` is in effect. That one
works everywhere, so comP writes it first.

comP writes a single `[mcp_servers.comp]` table and leaves the rest of the file
byte for byte. Three shapes cannot be rewritten safely without a full TOML
parser. comP refuses to edit the file rather than risk it, and hands you the
table to paste instead:

- servers declared as an inline table — `mcp_servers = { … }`
- a comp entry in dotted form — `comp = { … }` or `comp.command = …` under
  `[mcp_servers]`
- an existing `[mcp_servers.comp]` whose values span several lines (a multi-line
  `args` array, for instance)

A key named `comp` in an unrelated table, or a server whose name merely starts
with `comp`, is not affected — only the `[mcp_servers]` table is examined.

### Cursor

- Command Palette (`Ctrl/Cmd+Shift+P`) → **Developer: Reload Window**
- If the server does not appear, quit Cursor completely and start it again

Verify in **Settings → MCP**: `comp` should be listed and enabled.

### Cline

- Command Palette (`Ctrl/Cmd+Shift+P`) → **Developer: Reload Window**

Verify from the MCP icon at the top of the Cline chat panel: `comp` should appear
in the server list.

### Windsurf

- Command Palette (`Ctrl/Cmd+Shift+P`) → **Developer: Reload Window**
- Or open the MCP icon at the top right of the Cascade panel and choose
  **Refresh**

### Continue

- Command Palette (`Ctrl/Cmd+Shift+P`) → **Developer: Reload Window**

Continue does not pick up config changes on its own, so the reload is required.
MCP tools are only available in **Agent** mode — switch modes before testing.

### Antigravity

Quit Antigravity completely and start it again. Reloading the window is not
enough for a global config change.

### GitHub Copilot

- Command Palette (`Ctrl/Cmd+Shift+P`) → **Developer: Reload Window**

Switch Copilot Chat to **Agent** mode; `comp` should appear in the tool list.

### Aider

Quit the running Aider session and start it again.

Aider's MCP support differs between releases. If the block comP wrote to
`.aider.conf.yml` is ignored, check your version with `aider --version` against
the [Aider configuration docs](https://aider.chat/docs/config/aider_conf.html).

---

## Troubleshooting

### MCP stopped working after upgrading comP

VS Code installs extensions into a directory that carries the version number
(`~/.vscode/extensions/<publisher>.comp-vscode-<version>`) and deletes the old one
on upgrade. Config files record the absolute path of `comp-daemon`, so an upgrade
leaves them pointing at an executable that no longer exists. The VS Code sidebar
keeps working — it resolves its binary at runtime — but MCP clients fail to start
the server.

comP repairs this automatically: every time the extension activates it checks the
known config files and rewrites any `command` that no longer resolves. **Restart
your agent after seeing the "MCP 設定を修復しました" notification.**

Files that are checked:

| File | Scope |
| --- | --- |
| `.vscode/mcp.json` | workspace |
| `.mcp.json` | workspace |
| `.cursor/mcp.json` | workspace |
| `~/.cursor/mcp.json` | global |
| `~/.codeium/windsurf/mcp_config.json` | global |
| Cline `cline_mcp_settings.json` | global |
| `~/.gemini/antigravity-ide/mcp_config.json` | global |
| `.comp/config/*.json` (legacy) | workspace |

A value is deliberately left alone when it is a relative path, when it contains a
`${...}` variable, or when no replacement binary can be found. The YAML configs
(`.continue/mcpServers/comp.yaml`, `.aider.conf.yml`) are not repaired — re-run
**comP: Setup Agents** for those.

`COMP_WORKSPACE_ROOT` is refreshed in the same pass for workspace-scoped files, so
moving a project or opening the same checkout on another machine no longer breaks
indexing. The value inside a global config is never touched.

### A file was reported as failed

The report names the file and the reason. The usual cause is that the existing
config is not valid JSON, in which case comP refuses to overwrite it rather than
discard the servers it cannot read. Fix the JSON and run setup again, or merge
the entry from the document comP opened.

### Restoring a config

Every rewritten file has a `<file>.bak` sibling holding the version from just
before the last setup run. The report lists the backup path for each file.

### "MCP server not found"

- Verify the `comp-daemon` binary exists at the configured path
- Confirm you restarted the agent after setup
- Restart the agent application rather than just reloading, if a reload did not
  take effect

### No tools appear in chat

- Check that the agent is in **Agent** mode — several clients hide MCP tools in
  other modes
- Run `comP: Force Re-index` to rebuild the index
- Check the VS Code output panel (`View → Output → "comP"`) for errors
- Verify `.comp/index.db` exists in the workspace

### Token compression not working

- Update `comp.maxContextTokens` in VS Code settings (default: 8000)
- Run `run_pipeline` with an increased `max_tokens` parameter

---

## Multi-workspace setup

Open each workspace in VS Code and run **comP: Setup Agents** in each. Every
workspace gets its own `.comp/index.db` and its own project-level config with the
right `COMP_WORKSPACE_ROOT`.

The machine-wide configs written for Cursor, Windsurf, Cline and Continue carry no
`COMP_WORKSPACE_ROOT`, so a single registration follows whichever project the
agent has open.

---

## What's next?

- See [CONFIGURATION.md](./CONFIGURATION.md) for VS Code settings
- See [MCP_TOOLS.md](./MCP_TOOLS.md) for available MCP tools
- Check [GETTING_STARTED.md](./GETTING_STARTED.md) for usage tips
