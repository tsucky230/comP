# Beta Features

Beta features are complete enough to use, but their settings and behavior may still change between
releases. Every beta feature in comP is **off by default** and does nothing until you turn it on.
Feedback and bug reports are welcome on [GitHub Issues](https://github.com/tsucky230/comP/issues/new).

| Feature | Setting | Status |
| --- | --- | --- |
| [Conversation recording](#conversation-recording) | `comp.conversationRecording.enabled` | Beta (Claude Code only) |

Japanese version: [BETA_FEATURES_ja.md](./BETA_FEATURES_ja.md)

---

## Conversation recording

Records each turn you have with Claude Code — your request and its final answer — into
`.comp/history/`, so `session_recall` can bring it back in a later session, after a restart, or in
another agent. Without it, a turn is kept only if the model happens to call `session_log` itself.

The default stays **off**, also after this feature leaves beta: it writes conversation text to disk,
so it should only run for people who chose it.

### Requirements

- A comP release newer than 0.11.6 (the `comp-daemon record-turn` command is not in 0.11.6 or earlier)
- Claude Code. Other agents are not recorded automatically yet — see [Other agents](#other-agents)

Verified on 2026-10-05 with Windows 11 and Claude Code 2.1.172: the hook fired at the end of a real
turn and the turn was recorded; with the setting off, nothing was recorded.

### Turn it on

1. Open VS Code settings (`Ctrl+,`), search for `comp.conversationRecording`, and check
   **Conversation Recording: Enabled**. You can do this in User settings (all projects) or
   Workspace settings (this project only).
2. comP shows a notification. Click **Run Setup Agents** and pick **Claude Code**
   (or run **comP: Setup Agents** from the Command Palette).
3. Restart Claude Code so it loads the new hook.

The "comP Setup" output panel lists the hook file with `[OK  ]`. If it says `[SKIP]`, the reason is
printed on the next line — see [Troubleshooting](#troubleshooting).

### What gets written where

| File | What | Written by |
| --- | --- | --- |
| `.claude/settings.local.json` | A Stop hook: `"<comp-daemon>" record-turn "<workspace>"` | comP: Setup Agents |
| `.comp/config.json` | `conversationRecording: { enabled, agents: { "claude-code" } }` | The extension, on activation and whenever the setting changes |
| `.comp/history/log-YYYY-MM.jsonl` | One line per turn: request (up to 600 characters), answer (up to 400), `"agent": "claude-code"` | `comp-daemon record-turn`, at the end of each turn |

The hook goes into `settings.local.json`, never the shared `settings.json`, because it holds an
absolute path that differs per machine. Existing hooks and settings in that file are kept, and the
previous file is saved as `settings.local.json.bak`. When the extension is upgraded or the project is
moved, comP rewrites the hook's paths on the next activation.

### Check that it works

After a turn in Claude Code, open `.comp/history/log-YYYY-MM.jsonl` (the current month, UTC) and
look at the last line:

```json
{"query":"your request","outcome":"the answer","symbols":[],"files":[],"tokens":0,"stale":false,"timestamp":1791180000000,"agent":"claude-code"}
```

Or ask the agent to call `session_recall` — recent turns appear with `claude-code` as the agent.

### Turn it off

Uncheck **Conversation Recording: Enabled**. Recording stops at once: `record-turn` reads
`.comp/config.json` before it reads anything else, and does nothing while the switch is off. The hook
itself stays in `.claude/settings.local.json`; it keeps being called and exits without writing.

To record nothing from Claude Code while keeping the master switch on for future agents, uncheck
**Conversation Recording: Claude Code** instead.

### Remove the hook

Run **comP: Remove Conversation Recording Hooks** (`comp.removeHistoryHooks`) from the Command Palette
and confirm. comP removes only its own `record-turn` hook from `.claude/settings.local.json`, keeps
every other hook and setting, and saves the previous file as `.bak`. Restart Claude Code afterwards.
Existing records in `.comp/history/` are not deleted.

### Privacy

Records stay inside the workspace. comP does not send them anywhere. They do contain conversation
text, which may include secrets you typed or code the agent printed, so:

- keep `.comp/` out of version control (add it to `.gitignore`);
- delete `.comp/history/log-*.jsonl` when you no longer need old turns.

### Troubleshooting

**Nothing is recorded.** Check these in order:

1. `.comp/config.json` contains `"conversationRecording": {"enabled": true, ...}`. If comP warned
   that the file is not valid JSON, fix it — comP never overwrites a file it cannot read, and an
   unreadable switch counts as off.
2. `.claude/settings.local.json` contains a Stop hook with `record-turn`. If setup reported `[SKIP]`:
   - *conversation recording (beta) is off* — turn the setting on first;
   - *already records history with history-record* — the project has its own recording hook
     (the comP repository does); comP does not add a second one;
   - *daemon binary not found* or *shell metacharacters* — the path cannot be used safely in a hook.
3. Claude Code was restarted after setup.
4. Run the hook by hand to see its reason (it prints why it did not record; use forward slashes in the JSON paths):

   ```bash
   echo '{"transcript_path":"<path to a session .jsonl>","cwd":"<workspace>"}' \
     | "<comp-daemon>" record-turn "<workspace>"
   ```

   `nothing to record (conversation recording is off …)` means the switch; `record-turn failed`
   is followed by the cause.

**`spill-*.jsonl` files appear in `.comp/history/`.** A turn could not be appended (for example the
file was locked). It was saved to a spill file instead and is merged into the monthly log on the next
successful append. No action is needed.

**GitHub Copilot turns are recorded as Claude Code, or Copilot behaves oddly.** VS Code's
`chat.useClaudeHooks` setting makes Copilot run hooks from `.claude/settings.local.json` too. This is
not yet handled; turn that setting off if you use both.

### Known limitations

- Claude Code only. Other agents are asked to call `session_log` themselves (see below).
- `.comp/history/` grows every month. `comp-daemon compact-history <workspace>` removes only exact
  duplicate lines; there is no automatic cleanup yet.
- Only the final text of the request and answer is kept (truncated as above), not tool calls.

### Other agents

Codex, Gemini CLI, Cursor, Windsurf and others are not recorded automatically yet. comP's instruction
files (`AGENTS.md`, `GEMINI.md`, `.cursor/rules`, …) ask them to call `session_log` after each task,
which works only when the model follows the instruction. Most of these agents do have end-of-turn
hooks, and support is planned agent by agent under the same switch, starting with Gemini CLI. Status
and plan: [docs/dev/CONVERSATION_RECORDING_ja.md](../dev/CONVERSATION_RECORDING_ja.md) (Japanese).
