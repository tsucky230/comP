import * as fs from "fs";
import * as path from "path";
import { bundledDaemonBinaryPath } from "../../config/daemonBinary";
import {
  AgentSetupManager,
  ClaudeRegistrationFix,
  CommandRunner,
  defaultCommandRunner,
  RepairEntry,
  UserScopeResult,
} from "../AgentSetup";
import { describeError, extensionFolderVersion, quoteForShell, readObjectPath, samePath } from "./text";

/**
 * Characters that change what a cmd.exe command line means.
 *
 * A daemon path containing one of these cannot be handed to the shell safely,
 * so registration is skipped and the user runs the command themselves.
 */
const SHELL_METACHARACTERS = /["&|<>^%]/;

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
export async function registerClaudeCodeUserScope(host: AgentSetupManager, runner?: CommandRunner): Promise<UserScopeResult> {
  const daemonPath = host.getDaemonPath();
  const command = host.generateClaudeCodeCommand(daemonPath);
  const run = runner ?? defaultCommandRunner;

  // The Windows runner goes through cmd.exe (see defaultCommandRunner), where
  // these characters change what the command line means. Rather than try to
  // quote around them, hand the command to the user and let them run it.
  if (process.platform === "win32" && SHELL_METACHARACTERS.test(daemonPath)) {
    return {
      registered: false,
      command,
      reason: host.t(
        "Skipped automatic registration: the daemon path contains shell metacharacters",
        "daemon のパスにシェル特殊文字が含まれるため、自動登録を見送りました"
      ),
    };
  }

  const version = await run("claude", ["--version"]);
  if (!version.ok) {
    return {
      registered: false,
      command,
      reason: host.t("claude CLI was not found", "claude CLI が見つかりませんでした"),
    };
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
    "-e",
    `COMP_AGENT_ID=${AgentSetupManager.agentIdFor("Claude Code")}`,
    "--",
    daemonPath,
  ];
  const added = await run("claude", args);
  if (!added.ok) {
    return {
      registered: false,
      command,
      reason: (
        added.stderr ||
        added.stdout ||
        host.t("claude mcp add failed", "claude mcp add に失敗しました")
      ).trim(),
    };
  }
  return { registered: true, command };
}

/** Only paths shipped by this VS Code extension are safe to refresh by version. */
export function isInstalledExtensionBinary(_host: AgentSetupManager, command: string): boolean {
  const extensionDir = path.dirname(path.dirname(path.dirname(path.dirname(command))));
  if (!/^tsucky230\.comp-vscode-\d+\.\d+\.\d+(?:[-.][^\\/]*)?$/.test(path.basename(extensionDir))) {
    return false;
  }
  return samePath(command, bundledDaemonBinaryPath(extensionDir));
}

/**
 * A comP extension binary older than the running extension: the only kind a
 * repair moves off while the file still exists.
 *
 * WHY older, not merely different: VS Code and Cursor (or Insiders) can both
 * carry comP, and each refreshing toward its own copy would rewrite the
 * other's choice on every start. Versions only move forward this way.
 * When the running extension's folder carries no version (an Extension
 * Development Host), any other installed copy counts as older, as before.
 */
export function isOlderInstalledExtensionBinary(host: AgentSetupManager, command: string): boolean {
  if (!host.isInstalledExtensionBinary(command)) return false;
  const theirs = extensionFolderVersion(path.dirname(path.dirname(path.dirname(path.dirname(command)))));
  const ours = host.extensionPath ? extensionFolderVersion(host.extensionPath) : null;
  if (!theirs || !ours) return true;
  for (let i = 0; i < 3; i++) {
    if (theirs[i] !== ours[i]) return theirs[i] < ours[i];
  }
  return false;
}

/**
 * Refresh Claude Code's registrations that point at an older comP extension:
 * the user-scope one and the per-project (local-scope) ones under `projects`.
 * Unlike a general JSON repair, this never adopts an unrelated command:
 * ~/.claude.json is shared across all projects and owned by Claude.
 *
 * WHY per-project too: a local-scope entry overrides the user scope, and older
 * comP versions wrote one per project. Left alone, those projects kept a path
 * into a deleted extension folder and silently lost comP after an upgrade.
 */
export function repairClaudeCodeUserScope(host: AgentSetupManager): RepairEntry {
  // WHY read, change and write back to back: Claude Code rewrites this file
  // often, and anything it saves between our read and our write is lost. This
  // direct write is the fallback; repairClaudeCodeRegistrations goes through
  // the CLI when it can.
  const plan = host.planClaudeCodeRepairs();
  if (plan.done) return plan.done;
  for (const fix of plan.fixes) {
    fix.server["command"] = plan.replacement;
  }
  host.backupIfExists(plan.file);
  host.atomicWrite(plan.file, JSON.stringify(plan.doc, null, 2) + "\n");
  return { file: plan.file, status: "repaired", from: plan.fixes[0].from, to: plan.replacement };
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
export async function repairClaudeCodeRegistrations(host: AgentSetupManager, runner: CommandRunner = defaultCommandRunner): Promise<RepairEntry> {
  const plan = host.planClaudeCodeRepairs();
  if (plan.done) return plan.done;
  if (!(await runner("claude", ["--version"])).ok) return host.repairClaudeCodeUserScope();

  const userFix = plan.fixes.find((fix) => fix.scope === "user");
  if (userFix) {
    const add = host.claudeAddArgs(userFix, plan.replacement);
    if (add) {
      await runner("claude", ["mcp", "remove", "comp", "--scope", "user"]);
      await runner("claude", add);
    }
    if (!readObjectPath(host.readClaudeConfig(), ["mcpServers", "comp"])) {
      // The remove landed but the add did not: put the registration back by hand.
      host.writeUserScopeRegistration({ ...userFix.server, command: plan.replacement });
    }
  }

  if (host.userScopeServesProjects(plan.replacement)) {
    for (const fix of plan.fixes) {
      if (fix.scope !== "local" || !fix.project || !fs.existsSync(fix.project)) continue;
      await runner("claude", ["mcp", "remove", "comp", "--scope", "local"], { cwd: fix.project });
    }
  }

  const after = host.planClaudeCodeRepairs();
  if (after.done?.status === "failed") return after.done;
  if (!after.done) {
    const fallback = host.repairClaudeCodeUserScope();
    if (fallback.status !== "repaired") return fallback;
  }
  return { file: plan.file, status: "repaired", from: plan.fixes[0].from, to: plan.replacement };
}

export function readClaudeConfig(host: AgentSetupManager): unknown {
  try {
    return JSON.parse(fs.readFileSync(path.join(host.homeDir, ".claude.json"), "utf-8"));
  } catch {
    return null;
  }
}

/** True when the user-scope registration runs the current (or a newer) comP. */
export function userScopeServesProjects(host: AgentSetupManager, replacement: string): boolean {
  const command = readObjectPath(host.readClaudeConfig(), ["mcpServers", "comp"])?.["command"];
  return typeof command === "string" && fs.existsSync(command) &&
    (samePath(command, replacement) ||
      (host.isInstalledExtensionBinary(command) && !host.isOlderInstalledExtensionBinary(command)));
}

/** Re-read ~/.claude.json and set the user-scope comP registration in it. */
export function writeUserScopeRegistration(host: AgentSetupManager, server: Record<string, unknown>): void {
  const file = path.join(host.homeDir, ".claude.json");
  const doc = host.readClaudeConfig();
  if (doc === null || typeof doc !== "object" || Array.isArray(doc)) return;
  const root = doc as Record<string, unknown>;
  const servers = readObjectPath(root, ["mcpServers"]) ?? {};
  servers["comp"] = server;
  root["mcpServers"] = servers;
  host.backupIfExists(file);
  host.atomicWrite(file, JSON.stringify(root, null, 2) + "\n");
}

/**
 * `claude mcp add` arguments that re-create this registration with `command`,
 * or null when the CLI cannot reproduce it exactly: fields other than
 * type/command/args/env, a non-stdio type, or — on Windows, where the runner
 * goes through cmd.exe — a value with shell metacharacters or a trailing
 * backslash (`"C:\a b\"` would escape its own closing quote).
 *
 * A user-scope registration gains COMP_AGENT_ID=claude-code when it lacks one:
 * the projects whose overrides are removed run on it from now on, and their
 * history would otherwise be recorded as "unknown".
 */
export function claudeAddArgs(_host: AgentSetupManager, fix: ClaudeRegistrationFix, command: string): string[] | null {
  const server = fix.server;
  if (Object.keys(server).some((key) => !["type", "command", "args", "env"].includes(key))) return null;
  if (server["type"] !== undefined && server["type"] !== "stdio") return null;
  const args = server["args"] ?? [];
  const env = server["env"] ?? {};
  if (!Array.isArray(args) || !args.every((a) => typeof a === "string")) return null;
  if (env === null || typeof env !== "object" || Array.isArray(env)) return null;
  const envPairs = Object.entries(env as Record<string, unknown>);
  if (!envPairs.every(([key, value]) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(key) && typeof value === "string")) {
    return null;
  }
  if (fix.scope === "user" && !envPairs.some(([key]) => key === "COMP_AGENT_ID")) {
    envPairs.push(["COMP_AGENT_ID", AgentSetupManager.agentIdFor("Claude Code")]);
  }
  const values = [command, ...(args as string[]), ...envPairs.map(([, value]) => value as string)];
  if (process.platform === "win32" &&
    values.some((value) => SHELL_METACHARACTERS.test(value) || (value.endsWith("\\") && quoteForShell(value) !== value))) {
    return null;
  }
  return [
    "mcp", "add", "--scope", fix.scope, "--transport", "stdio", "comp",
    ...envPairs.flatMap(([key, value]) => ["-e", `${key}=${value as string}`]),
    "--", command, ...(args as string[]),
  ];
}

/**
 * Read ~/.claude.json and list the comP registrations to move to the bundled
 * binary. `done` is set when there is nothing to apply (missing, unreadable,
 * healthy, or skipped) and is the entry to report as is.
 *
 * Only commands that are comP extension binaries are ever touched. The same or
 * a newer comP in another editor is left alone while its file exists.
 */
export function planClaudeCodeRepairs(host: AgentSetupManager): {
  file: string;
  done?: RepairEntry;
  doc?: unknown;
  replacement: string;
  fixes: ClaudeRegistrationFix[];
} {
  const file = path.join(host.homeDir, ".claude.json");
  const finish = (done: RepairEntry) => ({ file, done, replacement: "", fixes: [] });
  if (!fs.existsSync(file)) return finish({ file, status: "missing" });

  let doc: unknown;
  try {
    doc = JSON.parse(fs.readFileSync(file, "utf-8"));
  } catch (error) {
    return finish({ file, status: "failed", reason: `invalid JSON: ${describeError(error)}` });
  }
  const registrations: Array<{ scope: "user" | "local"; project?: string; server: Record<string, unknown> }> = [];
  const userServer = readObjectPath(doc, ["mcpServers", "comp"]);
  if (userServer) registrations.push({ scope: "user", server: userServer });
  const projects = readObjectPath(doc, ["projects"]);
  for (const key of Object.keys(projects ?? {})) {
    const projectServer = readObjectPath(projects, [key, "mcpServers", "comp"]);
    if (projectServer) registrations.push({ scope: "local", project: key, server: projectServer });
  }
  if (registrations.length === 0) return finish({ file, status: "skipped", reason: "no comp server entry" });

  const replacement = host.bundledDaemonPath();
  if (!replacement) return finish({ file, status: "skipped", reason: "no bundled binary available" });

  const fixes: ClaudeRegistrationFix[] = [];
  let healthy = false;
  let skipReason: string | undefined;
  for (const registration of registrations) {
    const command = registration.server["command"];
    if (typeof command !== "string" || !path.isAbsolute(command) || command.includes("${")) {
      skipReason ??= "command is not an absolute binary path";
      continue;
    }
    if (!host.isInstalledExtensionBinary(command)) {
      skipReason ??= "command is not a comP extension binary";
      continue;
    }
    if (samePath(command, replacement) ||
      (!host.isOlderInstalledExtensionBinary(command) && fs.existsSync(command))) {
      healthy = true;
      continue;
    }
    fixes.push({ ...registration, from: command });
  }

  if (fixes.length === 0) {
    return finish(healthy ? { file, status: "healthy" } : { file, status: "skipped", reason: skipReason });
  }
  return { file, doc, replacement, fixes };
}

/**
 * The `claude mcp add` line that registers comP for every project.
 *
 * `--` matters: without it the CLI parses the daemon path as its own
 * positional URL argument. `--scope user` matters too — the default is
 * `local`, which would register comP for the current project only and quietly
 * contradict what this command is offered for.
 */
export function generateClaudeCodeCommand(_host: AgentSetupManager, daemonPath: string): string {
  const quote = (value: string) => `"${value.replace(/"/g, '\\"')}"`;
  return [
    "claude mcp add --scope user --transport stdio comp",
    "-e RUST_LOG=info",
    `-e COMP_AGENT_ID=${AgentSetupManager.agentIdFor("Claude Code")}`,
    "--",
    quote(daemonPath),
  ].join(" ");
}
