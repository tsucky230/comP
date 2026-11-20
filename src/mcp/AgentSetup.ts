// AgentSetup - Write MCP configuration into the files each AI agent actually reads
//
// Responsibilities:
// 1. Write comP into every config file the selected agent reads, merging with
//    entries that are already there
// 2. Append the comP usage rules to the agent's instruction file
// 3. Report per-file outcomes so a partial failure is visible, and hand back a
//    manual fallback only for the files that could not be written

import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { spawn } from "child_process";
import { DaemonManager } from "../daemon/DaemonManager";
import { bundledDaemonBinaryPath, resolveDaemonBinary } from "../config/daemonBinary";
import {
  ConversationRecordingSettings,
  DEFAULT_CONVERSATION_RECORDING,
} from "../config/conversationRecording";
import {
  isSelfContained,
  findCompSection,
  unsupportedServerShape,
  userLinesInCompSection,
  describeError,
  quoteForShell,
  yamlQuote,
  tomlQuote,
} from "./agentSetup/text";
import {
  registerClaudeCodeUserScope,
  isInstalledExtensionBinary,
  isOlderInstalledExtensionBinary,
  repairClaudeCodeUserScope,
  repairClaudeCodeRegistrations,
  readClaudeConfig,
  userScopeServesProjects,
  writeUserScopeRegistration,
  claudeAddArgs,
  planClaudeCodeRepairs,
  generateClaudeCodeCommand,
} from "./agentSetup/claudeRegistration";
import {
  historyHookFile,
  historyHookCommand,
  ensureHistoryHook,
  historyHookInstalled,
  removeHistoryHook,
  repairHistoryHook,
} from "./agentSetup/historyHook";
import {
  repairTargets,
  repairStaleConfigs,
  repairTarget,
  repairTomlTarget,
  refreshWorkspaceRootToml,
  refreshWorkspaceRootEnv,
} from "./agentSetup/repair";
import { compRuleSnippet } from "./agentSetup/snippets";
import {
  applyMagatamaToContent,
  findUserManaged,
  manageAt,
  resolveSettings,
  updateManaged,
} from "./agentSetup/magatama";

/**
 * How comP has to be encoded into one particular config file.
 *
 * - `json`          — merge a comp server object at the given key chain,
 *                     preserving every other server already configured
 * - `continue-block`— a standalone Continue block file that comP owns entirely,
 *                     so it is rendered whole rather than merged
 * - `aider`         — YAML appended to .aider.conf.yml
 * - `toml`          — a `[mcp_servers.comp]` table spliced into Codex's config.toml
 */
type TargetFormat =
  | { kind: "json"; serverKeys: string[] }
  | { kind: "continue-block" }
  | { kind: "aider" }
  | { kind: "toml" };

/**
 * One config file comP writes, and the rules that apply to it.
 *
 * WHY scope matters: a `workspace` file describes this project only, so it
 * carries COMP_WORKSPACE_ROOT. A `global` file is shared by every project on the
 * machine — writing this workspace's root into it would make every other project
 * index this one. Global entries omit the variable and let the daemon fall back
 * to its working directory, which the MCP client sets per project.
 */
export interface WriteTarget {
  path: string;
  scope: "workspace" | "global";
  format: TargetFormat;
}

/** What happened to one config file during a setup run. */
export interface WriteOutcome {
  path: string;
  scope: "workspace" | "global";
  /**
   * - `written` — the file now contains comP
   * - `skipped` — the target does not apply here (e.g. Cline without VS Code global storage)
   * - `failed`  — the file was left untouched; `reason` says why
   */
  status: "written" | "skipped" | "failed";
  /** Path of the `.bak` copy taken before an existing file was rewritten */
  backupPath?: string;
  reason?: string;
}

/**
 * Content the user has to place by hand.
 *
 * Only produced for targets that failed. On a fully successful run this is
 * absent — the whole point of the feature is that nothing gets copied by hand.
 */
export interface ManualFallback {
  path: string;
  content: string;
}

export interface GenerateConfigResult {
  /** First successfully written path, kept so existing callers keep working */
  configPath: string;
  success: boolean;
  message: string;
  /** Per-file outcome, in the order the targets were attempted */
  writes: WriteOutcome[];
  /** Instruction files the comP usage rules were appended to */
  constitutionFiles: string[];
  /** What the user must do for the agent to pick the config up */
  restartHint: string;
  /** `claude mcp add` line for user-scope registration (Claude Code only) */
  command?: string;
  /** Present only when at least one target failed */
  manualFallback?: ManualFallback[];
  /**
   * Claude Code only: the Stop hook (`comp-daemon record-turn`) written into
   * `<workspace>/.claude/settings.local.json`. Kept out of `writes` so a hook
   * that could not be installed never turns a working MCP registration into a
   * reported failure.
   */
  historyHook?: WriteOutcome;
  /** JSON と Codex TOML の対象ごとの MAGATAMA の登録の結果（tasks/magatama_sync_plan.md） */
  magatama?: MagatamaOutcome[];
}

/** Runs an external command. Injectable so tests never spawn a real process. */
export type CommandRunner = (
  file: string,
  args: string[],
  options?: { cwd?: string }
) => Promise<{ ok: boolean; stdout: string; stderr: string }>;

/** A comP registration in ~/.claude.json that should move to the bundled binary. */
export interface ClaudeRegistrationFix {
  /** `local` is a per-project entry under `projects[project]` */
  scope: "user" | "local";
  project?: string;
  /** The registration object inside the parsed document */
  server: Record<string, unknown>;
  from: string;
}

export interface UserScopeResult {
  registered: boolean;
  /** The exact command, shown to the user when we could not run it ourselves */
  command: string;
  reason?: string;
}

/**
 * Injection points that must not read the real machine during tests.
 *
 * `homeDir` in particular: several agents keep their only config under the home
 * directory, so without an override a test run would rewrite the developer's own
 * Cursor and Windsurf settings.
 */
export interface AgentSetupOptions {
  /** VS Code's per-extension global storage dir; its parent locates Cline's settings */
  globalStorageDir?: string;
  /** Overrides os.homedir() */
  homeDir?: string;
  /**
   * Overrides Codex's config directory.
   *
   * Needed on top of homeDir because Codex also honours the CODEX_HOME
   * environment variable: without an override, a developer who has it set would
   * have their real Codex configuration rewritten by the test suite.
   */
  codexHome?: string;
  /**
   * `comp.useWorkspaceDaemonBuild`: write this workspace's cargo build into
   * agent configs instead of the bundled binary. Off unless the caller passes
   * it — see resolveDaemonBinary in config/daemonBinary.ts.
   */
  useWorkspaceDaemonBuild?: boolean;
  /**
   * Language for generated prose (restart instructions, failure reasons, the
   * constitution snippet). Defaults to "en".
   *
   * WHY a plain option instead of reading vscode.env.language here: this class
   * has no dependency on the `vscode` module, which is what lets its unit tests
   * run directly under mocha instead of the VS Code extension host. The caller
   * — which already imports vscode — resolves the display language and passes
   * the result in, the same way it resolves globalStorageDir.
   */
  locale?: "en" | "ja";
  /**
   * The `comp.conversationRecording.*` settings. Omitted means off: recording
   * is beta and installs nothing unless the user switched it on.
   */
  conversationRecording?: ConversationRecordingSettings;
  /**
   * MAGATAMA の起動登録の管理（tasks/magatama_sync_plan.md）。省略時は管理しない（manage=false, uv なし）。
   * pin を省略したら MAGATAMA_PIN。テストと、uv の有無を調べた呼び出し側が渡す。
   */
  magatama?: MagatamaSettings;
}

/** MAGATAMA の登録の管理の設定。pin 省略時は MAGATAMA_PIN。 */
export interface MagatamaSettings {
  manage: boolean;
  uvAvailable: boolean;
  pin?: string;
}

/** MAGATAMA の登録について、1つの設定ファイルで何をしたか。 */
export interface MagatamaOutcome {
  path: string;
  scope: "workspace" | "global";
  outcome: "added" | "updated" | "kept" | "failed";
  /** kept の理由（MagatamaKeepReason）か、failed の理由 */
  reason?: string;
}

/** ユーザーが自分で書いた MAGATAMA の登録（版の確認と、管理を任せるボタンの対象）。 */
export interface UserMagatamaEntry {
  path: string;
  scope: "workspace" | "global";
  entry: unknown;
}

/**
 * Outcome of inspecting one MCP config file.
 *
 * - `missing`  — file does not exist
 * - `failed`   — file exists but could not be parsed or written
 * - `skipped`  — no comp entry, or the recorded command must not be touched
 *                (variable reference, relative path, unresolvable replacement)
 * - `healthy`  — recorded command already points at a suitable file
 * - `repaired` — command and/or COMP_WORKSPACE_ROOT was rewritten
 */
export type RepairStatus = "repaired" | "healthy" | "skipped" | "missing" | "failed";

export interface RepairEntry {
  file: string;
  status: RepairStatus;
  /** Previous command value, present only when the command itself was rewritten */
  from?: string;
  /** Replacement command value, present only when the command itself was rewritten */
  to?: string;
  /** True when env.COMP_WORKSPACE_ROOT was rewritten */
  envRepaired?: boolean;
  /** Why the file was skipped or failed */
  reason?: string;
}

/**
 * One MCP config file that may carry a stale daemon path.
 *
 * `scope` decides two things: which binary a repair is allowed to write, and
 * whether COMP_WORKSPACE_ROOT may be touched. A global file is shared by every
 * project, so it must never receive this workspace's dev build or its root path.
 */
export interface McpConfigTarget {
  path: string;
  /** Key chain from the document root down to the comp server object; unused for TOML */
  serverKeys: string[];
  scope: "workspace" | "global";
  /** How the file is encoded. Decides which repair routine may touch it. */
  format: "json" | "toml";
}

/**
 * Writes comP into the config files AI coding agents actually read.
 *
 * Each agent gets its real config location, merged rather than overwritten, so
 * a completed setup needs nothing pasted anywhere — only a restart of the agent,
 * because none of these tools re-read their config while running.
 *
 * | Agent          | Workspace                        | Global                                      |
 * | -------------- | -------------------------------- | ------------------------------------------- |
 * | Claude Code    | `.mcp.json`                      | `claude mcp add --scope user` (CLI)         |
 * | Cursor         | `.cursor/mcp.json`               | `~/.cursor/mcp.json`                        |
 * | Cline          | —                                | VS Code globalStorage/saoudrizwan.claude-dev |
 * | Windsurf       | —                                | `~/.codeium/windsurf/mcp_config.json`       |
 * | Continue       | `.continue/mcpServers/comp.yaml` | `~/.continue/mcpServers/comp.yaml`          |
 * | Antigravity    | —                                | `~/.gemini/config/mcp_config.json`          |
 * | GitHub Copilot | `.vscode/mcp.json`               | —                                           |
 * | Aider          | `.aider.conf.yml`                | —                                           |
 * | Codex          | `.codex/config.toml`             | `~/.codex/config.toml` (or `$CODEX_HOME`)   |
 *
 * All servers are registered as stdio: the MCP client spawns the daemon binary.
 */
export class AgentSetupManager {
  /** @internal agentSetup/ のモジュールから使う */
  workspaceRoot: string;
  /** @internal agentSetup/ のモジュールから使う */
  extensionPath: string | undefined;
  private globalStorageDir: string | undefined;
  /** @internal agentSetup/ のモジュールから使う */
  homeDir: string;
  private codexHomeOverride: string | undefined;
  private useWorkspaceDaemonBuild: boolean;
  private locale: "en" | "ja";
  private conversationRecording: ConversationRecordingSettings;
  private magatamaSettings: Required<MagatamaSettings>;

  constructor(
    _daemonManager: DaemonManager,
    workspaceRoot: string,
    extensionPath?: string,
    options?: AgentSetupOptions
  ) {
    // daemonManager reserved for future use (e.g., querying indexing status during config generation)
    this.workspaceRoot = workspaceRoot;
    this.extensionPath = extensionPath;
    this.globalStorageDir = options?.globalStorageDir;
    this.homeDir = options?.homeDir ?? os.homedir();
    this.codexHomeOverride = options?.codexHome;
    this.useWorkspaceDaemonBuild = options?.useWorkspaceDaemonBuild ?? false;
    this.locale = options?.locale ?? "en";
    this.conversationRecording = options?.conversationRecording ?? DEFAULT_CONVERSATION_RECORDING;
    this.magatamaSettings = resolveSettings(options?.magatama);
  }

  /** Pick the string matching the configured locale; English is the default. */
  /** @internal agentSetup/ のモジュールから使う */
  t(en: string, ja: string): string {
    return this.locale === "ja" ? ja : en;
  }

  /**
   * Codex's configuration directory.
   *
   * CODEX_HOME is best-effort: a value exported from the user's shell profile is
   * often absent from the environment a desktop-launched VS Code inherits, so a
   * user who moved their Codex home may still have to point comP at it by hand.
   * Reading it when it is there is still better than ignoring it.
   */
  /** @internal agentSetup/ のモジュールから使う */
  codexHome(): string {
    const fromEnv = process.env.CODEX_HOME;
    if (this.codexHomeOverride) {
      return this.codexHomeOverride;
    }
    if (fromEnv && fromEnv.length > 0 && path.isAbsolute(fromEnv)) {
      return fromEnv;
    }
    return path.join(this.homeDir, ".codex");
  }

  private readCompConfig(): { autoGenerateConstitution?: boolean } {
    try {
      const configPath = path.join(this.workspaceRoot, ".comp", "config.json");
      if (fs.existsSync(configPath)) {
        const content = fs.readFileSync(configPath, "utf-8");
        return JSON.parse(content);
      }
    } catch (error) {
      // Silently ignore read/parse errors
      console.warn(`Warning: failed to read .comp/config.json: ${error}`);
    }
    return {};
  }

  private sessionContinuitySnippet(): string {
    // The marker ensureInstructions() dedups on ("Session Continuity") stays
    // literal English in both variants — see the header line below — so a
    // repeat run recognizes either language's copy and does not append a
    // second one.
    if (this.locale === "ja") {
      return [
        "## Session Continuity (デーモン再起動・セッション切れ対応)",
        "",
        "セッションはデーモン再起動をまたいで永続化されます。",
        "作業を再開したり、エージェントを再起動した場合は、まず `session_recall` を呼んで過去の文脈を復帰させてください。",
        "",
        "**作業を再開する場合**:",
        "",
        "1. `session_recall()` を呼んで過去のやり取りを確認する",
        "   - `session_recall({ \"query\": \"keyword\" })` — タスクのキーワードで絞り込み",
        "   - `session_recall({ \"limit\": 5 })` — 直近N件を表示",
        "2. 過去に何をしたかを確認し、その文脈のまま作業を続ける",
        "",
        "**補足**: 会話の記録（β版）をONにした場合、comP の初期設定が `.claude/settings.local.json` に Stop フック（`comp-daemon record-turn`）を入れ、",
        "Claude Code の各往復を `.comp/history/` に記録します。`session_recall` はこの記録を読みます。",
      ].join("\n");
    }
    return [
      "## Session Continuity (daemon restarts / session resets)",
      "",
      "Sessions persist across daemon restarts.",
      "When resuming work or restarting the agent, call `session_recall` first to restore prior context.",
      "",
      "**When resuming work**:",
      "",
      "1. Call `session_recall()` to view past interactions",
      "   - `session_recall({ \"query\": \"keyword\" })` — filter by task keywords",
      "   - `session_recall({ \"limit\": 5 })` — show last N interactions",
      "2. Review what was done previously and continue in that context",
      "",
      "**Note**: If conversation recording (beta) is enabled, comP setup installs a Stop hook (`comp-daemon record-turn`) in",
      "`.claude/settings.local.json` that records each Claude Code turn to `.comp/history/`; `session_recall` reads those records.",
    ].join("\n");
  }

  /**
   * Ask agents to record each task with session_log.
   *
   * WHY: only Claude Code has a Stop hook comP can attach to (record-turn).
   * Every other agent leaves no turn record unless the model calls session_log
   * itself, and nothing told it to. Claude Code is told not to, so that its
   * turns are not recorded twice. Separate marker from the other two sections
   * so files written by earlier versions still receive it on the next setup.
   */
  private sessionLoggingSnippet(): string {
    if (this.locale === "ja") {
      return [
        "## comP Session Logging (作業の記録)",
        "",
        "タスクを1つ終えたら、`session_log` を呼んで記録してください。",
        "`request` にユーザーの依頼、`outcome` に行ったことと結果を短く書きます（関係したファイルがあれば `files` にも）。",
        "記録は `.comp/history/` に残り、次のセッションで `session_recall` から読めます。",
        "",
        "Claude Code で、`.claude/settings.local.json` に `record-turn` の Stop フックがある場合は、各往復が自動で記録されるので `session_log` を呼ばないでください（二重に記録されます）。",
      ].join("\n");
    }
    return [
      "## comP Session Logging",
      "",
      "After finishing each task, call `session_log` to record it:",
      "`request` = what the user asked for, `outcome` = what you did and the result (add `files` when relevant).",
      "Records go to `.comp/history/` and are available to `session_recall` in later sessions.",
      "",
      "Claude Code: if `.claude/settings.local.json` has a `record-turn` Stop hook, do not call session_log — the hook already records every turn, so calling it would record the turn twice.",
    ].join("\n");
  }

  /**
   * Append a snippet to an instruction file unless it is already there.
   *
   * `marker` is the text that proves the snippet is present; it is checked
   * instead of the whole snippet so that a user who reworded the section keeps
   * their edit rather than getting a second copy appended on every setup run.
   *
   * Best-effort by design: a missing instruction file degrades how well the
   * agent uses comP, but it must not fail the MCP registration that already
   * succeeded. Returns true when the file was created or extended.
   */
  private ensureSnippet(filePath: string, marker: string, snippet: string): boolean {
    try {
      // `.cursor/rules` is a file in older Cursor versions and a directory of
      // .mdc rules in newer ones. Writing a file over the directory would
      // destroy every rule in it, so drop a rule file inside instead.
      if (fs.existsSync(filePath) && fs.statSync(filePath).isDirectory()) {
        filePath = path.join(filePath, "comp.md");
      }

      const dir = path.dirname(filePath);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }

      if (fs.existsSync(filePath)) {
        const content = fs.readFileSync(filePath, "utf-8");
        if (content.includes(marker)) {
          return false;
        }
        this.atomicWrite(filePath, content.trimEnd() + "\n\n---\n\n" + snippet + "\n");
        return true;
      }

      this.atomicWrite(filePath, snippet + "\n");
      return true;
    } catch (error) {
      console.warn(`Warning: failed to write ${filePath}: ${error}`);
      return false;
    }
  }

  /**
   * Write both comP snippets into one agent instruction file.
   *
   * The usage rules and the session-continuity notes are tracked by separate
   * markers so a file that predates one of them still receives the other.
   */
  private ensureInstructions(filePath: string): boolean {
    const rules = this.ensureSnippet(filePath, "comP MCP Tool Usage", compRuleSnippet());
    const continuity = this.ensureSnippet(
      filePath,
      "Session Continuity",
      this.sessionContinuitySnippet()
    );
    const logging = this.ensureSnippet(filePath, "comP Session Logging", this.sessionLoggingSnippet());
    return rules || continuity || logging;
  }

  /** Agent names the setup flow offers, in display order. */
  static readonly AGENTS = [
    "Claude Code",
    "Codex",
    "Cursor",
    "Cline",
    "Windsurf",
    "Continue",
    "Antigravity",
    "GitHub Copilot",
    "Aider",
    "Gemini CLI",
  ];

  /**
   * The COMP_AGENT_ID value written into every config generated for this agent.
   *
   * WHY: comP's session memory and history are shared across every MCP client
   * pointed at the same workspace (see docs/ARCHITECTURE_ja.md 4.1). Without
   * this, the daemon falls back to "unknown" for every client, and entries from
   * Cursor, Gemini CLI, Copilot etc. become indistinguishable from each other.
   * Lowercase-hyphenated to match the id comP's own Stop hook already uses for
   * Claude Code ("claude-code" — see .claude/hooks/history-record.sh).
   */
  /** @internal agentSetup/ のモジュールから使う */
  static agentIdFor(agentName: string): string {
    return agentName.toLowerCase().replace(/[^a-z0-9]+/g, "-");
  }

  /**
   * Cline keeps its MCP list inside VS Code's global storage, so the path
   * depends on which VS Code flavour is running.
   *
   * WHY derive it from our own globalStorage directory instead of building it
   * from the OS: the sibling-directory form covers Insiders, VSCodium and every
   * platform's application-data location without branching on any of them.
   * Returns null when the extension host did not give us one — a path guessed
   * from os.homedir() would be wrong more often than right.
   */
  /** @internal agentSetup/ のモジュールから使う */
  clineSettingsPath(): string | null {
    if (!this.globalStorageDir) {
      return null;
    }
    return path.join(
      path.dirname(this.globalStorageDir),
      "saoudrizwan.claude-dev",
      "settings",
      "cline_mcp_settings.json"
    );
  }

  /**
   * Every config file comP writes for one agent, in the order they are attempted.
   *
   * Returns null for an agent we do not support, and an empty array for one we
   * support but whose config location could not be resolved on this machine —
   * the two need different messages, so they must not collapse into one value.
   *
   * `serverKeys` here is the chain down to the *container* of server entries;
   * `comp` is added inside it. That differs from repairTargets(), which points
   * at the comp entry itself, because the two walk the document for different
   * reasons.
   */
  /** @internal agentSetup/ のモジュールから使う */
  targetsForAgent(agentName: string): WriteTarget[] {
    return this.targetsFor(agentName) ?? [];
  }

  private targetsFor(agentName: string): WriteTarget[] | null {
    const ws = (...segments: string[]) => path.join(this.workspaceRoot, ...segments);
    const home = (...segments: string[]) => path.join(this.homeDir, ...segments);
    const json = (serverKeys: string[]): TargetFormat => ({ kind: "json", serverKeys });

    switch (agentName) {
      case "Claude Code":
        // Only the project file is written here; user scope goes through the
        // official CLI, which owns ~/.claude.json and its format.
        return [{ path: ws(".mcp.json"), scope: "workspace", format: json(["mcpServers"]) }];

      case "Codex":
        // One TOML file serves both the CLI and the VS Code extension.
        //
        // WHY global first: Codex reads the global file unconditionally, but
        // loads a project-level .codex/ only for a project the user has marked
        // trusted. The target that always takes effect therefore leads, so the
        // reported configPath points at the one the user can rely on.
        return [
          {
            path: path.join(this.codexHome(), "config.toml"),
            scope: "global",
            format: { kind: "toml" },
          },
          { path: ws(".codex", "config.toml"), scope: "workspace", format: { kind: "toml" } },
        ];

      case "Cursor":
        return [
          { path: ws(".cursor", "mcp.json"), scope: "workspace", format: json(["mcpServers"]) },
          { path: home(".cursor", "mcp.json"), scope: "global", format: json(["mcpServers"]) },
        ];

      case "Cline": {
        const settings = this.clineSettingsPath();
        return settings ? [{ path: settings, scope: "global", format: json(["mcpServers"]) }] : [];
      }

      case "Windsurf":
        // Windsurf reads one global file; it has no project-level equivalent.
        return [
          {
            path: home(".codeium", "windsurf", "mcp_config.json"),
            scope: "global",
            format: json(["mcpServers"]),
          },
        ];

      case "Continue":
        // Continue scans both directories for standalone block files, so comP
        // owns one file in each and never has to merge into the user's YAML.
        return [
          {
            path: ws(".continue", "mcpServers", "comp.yaml"),
            scope: "workspace",
            format: { kind: "continue-block" },
          },
          {
            path: home(".continue", "mcpServers", "comp.yaml"),
            scope: "global",
            format: { kind: "continue-block" },
          },
        ];

      case "Antigravity":
        // WHY not also .agents/mcp_config.json: Antigravity reads both, but does
        // not document which wins when each declares `comp`, so one global
        // entry is the only shape whose behaviour is known.
        return [
          { path: this.antigravityConfigPath(), scope: "global", format: json(["mcpServers"]) },
        ];

      case "GitHub Copilot":
        // VS Code's own MCP file uses `servers`, not `mcpServers`.
        return [{ path: ws(".vscode", "mcp.json"), scope: "workspace", format: json(["servers"]) }];

      case "Aider":
        return [{ path: ws(".aider.conf.yml"), scope: "workspace", format: { kind: "aider" } }];

      case "Gemini CLI":
        return [
          {
            path: ws(".gemini", "settings.json"),
            scope: "workspace",
            format: json(["mcpServers"]),
          },
          { path: home(".gemini", "settings.json"), scope: "global", format: json(["mcpServers"]) },
        ];

      default:
        return null;
    }
  }

  /**
   * Instruction files that should carry the comP usage rules for this agent.
   *
   * CLAUDE.md and .claude/CLAUDE.md are written for every agent, not just
   * Claude Code: a repository is rarely worked on by one tool only, and the
   * rules are inert for an agent that does not read them.
   */
  private instructionFilesFor(agentName: string): string[] {
    const ws = (...segments: string[]) => path.join(this.workspaceRoot, ...segments);
    const shared = [ws("CLAUDE.md"), ws(".claude", "CLAUDE.md")];

    switch (agentName) {
      case "Codex":
      case "Antigravity":
        return [...shared, ws("AGENTS.md")];
      case "Cursor":
        return [...shared, ws(".cursor", "rules")];
      case "Cline":
        return [...shared, ws(".clinerules")];
      case "Windsurf":
        return [...shared, ws(".windsurfrules")];
      case "GitHub Copilot":
        return [...shared, ws(".github", "copilot-instructions.md")];
      case "Aider":
        return [...shared, ws("CONVENTIONS.md")];
      case "Gemini CLI":
        return [...shared, ws("GEMINI.md")];
      default:
        return shared;
    }
  }

  /** What the user has to do before the agent sees the new config. */
  private restartHint(agentName: string): string {
    switch (agentName) {
      case "Claude Code":
        return this.t(
          [
            "Restart Claude Code.",
            "- CLI: quit the running session, then start `claude` again from the project directory",
            "- VS Code / JetBrains extension: Command Palette → \"Developer: Reload Window\"",
            "On first start it asks whether to trust this project's MCP servers — approve it.",
            "`claude mcp list` should show comp as Connected.",
          ].join("\n"),
          [
            "Claude Code を再起動してください。",
            "- CLI: 実行中のセッションを終了し、プロジェクトディレクトリで `claude` を起動し直す",
            "- VS Code / JetBrains 拡張: コマンドパレット →「Developer: Reload Window」",
            "初回起動時に「このプロジェクトの MCP サーバーを使うか」と確認されるので承認してください。",
            "`claude mcp list` で comp が Connected になっていれば成功です。",
          ].join("\n")
        );
      case "Codex":
        return this.t(
          [
            "Quit the running Codex and start it again.",
            "- CLI: leave the session and start `codex` again",
            "- VS Code extension: Command Palette → \"Developer: Reload Window\"",
            "Run `/mcp` inside a session — comp should appear.",
            "",
            "The project-level `.codex/config.toml` is only read for a project you have trusted.",
            "Until then, only the global config (~/.codex/config.toml) is in effect.",
          ].join("\n"),
          [
            "実行中の Codex を終了し、起動し直してください。",
            "- CLI: セッションを抜けて `codex` を起動し直す",
            "- VS Code 拡張: コマンドパレット →「Developer: Reload Window」",
            "セッション内で `/mcp` を実行し comp が表示されれば成功です。",
            "",
            "プロジェクト側の `.codex/config.toml` は「信頼済み」にしたプロジェクトでしか読み込まれません。",
            "未信頼のままだとグローバル設定（~/.codex/config.toml）だけが有効になります。",
          ].join("\n")
        );
      case "Cursor":
        return this.t(
          [
            "Reload Cursor.",
            "- Command Palette (Ctrl/Cmd+Shift+P) → \"Developer: Reload Window\"",
            "If it still doesn't pick it up, quit Cursor completely and start it again.",
            "Settings → MCP should show comp as enabled.",
          ].join("\n"),
          [
            "Cursor をリロードしてください。",
            "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
            "反映されない場合は Cursor を完全に終了して起動し直してください。",
            "Settings → MCP に comp が有効として表示されれば成功です。",
          ].join("\n")
        );
      case "Cline":
        return this.t(
          [
            "Reload VS Code.",
            "- Command Palette (Ctrl/Cmd+Shift+P) → \"Developer: Reload Window\"",
            "You can check whether comp is enabled from the MCP icon at the top of Cline's chat view.",
          ].join("\n"),
          [
            "VS Code をリロードしてください。",
            "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
            "Cline のチャット画面上部の MCP アイコンから comp が有効か確認できます。",
          ].join("\n")
        );
      case "Windsurf":
        return this.t(
          [
            "Reload Windsurf.",
            "- Command Palette (Ctrl/Cmd+Shift+P) → \"Developer: Reload Window\"",
            "- or the MCP icon at the top right of the Cascade panel → \"Refresh\"",
          ].join("\n"),
          [
            "Windsurf をリロードしてください。",
            "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
            "- または Cascade パネル右上の MCP アイコン →「Refresh」",
          ].join("\n")
        );
      case "Continue":
        return this.t(
          [
            "Reload VS Code.",
            "- Command Palette (Ctrl/Cmd+Shift+P) → \"Developer: Reload Window\"",
            "Continue does not pick up config changes automatically, and MCP tools only work in Agent mode.",
          ].join("\n"),
          [
            "VS Code をリロードしてください。",
            "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
            "Continue は設定変更を自動では読み直しません。また MCP ツールは Agent モードでのみ使えます。",
          ].join("\n")
        );
      case "Antigravity":
        return this.t(
          [
            "comp was written to ~/.gemini/config/mcp_config.json, which every Antigravity flavour reads.",
            "- App / IDE: quit Antigravity completely and start it again",
            "- CLI: leave the session and start `agy` again",
            "- VS Code extension: quit VS Code completely and start it again",
            "Type `/mcp` in an `agy` session to open the MCP manager — comp should be listed as connected.",
          ].join("\n"),
          [
            "comp を ~/.gemini/config/mcp_config.json に書きました。Antigravity のどの形態もこのファイルを読みます。",
            "- App / IDE: Antigravity を完全に終了して起動し直す",
            "- CLI: セッションを終了し、`agy` を起動し直す",
            "- VS Code 拡張: VS Code を完全に終了して起動し直す",
            "`agy` のセッションで `/mcp` と入力すると MCP の管理画面が開きます。comp が接続済みになっていれば成功です。",
          ].join("\n")
        );
      case "GitHub Copilot":
        return this.t(
          [
            "Reload VS Code.",
            "- Command Palette (Ctrl/Cmd+Shift+P) → \"Developer: Reload Window\"",
            "Switch Copilot Chat to Agent mode — comp should appear in the tool list.",
          ].join("\n"),
          [
            "VS Code をリロードしてください。",
            "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
            "Copilot Chat を Agent モードに切り替え、ツール一覧に comp が出れば成功です。",
          ].join("\n")
        );
      case "Aider":
        return this.t(
          [
            "Quit the running Aider and start it again.",
            "Aider's MCP support varies by version — if it's not picked up, check `aider --version`.",
          ].join("\n"),
          [
            "実行中の Aider を終了し、起動し直してください。",
            "Aider の MCP 対応はバージョンによって差があります。認識されない場合は `aider --version` を確認してください。",
          ].join("\n")
        );
      case "Gemini CLI":
        return this.t(
          [
            "Restart the gemini CLI session.",
            "Run `/mcp list` inside a session, or `gemini mcp list` from the shell — comp should show as Connected.",
            "stdio servers only connect in a trusted folder; run `gemini trust` first if comp shows as Disconnected.",
            "Since 2026-06-18 Gemini CLI no longer serves Google AI Pro / Ultra or free personal accounts. It keeps working with a paid Gemini API key or a Gemini Code Assist Standard / Enterprise license; otherwise set up Antigravity instead.",
          ].join("\n"),
          [
            "gemini CLI のセッションを再起動してください。",
            "セッション内で `/mcp list`、またはシェルから `gemini mcp list` を実行し、comp が Connected になっていれば成功です。",
            "stdio サーバーは信頼済みフォルダでしか接続されません。Disconnected の場合は先に `gemini trust` を実行してください。",
            "2026-06-18 以降、Gemini CLI は Google AI Pro / Ultra と無料の個人アカウントでは使えません。有料の Gemini API キーか Gemini Code Assist Standard / Enterprise のライセンスがあれば引き続き使えます。どちらもない場合は、代わりに Antigravity を設定してください。",
          ].join("\n")
        );
      default:
        return this.t("Restart the agent.", "エージェントを再起動してください。");
    }
  }

  /**
   * The comp server object written into every JSON-shaped MCP config.
   *
   * COMP_AGENT_ID is set regardless of scope — unlike COMP_WORKSPACE_ROOT it
   * identifies which client is calling, not which project, so a global config
   * shared across projects should still carry it (see agentIdFor's doc).
   */
  private serverEntry(
    daemonPath: string,
    scope: "workspace" | "global",
    agentId: string
  ): Record<string, unknown> {
    const env: Record<string, string> = { RUST_LOG: "info", COMP_AGENT_ID: agentId };
    if (scope === "workspace") {
      env.COMP_WORKSPACE_ROOT = this.workspaceRoot;
    }
    return { command: daemonPath, args: [], env };
  }

  private static yamlQuote(value: string): string {
    return yamlQuote(value);
  }

  /**
   * Render a standalone Continue block file.
   *
   * comP owns this file outright — Continue discovers every file under
   * `.continue/mcpServers/` — so it is written whole. Nothing the user wrote is
   * at risk, which is why this is the one format with no merge step.
   */
  private renderContinueBlock(daemonPath: string, scope: "workspace" | "global", agentId: string): string {
    const q = AgentSetupManager.yamlQuote;
    const lines = [
      "# comP MCP server — generated by the comP VS Code extension",
      "name: comP",
      "version: 0.0.1",
      "schema: v1",
      "mcpServers:",
      "  - name: comp",
      "    type: stdio",
      `    command: ${q(daemonPath)}`,
      "    env:",
      `      RUST_LOG: ${q("info")}`,
      `      COMP_AGENT_ID: ${q(agentId)}`,
    ];
    if (scope === "workspace") {
      lines.push(`      COMP_WORKSPACE_ROOT: ${q(this.workspaceRoot)}`);
    }
    return lines.join("\n") + "\n";
  }

  /**
   * The comp block for .aider.conf.yml.
   *
   * The version note stays in the generated file on purpose: Aider's MCP
   * support differs between releases, so a user whose Aider ignores the block
   * needs to find out why from the file itself.
   */
  private aiderBlock(daemonPath: string, scope: "workspace" | "global", agentId: string): string {
    const q = AgentSetupManager.yamlQuote;
    const lines = [
      "# comP MCP server configuration",
      "# Generated by the comP VS Code extension.",
      "# NOTE: Aider's MCP support varies by release. If this block is ignored,",
      "# check `aider --version` against https://aider.chat/docs/config/aider_conf.html",
      "mcp-servers:",
      "  comp:",
      `    command: ${q(daemonPath)}`,
      "    args: []",
      "    env:",
      `      RUST_LOG: ${q("info")}`,
      `      COMP_AGENT_ID: ${q(agentId)}`,
    ];
    if (scope === "workspace") {
      lines.push(`      COMP_WORKSPACE_ROOT: ${q(this.workspaceRoot)}`);
    }
    return lines.join("\n") + "\n";
  }

  /**
   * The `[mcp_servers.comp]` table written into Codex's config.toml.
   *
   * WHY `env` is an inline table rather than a `[mcp_servers.comp.env]`
   * sub-table: it keeps the whole entry one contiguous block, so the end of
   * comP's section is unambiguously "the next line starting a table". A
   * sub-table would split the entry in two and make replacing it a guess.
   */
  /** @internal agentSetup/ のモジュールから使う */
  static readonly CODEX_MARKER = "# comP MCP server — generated by the comP VS Code extension";

  private codexBlock(daemonPath: string, scope: "workspace" | "global", agentId: string): string {
    const q = AgentSetupManager.tomlQuote;
    const env = [`RUST_LOG = ${q("info")}`, `COMP_AGENT_ID = ${q(agentId)}`];
    if (scope === "workspace") {
      env.push(`COMP_WORKSPACE_ROOT = ${q(this.workspaceRoot)}`);
    }
    return (
      [
        AgentSetupManager.CODEX_MARKER,
        "[mcp_servers.comp]",
        `command = ${q(daemonPath)}`,
        "args = []",
        `env = { ${env.join(", ")} }`,
      ].join("\n") + "\n"
    );
  }

  /** @internal agentSetup/ のモジュールから使う */
  static tomlQuote(value: string): string {
    return tomlQuote(value);
  }

  /**
   * Splice the comp table into Codex's config.toml, preserving everything else.
   *
   * Unlike the Aider block, an existing comP section is always rewritten rather
   * than left alone: it carries the daemon path, which is exactly the value that
   * goes stale when the extension is upgraded.
   *
   * Throws when the file encodes its servers in a shape that cannot be edited
   * without a TOML parser. Reporting that is far better than guessing at a merge
   * and destroying the user's other servers.
   */
  private renderCodexConfig(daemonPath: string, target: WriteTarget, agentId: string): string {
    const block = this.codexBlock(daemonPath, target.scope, agentId);

    if (!fs.existsSync(target.path)) {
      return block;
    }
    const raw = fs.readFileSync(target.path, "utf-8");
    if (raw.trim().length === 0) {
      return block;
    }

    const unsupported = unsupportedServerShape(raw);
    if (unsupported) {
      throw new Error(unsupported);
    }

    // The generated block carries its own marker, so every earlier one goes —
    // not just one directly above the header. Codex rewrites config.toml itself
    // (trust, approvals) and can strand the marker away from the table, after
    // which each setup used to leave one more behind.
    const existing = raw
      .split("\n")
      .filter((line) => line.trim() !== AgentSetupManager.CODEX_MARKER)
      .join("\n");

    const section = findCompSection(existing);
    if (section) {
      const before = existing.slice(0, section.start);
      const after = existing.slice(section.end);
      const sectionText = existing.slice(section.start, section.end);
      if (!isSelfContained(sectionText)) {
        throw new Error("the existing comp table spans a multi-line value; merge comP by hand");
      }
      const carried = userLinesInCompSection(sectionText);
      return before + block + carried + (after.startsWith("\n") ? after : after ? "\n" + after : "");
    }

    return existing.trimEnd() + "\n\n" + block;
  }

  /**
   * Merge the comp entry into an existing JSON config.
   *
   * Throws when the file exists but does not parse: overwriting it would
   * silently discard every other MCP server the user configured, which is far
   * worse than reporting that the file needs a look.
   */
  private mergeJsonConfig(
    filePath: string,
    containerKeys: string[],
    entry: Record<string, unknown>
  ): string {
    let doc: Record<string, unknown> = {};

    if (fs.existsSync(filePath)) {
      const raw = fs.readFileSync(filePath, "utf-8").trim();
      if (raw.length > 0) {
        const parsed = JSON.parse(raw);
        if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
          throw new Error("config root is not a JSON object");
        }
        doc = parsed as Record<string, unknown>;
      }
    }

    let node = doc;
    for (const key of containerKeys) {
      const next = node[key];
      if (next === null || typeof next !== "object" || Array.isArray(next)) {
        node[key] = {};
      }
      node = node[key] as Record<string, unknown>;
    }
    node["comp"] = entry;

    return JSON.stringify(doc, null, 2) + "\n";
  }

  /** Append the comp block to .aider.conf.yml, preserving what is there. */
  private renderAiderConfig(daemonPath: string, target: WriteTarget, agentId: string): string {
    const block = this.aiderBlock(daemonPath, target.scope, agentId);

    if (!fs.existsSync(target.path)) {
      return block;
    }
    const existing = fs.readFileSync(target.path, "utf-8");
    if (existing.includes("comP MCP server configuration")) {
      // Already ours — rewriting the whole file would drop the user's own keys.
      return existing;
    }
    if (existing.includes("mcp-servers:")) {
      // YAML has no safe merge without a parser, and pulling in a runtime
      // dependency for this single case is not worth it. Report it instead.
      throw new Error("mcp-servers block already exists; merge it by hand");
    }
    return existing.trimEnd() + "\n\n" + block;
  }

  /** Build the file content for one target, merging with what is already there. */
  private renderTarget(target: WriteTarget, daemonPath: string, agentId: string): string {
    switch (target.format.kind) {
      case "json":
        return this.mergeJsonConfig(
          target.path,
          target.format.serverKeys,
          this.serverEntry(daemonPath, target.scope, agentId)
        );
      case "continue-block":
        return this.renderContinueBlock(daemonPath, target.scope, agentId);
      case "aider":
        return this.renderAiderConfig(daemonPath, target, agentId);
      case "toml":
        return this.renderCodexConfig(daemonPath, target, agentId);
    }
  }

  /**
   * Content for a file that could not be written, rendered standalone.
   *
   * Deliberately ignores what is on disk: the usual reason for landing here is
   * that the existing file could not be parsed, so there is nothing to merge
   * into. The user pastes this next to their own entries.
   */
  private fallbackContent(target: WriteTarget, daemonPath: string, agentId: string): string {
    switch (target.format.kind) {
      case "json": {
        const doc: Record<string, unknown> = {};
        let node = doc;
        for (const key of target.format.serverKeys) {
          node[key] = {};
          node = node[key] as Record<string, unknown>;
        }
        node["comp"] = this.serverEntry(daemonPath, target.scope, agentId);
        return JSON.stringify(doc, null, 2) + "\n";
      }
      case "continue-block":
        return this.renderContinueBlock(daemonPath, target.scope, agentId);
      case "aider":
        return this.aiderBlock(daemonPath, target.scope, agentId);
      case "toml":
        return this.codexBlock(daemonPath, target.scope, agentId);
    }
  }

  /**
   * Copy a file to `<file>.bak` before it is rewritten.
   *
   * One generation only — this exists so the user can undo a setup run, not to
   * keep a history. Throws on failure so the caller aborts the write: silently
   * overwriting a global config the user cannot restore is the one outcome
   * worth refusing outright.
   *
   * @internal agentSetup/ のモジュールから使う
   */
  backupIfExists(filePath: string): string | null {
    if (!fs.existsSync(filePath)) {
      return null;
    }
    const backupPath = filePath + ".bak";
    fs.copyFileSync(filePath, backupPath);
    return backupPath;
  }

  /**
   * Write through a temp file in the same directory, then rename over the target.
   *
   * A rename within one filesystem is atomic, so an interrupted run leaves the
   * previous config intact rather than a truncated file the agent would fail to
   * parse — that would break a working setup instead of merely not improving it.
   *
   * @internal agentSetup/ のモジュールから使う
   */
  atomicWrite(filePath: string, content: string): void {
    const dir = path.dirname(filePath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }
    const temp = path.join(dir, `.${path.basename(filePath)}.comp-${process.pid}.tmp`);
    fs.writeFileSync(temp, content, "utf-8");
    try {
      fs.renameSync(temp, filePath);
    } catch (error) {
      try {
        fs.unlinkSync(temp);
      } catch {
        // Temp file already gone; the rename error below is what matters.
      }
      throw error;
    }
  }

  /**
   * Render, back up, then write one config file.
   *
   * Never throws: one unwritable file must not abandon the others, and the
   * caller reports every outcome. Rendering happens first so a config that
   * cannot be parsed leaves no stray `.bak` behind.
   */
  private writeTarget(
    target: WriteTarget,
    daemonPath: string,
    agentId: string,
    magatamaOutcomes?: MagatamaOutcome[]
  ): WriteOutcome {
    const base = { path: target.path, scope: target.scope };
    const failMagatama = (reason: string): void => {
      if (target.format.kind === "json" || target.format.kind === "toml") {
        magatamaOutcomes?.push({ ...base, outcome: "failed", reason });
      }
    };

    let content: string;
    try {
      content = this.renderTarget(target, daemonPath, agentId);
    } catch (error) {
      failMagatama("comp config could not be rendered");
      return { ...base, status: "failed", reason: describeError(error) };
    }

    // comP と MAGATAMA は同じファイルの1回の書き込みにまとめる（バックアップも1回）。
    const magatama = applyMagatamaToContent(content, target, this.magatamaSettings);
    if (magatama) {
      content = magatama.content;
    }
    const record = (): void => {
      if (magatama) magatamaOutcomes?.push(magatama.outcome);
    };

    let backupPath: string | null;
    try {
      backupPath = this.backupIfExists(target.path);
    } catch (error) {
      failMagatama("backup failed");
      return { ...base, status: "failed", reason: `backup failed: ${describeError(error)}` };
    }

    try {
      this.atomicWrite(target.path, content);
    } catch (error) {
      failMagatama("write failed");
      return {
        ...base,
        status: "failed",
        reason: describeError(error),
        ...(backupPath ? { backupPath } : {}),
      };
    }

    record();
    return { ...base, status: "written", ...(backupPath ? { backupPath } : {}) };
  }

  /**
   * 起動時の修復: 既にある設定ファイルの、comP が管理する MAGATAMA の登録だけを PIN へ上げる。
   * 登録を足さず、ファイルを作らない。ユーザーの登録には触れない。
   */
  async updateManagedMagatama(): Promise<MagatamaOutcome[]> {
    return updateManaged(this, this.magatamaSettings);
  }

  /** 既にある JSON・Codex TOML の設定から、ユーザーが自分で書いた MAGATAMA の登録を集める。 */
  async findUserManagedMagatama(): Promise<UserMagatamaEntry[]> {
    return findUserManaged(this);
  }

  /**
   * 通知の「comP に管理を任せる」: 指定した設定ファイルの MAGATAMA の登録を、管理登録に置き換える。
   * uv がなければ置き換えない（outcome: failed, reason: uv-missing）。指定外のファイルには触れない。
   */
  async manageMagatamaAt(paths: string[]): Promise<MagatamaOutcome[]> {
    return manageAt(this, paths, this.magatamaSettings);
  }

  /**
   * Write comP into every config file the agent reads, and append the usage
   * rules to its instruction files.
   *
   * Succeeds when at least one config file was written. A manual fallback is
   * attached only for targets that failed — on a clean run the user has nothing
   * to copy anywhere, which is the entire point of this method.
   */
  async generateConfig(agentName: string): Promise<GenerateConfigResult> {
    const targets = this.targetsFor(agentName);

    if (targets === null) {
      return {
        configPath: "",
        success: false,
        message: `Agent ${agentName} is not supported`,
        writes: [],
        constitutionFiles: [],
        restartHint: "",
      };
    }

    const restartHint = this.restartHint(agentName);

    if (targets.length === 0) {
      return {
        configPath: "",
        success: false,
        message: this.t(
          `Could not determine ${agentName}'s config file location.`,
          `${agentName} の設定ファイルの場所を特定できませんでした。`
        ),
        writes: [],
        constitutionFiles: [],
        restartHint,
      };
    }

    const daemonPath = this.getDaemonPath();
    const agentId = AgentSetupManager.agentIdFor(agentName);
    const magatamaOutcomes: MagatamaOutcome[] = [];
    const attempts = targets.map((target) => ({
      target,
      outcome: this.writeTarget(target, daemonPath, agentId, magatamaOutcomes),
    }));
    const writes = attempts.map((a) => a.outcome);

    const constitutionFiles: string[] = [];
    if (this.readCompConfig().autoGenerateConstitution !== false) {
      for (const file of this.instructionFilesFor(agentName)) {
        if (this.ensureInstructions(file)) {
          constitutionFiles.push(file);
        }
      }
    }

    const written = writes.filter((w) => w.status === "written");
    const failed = attempts.filter((a) => a.outcome.status === "failed");
    const success = written.length > 0;

    const result: GenerateConfigResult = {
      configPath: written[0]?.path ?? "",
      success,
      message: success
        ? `MCP configuration created for ${agentName}`
        : `Failed to write MCP configuration for ${agentName}`,
      writes,
      constitutionFiles,
      restartHint,
    };
    if (magatamaOutcomes.length > 0) {
      result.magatama = magatamaOutcomes;
    }

    if (agentName === "Claude Code") {
      result.command = this.generateClaudeCodeCommand(daemonPath);
      result.historyHook =
        this.conversationRecording.enabled && this.conversationRecording.claudeCode
          ? this.ensureHistoryHook(daemonPath)
          : {
              path: this.historyHookFile(),
              scope: "workspace",
              status: "skipped",
              reason: this.t(
                "conversation recording (beta) is off — enable comp.conversationRecording.enabled to install the hook",
                "会話の記録（β版）がOFFです。comp.conversationRecording.enabled をONにするとフックを入れます"
              ),
            };
    }

    if (failed.length > 0) {
      result.manualFallback = failed.map((a) => ({
        path: a.target.path,
        content: this.fallbackContent(a.target, daemonPath, agentId),
      }));
    }

    return result;
  }

  /**
   * Agents this machine appears to have installed.
   *
   * Used to preselect entries in the setup list. Detection is a hint, never a
   * gate: a false negative would silently drop an agent the user actually has,
   * so callers must still offer every entry in AGENTS.
   */
  detectInstalledAgents(): string[] {
    const ws = (...segments: string[]) => path.join(this.workspaceRoot, ...segments);
    const home = (...segments: string[]) => path.join(this.homeDir, ...segments);
    const any = (...candidates: (string | null)[]) =>
      candidates.some((p) => p !== null && fs.existsSync(p));

    const detected: string[] = [];
    if (any(ws(".mcp.json"), ws("CLAUDE.md"), home(".claude"), home(".claude.json"))) {
      detected.push("Claude Code");
    }
    if (any(this.codexHome(), ws(".codex"))) detected.push("Codex");
    if (any(ws(".cursor"), home(".cursor"))) detected.push("Cursor");
    if (any(this.clineSettingsPath())) detected.push("Cline");
    if (any(home(".codeium", "windsurf"))) detected.push("Windsurf");
    if (any(ws(".continue"), home(".continue"))) detected.push("Continue");
    // WHY four directories: the shared config/ only appears once something was
    // configured, while each flavour (App, IDE, CLI) creates its own data folder.
    if (
      any(
        home(".gemini", "config"),
        home(".gemini", "antigravity"),
        home(".gemini", "antigravity-ide"),
        home(".gemini", "antigravity-cli")
      )
    ) {
      detected.push("Antigravity");
    }
    if (any(ws(".vscode"))) detected.push("GitHub Copilot");
    if (any(ws(".aider.conf.yml"), home(".aider.conf.yml"))) detected.push("Aider");
    // WHY settings.json specifically, not the bare ~/.gemini directory: that
    // directory also holds Antigravity's config/ and antigravity*/ subfolders
    // (see the Antigravity check above), so testing for it alone would misdetect
    // Gemini CLI on any machine that only has Antigravity installed.
    if (any(ws(".gemini", "settings.json"), home(".gemini", "settings.json"))) {
      detected.push("Gemini CLI");
    }
    return detected;
  }

  /**
   * Register comp at Claude Code's user scope through the official CLI.
   *
   * WHY the CLI rather than writing ~/.claude.json directly: that file is
   * Claude Code's own state, holds settings unrelated to MCP, and its layout is
   * not a published format. `claude mcp add` is the supported way in.
   *
   * The command is returned whether or not it ran, so a caller that could not
   * execute it still has something to show the user.
   */
  async registerClaudeCodeUserScope(runner?: CommandRunner): Promise<UserScopeResult> {
    return registerClaudeCodeUserScope(this, runner);
  }

  /**
   * Get daemon binary path for MCP stdio communication
   *
   * # Outputs
   * - Absolute path of the daemon executable
   *
   * # Precedence
   * 1. This workspace's cargo build, only with useWorkspaceDaemonBuild
   * 2. The binary bundled with the installed extension
   * 3. Nothing exists: the bundled path anyway (callers that adopt a path check
   *    it exists), or the bare binary name when there is no extension path
   *
   * WHY no `<workspaceRoot>/.comp/bin/` fallback any more: nothing ever installs
   * there, so a binary at that path could only have come with the repository.
   */
  /** @internal agentSetup/ のモジュールから使う */
  getDaemonPath(): string {
    const resolved = resolveDaemonBinary({
      extensionPath: this.extensionPath,
      workspaceRoot: this.workspaceRoot,
      useWorkspaceBuild: this.useWorkspaceDaemonBuild,
    });
    if (resolved) return resolved;
    if (this.extensionPath) return bundledDaemonBinaryPath(this.extensionPath);
    return process.platform === "win32" ? "comp-daemon.exe" : "comp-daemon";
  }

  /**
   * Path of the binary shipped inside the installed extension, if it is there.
   *
   * WHY separate from getDaemonPath(): global config files are shared by every
   * project on the machine. getDaemonPath() may prefer this workspace's cargo
   * build, which would point other projects at a dev binary that moves or disappears.
   */
  /** @internal agentSetup/ のモジュールから使う */
  bundledDaemonPath(): string | null {
    if (!this.extensionPath) {
      return null;
    }
    const candidate = bundledDaemonBinaryPath(this.extensionPath);
    return fs.existsSync(candidate) ? candidate : null;
  }

  /**
   * Binary a repair is allowed to write for the given scope, or null when none exists.
   *
   * Returning null means "leave the file alone" — replacing a broken path with
   * another broken path only hides the problem from the user.
   */
  /** @internal agentSetup/ のモジュールから使う */
  resolveRepairPath(scope: "workspace" | "global"): string | null {
    if (scope === "global") {
      return this.bundledDaemonPath();
    }
    const candidate = this.getDaemonPath();
    return fs.existsSync(candidate) ? candidate : null;
  }

  /** @internal agentSetup/ のモジュールから使う */
  isInstalledExtensionBinary(command: string): boolean {
    return isInstalledExtensionBinary(this, command);
  }

  /** @internal agentSetup/ のモジュールから使う */
  isOlderInstalledExtensionBinary(command: string): boolean {
    return isOlderInstalledExtensionBinary(this, command);
  }

  /** @internal agentSetup/ のモジュールから使う */
  repairClaudeCodeUserScope(): RepairEntry {
    return repairClaudeCodeUserScope(this);
  }

  /**
   * Same repair as repairClaudeCodeUserScope, applied through `claude mcp`
   * wherever it can be.
   *
   * WHY the CLI: ~/.claude.json is Claude Code's own state and a running Claude
   * Code rewrites it, so a direct write can lose its changes or be lost to them.
   *
   * WHY per-project registrations are removed rather than re-added: a stale one
   * is only an override of the user-scope registration. Removing it is the one
   * CLI step that cannot leave a project without comP — a failed or late remove,
   * or two windows removing at once, all end with either the old override or the
   * user scope in effect. So projects lose their override only while a current
   * user-scope registration exists to take over; otherwise their command is
   * rewritten in the file as before. The user scope itself is replaced with
   * remove + add, and written into the file directly if the add did not land.
   *
   * Whatever is still stale afterwards (no CLI, a project folder that is gone, a
   * key the CLI spells differently) is finished by the direct write.
   */
  async repairClaudeCodeRegistrations(runner: CommandRunner = defaultCommandRunner): Promise<RepairEntry> {
    return repairClaudeCodeRegistrations(this, runner);
  }

  /** @internal agentSetup/ のモジュールから使う */
  readClaudeConfig(): unknown {
    return readClaudeConfig(this);
  }

  /** @internal agentSetup/ のモジュールから使う */
  userScopeServesProjects(replacement: string): boolean {
    return userScopeServesProjects(this, replacement);
  }

  /** @internal agentSetup/ のモジュールから使う */
  writeUserScopeRegistration(server: Record<string, unknown>): void {
    return writeUserScopeRegistration(this, server);
  }

  /** @internal agentSetup/ のモジュールから使う */
  claudeAddArgs(fix: ClaudeRegistrationFix, command: string): string[] | null {
    return claudeAddArgs(this, fix, command);
  }

  /** @internal agentSetup/ のモジュールから使う */
  planClaudeCodeRepairs(): {
    file: string;
    done?: RepairEntry;
    doc?: unknown;
    replacement: string;
    fixes: ClaudeRegistrationFix[];
  } {
    return planClaudeCodeRepairs(this);
  }

  /** @internal agentSetup/ のモジュールから使う */
  repairTargets(): McpConfigTarget[] {
    return repairTargets(this);
  }

  /**
   * Rewrite MCP config files whose daemon path no longer resolves or points at
   * an older installed version of this extension.
   *
   * WHY this exists: VS Code installs extensions into
   * `<publisher>.<name>-<version>` and may remove the old directory on upgrade.
   * Config files generated earlier keep pointing at the old executable, so
   * MCP clients run an outdated daemon or fail to start while the extension
   * itself keeps working — the daemon it spawns is resolved at runtime, the
   * config files are not.
   *
   * WHY it runs unconditionally instead of on version change: globalState is
   * shared across the whole VS Code installation, not per workspace. Guarding on
   * a stored version repairs whichever workspace is opened first and silently
   * leaves every other one broken.
   *
   * Never throws — a failure here must not block activation.
   */
  repairStaleConfigs(options?: { skipClaudeCodeConfig?: boolean }): RepairEntry[] {
    return repairStaleConfigs(this, options);
  }

  /** @internal agentSetup/ のモジュールから使う */
  historyHookFile(): string {
    return historyHookFile(this);
  }

  /** @internal agentSetup/ のモジュールから使う */
  historyHookCommand(daemonPath: string): string {
    return historyHookCommand(this, daemonPath);
  }

  private ensureHistoryHook(daemonPath: string): WriteOutcome {
    return ensureHistoryHook(this, daemonPath);
  }

  /**
   * The repairTarget() counterpart for the Stop hook: rewrite its daemon path
   * after an extension upgrade, and its workspace argument after a move.
   */
  /**
   * Whether this project already records Claude Code turns: comP's record-turn
   * hook in settings.local.json, or a history-record hook in either file (the
   * comP repository's own development hook). Used to decide whether to prompt
   * for setup when recording is switched on.
   */
  historyHookInstalled(): boolean {
    return historyHookInstalled(this);
  }

  /**
   * Remove comP's record-turn Stop hook from `.claude/settings.local.json`.
   * Backs up first, keeps every other hook and key, never touches settings.json.
   */
  removeHistoryHook(): WriteOutcome {
    return removeHistoryHook(this);
  }

  /** @internal agentSetup/ のモジュールから使う */
  repairHistoryHook(): RepairEntry {
    return repairHistoryHook(this);
  }

  /** @internal agentSetup/ のモジュールから使う */
  repairTarget(target: McpConfigTarget): RepairEntry {
    return repairTarget(this, target);
  }

  /** @internal agentSetup/ のモジュールから使う */
  repairTomlTarget(target: McpConfigTarget): RepairEntry {
    return repairTomlTarget(this, target);
  }

  /** @internal agentSetup/ のモジュールから使う */
  refreshWorkspaceRootToml(block: string): string | null {
    return refreshWorkspaceRootToml(this, block);
  }

  /** @internal agentSetup/ のモジュールから使う */
  refreshWorkspaceRootEnv(server: Record<string, unknown>): boolean {
    return refreshWorkspaceRootEnv(this, server);
  }

  /** @internal agentSetup/ のモジュールから使う */
  generateClaudeCodeCommand(daemonPath: string): string {
    return generateClaudeCodeCommand(this, daemonPath);
  }

  /**
   * The global MCP config every Antigravity flavour reads — the App, the IDE,
   * the `agy` CLI and the VS Code extension (which runs the same `agy` backend).
   * See https://antigravity.google/docs/mcp.
   */
  /** @internal agentSetup/ のモジュールから使う */
  antigravityConfigPath(): string {
    return path.join(this.homeDir, ".gemini", "config", "mcp_config.json");
  }

  /**
   * Where comP wrote Antigravity's entry before Antigravity 2.0 moved its
   * config to ~/.gemini/config/. Repaired only, never newly written: the
   * directory is now just a per-flavour cache, so an entry here is inert, but
   * one left by an older comP should not keep pointing at a removed binary.
   */
  /** @internal agentSetup/ のモジュールから使う */
  legacyAntigravityConfigPath(): string {
    return path.join(this.homeDir, ".gemini", "antigravity-ide", "mcp_config.json");
  }
}

/** How long an external CLI may run before it is killed. */
const COMMAND_TIMEOUT_MS = 15_000;

/**
 * Run a command without ever letting it block the extension host.
 *
 * WHY spawn with a shell on Windows: Claude Code installed from npm is
 * `claude.cmd`, and a `.cmd` cannot be started directly — the spawn fails with
 * ENOENT, or EINVAL on Node versions carrying the CVE-2024-27980 fix. Without
 * the shell the CLI path is dead on any Windows machine that does not also have
 * the native executable.
 *
 * WHY stdin is closed: a CLI that stops to ask something — a first-run consent
 * prompt, an expired login — would otherwise sit there until the timeout fires.
 * With no stdin it reads EOF and gives up immediately.
 *
 * A non-zero exit is a normal result here, not an exception: the caller reports
 * it and falls back to showing the user the command.
 */
export const defaultCommandRunner: CommandRunner = (file, args, options) =>
  new Promise((resolve) => {
    // WHY the command line is assembled here: with `shell: true`, Node
    // concatenates the argument array without escaping it (DEP0190), so a
    // daemon path under `C:\Users\John Smith\` would break at the space.
    // Passing one already-quoted string is the safe form of that call.
    const useShell = process.platform === "win32";
    const child = useShell
      ? spawn([file, ...args].map(quoteForShell).join(" "), {
          windowsHide: true,
          shell: true,
          cwd: options?.cwd,
          // WHY: cmd.exe looks for the command in the current directory before
          // PATH. Runs whose cwd is a project folder would otherwise start a
          // claude.bat or claude.exe the repository put there.
          env: { ...process.env, NoDefaultCurrentDirectoryInExePath: "1" },
          stdio: ["ignore", "pipe", "pipe"],
        })
      : spawn(file, args, {
          windowsHide: true,
          cwd: options?.cwd,
          stdio: ["ignore", "pipe", "pipe"],
        });

    let stdout = "";
    let stderr = "";
    let settled = false;

    const finish = (ok: boolean): void => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      resolve({ ok, stdout, stderr });
    };

    const timer = setTimeout(() => {
      child.kill();
      stderr += `\ntimed out after ${COMMAND_TIMEOUT_MS}ms`;
      finish(false);
    }, COMMAND_TIMEOUT_MS);

    child.stdout?.on("data", (chunk) => {
      stdout += String(chunk);
    });
    child.stderr?.on("data", (chunk) => {
      stderr += String(chunk);
    });
    child.on("error", (error) => {
      stderr += describeError(error);
      finish(false);
    });
    child.on("close", (code) => finish(code === 0));
  });
