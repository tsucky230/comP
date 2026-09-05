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
interface WriteTarget {
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
}

/** Runs an external command. Injectable so tests never spawn a real process. */
export type CommandRunner = (
  file: string,
  args: string[]
) => Promise<{ ok: boolean; stdout: string; stderr: string }>;

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
}

/**
 * Outcome of inspecting one MCP config file.
 *
 * - `missing`  — file does not exist
 * - `failed`   — file exists but could not be parsed or written
 * - `skipped`  — no comp entry, or the recorded command must not be touched
 *                (variable reference, relative path, unresolvable replacement)
 * - `healthy`  — recorded command points at an existing file, nothing to do
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
interface McpConfigTarget {
  path: string;
  /** Key chain from the document root down to the comp server object; unused for TOML */
  serverKeys: string[];
  scope: "workspace" | "global";
  /** How the file is encoded. Decides which repair routine may touch it. */
  format: "json" | "toml";
}

/**
 * Walk a key chain and return the object at the end, or null if any hop is
 * missing or is not a plain object.
 */
function readObjectPath(doc: unknown, keys: string[]): Record<string, unknown> | null {
  let node: unknown = doc;
  for (const key of keys) {
    if (node === null || typeof node !== "object" || Array.isArray(node)) {
      return null;
    }
    node = (node as Record<string, unknown>)[key];
  }
  if (node === null || typeof node !== "object" || Array.isArray(node)) {
    return null;
  }
  return node as Record<string, unknown>;
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
 * | Antigravity    | —                                | `~/.gemini/antigravity-ide/mcp_config.json` |
 * | GitHub Copilot | `.vscode/mcp.json`               | —                                           |
 * | Aider          | `.aider.conf.yml`                | —                                           |
 * | Codex          | `.codex/config.toml`             | `~/.codex/config.toml` (or `$CODEX_HOME`)   |
 *
 * All servers are registered as stdio: the MCP client spawns the daemon binary.
 */
export class AgentSetupManager {
  private workspaceRoot: string;
  private extensionPath: string | undefined;
  private globalStorageDir: string | undefined;
  private homeDir: string;
  private codexHomeOverride: string | undefined;

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
  }

  /**
   * Codex's configuration directory.
   *
   * CODEX_HOME is best-effort: a value exported from the user's shell profile is
   * often absent from the environment a desktop-launched VS Code inherits, so a
   * user who moved their Codex home may still have to point comP at it by hand.
   * Reading it when it is there is still better than ignoring it.
   */
  private codexHome(): string {
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

  /**
   * Get configuration template for specific agent
   *
   * # Inputs
   * - agentName: "Claude Code", "Cursor", "Cline", "Windsurf", etc.
   *
   * # Outputs
   * - AgentConfig object (path, template)
   *
   * # Prerequisites
   * - The daemon binary exists in a known location
   */
  private compRuleSnippet(): string {
    return [
      "## comP MCP Tool Usage",
      "",
      "At the start of every coding or documentation task, ALWAYS call `run_pipeline` FIRST.",
      "Do NOT read files, run grep/find/Bash searches, or explore the codebase manually before calling run_pipeline.",
      "run_pipeline indexes the entire codebase and returns exactly the relevant files — use it every time.",
    ].join("\n");
  }

  private sessionContinuitySnippet(): string {
    return [
      "## Session Continuity (デーモン再起動・セッション切れ対応)",
      "",
      "セッションはデーモン再起動をまたいで永続化されます。",
      "作業を再開したり、エージェントを再起動した場合は、まず `session_recall` を呼んで過去の文脈を復帰させてください。",
      "",
      "**When resuming work**:",
      "",
      "1. Call `session_recall()` to view past interactions",
      "   - `session_recall({ \"query\": \"keyword\" })` — filter by task keywords",
      "   - `session_recall({ \"limit\": 5 })` — show last N interactions",
      "2. Review what was done previously and continue in that context",
      "",
      "**Note**: The hook system also auto-injects recent history into each prompt (`<system-reminder>`),",
      "but explicit `session_recall` is useful to manually review past work or search specific tasks.",
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
    const rules = this.ensureSnippet(filePath, "comP MCP Tool Usage", this.compRuleSnippet());
    const continuity = this.ensureSnippet(
      filePath,
      "Session Continuity",
      this.sessionContinuitySnippet()
    );
    return rules || continuity;
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
  ];

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
  private clineSettingsPath(): string | null {
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
        return [
          { path: this.antigravityConfigPath(), scope: "global", format: json(["mcpServers"]) },
        ];

      case "GitHub Copilot":
        // VS Code's own MCP file uses `servers`, not `mcpServers`.
        return [{ path: ws(".vscode", "mcp.json"), scope: "workspace", format: json(["servers"]) }];

      case "Aider":
        return [{ path: ws(".aider.conf.yml"), scope: "workspace", format: { kind: "aider" } }];

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
      default:
        return shared;
    }
  }

  /** What the user has to do before the agent sees the new config. */
  private static restartHint(agentName: string): string {
    switch (agentName) {
      case "Claude Code":
        return [
          "Claude Code を再起動してください。",
          "- CLI: 実行中のセッションを終了し、プロジェクトディレクトリで `claude` を起動し直す",
          "- VS Code / JetBrains 拡張: コマンドパレット →「Developer: Reload Window」",
          "初回起動時に「このプロジェクトの MCP サーバーを使うか」と確認されるので承認してください。",
          "`claude mcp list` で comp が Connected になっていれば成功です。",
        ].join("\n");
      case "Codex":
        return [
          "実行中の Codex を終了し、起動し直してください。",
          "- CLI: セッションを抜けて `codex` を起動し直す",
          "- VS Code 拡張: コマンドパレット →「Developer: Reload Window」",
          "セッション内で `/mcp` を実行し comp が表示されれば成功です。",
          "",
          "プロジェクト側の `.codex/config.toml` は「信頼済み」にしたプロジェクトでしか読み込まれません。",
          "未信頼のままだとグローバル設定（~/.codex/config.toml）だけが有効になります。",
        ].join("\n");
      case "Cursor":
        return [
          "Cursor をリロードしてください。",
          "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
          "反映されない場合は Cursor を完全に終了して起動し直してください。",
          "Settings → MCP に comp が有効として表示されれば成功です。",
        ].join("\n");
      case "Cline":
        return [
          "VS Code をリロードしてください。",
          "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
          "Cline のチャット画面上部の MCP アイコンから comp が有効か確認できます。",
        ].join("\n");
      case "Windsurf":
        return [
          "Windsurf をリロードしてください。",
          "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
          "- または Cascade パネル右上の MCP アイコン →「Refresh」",
        ].join("\n");
      case "Continue":
        return [
          "VS Code をリロードしてください。",
          "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
          "Continue は設定変更を自動では読み直しません。また MCP ツールは Agent モードでのみ使えます。",
        ].join("\n");
      case "Antigravity":
        return "Antigravity を完全に終了して起動し直してください。";
      case "GitHub Copilot":
        return [
          "VS Code をリロードしてください。",
          "- コマンドパレット（Ctrl/Cmd+Shift+P）→「Developer: Reload Window」",
          "Copilot Chat を Agent モードに切り替え、ツール一覧に comp が出れば成功です。",
        ].join("\n");
      case "Aider":
        return [
          "実行中の Aider を終了し、起動し直してください。",
          "Aider の MCP 対応はバージョンによって差があります。認識されない場合は `aider --version` を確認してください。",
        ].join("\n");
      default:
        return "エージェントを再起動してください。";
    }
  }

  /** The comp server object written into every JSON-shaped MCP config. */
  private serverEntry(daemonPath: string, scope: "workspace" | "global"): Record<string, unknown> {
    const env: Record<string, string> = { RUST_LOG: "info" };
    if (scope === "workspace") {
      env.COMP_WORKSPACE_ROOT = this.workspaceRoot;
    }
    return { command: daemonPath, args: [], env };
  }

  /**
   * Quote a value as a YAML single-quoted scalar.
   *
   * WHY single quotes: a Windows daemon path is full of backslashes, and YAML
   * reads a backslash inside double quotes as an escape character, so a path
   * like E:\dev would lose the \d. Inside single quotes every character is
   * literal and only the quote itself needs doubling.
   */
  private static yamlQuote(value: string): string {
    return "'" + value.replace(/'/g, "''") + "'";
  }

  /**
   * Render a standalone Continue block file.
   *
   * comP owns this file outright — Continue discovers every file under
   * `.continue/mcpServers/` — so it is written whole. Nothing the user wrote is
   * at risk, which is why this is the one format with no merge step.
   */
  private renderContinueBlock(daemonPath: string, scope: "workspace" | "global"): string {
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
  private aiderBlock(daemonPath: string, scope: "workspace" | "global"): string {
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
  private codexBlock(daemonPath: string, scope: "workspace" | "global"): string {
    const q = AgentSetupManager.tomlQuote;
    const env = [`RUST_LOG = ${q("info")}`];
    if (scope === "workspace") {
      env.push(`COMP_WORKSPACE_ROOT = ${q(this.workspaceRoot)}`);
    }
    return (
      [
        "# comP MCP server — generated by the comP VS Code extension",
        "[mcp_servers.comp]",
        `command = ${q(daemonPath)}`,
        "args = []",
        `env = { ${env.join(", ")} }`,
      ].join("\n") + "\n"
    );
  }

  /**
   * Quote a value as a TOML string.
   *
   * WHY a literal string by default: a Windows daemon path is full of
   * backslashes, and TOML reads a backslash inside a basic string as an escape,
   * so `C:\Users` would be rejected as an unknown escape sequence. A literal
   * string takes every character as-is. It cannot contain a single quote or a
   * control character, so those rare values fall back to a basic string.
   *
   * The fallback escapes the control characters too: a directory name may
   * legally contain a newline on macOS and Linux, and emitting it raw would
   * split the value across lines and leave the whole config unparseable.
   */
  private static tomlQuote(value: string): string {
    // eslint-disable-next-line no-control-regex
    if (!/['\u0000-\u001f\u007f]/.test(value)) {
      return "'" + value + "'";
    }
    const escaped = value
      .replace(/\\/g, "\\\\")
      .replace(/"/g, '\\"')
      .replace(/\n/g, "\\n")
      .replace(/\r/g, "\\r")
      .replace(/\t/g, "\\t")
      // eslint-disable-next-line no-control-regex
      .replace(/[\u0000-\u001f\u007f]/g, (char) => {
        const code = char.charCodeAt(0).toString(16).padStart(4, "0");
        return `\\u${code}`;
      });
    return '"' + escaped + '"';
  }

  /**
   * Locate comP's own table in a Codex config.
   *
   * Returns the character range covering `[mcp_servers.comp]`, everything under
   * it, and any `[mcp_servers.comp.*]` sub-tables that follow — a config written
   * by `codex mcp add` uses a `[mcp_servers.comp.env]` sub-table, and leaving it
   * behind after replacing the parent would resurrect a stale environment.
   *
   * Shared by the writer and the repair pass so that "where comP's block is" has
   * exactly one definition.
   */
  private static findCompSection(text: string): { start: number; end: number } | null {
    const lines = text.split("\n");
    // Header of comP's table, or of one of its sub-tables. Bare and quoted key
    // forms both occur: `codex mcp add` writes the bare one, hand edits vary.
    const isCompHeader = (line: string): boolean =>
      /^\s*\[\s*mcp_servers\s*\.\s*("comp"|'comp'|comp)\s*(\.[^\]]*)?\]/.test(line);
    const isAnyHeader = (line: string): boolean => /^\s*\[/.test(line);

    let start = -1;
    let end = -1;
    let offset = 0;
    for (const line of lines) {
      const lineEnd = offset + line.length + 1;
      if (start === -1) {
        if (isCompHeader(line)) {
          start = offset;
          end = Math.min(lineEnd, text.length);
        }
      } else if (isAnyHeader(line)) {
        // A sub-table still belongs to comP; anything else ends the section.
        if (!isCompHeader(line)) {
          break;
        }
        end = Math.min(lineEnd, text.length);
      } else {
        end = Math.min(lineEnd, text.length);
      }
      offset = lineEnd;
    }

    return start === -1 ? null : { start, end };
  }

  /**
   * Whether a located section can be replaced as a unit.
   *
   * findCompSection ends the section at the next line that opens a table, and a
   * nested array element written on its own line — `["--flag"],` inside a
   * multi-line `args` — looks exactly like one. Splicing there would strip the
   * rest of the collection and leave the file unparseable, taking down the
   * user's whole Codex configuration rather than just comP.
   *
   * A truncated section always ends with brackets still open, so that is what is
   * checked. Anything this cannot account for — a multi-line basic string, say —
   * also comes back unbalanced, which is the safe direction to be wrong in.
   */
  private static isSelfContained(section: string): boolean {
    let depth = 0;
    let quote: '"' | "'" | null = null;

    for (let i = 0; i < section.length; i++) {
      const char = section[i];

      if (quote !== null) {
        // Only basic strings have escapes; in a literal string a backslash is
        // just a backslash, which is the whole reason paths are written that way.
        if (char === "\\" && quote === '"') {
          i++;
        } else if (char === quote) {
          quote = null;
        } else if (char === "\n") {
          return false;
        }
        continue;
      }

      if (char === '"' || char === "'") {
        quote = char;
      } else if (char === "#") {
        while (i < section.length && section[i] !== "\n") {
          i++;
        }
      } else if (char === "[" || char === "{") {
        depth++;
      } else if (char === "]" || char === "}") {
        depth--;
        if (depth < 0) {
          return false;
        }
      }
    }

    return depth === 0 && quote === null;
  }

  /**
   * Servers written in a shape this line-oriented splice cannot rewrite.
   *
   * Both checks are scoped to the table they matter in: an unrelated key called
   * `comp` under `[tui]` is none of comP's business, and refusing to configure
   * Codex because of one would be a plain false alarm.
   *
   * Returns the reason to refuse, or null when the file can be edited.
   */
  private static unsupportedServerShape(text: string): string | null {
    let table = "";

    for (const rawLine of text.split("\n")) {
      const line = rawLine.trim();

      const header = /^\[\s*([^\]]*?)\s*\]/.exec(line);
      if (header) {
        table = header[1].replace(/\s+/g, "");
        continue;
      }
      // `mcp_servers = { ... }` — the entire server table written inline.
      if (table === "" && /^mcp_servers\s*=/.test(line)) {
        return "mcp_servers is written as an inline table; merge comP by hand";
      }
      // `comp = { ... }` or `comp.command = ...` under `[mcp_servers]`.
      if (
        table === "mcp_servers" &&
        /^("comp"|'comp'|comp)\s*(\.[A-Za-z0-9_-]+)*\s*=/.test(line)
      ) {
        return "a comp entry is written in inline/dotted form; merge comP by hand";
      }
    }

    return null;
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
  private renderCodexConfig(daemonPath: string, target: WriteTarget): string {
    const block = this.codexBlock(daemonPath, target.scope);

    if (!fs.existsSync(target.path)) {
      return block;
    }
    const existing = fs.readFileSync(target.path, "utf-8");
    if (existing.trim().length === 0) {
      return block;
    }

    const unsupported = AgentSetupManager.unsupportedServerShape(existing);
    if (unsupported) {
      throw new Error(unsupported);
    }

    const section = AgentSetupManager.findCompSection(existing);
    if (section) {
      const before = existing.slice(0, section.start);
      const after = existing.slice(section.end);
      if (!AgentSetupManager.isSelfContained(existing.slice(section.start, section.end))) {
        throw new Error("the existing comp table spans a multi-line value; merge comP by hand");
      }
      // The generated block carries its own comment line, so drop the one the
      // previous run left immediately above the header.
      const trimmedBefore = before.replace(
        /(^|\n)# comP MCP server[^\n]*\n$/,
        (_match, lead: string) => lead
      );
      return trimmedBefore + block + (after.startsWith("\n") ? after : after ? "\n" + after : "");
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
  private renderAiderConfig(daemonPath: string, target: WriteTarget): string {
    const block = this.aiderBlock(daemonPath, target.scope);

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
  private renderTarget(target: WriteTarget, daemonPath: string): string {
    switch (target.format.kind) {
      case "json":
        return this.mergeJsonConfig(
          target.path,
          target.format.serverKeys,
          this.serverEntry(daemonPath, target.scope)
        );
      case "continue-block":
        return this.renderContinueBlock(daemonPath, target.scope);
      case "aider":
        return this.renderAiderConfig(daemonPath, target);
      case "toml":
        return this.renderCodexConfig(daemonPath, target);
    }
  }

  /**
   * Content for a file that could not be written, rendered standalone.
   *
   * Deliberately ignores what is on disk: the usual reason for landing here is
   * that the existing file could not be parsed, so there is nothing to merge
   * into. The user pastes this next to their own entries.
   */
  private fallbackContent(target: WriteTarget, daemonPath: string): string {
    switch (target.format.kind) {
      case "json": {
        const doc: Record<string, unknown> = {};
        let node = doc;
        for (const key of target.format.serverKeys) {
          node[key] = {};
          node = node[key] as Record<string, unknown>;
        }
        node["comp"] = this.serverEntry(daemonPath, target.scope);
        return JSON.stringify(doc, null, 2) + "\n";
      }
      case "continue-block":
        return this.renderContinueBlock(daemonPath, target.scope);
      case "aider":
        return this.aiderBlock(daemonPath, target.scope);
      case "toml":
        return this.codexBlock(daemonPath, target.scope);
    }
  }

  /**
   * Copy a file to `<file>.bak` before it is rewritten.
   *
   * One generation only — this exists so the user can undo a setup run, not to
   * keep a history. Throws on failure so the caller aborts the write: silently
   * overwriting a global config the user cannot restore is the one outcome
   * worth refusing outright.
   */
  private backupIfExists(filePath: string): string | null {
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
   */
  private atomicWrite(filePath: string, content: string): void {
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
  private writeTarget(target: WriteTarget, daemonPath: string): WriteOutcome {
    const base = { path: target.path, scope: target.scope };

    let content: string;
    try {
      content = this.renderTarget(target, daemonPath);
    } catch (error) {
      return { ...base, status: "failed", reason: describeError(error) };
    }

    let backupPath: string | null;
    try {
      backupPath = this.backupIfExists(target.path);
    } catch (error) {
      return { ...base, status: "failed", reason: `backup failed: ${describeError(error)}` };
    }

    try {
      this.atomicWrite(target.path, content);
    } catch (error) {
      return {
        ...base,
        status: "failed",
        reason: describeError(error),
        ...(backupPath ? { backupPath } : {}),
      };
    }

    return { ...base, status: "written", ...(backupPath ? { backupPath } : {}) };
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

    const restartHint = AgentSetupManager.restartHint(agentName);

    if (targets.length === 0) {
      return {
        configPath: "",
        success: false,
        message: `${agentName} の設定ファイルの場所を特定できませんでした。`,
        writes: [],
        constitutionFiles: [],
        restartHint,
      };
    }

    const daemonPath = this.getDaemonPath();
    const attempts = targets.map((target) => ({
      target,
      outcome: this.writeTarget(target, daemonPath),
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

    if (agentName === "Claude Code") {
      result.command = this.generateClaudeCodeCommand(daemonPath);
    }

    if (failed.length > 0) {
      result.manualFallback = failed.map((a) => ({
        path: a.target.path,
        content: this.fallbackContent(a.target, daemonPath),
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
    if (any(home(".gemini", "antigravity-ide"))) detected.push("Antigravity");
    if (any(ws(".vscode"))) detected.push("GitHub Copilot");
    if (any(ws(".aider.conf.yml"), home(".aider.conf.yml"))) detected.push("Aider");
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
    const daemonPath = this.getDaemonPath();
    const command = this.generateClaudeCodeCommand(daemonPath);
    const run = runner ?? defaultCommandRunner;

    // The Windows runner goes through cmd.exe (see defaultCommandRunner), where
    // these characters change what the command line means. Rather than try to
    // quote around them, hand the command to the user and let them run it.
    if (process.platform === "win32" && SHELL_METACHARACTERS.test(daemonPath)) {
      return {
        registered: false,
        command,
        reason: "daemon のパスにシェル特殊文字が含まれるため、自動登録を見送りました",
      };
    }

    const version = await run("claude", ["--version"]);
    if (!version.ok) {
      return { registered: false, command, reason: "claude CLI が見つかりませんでした" };
    }

    // User scope is shared by every project, so no COMP_WORKSPACE_ROOT here:
    // the daemon falls back to its working directory, which the client sets per
    // project. Everything after `--` is the server command, untouched by the CLI.
    const args = [
      "mcp",
      "add",
      "--scope",
      "user",
      "--transport",
      "stdio",
      "comp",
      "-e",
      "RUST_LOG=info",
      "--",
      daemonPath,
    ];
    const added = await run("claude", args);
    if (!added.ok) {
      return {
        registered: false,
        command,
        reason: (added.stderr || added.stdout || "claude mcp add に失敗しました").trim(),
      };
    }
    return { registered: true, command };
  }

  /**
   * Get daemon binary path for MCP stdio communication
   *
   * # Outputs
   * - Absolute path of the daemon executable
   *
   * # Precedence
   * 1. Development build: <workspaceRoot>/daemon/target/release/
   * 2. Bundled package: <workspaceRoot>/.comp/bin/
   */
  private getDaemonPath(): string {
    const binaryName = process.platform === "win32" ? "comp-daemon.exe" : "comp-daemon";
    const bundledBinaryName = process.platform === "win32" ? "comp-daemon-win.exe"
      : process.platform === "darwin" ? "comp-daemon-macos"
      : "comp-daemon-linux";

    // Development: cargo build output in workspace (default cargo output name)
    const devPath = path.join(this.workspaceRoot, "daemon", "target", "release", binaryName);
    if (fs.existsSync(devPath)) return devPath;

    // Extension: binary bundled with the installed extension (platform-specific name)
    if (this.extensionPath) {
      const extPath = path.join(this.extensionPath, "daemon", "target", "release", bundledBinaryName);
      if (fs.existsSync(extPath)) return extPath;
    }

    // Production: bundled binary in workspace .comp/bin
    return path.join(this.workspaceRoot, ".comp", "bin", binaryName);
  }

  /**
   * Path of the binary shipped inside the installed extension, if it is there.
   *
   * WHY separate from getDaemonPath(): global config files are shared by every
   * project on the machine. getDaemonPath() prefers this workspace's cargo build,
   * which would point other projects at a dev binary that moves or disappears.
   */
  private bundledDaemonPath(): string | null {
    if (!this.extensionPath) {
      return null;
    }
    const bundledBinaryName = process.platform === "win32" ? "comp-daemon-win.exe"
      : process.platform === "darwin" ? "comp-daemon-macos"
      : "comp-daemon-linux";
    const candidate = path.join(this.extensionPath, "daemon", "target", "release", bundledBinaryName);
    return fs.existsSync(candidate) ? candidate : null;
  }

  /**
   * Binary a repair is allowed to write for the given scope, or null when none exists.
   *
   * Returning null means "leave the file alone" — replacing a broken path with
   * another broken path only hides the problem from the user.
   */
  private resolveRepairPath(scope: "workspace" | "global"): string | null {
    if (scope === "global") {
      return this.bundledDaemonPath();
    }
    const candidate = this.getDaemonPath();
    return fs.existsSync(candidate) ? candidate : null;
  }

  /**
   * Every MCP config file comP is known to have written, in a fixed order.
   *
   * The `.comp/config/*.json` entries are kept even though setup no longer
   * writes there: a user who copied one of those files into place before this
   * version still has a daemon path that goes stale on every upgrade.
   */
  private repairTargets(): McpConfigTarget[] {
    const ws = (...segments: string[]) => path.join(this.workspaceRoot, ...segments);
    const home = (...segments: string[]) => path.join(this.homeDir, ...segments);
    const cline = this.clineSettingsPath();
    const json = (path: string, serverKeys: string[], scope: "workspace" | "global") => ({
      path,
      serverKeys,
      scope,
      format: "json" as const,
    });
    return [
      json(ws(".vscode", "mcp.json"), ["servers", "comp"], "workspace"),
      json(ws(".mcp.json"), ["mcpServers", "comp"], "workspace"),
      json(ws(".cursor", "mcp.json"), ["mcpServers", "comp"], "workspace"),
      json(home(".cursor", "mcp.json"), ["mcpServers", "comp"], "global"),
      json(home(".codeium", "windsurf", "mcp_config.json"), ["mcpServers", "comp"], "global"),
      ...(cline ? [json(cline, ["mcpServers", "comp"], "global")] : []),
      json(this.antigravityConfigPath(), ["mcpServers", "comp"], "global"),
      // Codex keeps its servers in TOML, so these go through repairTomlTarget.
      {
        path: path.join(this.codexHome(), "config.toml"),
        serverKeys: [],
        scope: "global" as const,
        format: "toml" as const,
      },
      {
        path: ws(".codex", "config.toml"),
        serverKeys: [],
        scope: "workspace" as const,
        format: "toml" as const,
      },
      // Legacy locations, still repaired for anyone who copied them by hand.
      json(ws(".comp", "config", "cursor_config.json"), ["comp"], "workspace"),
      json(ws(".comp", "config", "cline_config.json"), ["mcpServers", "comp"], "workspace"),
      json(ws(".comp", "config", "windsurf_config.json"), ["mcpServers", "comp"], "workspace"),
    ];
  }

  /**
   * Rewrite MCP config files whose daemon path no longer resolves.
   *
   * WHY this exists: VS Code installs extensions into
   * `<publisher>.<name>-<version>` and deletes the old directory on upgrade.
   * Config files generated earlier keep pointing at the removed executable, so
   * MCP clients fail to start while the extension itself keeps working — the
   * daemon it spawns is resolved at runtime, the config files are not.
   *
   * WHY it runs unconditionally instead of on version change: globalState is
   * shared across the whole VS Code installation, not per workspace. Guarding on
   * a stored version repairs whichever workspace is opened first and silently
   * leaves every other one broken.
   *
   * Never throws — a failure here must not block activation.
   */
  repairStaleConfigs(): RepairEntry[] {
    return this.repairTargets().map((target) => {
      try {
        return target.format === "toml" ? this.repairTomlTarget(target) : this.repairTarget(target);
      } catch (error) {
        return {
          file: target.path,
          status: "failed" as const,
          reason: error instanceof Error ? error.message : String(error),
        };
      }
    });
  }

  /**
   * Inspect and, if needed, repair a single config file.
   *
   * Checks run in this order, each one a reason to stop:
   * 1. file absent
   * 2. unparseable JSON
   * 3. no comp server object, or command absent/empty/non-string
   * 4. command contains `${...}` — a variable the host expands, not our business
   * 5. command is relative — deliberately made portable by the user
   * 6. command resolves to an existing file — nothing to rewrite there
   * 7. otherwise rewrite, but only if a replacement binary actually exists
   *
   * COMP_WORKSPACE_ROOT is refreshed in the same pass for workspace-scoped files
   * only, since a global file legitimately points at a different project. Steps 4
   * and 5 stop before that refresh: a config deliberately made portable should be
   * left portable in both fields.
   */
  private repairTarget(target: McpConfigTarget): RepairEntry {
    const file = target.path;

    if (!fs.existsSync(file)) {
      return { file, status: "missing" };
    }

    let doc: unknown;
    try {
      doc = JSON.parse(fs.readFileSync(file, "utf-8"));
    } catch (error) {
      return {
        file,
        status: "failed",
        reason: `invalid JSON: ${error instanceof Error ? error.message : String(error)}`,
      };
    }

    const server = readObjectPath(doc, target.serverKeys);
    if (!server) {
      return { file, status: "skipped", reason: "no comp server entry" };
    }

    const command = server["command"];
    if (typeof command !== "string" || command.length === 0) {
      return { file, status: "skipped", reason: "no command value" };
    }
    // The host expands these. Resolving one here would freeze in the path of
    // whichever machine happened to run the repair.
    if (command.includes("${")) {
      return { file, status: "skipped", reason: "command uses a variable reference" };
    }
    // A relative command was made portable on purpose; overwriting undoes that intent.
    if (!path.isAbsolute(command)) {
      return { file, status: "skipped", reason: "command is relative" };
    }

    const commandBroken = !fs.existsSync(command);
    let replacement: string | null = null;
    if (commandBroken) {
      replacement = this.resolveRepairPath(target.scope);
      if (!replacement) {
        // Swapping one broken path for another would only hide the failure.
        return { file, status: "skipped", reason: "no replacement binary available" };
      }
    }

    const envRepaired = target.scope === "workspace" && this.refreshWorkspaceRootEnv(server);

    if (!commandBroken && !envRepaired) {
      return { file, status: "healthy" };
    }

    if (replacement) {
      server["command"] = replacement;
    }
    fs.writeFileSync(file, JSON.stringify(doc, null, 2), "utf-8");

    const entry: RepairEntry = { file, status: "repaired" };
    if (replacement) {
      entry.from = command;
      entry.to = replacement;
    }
    if (envRepaired) {
      entry.envRepaired = true;
    }
    return entry;
  }

  /**
   * The TOML counterpart of repairTarget(), for Codex's config.toml.
   *
   * WHY it exists rather than letting repairTarget() handle every file: that one
   * parses JSON, so a perfectly valid Codex config would be reported as broken
   * on every single activation while the daemon path it holds went stale
   * forever — the one failure mode this whole repair pass was written to stop.
   *
   * The checks mirror repairTarget() exactly, in the same order and for the same
   * reasons; only the parsing differs.
   */
  private repairTomlTarget(target: McpConfigTarget): RepairEntry {
    const file = target.path;

    if (!fs.existsSync(file)) {
      return { file, status: "missing" };
    }

    const text = fs.readFileSync(file, "utf-8");
    const section = AgentSetupManager.findCompSection(text);
    if (!section) {
      return { file, status: "skipped", reason: "no comp server entry" };
    }

    const block = text.slice(section.start, section.end);
    // Same guard as the writer: a section ending with brackets still open was
    // cut short by a nested array element, and splicing it would destroy the
    // rest of the file.
    if (!AgentSetupManager.isSelfContained(block)) {
      return { file, status: "skipped", reason: "comp table spans a multi-line value" };
    }

    const commandMatch = /^\s*command\s*=\s*('[^'\n]*'|"(?:[^"\\\n]|\\.)*")/m.exec(block);
    if (!commandMatch) {
      return { file, status: "skipped", reason: "no command value" };
    }
    const command = decodeTomlString(commandMatch[1]);
    if (command.length === 0) {
      return { file, status: "skipped", reason: "no command value" };
    }
    if (command.includes("${")) {
      return { file, status: "skipped", reason: "command uses a variable reference" };
    }
    if (!path.isAbsolute(command)) {
      return { file, status: "skipped", reason: "command is relative" };
    }

    const commandBroken = !fs.existsSync(command);
    let replacement: string | null = null;
    if (commandBroken) {
      replacement = this.resolveRepairPath(target.scope);
      if (!replacement) {
        return { file, status: "skipped", reason: "no replacement binary available" };
      }
    }

    let repairedBlock = block;
    if (replacement) {
      // WHY offsets rather than String.replace: a replacement path containing
      // `$&` or `$1` would be reinterpreted as a capture reference and splice
      // the old value back into the middle of the new one, leaving a command
      // that is both wrong and unquoted. The quoted value is the tail of the
      // match, so its start is derivable without searching for it.
      const valueStart = commandMatch.index + commandMatch[0].length - commandMatch[1].length;
      repairedBlock =
        repairedBlock.slice(0, valueStart) +
        AgentSetupManager.tomlQuote(replacement) +
        repairedBlock.slice(valueStart + commandMatch[1].length);
    }

    const withFreshRoot =
      target.scope === "workspace" ? this.refreshWorkspaceRootToml(repairedBlock) : null;
    const envRepaired = withFreshRoot !== null;
    if (withFreshRoot !== null) {
      repairedBlock = withFreshRoot;
    }

    if (!commandBroken && !envRepaired) {
      return { file, status: "healthy" };
    }

    fs.writeFileSync(
      file,
      text.slice(0, section.start) + repairedBlock + text.slice(section.end),
      "utf-8"
    );

    const entry: RepairEntry = { file, status: "repaired" };
    if (replacement) {
      entry.from = command;
      entry.to = replacement;
    }
    if (envRepaired) {
      entry.envRepaired = true;
    }
    return entry;
  }

  /**
   * TOML counterpart of refreshWorkspaceRootEnv(), with the same skip rules.
   *
   * Returns the rewritten block, or null when nothing needed changing. Callers
   * must only invoke this for workspace-scoped files.
   */
  private refreshWorkspaceRootToml(block: string): string | null {
    const match = /(COMP_WORKSPACE_ROOT\s*=\s*)('[^'\n]*'|"(?:[^"\\\n]|\\.)*")/.exec(block);
    if (!match) {
      return null;
    }
    const current = decodeTomlString(match[2]);
    if (current.length === 0 || current.includes("${") || !path.isAbsolute(current)) {
      return null;
    }
    if (path.resolve(current) === path.resolve(this.workspaceRoot)) {
      return null;
    }
    return (
      block.slice(0, match.index) +
      match[1] +
      AgentSetupManager.tomlQuote(this.workspaceRoot) +
      block.slice(match.index + match[0].length)
    );
  }

  /**
   * Point COMP_WORKSPACE_ROOT at the workspace that is actually open.
   *
   * WHY: the value is written as an absolute path at generation time, so moving
   * the project — or opening the same checkout from a different machine — leaves
   * the daemon indexing a directory that is no longer there.
   *
   * Returns true when the value changed. Callers must only invoke this for
   * workspace-scoped files.
   */
  private refreshWorkspaceRootEnv(server: Record<string, unknown>): boolean {
    const env = server["env"];
    if (env === null || typeof env !== "object" || Array.isArray(env)) {
      return false;
    }

    const envRecord = env as Record<string, unknown>;
    const current = envRecord["COMP_WORKSPACE_ROOT"];
    if (typeof current !== "string" || current.length === 0) {
      return false;
    }
    if (current.includes("${") || !path.isAbsolute(current)) {
      return false;
    }
    if (path.resolve(current) === path.resolve(this.workspaceRoot)) {
      return false;
    }

    envRecord["COMP_WORKSPACE_ROOT"] = this.workspaceRoot;
    return true;
  }

  /**
   * The `claude mcp add` line that registers comP for every project.
   *
   * `--` matters: without it the CLI parses the daemon path as its own
   * positional URL argument. `--scope user` matters too — the default is
   * `local`, which would register comP for the current project only and quietly
   * contradict what this command is offered for.
   */
  private generateClaudeCodeCommand(daemonPath: string): string {
    const quote = (value: string) => `"${value.replace(/"/g, '\\"')}"`;
    return [
      "claude mcp add --scope user --transport stdio comp",
      "-e RUST_LOG=info",
      "--",
      quote(daemonPath),
    ].join(" ");
  }

  /**
   * Antigravity MCP config path
   *
   * Antigravity (Google Gemini-based IDE) stores global MCP config at
   * ~/.gemini/antigravity-ide/mcp_config.json.
   */
  private antigravityConfigPath(): string {
    return path.join(this.homeDir, ".gemini", "antigravity-ide", "mcp_config.json");
  }
}

/** Message of an unknown throwable, for reporting into a WriteOutcome. */
function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Read the value out of a quoted TOML scalar.
 *
 * Only the two single-line forms are handled, which is all comP writes and all
 * a daemon path can be: a literal string takes its content verbatim, a basic
 * string has its backslash escapes undone.
 */
function decodeTomlString(raw: string): string {
  if (raw.startsWith("'")) {
    return raw.slice(1, -1);
  }
  return raw.slice(1, -1).replace(/\\(.)/g, (_match, char: string) => {
    switch (char) {
      case "n":
        return "\n";
      case "t":
        return "\t";
      case "r":
        return "\r";
      default:
        return char;
    }
  });
}

/** How long an external CLI may run before it is killed. */
const COMMAND_TIMEOUT_MS = 15_000;

/**
 * Characters that change what a cmd.exe command line means.
 *
 * A daemon path containing one of these cannot be handed to the shell safely,
 * so registration is skipped and the user runs the command themselves.
 */
const SHELL_METACHARACTERS = /["&|<>^%]/;

/**
 * Quote one token for cmd.exe.
 *
 * Anything outside the plain path alphabet gets wrapped in double quotes, which
 * covers the case that actually bites — spaces in a Windows user directory.
 * Callers reject SHELL_METACHARACTERS first, so no escaping beyond this is
 * needed.
 */
function quoteForShell(value: string): string {
  return /^[A-Za-z0-9_.:\\/=-]+$/.test(value) ? value : `"${value}"`;
}

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
const defaultCommandRunner: CommandRunner = (file, args) =>
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
          stdio: ["ignore", "pipe", "pipe"],
        })
      : spawn(file, args, {
          windowsHide: true,
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
