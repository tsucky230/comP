<p align="center">
  <img src="resources/comp-icon.png" width="128" height="128" alt="comP Logo">
</p>

# comP — The Memory System for Your AI Coding Assistant

**Open-source, 100% local code analysis engine. Works with Claude Code, Cursor, Cline, and Antigravity.**

🌐 **[Official Website](https://tsucky230.github.io/comP/)**

---

## Why comP Exists

Claude Code, Cursor, and other AI coding assistants share **one critical limitation**:

**Every time you ask a question, the AI re-reads your entire project.**

1. You ask "how does this function behave?"
2. The AI reads **the whole project** to understand context
3. DB connections, config, type definitions, dependencies—everything—before it can answer

This "read everything, every time" pattern causes three problems:

| Problem | Impact |
| --- | --- |
| **Massive token spend** | $0.10 per question. 10 questions = $1 |
| **Slow responses** | 5,000 tokens read before answering. 15s first response |
| **Context lost between sessions** | Past decisions vanish; you re-explain the same thing tomorrow |

And there's a **fourth problem** that's easy to overlook:

| Problem | Impact |
| --- | --- |
| **Getting lost while exploring → failed retries** | The AI greps around, reads the wrong file, implements on a wrong assumption, then redoes it. **One failed loop like this burns thousands to tens of thousands of tokens** |

---

## How comP Solves It

comP automatically builds a **"project map + index"** so the AI can grasp "this file does X" **instantly, without searching around.**

```
With comP:
  Question → comP extracts only the relevant files → AI answers via the shortest path
  Next day → session_recall restores yesterday's decisions → no re-explaining
```

---

## How Much Does It Actually Help? An Honest Estimate

comP's impact **depends heavily on your use case**. Here's an honest breakdown:

| Use case | Estimated input token reduction | Why |
| --- | --- | --- |
| **Investigating/fixing code in medium-to-large repos** | **60–94%** | "Read everything or grep blindly" is replaced by "pull only the relevant spots from the index." The 94% figure up top comes from this category |
| **Impact analysis ("what breaks if I change this function?")** | **Large** | `get_impact_graph` mechanically enumerates downstream effects from the dependency graph. No more AI guesswork exploration |
| **Cross-session continued development** | **Large** | Re-explaining and re-investigating context collapses into one `session_recall` call |
| **Small repos (a few dozen files)** | ~20–40% | Reading everything was already cheap, so there's less room to save |
| **Non-code work (writing, planning, etc.)** | Near zero | Nothing to index |

### The Biggest Effect the Numbers Don't Show: Fewer Failed Retries

Looking only at the token-reduction percentage understates the real impact. Here's the actual cost structure:

```
Traditional failure pattern (especially common with cheaper models):
  explore → read the wrong file → implement on a wrong assumption → test fails
  → re-explore → re-implement → ... (thousands of tokens per loop × N)

With comP:
  run_pipeline surfaces the right related code up front
  → less room for wrong assumptions → retries become rare in the first place
```

In other words, comP doesn't just reduce "tokens per call"—it reduces **the number of calls itself**. Cheaper models get lost while exploring more easily, so **the cheaper the model, the bigger comP's benefit**. Tasks that used to require a top-tier model now fall within reach of a cheap one—that's arguably comP's biggest real-world effect.

---

## Installation & Setup (3 Steps)

### 1. Install in VS Code

1. Open VS Code
2. Go to **Extensions** (`Ctrl+Shift+X`) and search for **"comP - Code Context Engine"**
3. Click **Install**

### 2. Open Your Folder

Open the project you're working on in VS Code (a Git repository works best).

### 3. Start comP

- Click the **comP icon** in the Activity Bar (left sidebar)
- Click **"▶ Start"**
- Indexing begins in the background
- Watch the **status bar** (bottom of VS Code) for progress

```text
◈ comP: 12,534 symbols | ✓ Ready
```

---

## Connect Your AI Agent (One-Time Setup)

```text
Ctrl+Shift+P → "comP: Setup Agents"
```

| Agent | What to Do |
| --- | --- |
| **Claude Code** | Copy the generated `claude mcp add` command and run it in your terminal |
| **GitHub Copilot** | Auto-written to `.vscode/mcp.json` |
| **Cursor** | Copy the generated config into `~/.cursor/mcp.json` |
| **Cline** | Paste into Cline's MCP settings |
| **Antigravity / Aider** | Auto-configured |
| **Windsurf** | Copy into `~/.codeium/windsurf/mcp_config.json` |
| **Continue.dev** | Add to `~/.continue/config.py` |

Details: [docs/user/MCP_SETUP.md](docs/user/MCP_SETUP.md)

---

## How to Use It

### Claude Code (Simplest)

```markdown
@comP run_pipeline
Analyze every function affected if I change the authenticate() function
```

### VS Code Native Chat (@comp)

```markdown
@comp #file:src/main.rs Explain what this function does
```

Attached files are automatically compressed (comments stripped, skeletonized) before being sent to the LLM.

---

## A Prompt Kit for Growing CLAUDE.md into a "comP-first" Constitution

Installing comP isn't enough—**if you don't update the AI's own behavioral rules, you leave half the benefit on the table.** When instructions are ambiguous, the AI defaults to "it's easier to just read the file directly." Feed the prompts below to Claude and it will rewrite your CLAUDE.md (or `.github/copilot-instructions.md` for Copilot) to be comP-first.

### Prompt 1: Initial rollout (add comP rules)

```markdown
Update CLAUDE.md. Add the following as MUST/NEVER rules:

MUST:
- For code investigation/search, call comP's run_pipeline first instead of grep/find/Bash
- Before changing existing code, check downstream impact with get_impact_graph
- At the start of a session, restore relevant past decisions with session_recall

NEVER:
- Reading the entire codebase without going through comP
- Implementing based on guesses about files not present in run_pipeline's results

Add a one-line reason for each rule, and flag any conflicts with existing rules.
```

### Prompt 2: When direct file-reading creeps back in (learning from violations)

```markdown
In the current task, you read src/ directly instead of using run_pipeline.
State in one line why that happened, and propose a one-line addition
to CLAUDE.md's NEVER section to prevent it from happening again.
```

> 💡 Think of CLAUDE.md as something that "grows one line per failure." Loop through violation → root cause → new rule, and direct-reading tends to disappear within a few cycles.

### Prompt 3: comP-optimizing your entire workflow

```markdown
My dev workflow has three stages: design → implementation → review.
For each stage, write a draft to add to CLAUDE.md titled "Stage-by-stage comP usage"
describing which comP tool (run_pipeline / get_context / get_impact_graph / session_recall)
should be used and how. Prioritize minimizing token usage above all else.
```

### Prompt 4: Making session handoff a habit

```markdown
Add the following to CLAUDE.md:
"Before ending a session, summarize this session's decisions and open issues
in 3 lines or fewer. Since the next session will search for this via
session_recall, always include proper nouns (function names, file names)
that are likely search keywords."
```

---

## Session Memory: How It Differs from Claude's Built-in Memory

"Claude already has a memory feature—do I still need comP's session memory?" The answer: **they operate at different layers, so you need both.**

| | Claude's Built-in Memory | comP session_recall |
| --- | --- | --- |
| **Granularity** | Conversation summaries, people, preferences | Index of code, symbols, and technical decisions |
| **Storage** | Cloud (Anthropic-side) | **100% local** (`.comp/`) |
| **Good at** | "You prefer QA-focused, diff-only output" | "We set the JWT expiry to 30 minutes last week, because of the refresh-token spec" |
| **Weak at** | Code details (lost during summarization) | The user's personality, non-code context |
| **Scope** | Across all conversations | All sessions within the same project |

### Where It Really Shines (Real Examples)

**Case 1: "Wait, why did we do it this way?" a week later**
```
@comP session_recall
Check why we limited retries to 3 last week
```
Claude's built-in memory keeps a summary, so "we discussed retry limits" might survive, but the technical rationale ("why 3") tends to get lost. comP's BM25 index pulls up that exact conversation.

**Case 2: Handoff to a cheaper model**
When design work happens on a top-tier model and implementation on a cheap one, the cheap model tends to get lost in long context explanations. `session_recall` injects **only the decisions that matter**, drastically cutting handoff cost.

**Case 3: Environments where cloud memory is disabled for confidentiality**
Even when corporate policy disables cloud-side memory, comP is 100% local, so you can **keep session memory while staying compliant.**

### The Rule of Thumb (worth one line in your CLAUDE.md)

```markdown
comP (in-repo) is the source of truth for technical decisions;
Claude's memory is the source of truth for personal/preference context.
```

Recording the same decision in both risks drift when only one gets updated—**fixing the division of responsibility** is the key.

### How It Works

1. **Auto-logging**: Every conversation is BM25-indexed when the chat ends
2. **Persistence**: Saved to `~/.claude/projects/comP/memory/session/`
3. **Retrieval**: Keyword search instantly restores relevant past conversations

---

## Excluding Files & Folders

Create `.comp/ignore` in your project root (same syntax as `.gitignore`):

```gitignore
node_modules/
vendor/
dist/
build/
target/
__pycache__/
*.min.js
```

Auto-excluded: hidden directories starting with `.`, anything matching `.gitignore`, `node_modules`, `venv`, `__pycache__`, `coverage`, `vendor`, `out`

You can also exclude paths from VS Code settings:

```json
{ "comp.exclude": ["env", "data", "logs"] }
```

---

## Controlling Token Budget & Compression Level

Customize via `.comp/config.json`:

```json
{
  "max_nodes": 100000,
  "on_limit_exceeded": "warn",
  "default_budget_tokens": 8000,
  "compression_rules": { "*.md": 0, "*.rs": 2, "*.ts": 1 }
}
```

| Option | Description |
| --- | --- |
| `max_nodes` | Upper limit on indexed node count |
| `on_limit_exceeded` | `"warn"` = notify and continue / `"stop"` = halt |
| `default_budget_tokens` | Token budget for `run_pipeline` (auto-selects compression level) |
| `compression_rules` | Compression level per file extension (0=full / 1=compact / 2=skeleton) |

> **DB size guide**: small repo (~1k files) 1–5 MB, medium (~10k) 20–80 MB, large (100k+) 200 MB–1 GB. Metadata only.

---

## How It Works (Technical Details)

1. **Indexer (Rust daemon)**: Parses 30+ languages with tree-sitter, stores results in SQLite
2. **Search engine**: BM25 full-text search + graph traversal + semantic scoring
3. **MCP server**: Exposes `run_pipeline`, `get_context`, `get_impact_graph`, `session_recall`
4. **VS Code extension**: Manages the daemon, UI, and commands

```text
Code files (30+ languages)
   ↓ [tree-sitter parsing]
SQLite graph DB (.comp/index.db)
   ↓ [BM25 + graph traversal]
MCP server
   ↓
AI agent (Claude Code, Cursor, Cline, etc.)
   ↓ [context compression]
LLM API (fewer tokens = lower cost)
```

**Supported languages (30+)**: C, C++, C#, Go, Java, JavaScript, TypeScript, Python, Rust, Ruby, Bash, Kotlin, Swift, PHP, Dart, Elixir, Haskell, Lua, R, Zig, SQL, HTML, CSS, YAML, Scala, and more.

---

## Security & Privacy

- **🔐 100% local execution**: code and session history are never sent to the cloud
- **🛡️ Auto-excluded**: `.comp/` is automatically added to `.gitignore`
- **📋 Auditable**: no external APIs, no telemetry
- **🏢 Enterprise-ready**: fully isolated on-prem operation, safe for confidential code

---

## Troubleshooting

### "comP isn't indexing"

1. Check progress in the status bar
2. If stuck: `Ctrl+Shift+P` → **"comP: Force Re-index"**
3. Verify `.comp/` exists and is listed in `.gitignore`

### "MCP connection failed"

1. Re-run **"comP: Setup Agents"**
2. Verify the config file was generated
3. Check the Output panel (View → Output → "comP") for logs

### "The AI reads files directly instead of using comP"

→ See the "**A Prompt Kit for Growing CLAUDE.md into a comP-first Constitution**" section above. Prompt 1 resolves this in most cases.

### "Indexing is slow"

- Large repos (>100k files) take time on first run only. Subsequent runs are incremental (fast)
- The comP daemon typically uses <500MB RAM

---

## Agent Compatibility

| Agent | Status |
| --- | --- |
| Claude Code / GitHub Copilot | ✅ Supported & verified |
| Cursor / Cline / Windsurf / Antigravity / Aider / Continue.dev | ✅ Supported |
| Gemini | ❌ Not supported |

Any MCP 2024-11-05-compliant client should work in principle. [Open an issue](https://github.com/tsucky230/comP/issues/new) if you hit a problem.

---

## Roadmap

| Version | Features | Status |
| --- | --- | --- |
| v0.1–v0.8 | Core indexing, MCP, Office/PDF support, compression, large-repo optimization | ✅ Released |
| **v0.9** | **Session history, persistent memory, session_log / session_recall** | ✅ Released |
| v1.0 | API stabilization, community integrations | ⚪ Planned |

---

## License, Contributing & Support

- **MIT License** — [LICENSE](LICENSE)
- Contributions welcome — [CONTRIBUTING.md](CONTRIBUTING.md)
- ☕ **[GitHub Sponsors](https://github.com/sponsors/tsucky230)** / 💖 **Star this repo**

## Questions & Bug Reports

- 📖 [docs/](docs/) / 🐛 [Issue](https://github.com/tsucky230/comP/issues/new) / 💬 [Discussions](https://github.com/tsucky230/comP/discussions/new)
