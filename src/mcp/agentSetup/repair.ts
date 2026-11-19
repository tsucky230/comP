import * as fs from "fs";
import * as path from "path";
import { AgentSetupManager, McpConfigTarget, RepairEntry } from "../AgentSetup";
import {
  decodeTomlString,
  describeError,
  findCompSection,
  isSelfContained,
  maskComments,
  readObjectPath,
  samePath,
} from "./text";

/**
 * Every MCP config file comP is known to have written, in a fixed order.
 *
 * The `.comp/config/*.json` entries are kept even though setup no longer
 * writes there: a user who copied one of those files into place before this
 * version still has a daemon path that goes stale on every upgrade.
 */
export function repairTargets(host: AgentSetupManager): McpConfigTarget[] {
  const ws = (...segments: string[]) => path.join(host.workspaceRoot, ...segments);
  const home = (...segments: string[]) => path.join(host.homeDir, ...segments);
  const cline = host.clineSettingsPath();
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
    json(ws(".gemini", "settings.json"), ["mcpServers", "comp"], "workspace"),
    json(home(".gemini", "settings.json"), ["mcpServers", "comp"], "global"),
    json(home(".codeium", "windsurf", "mcp_config.json"), ["mcpServers", "comp"], "global"),
    ...(cline ? [json(cline, ["mcpServers", "comp"], "global")] : []),
    json(host.antigravityConfigPath(), ["mcpServers", "comp"], "global"),
    json(host.legacyAntigravityConfigPath(), ["mcpServers", "comp"], "global"),
    // Codex keeps its servers in TOML, so these go through repairTomlTarget.
    {
      path: path.join(host.codexHome(), "config.toml"),
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
export function repairStaleConfigs(host: AgentSetupManager, options?: { skipClaudeCodeConfig?: boolean }): RepairEntry[] {
  const entries = host.repairTargets().map((target) => {
    try {
      return target.format === "toml" ? host.repairTomlTarget(target) : host.repairTarget(target);
    } catch (error) {
      return {
        file: target.path,
        status: "failed" as const,
        reason: error instanceof Error ? error.message : String(error),
      };
    }
  });
  try {
    // The extension passes skipClaudeCodeConfig and runs repairClaudeCodeRegistrations
    // (the CLI route) instead; this direct write stays for every other caller.
    if (!options?.skipClaudeCodeConfig) entries.push(host.repairClaudeCodeUserScope());
  } catch (error) {
    entries.push({
      file: path.join(host.homeDir, ".claude.json"),
      status: "failed",
      reason: describeError(error),
    });
  }
  try {
    entries.push(host.repairHistoryHook());
  } catch (error) {
    entries.push({ file: host.historyHookFile(), status: "failed", reason: describeError(error) });
  }
  return entries;
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
 * 6. command resolves to an existing file — rewrite only if it belongs to an
 *    older installed version of this extension
 * 7. rewrite only if a replacement binary actually exists
 *
 * COMP_WORKSPACE_ROOT is refreshed in the same pass for workspace-scoped files
 * only, since a global file legitimately points at a different project. Steps 4
 * and 5 stop before that refresh: a config deliberately made portable should be
 * left portable in both fields.
 */
export function repairTarget(host: AgentSetupManager, target: McpConfigTarget): RepairEntry {
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
  const availableReplacement = host.resolveRepairPath(target.scope);
  const installedExtensionChanged = availableReplacement !== null &&
    host.isOlderInstalledExtensionBinary(command) &&
    !samePath(command, availableReplacement);
  if (commandBroken || installedExtensionChanged) {
    replacement = availableReplacement;
    if (!replacement) {
      // Swapping one broken path for another would only hide the failure.
      return { file, status: "skipped", reason: "no replacement binary available" };
    }
  }

  const envRepaired = target.scope === "workspace" && host.refreshWorkspaceRootEnv(server);

  if (!replacement && !envRepaired) {
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
export function repairTomlTarget(host: AgentSetupManager, target: McpConfigTarget): RepairEntry {
  const file = target.path;

  if (!fs.existsSync(file)) {
    return { file, status: "missing" };
  }

  const text = fs.readFileSync(file, "utf-8");
  const section = findCompSection(text);
  if (!section) {
    return { file, status: "skipped", reason: "no comp server entry" };
  }

  const block = text.slice(section.start, section.end);
  // Same guard as the writer: a section ending with brackets still open was
  // cut short by a nested array element, and splicing it would destroy the
  // rest of the file.
  if (!isSelfContained(block)) {
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

  // Same rule as the JSON targets: an older installed extension's binary still
  // exists right after an upgrade, so "broken" alone would keep it in use.
  const commandBroken = !fs.existsSync(command);
  let replacement: string | null = null;
  const availableReplacement = host.resolveRepairPath(target.scope);
  const installedExtensionChanged = availableReplacement !== null &&
    host.isOlderInstalledExtensionBinary(command) &&
    !samePath(command, availableReplacement);
  if (commandBroken || installedExtensionChanged) {
    replacement = availableReplacement;
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
    target.scope === "workspace" ? host.refreshWorkspaceRootToml(repairedBlock) : null;
  const envRepaired = withFreshRoot !== null;
  if (withFreshRoot !== null) {
    repairedBlock = withFreshRoot;
  }

  if (!replacement && !envRepaired) {
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
export function refreshWorkspaceRootToml(host: AgentSetupManager, block: string): string | null {
  const masked = maskComments(block);
  const match = /(COMP_WORKSPACE_ROOT\s*=\s*)('[^'\n]*'|"(?:[^"\\\n]|\\.)*")/.exec(masked);
  if (!match) {
    return null;
  }
  const current = decodeTomlString(match[2]);
  if (current.length === 0 || current.includes("${") || !path.isAbsolute(current)) {
    return null;
  }
  if (path.resolve(current) === path.resolve(host.workspaceRoot)) {
    return null;
  }
  return (
    block.slice(0, match.index) +
    match[1] +
    AgentSetupManager.tomlQuote(host.workspaceRoot) +
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
export function refreshWorkspaceRootEnv(host: AgentSetupManager, server: Record<string, unknown>): boolean {
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
  if (path.resolve(current) === path.resolve(host.workspaceRoot)) {
    return false;
  }

  envRecord["COMP_WORKSPACE_ROOT"] = host.workspaceRoot;
  return true;
}
