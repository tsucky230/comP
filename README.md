<p align="center">
  <img src="resources/comp-icon.png" width="128" height="128" alt="comP Logo">
</p>

# comP — Give your AI a map of your repository

comP is a VS Code extension for AI coding agents such as Claude Code, Codex and Cursor. It indexes your repository's files, functions and call relationships on your own machine and hands that index to the agent. Ask comP at the start of a task, and the agent learns in one step where things live and what depends on what. comP is open source (MIT), and the index and the work records stay on your machine.

🌐 [Website](https://tsucky230.github.io/comP/) / [日本語](README_ja.md)

The sections from "I built it to cut token costs" through "MAGATAMA" give you the whole picture. To set it up, start at "Installation & Setup". The sections after that are for looking things up when you need them.

---

## I built it to cut token costs

The original goal was simple: make the AI read fewer tokens. Every time I asked Claude Code for a small fix, it ran grep after grep and opened files that had nothing to do with the task before it got anywhere. Paying for that exploration, in money and in waiting, on every request felt like a waste.

So I built a way to index the repository once and, given a description of the task, return only the files and symbols (names of functions and classes) that matter. Token use did go down. The longer I used it, though, the more I noticed a bigger change: the quality of the AI's work had changed.

An agent with a map more often opens the right file first, checks the callers before it changes a function, and picks up from the decisions of the last session. Its view widens from the file in front of it to the whole repository. I think that change is the most valuable thing comP does. If you plan to hand more of your work to an AI, I would love for you to try it on your own repository.

---

## An AI with a map does better work

comP runs as an MCP server (MCP is the common protocol AI agents use to call outside tools) and offers tools such as `run_pipeline` to the agent. Normally you just ask for the work, and the agent calls these tools itself.

### It reaches the right files on its first move

When the agent describes the task in one sentence to `run_pipeline`, comP searches the index and returns files ranked by relevance. Files with uncommitted changes (what `git diff HEAD` reports) come first, and files one dependency step away come along. Because the agent starts reading from there, it is less likely to open the wrong file, build on a wrong assumption, and have to start over. You ask the way you always do. Even if you ask in another language, the agent translates the task into English when it calls the tool.

```text
Fix the JWT expiry bug in the auth middleware. Start by finding the related code with comP's run_pipeline.
```

When I asked for multi-root workspace support, the agent caught an existing bug from its first `run_pipeline` result, before writing any of the new feature. It read the returned `daemon/src/main.rs` and found that the `additional_paths` feature was wiping the main index. I think a map is what lets the agent see past the spot it was asked about.

### It sees the blast radius before it changes anything

With changes made by an AI, what I fear most is fixing one function and quietly breaking a caller in another module. `get_impact_graph` follows the dependency graph and lists every function and file that depends on a given symbol. To look up dependents by name, `get_dependencies` works too. Here is part of a real result for a path-checking function in comP itself.

```text
get_dependencies({ "name": "validate_within_workspace", "direction": "in" })
- Depended on by `handle_compress_file` (function) via `function_call` in `daemon/src/mcp/mod.rs`
```

Before touching anything, you know that changing this check means checking the behaviour and tests of `compress_file` as well. When the agent calls this before an edit, it goes in with the callers and tests to check already in hand. I want this to stop the slow decay in quality that is common when an AI makes change after change.

### The next session inherits yesterday's work and decisions

A session with an AI ends when you close it. Ask the next day why retries were capped at three, and the new session has no idea.

comP automatically records what the agent looked up with `run_pipeline` and `get_context` (the question, and the files and symbols it found) across sessions. Decisions are kept when the agent writes the request and the outcome to `session_log` at the end of a piece of work. The next session calls `session_recall` and carries on from there. Below is an excerpt of a real record from comP's own development (formatting and long lines trimmed).

```text
session_recall({ "query": "stale Claude Code comp MCP paths" })
- 2026-10-19 13:23  Query: "Implement automatic repair of stale Claude Code comp MCP paths on VS Code startup, ..."
  - Outcome: Updated src/mcp/AgentSetup.ts to refresh versioned old comP extension binary paths
    even if old files exist, ... Did not alter live user-scope Claude configuration or remove
    duplicate registration.
  - Files: src/mcp/AgentSetup.ts, src/mcp/tests/AgentSetup.test.ts, docs/user/MCP_SETUP.md, CHANGELOG.md
```

The records survive a restart of comP's daemon (the background process that manages the index), and a switch from Claude Code to Codex loses nothing. Questions in Japanese work too. In my experience, the memory features in Claude and ChatGPT are good at remembering conversation summaries and preferences, while code details tend to drop out of those summaries. comP's records are an index of technical decisions in one repository. They remember different things, so I recommend using both.

---

## The numbers

How much comP helps depends heavily on the size of the repository and the kind of work. Here are the measurements I have, with their conditions.

| Situation | Value | How to read it |
| --- | --- | --- |
| comP's own repository (about 440,000 tokens of indexed files): what `run_pipeline` returned for the multi-root task described above | About 8,000 tokens (9 files) | Compressed to fit the default budget of 8,000 tokens, which is about 1.8% of the whole repository. How much an agent would actually read depends on the task, so treat any saving computed from this as an upper bound |
| MAGATAMA's repository (described below): one "get an overview" task, measured on the MAGATAMA side | An estimated 145,978 tokens of selected source, against a 289-token comP summary | A single measurement. Other projects will not see the same ratio |
| A small repository of a few dozen files | Little difference [not measured; my expectation] | Reading everything is cheap anyway, so there is less to gain |

Fewer do-overs from an agent that got lost while exploring is an effect the numbers barely show. A lost agent reads the wrong file, builds on a wrong assumption, fails the tests and starts over, and each round trip costs thousands of tokens [rough estimate]. In my own use, those round trips dropped noticeably once comP was in place, and the drop was largest with cheaper models.

---

## With MAGATAMA, you get the places to check before a change, with evidence

[MAGATAMA](https://github.com/tsucky230/MAGATAMA) is a separate MCP server that reads comP's index and answers the question "if I change this file, what do I need to check?". comP draws the map; MAGATAMA uses it to inspect a change before you make it.

comP's `get_impact_graph` starts from one symbol and lists what depends on it. MAGATAMA's `analyze_change_scope` instead takes the files you plan to change. It walks the dependencies backwards from every symbol in those files, and for each candidate the change may ripple out to, it attaches the path the impact travels as evidence. Where the index lacks the clues to decide, it writes "unknown (needs checking)" instead of guessing.

| What you want to know | Where to ask |
| --- | --- |
| Which function to read first | comP's `run_pipeline` / `get_context` |
| What to check if this file changes | MAGATAMA's `analyze_change_scope` |
| Why this file was changed last time | MAGATAMA's history lookup and comP's `session_recall` |
| What has changed since the last session | MAGATAMA's patrol (checks changes made while you were away and keeps them for the next session) and comP's `session_recall` |

Give `analyze_change_scope` the files you plan to change, and it returns three things: the symbols in those files, the candidates the change may ripple out to (callers and so on), and the dependencies that put each candidate on the list. When a file name is ambiguous, or the dependencies are missing from the index, the answer is "unknown". I like that honesty. Treat the result as a starting point for review, not as proof of every runtime effect.

comP can register MAGATAMA for you and keep it current. Install [uv](https://docs.astral.sh/uv/), turn on the `comp.magatama.manage` setting (or run **comP: Let comP manage MAGATAMA**), then run **comP: Setup Agents**. comP starts MAGATAMA as `uvx magatama@<version> serve`, pinned to the release comP was tested with, so when comP updates from the Marketplace, MAGATAMA follows. comP only rewrites entries it wrote itself, and never to an older version. If you registered MAGATAMA by hand, comP checks its version at startup and tells you when it is older than the one comP was tested with. Manual setup steps are in MAGATAMA's README.

---

## Installation & Setup (3 Steps)

comP needs VS Code 1.91 or later. The daemon ships for Windows, macOS and Linux. In a git repository, prioritizing uncommitted changes and detecting branch switches also work.

### 1. Install it in VS Code

Search the Extensions view (`Ctrl+Shift+X`) for "comP - Code Context Engine" and install it.

### 2. Open a folder and build the index

Open the repository you work on and press **Start** from the comP icon in the Activity Bar (far left). The index is built in the background, and the status bar shows its progress. After the first run, only changed files are parsed again.

```text
◈ comP: 12534 symbols | ✓ Ready
```

The index and the work records go into the repository's `.comp/`. If you commit it as is, your work records and even conversation text end up on the remote, so add the two lines below to `.gitignore`. The second line keeps `.comp/ignore` alone, in case your team wants to share its exclusion settings.

```gitignore
.comp/*
!.comp/ignore
```

### 3. Connect your AI agents

Run `comP: Setup Agents` from the Command Palette (`Ctrl+Shift+P`) and pick your agents. Agents found on this PC are already selected. comP writes to the configuration files each agent actually reads and leaves your other MCP servers untouched.

| Agent | Where comP writes |
| --- | --- |
| Claude Code | `.mcp.json` (optionally also runs `claude mcp add --scope user`). With conversation recording (beta) on, also a Stop hook in `.claude/settings.local.json` |
| Codex | `~/.codex/config.toml` and `.codex/config.toml` |
| GitHub Copilot | `.vscode/mcp.json` |
| Cursor | `.cursor/mcp.json` and `~/.cursor/mcp.json` |
| Cline | The MCP settings in VS Code globalStorage |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Continue.dev | `.continue/mcpServers/comp.yaml` (project and home) |
| Antigravity (App, IDE, `agy` CLI, VS Code extension) | `~/.gemini/config/mcp_config.json` |
| Aider | `.aider.conf.yml` |
| Gemini CLI | `.gemini/settings.json` and `~/.gemini/settings.json` (only with a paid Gemini API key or a Code Assist Standard / Enterprise license; personal accounts lost access on 2026-06-18) |

Then restart the agents. None of them re-read MCP settings while running. In a VS Code-based editor, **Developer: Reload Window** from the Command Palette is enough. Per-agent instructions appear under "comP Setup" in the Output panel, and the details are in the [MCP setup guide](docs/user/MCP_SETUP.md).

---

## Getting the AI to actually use comP

Even with comP installed, an agent may decide on its own to go back to grep or to reading files directly. In my experience, rules in its instruction file (`CLAUDE.md` for Claude Code, `AGENTS.md` for Codex, `.github/copilot-instructions.md` for Copilot) settled most of that. `comP: Setup Agents` writes a minimal set of rules, and if you give the agent the prompt below, it will rewrite them to fit your project.

```markdown
Please update CLAUDE.md. Add the following as MUST / NEVER rules, with a one-line reason for each.
Point out anything that contradicts the existing rules.

MUST:
- Before grep or find, call comP's run_pipeline to investigate code
- Before changing existing code, check the blast radius with get_impact_graph
- At the start of a session, check past related decisions with session_recall
- At the end of a piece of work, record the request and the outcome with session_log

NEVER:
- Assume a file that run_pipeline did not return
```

If the agent still reads files directly, ask it: "Why didn't you use run_pipeline? State the cause in one line and propose one rule that would prevent it." Grow the rules one line per slip; for me, things settled within a few rounds.

---

## Configuration

To leave things out of the index, create `.comp/ignore` at the project root and list them in `.gitignore` syntax. Files and folders starting with `.`, paths matched by `.gitignore`, files over 5 MiB, and `node_modules`, `venv`, `__pycache__`, `coverage`, `vendor` and `out` are left out without any setup. Folder names listed in the VS Code setting `comp.exclude` are left out too when the name matches (the change takes effect after Force Re-index).

```gitignore
dist/
build/
*.min.js
```

To bring other folders into the same index, list them under `additional_paths` in `.comp/config.json`. Since v0.12.0, when a VS Code workspace has several folders open, comP writes the paths of the other folders as `workspace_folders` into the `.comp/config.json` of each folder that has a `.comp/`. An agent started in any folder can then search all of them (`comp.indexWorkspaceFolders`, on by default). Each folder indexes everything, so indexing takes longer.

Also since v0.12.0, hidden folders directly under your home directory (folders starting with `.`, `AppData` and `Library`, except `AppData\Local\Temp`) are never indexed, even if a repository's settings file names them (`comp.blockHiddenHomeFolders`, on by default). This VS Code setting applies to the daemon the extension starts; for a daemon an agent starts, an environment variable turns the protection off.

`.comp/config.json` also controls how much `run_pipeline` returns and how hard it compresses.

| Key | Meaning |
| --- | --- |
| `default_budget_tokens` | The token budget for `run_pipeline` (default 8000). comP picks the compression level that fits |
| `compression_rules` | Compression level per file name pattern such as `*.md` (0 keeps the source, 1 strips comments, 2 keeps signatures only) |
| `max_nodes` / `on_limit_exceeded` | The cap on indexed symbols (default 200000), and what happens past it (default `warn`; `stop` halts) |

Every setting is listed in the [configuration guide](docs/user/CONFIGURATION.md).

---

## Beta features

There are two beta features: conversation recording and rule sharing. Both are off by default.

Conversation recording saves each turn with Claude Code to `.comp/history/` automatically, so `session_recall` can read it later. Turn on `comp.conversationRecording.enabled` in VS Code settings, then pick Claude Code in `comP: Setup Agents`, and a Stop hook for recording is installed. Only Claude Code is supported for now.

Rule sharing returns the relevant sections of instruction files alongside the `run_pipeline` result. The sections come from instruction files written for other agents, such as `CLAUDE.md` and `AGENTS.md`. `check_rule_conflicts` lists pairs of sections that may contradict each other. Only git-tracked files are read.

See [Beta features](docs/user/BETA_FEATURES.md) for how to use them.

---

## How it works

The VS Code extension starts a daemon written in Rust (`comp-daemon`), which reads the repository and builds a SQLite index (`.comp/index.db`). Each AI agent starts its own comp-daemon as an MCP server, and they all share that index.

```text
Files in the repository
  ↓ functions and classes via tree-sitter, plus call relationships
.comp/index.db (a graph in SQLite)
  ↓ searched with BM25, TF-IDF and the dependency graph, compressed to fit the budget
MCP server (comp-daemon)
  ↓
Claude Code / Codex / Cursor / Copilot and others
```

Functions, classes and call relationships are extracted for five languages: Rust, TypeScript, JavaScript, Python and Go. From HTML, only element names are taken. Full-text search covers Markdown, Word, PowerPoint, Excel, PDF and the session records (JSONL). JSON top-level keys, XML element names and Parquet schema columns are indexed as names. Files in other languages do not yield symbols yet.

Saved code and Markdown files are re-indexed one file at a time. A newly created file joins the index at the next start or Force Re-index. Switching branches in git changes too many files at once for that, so comP watches for branch changes and re-indexes everything (`comp.autoIndex` turns this off).

`session_recall` lists records that contain the question as typed first, then ranks the rest with BM25. Alphanumeric text is matched by word and Japanese by one- and two-character groups, with newer records ranked higher. There are 13 MCP tools, listed in [MCP tools](docs/user/MCP_TOOLS.md).

---

## Your data stays on your machine

The index, the work records and the conversation records are all stored in the repository's `.comp/`. The comP daemon does not talk to any outside service, and the extension sends no telemetry. An agent receives only what it asks comP's tools for. What the agent then sends to its LLM depends on that agent's own settings.

There is one exception. When you use `@comp` in VS Code chat, your request and a compressed copy of the attached files are sent to the language model selected in VS Code.

---

## Troubleshooting

### Indexing stalls → run Force Re-index

Check progress in the status bar, and if it has stopped, run `comP: Force Re-index` from the Command Palette. In a very large repository (over 100,000 files), the first run takes a while. After that, only changed files are parsed again.

### MCP tools vanished after an update → they repair themselves on startup

VS Code installs each extension version in its own folder and deletes the old folder on update. Since v0.9.4, comP rewrites stale daemon paths left in agent settings to the current path every time it starts. When the repair notice appears, restart the agent.

### Claude Code shows "Conflicting scopes" → keep only one registration

This warning means `comp` is registered both for the project (`.mcp.json`) and for all projects (user scope), with different paths. Since v0.11.10, paths left stale by an extension update are fixed when VS Code starts. If both paths are valid but differ from each other, for example because you run a development build, the warning stays.

In the project folder, run `claude mcp list` and `claude mcp get comp`, and decide which one to keep. To keep the project entry, run `claude mcp remove comp --scope user` (the shared registration disappears from your other projects too). To keep the shared one, run `claude mcp remove comp --scope project` (comp disappears from this project's `.mcp.json`). Run only one of them, then restart Claude Code. The [MCP setup guide](docs/user/MCP_SETUP.md#resolve-a-claude-code-scope-conflict) lays out how to choose.

### The AI reads files instead of using comP → add rules to its instruction file

Try the prompt in "Getting the AI to actually use comP".

### MCP will not connect → run Setup Agents again

Run `comP: Setup Agents` again, choose "comP Setup" in the Output panel, and check the log. If nothing here solves it, please tell me in an [Issue](https://github.com/tsucky230/comP/issues/new).

---

## Supported agents

| Agent | Status |
| --- | --- |
| Claude Code / GitHub Copilot | Supported and tested |
| Codex (CLI and VS Code extension) | Supported |
| Cursor / Cline / Windsurf / Antigravity / Aider / Continue.dev | Supported |
| Gemini CLI | Supported (see the Setup Agents table above for the conditions) |

Most clients that follow MCP 2024-11-05 should work. If yours does not, please tell me in an [Issue](https://github.com/tsucky230/comP/issues/new).

---

## Where it has been, and where it is going

| Version | Main changes |
| --- | --- |
| v0.1–v0.8.5 | The index and MCP server, Office and PDF indexing, compression, tuning for large repositories |
| v0.8.6–v0.9 | Records that span sessions (`session_log` / `session_recall`) |
| v0.10.0 | Setup Agents writes straight into each agent's configuration files |
| v0.11.0–v0.11.6 | Codex support, English/Japanese switching, re-indexing on branch switches, automatic recovery of a broken index, exact token counts |
| v0.11.7–v0.11.8 | Beta: conversation recording, rule sharing |
| v0.11.9–v0.11.10 | Sturdier agent settings (Codex auto-review, path repair after updates) |
| v0.11.11 | Antigravity 2.0 support; zstd vulnerabilities removed from the daemon |
| v0.12.0 | Multi-root workspaces; protection for hidden folders in the home directory; deleted, moved and renamed folders reach the index; dependency edges and search results stay correct after re-indexing |
| v0.12.1 | Upgrading the extension reliably upgrades the daemon too: the bundled daemon is used unless you opt in, stale agent registrations are repaired, and a mismatched or outdated daemon raises a warning |
| v0.12.2 | Internal restructuring with no change in behavior; dependency updates that clear two RustSec warnings |
| v0.12.3 | comP can register MAGATAMA and keep it on the release comP was tested with; the index records its format version for MAGATAMA to check |
| v1.0 | A stable API |

The full history is in the [CHANGELOG](CHANGELOG.md).

---

## License and contributing

comP is MIT licensed ([LICENSE](LICENSE)). Please send bug reports and ideas to [Issues](https://github.com/tsucky230/comP/issues/new) or [Discussions](https://github.com/tsucky230/comP/discussions/new). For code contributions, see [CONTRIBUTING.md](CONTRIBUTING.md). If comP helps you, a Star or [GitHub Sponsors](https://github.com/sponsors/tsucky230) keeps me going.
