// AgentSetup - Generate MCP configuration for AI agents
//
// Responsibilities:
// 1. Generate config files for Claude Code, Cursor, Cline, Windsurf
// 2. Write MCP_SERVERS env var or config.json based on agent type
// 3. Provide UI for selecting agent and copying config

import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { DaemonManager } from "../daemon/DaemonManager";

interface AgentConfig {
  name: string;
  configPath: string;
  envVar?: string;
  template: (daemonPath: string) => string;
  command?: (daemonPath: string) => string;
  llmPrompt?: (daemonPath: string, configPath: string) => string;
  constitutionGuide?: ConstitutionGuide;
}

export interface ConstitutionGuide {
  filePath: string;
  snippet: string;
  llmInstruction: string;
}

export interface GenerateConfigResult {
  configPath: string;
  success: boolean;
  message: string;
  command?: string;
  llmPrompt?: string;
  constitutionGuide?: ConstitutionGuide;
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
  /** Key chain from the document root down to the comp server object */
  serverKeys: string[];
  scope: "workspace" | "global";
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
 * AgentSetup - Generate MCP configuration files for various AI agents
 *
 * # Supported Agents
 * - Claude Code (claude_desktop_config.json)
 * - Cursor (.cursor/rules)
 * - Cline (.cline/config.json)
 * - Windsurf (.windsurf/config.json)
 * - GitHub Copilot (N/A - requires different approach)
 * - Continue.dev (.continue/config.py)
 *
 * # MCP Server Setup
 * - stdio: Spawn daemon as an MCP server subprocess
 * - Init parameters: workspace_root
 */
export class AgentSetupManager {
  private workspaceRoot: string;
  private extensionPath: string | undefined;

  constructor(_daemonManager: DaemonManager, workspaceRoot: string, extensionPath?: string) {
    // daemonManager reserved for future use (e.g., querying indexing status during config generation)
    this.workspaceRoot = workspaceRoot;
    this.extensionPath = extensionPath;
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

  private buildConstitutionGuide(filePath: string): ConstitutionGuide {
    const snippet = this.compRuleSnippet();
    const llmInstruction = [
      `\`${filePath}\` にプロジェクトルート基準で以下を追記してください。`,
      `ファイルが存在しない場合は新規作成し、既存の場合は末尾に追記してください。`,
      `すでに "comP MCP Tool Usage" というセクションがある場合は追記不要です。`,
      ``,
      "```",
      snippet,
      "```",
    ].join("\n");
    return { filePath, snippet, llmInstruction };
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

  private ensureSessionContinuityInstructions(filePath: string): void {
    try {
      const dir = path.dirname(filePath);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }

      const snippet = this.sessionContinuitySnippet();
      let content = "";

      // Read existing file if it exists
      if (fs.existsSync(filePath)) {
        content = fs.readFileSync(filePath, "utf-8");
        // Skip if Session Continuity section already exists
        if (content.includes("Session Continuity")) {
          return;
        }
        // Append to existing content
        content = content.trimEnd() + "\n\n---\n\n" + snippet;
      } else {
        // Create new file with minimal header (only for .claude/CLAUDE.md)
        if (filePath.endsWith(".claude/CLAUDE.md")) {
          content = [
            "# comP — Context-Aware AI Coding",
            "",
            "## MANDATORY: use comP MCP pipeline — do NOT grep or glob the codebase",
            "",
            "For every task — bug fixes, features, refactors, debugging:",
            "**call `run_pipeline` FIRST**. It searches the indexed codebase and returns",
            "the most relevant files and symbols for your task.",
            "",
            "---",
            "",
            snippet,
          ].join("\n");
        } else {
          // For CLAUDE.md (project root), just the snippet
          content = snippet;
        }
      }

      fs.writeFileSync(filePath, content, "utf-8");
    } catch (error) {
      // Best-effort: log but don't fail the whole operation
      console.warn(`Warning: failed to ensure session continuity in ${filePath}: ${error}`);
    }
  }

  getAgentConfig(agentName: string): AgentConfig | null {

    switch (agentName) {
      case "Claude Code":
        return {
          name: "Claude Code",
          configPath: this.claudeDesktopConfigPath(),
          template: (path) => this.generateClaudeCodeConfig(path),
          command: (path) => this.generateClaudeCodeCommand(path),
          llmPrompt: (path) => this.generateClaudeCodeLLMPrompt(path),
          constitutionGuide: this.buildConstitutionGuide("CLAUDE.md"),
        };

      case "Cursor":
        return {
          name: "Cursor",
          configPath: this.cursorConfigPath(),
          template: (path) => this.generateCursorConfig(path),
          llmPrompt: (_, configPath) => `以下のMCPサーバー設定ファイルが生成されました。このプロジェクトで comP を MCP サーバーとして利用できるように、あなたの設定を更新してください。\n設定ファイルパス: ${configPath}`,
          constitutionGuide: this.buildConstitutionGuide(".cursor/rules"),
        };

      case "Cline":
        return {
          name: "Cline",
          configPath: this.clineConfigPath(),
          template: (path) => this.generateClineConfig(path),
          llmPrompt: (_, configPath) => `以下のMCPサーバー設定ファイルが生成されました。このプロジェクトで comP を MCP サーバーとして利用できるように、あなたの設定を更新してください。\n設定ファイルパス: ${configPath}`,
          constitutionGuide: this.buildConstitutionGuide(".clinerules"),
        };

      case "Windsurf":
        return {
          name: "Windsurf",
          configPath: this.windsurfConfigPath(),
          template: (path) => this.generateWindsurfConfig(path),
          llmPrompt: (_, configPath) => `以下のMCPサーバー設定ファイルが生成されました。このプロジェクトで comP を MCP サーバーとして利用できるように、あなたの設定を更新してください。\n設定ファイルパス: ${configPath}`,
          constitutionGuide: this.buildConstitutionGuide(".windsurfrules"),
        };

      case "Continue":
        return {
          name: "Continue",
          configPath: this.continueConfigPath(),
          template: (path) => this.generateContinueConfig(path),
          llmPrompt: (_, configPath) => `以下のMCPサーバー設定ファイルが生成されました。このプロジェクトで comP を MCP サーバーとして利用できるように、あなたの設定を更新してください。\n設定ファイルパス: ${configPath}`
        };

      case "Antigravity":
        return {
          name: "Antigravity",
          configPath: this.antigravityConfigPath(),
          template: (path) => this.generateAntigravityConfig(path),
        };

      case "GitHub Copilot":
        return {
          name: "GitHub Copilot",
          configPath: this.copilotConfigPath(),
          template: (path) => this.generateCopilotConfig(path),
          constitutionGuide: this.buildConstitutionGuide(".github/copilot-instructions.md"),
        };

      case "Aider":
        return {
          name: "Aider",
          configPath: this.aiderConfigPath(),
          template: (path) => this.generateAiderConfig(path),
          llmPrompt: (_, configPath) => `以下のMCPサーバー設定ファイルが生成されました。このプロジェクトで comP を MCP サーバーとして利用できるように、あなたの設定を更新してください。\n設定ファイルパス: ${configPath}`,
          constitutionGuide: this.buildConstitutionGuide("CONVENTIONS.md"),
        };

      default:
        return null;
    }
  }

  /**
   * Generate and write configuration for agent
   *
   * # Inputs
   * - agentName: The name of the agent
   *
   * # Outputs
   * - { configPath, success, message }
   *
   * # Prerequisites
   * - The user has file write permissions
   */
  async generateConfig(agentName: string): Promise<GenerateConfigResult> {
    const config = this.getAgentConfig(agentName);

    if (!config) {
      return {
        configPath: "",
        success: false,
        message: `Agent ${agentName} is not supported`,
      };
    }

    try {
      const daemonPath = this.getDaemonPath();
      const configContent = config.template(daemonPath);
      // WHY: Global configs like Antigravity return absolute paths so no join is required
      const fullPath = path.isAbsolute(config.configPath)
        ? config.configPath
        : path.join(this.workspaceRoot, config.configPath);

      // Create directories if needed
      const dir = path.dirname(fullPath);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }

      // Write config file
      fs.writeFileSync(fullPath, configContent, "utf-8");

      const result: GenerateConfigResult = {
        configPath: fullPath,
        success: true,
        message: `MCP configuration created for ${agentName}`,
      };

      if (config.command) {
        result.command = config.command(daemonPath);
      }
      if (config.llmPrompt) {
        result.llmPrompt = config.llmPrompt(daemonPath, fullPath);
      }
      // Check if auto-generation of constitution files is enabled (default: true)
      const compConfig = this.readCompConfig();
      const autoGenerateConstitution = compConfig.autoGenerateConstitution !== false;

      if (autoGenerateConstitution) {
        if (config.constitutionGuide) {
          result.constitutionGuide = config.constitutionGuide;
          // Also append Session Continuity to agent-specific constitution file
          // (e.g., .github/copilot-instructions.md, .cursor/rules, etc.)
          const constitutionPath = path.isAbsolute(config.constitutionGuide.filePath)
            ? config.constitutionGuide.filePath
            : path.join(this.workspaceRoot, config.constitutionGuide.filePath);
          this.ensureSessionContinuityInstructions(constitutionPath);
        }

        // Auto-generate .claude/CLAUDE.md with session_recall instructions
        this.ensureSessionContinuityInstructions(path.join(this.workspaceRoot, ".claude", "CLAUDE.md"));
        // Auto-generate CLAUDE.md (project root) with session continuity
        this.ensureSessionContinuityInstructions(path.join(this.workspaceRoot, "CLAUDE.md"));
      }

      return result;
    } catch (error) {
      return {
        configPath: "",
        success: false,
        message: `Failed to generate config: ${error instanceof Error ? error.message : String(error)}`,
      };
    }
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

  /** Every MCP config file comP is known to have written, in a fixed order. */
  private repairTargets(): McpConfigTarget[] {
    const ws = (...segments: string[]) => path.join(this.workspaceRoot, ...segments);
    return [
      { path: ws(".vscode", "mcp.json"), serverKeys: ["servers", "comp"], scope: "workspace" },
      { path: ws(".mcp.json"), serverKeys: ["mcpServers", "comp"], scope: "workspace" },
      { path: ws(".comp", "config", "cursor_config.json"), serverKeys: ["comp"], scope: "workspace" },
      { path: ws(".comp", "config", "cline_config.json"), serverKeys: ["mcpServers", "comp"], scope: "workspace" },
      { path: ws(".comp", "config", "windsurf_config.json"), serverKeys: ["mcpServers", "comp"], scope: "workspace" },
      { path: this.antigravityConfigPath(), serverKeys: ["mcpServers", "comp"], scope: "global" },
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
        return this.repairTarget(target);
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
   * Generate Claude Code MCP configuration
   *
   * Writes to .mcp.json at the workspace root — the file Claude Code CLI
   * reads for project-scoped MCP servers. Merges with any existing entries
   * so other MCP servers the user has configured are preserved.
   */
  private generateClaudeCodeConfig(daemonPath: string): string {
    let existing: { mcpServers: Record<string, unknown> } = { mcpServers: {} };

    const fullPath = path.join(this.workspaceRoot, ".mcp.json");
    try {
      if (fs.existsSync(fullPath)) {
        const raw = fs.readFileSync(fullPath, "utf-8");
        const parsed = JSON.parse(raw);
        existing = { mcpServers: {}, ...parsed };
      }
    } catch {
      // File absent or invalid — start from scratch
    }

    existing.mcpServers["comp"] = {
      command: daemonPath,
      args: [],
      env: {
        COMP_WORKSPACE_ROOT: this.workspaceRoot,
        RUST_LOG: "info",
      },
    };

    return JSON.stringify(existing, null, 2);
  }

  private generateClaudeCodeCommand(daemonPath: string): string {
    const escapeQuotes = (value: string) => value.replace(/"/g, '\\"');
    const escapedPath = escapeQuotes(daemonPath);
    const escapedWorkspaceRoot = escapeQuotes(this.workspaceRoot);

    return `claude mcp add comp "${escapedPath}" -e COMP_WORKSPACE_ROOT="${escapedWorkspaceRoot}" -e RUST_LOG=info`;
  }

  private generateClaudeCodeLLMPrompt(daemonPath: string): string {
    const command = this.generateClaudeCodeCommand(daemonPath);
    return `プロジェクトルートの .mcp.json に comP MCP サーバーの設定を書き込みました。Claude Code でこのプロジェクトを開くと comP が自動的に利用可能になります。\n\nユーザーレベルで登録したい場合は次のコマンドを実行してください。\n\n${command}`;
  }

  /**
   * Generate Cursor MCP configuration
   */
  private generateCursorConfig(daemonPath: string): string {
    // Cursor uses .cursor/rules or environment variables
    // Format: CURSOR_MCP_SERVERS={"comp":{"command":"...","env":{...}}}
    const mcpConfig = {
      comp: {
        command: daemonPath,
        args: [],
        env: {
          COMP_WORKSPACE_ROOT: this.workspaceRoot,
          RUST_LOG: "info",
        },
      },
    };

    return JSON.stringify(mcpConfig, null, 2);
  }

  /**
   * Generate Cline MCP configuration
   */
  private generateClineConfig(daemonPath: string): string {
    const config = {
      mcpServers: {
        comp: {
          command: daemonPath,
          args: [],
          env: {
            COMP_WORKSPACE_ROOT: this.workspaceRoot,
            RUST_LOG: "info",
          },
        },
      },
    };

    return JSON.stringify(config, null, 2);
  }

  /**
   * Generate Windsurf MCP configuration
   */
  private generateWindsurfConfig(daemonPath: string): string {
    const config = {
      mcpServers: {
        comp: {
          command: daemonPath,
          args: [],
          env: {
            COMP_WORKSPACE_ROOT: this.workspaceRoot,
            RUST_LOG: "info",
          },
        },
      },
    };

    return JSON.stringify(config, null, 2);
  }

  /**
   * Generate Continue.dev MCP configuration (Python-based)
   */
  private generateContinueConfig(daemonPath: string): string {
    const escapePy = (s: string) => s.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    const pythonConfig = `
# Continue.dev MCP Server Configuration
# Add this to your continue/config.py

mcp_servers = {
    "comp": {
        "command": "${escapePy(daemonPath)}",
        "args": [],
        "env": {
            "COMP_WORKSPACE_ROOT": "${escapePy(this.workspaceRoot)}",
            "RUST_LOG": "info"
        }
    }
}
`;

    return pythonConfig;
  }

  /**
   * Configuration file paths for each agent
   */
  private claudeDesktopConfigPath(): string {
    return ".mcp.json";
  }

  private cursorConfigPath(): string {
    return ".comp/config/cursor_config.json";
  }

  private clineConfigPath(): string {
    return ".comp/config/cline_config.json";
  }

  private windsurfConfigPath(): string {
    return ".comp/config/windsurf_config.json";
  }

  private continueConfigPath(): string {
    return ".comp/config/continue_config.py";
  }

  /**
   * Antigravity MCP config path
   *
   * Antigravity (Google Gemini-based IDE) stores global MCP config at
   * ~/.gemini/antigravity-ide/mcp_config.json — absolute path so generateConfig
   * writes directly without prepending workspaceRoot.
   */
  private antigravityConfigPath(): string {
    return path.join(os.homedir(), ".gemini", "antigravity-ide", "mcp_config.json");
  }

  /**
   * Generate Antigravity MCP configuration
   *
   * Merges comP into the existing mcp_config.json rather than overwriting,
   * to preserve other MCP servers the user may have configured.
   */
  private generateAntigravityConfig(daemonPath: string): string {
    let existing: { mcpServers: Record<string, unknown> } = { mcpServers: {} };

    try {
      const raw = fs.readFileSync(this.antigravityConfigPath(), "utf-8");
      const parsed = JSON.parse(raw);
      existing = { mcpServers: {}, ...parsed };
    } catch {
      // File absent or invalid — start from scratch
    }

    existing.mcpServers["comp"] = {
      command: daemonPath,
      args: [],
      env: {
        COMP_WORKSPACE_ROOT: this.workspaceRoot,
        RUST_LOG: "info",
      },
    };

    return JSON.stringify(existing, null, 2);
  }

  private copilotConfigPath(): string {
    return ".vscode/mcp.json";
  }

  private aiderConfigPath(): string {
    return ".aider.conf.yml";
  }

  /**
   * Generate Aider MCP configuration
   *
   * Merges the comp MCP server entry into .aider.conf.yml using YAML append.
   * If the file already contains an mcp-servers block the user must merge manually
   * (YAML has no safe programmatic merge without a parser dependency).
   */
  private generateAiderConfig(daemonPath: string): string {
    const escapeYaml = (s: string) => s.replace(/\\/g, "\\\\");

    const block = `# comP MCP server configuration
# Generated by comP VSCode extension
#
# NOTE: Aider MCP support requires Aider v0.69.0+.
# Verify this format against: https://aider.chat/docs/config/aider_conf.html
# If the key "mcp-servers" is not recognized, check your Aider version and docs.
mcp-servers:
  comp:
    command: "${escapeYaml(daemonPath)}"
    args: []
    env:
      COMP_WORKSPACE_ROOT: "${escapeYaml(this.workspaceRoot)}"
      RUST_LOG: info
`;

    const fullPath = path.join(this.workspaceRoot, this.aiderConfigPath());
    try {
      if (fs.existsSync(fullPath)) {
        const existing = fs.readFileSync(fullPath, "utf-8");
        if (existing.includes("mcp-servers:")) {
          // Prepend a warning comment rather than silently overwriting
          return `# WARNING: mcp-servers block already exists. Merge the section below manually.\n\n${block}\n\n# --- existing config below ---\n${existing}`;
        }
        return existing.trimEnd() + "\n\n" + block;
      }
    } catch {
      // File absent or unreadable — start fresh
    }

    return block;
  }

  /**
   * Generate GitHub Copilot MCP configuration
   *
   * Writes to workspace .vscode/mcp.json, merging with existing config if present.
   */
  private generateCopilotConfig(daemonPath: string): string {
    let existing: { servers: Record<string, unknown> } = { servers: {} };

    const fullPath = path.join(this.workspaceRoot, this.copilotConfigPath());
    try {
      if (fs.existsSync(fullPath)) {
        const raw = fs.readFileSync(fullPath, "utf-8");
        const parsed = JSON.parse(raw);
        existing = { servers: {}, ...parsed };
      }
    } catch {
      // File absent or invalid — start from scratch
    }

    existing.servers["comp"] = {
      command: daemonPath,
      args: [],
      env: {
        COMP_WORKSPACE_ROOT: this.workspaceRoot,
        RUST_LOG: "info",
      },
    };

    return JSON.stringify(existing, null, 2);
  }
}
