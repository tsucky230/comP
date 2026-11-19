import * as fs from "fs";
import * as path from "path";
import type { AgentSetupManager, RepairEntry, WriteOutcome } from "../AgentSetup";
import { loadHookSettings, recordTurnHooks } from "./hookSettings";
import { describeError, samePath } from "./text";

/**
 * Characters that make a quoted path unsafe in a hook command line.
 *
 * Wider than SHELL_METACHARACTERS because the hook may run under Git Bash,
 * where `$` and backticks still expand inside double quotes.
 */
const HOOK_UNSAFE_CHARACTERS = /["&|<>^%$`]/;

/** Claude Code hook timeout, in seconds. record-turn reads one file and appends one line. */
const HOOK_TIMEOUT_SECONDS = 10;

/** Where setup puts the Claude Code Stop hook: machine-specific, so never the shared settings.json. */
export function historyHookFile(host: AgentSetupManager): string {
  return path.join(host.workspaceRoot, ".claude", "settings.local.json");
}

/**
 * The hook command line: both paths quoted and with forward slashes.
 *
 * WHY forward slashes: Claude Code runs hook commands through a shell, which
 * on Windows may be Git Bash, where backslashes inside double quotes are
 * fragile; cmd.exe and PowerShell accept forward slashes just as well.
 */
export function historyHookCommand(host: AgentSetupManager, daemonPath: string): string {
  const fwd = (p: string) => p.replace(/\\/g, "/");
  return `"${fwd(daemonPath)}" record-turn "${fwd(host.workspaceRoot)}"`;
}

/**
 * Install the Stop hook that records each Claude Code turn into .comp/history.
 *
 * Never throws. Skips (rather than fails) when installing would be wrong:
 * a path the shell would misread, a binary that does not exist yet, or a
 * project that already records through its own history-record hook — adding
 * ours there would write every turn twice.
 */
export function ensureHistoryHook(host: AgentSetupManager, daemonPath: string): WriteOutcome {
  const file = host.historyHookFile();
  const base = { path: file, scope: "workspace" as const };

  if (HOOK_UNSAFE_CHARACTERS.test(daemonPath) || HOOK_UNSAFE_CHARACTERS.test(host.workspaceRoot)) {
    return { ...base, status: "skipped", reason: "the daemon or workspace path contains shell metacharacters" };
  }
  if (!fs.existsSync(daemonPath)) {
    return { ...base, status: "skipped", reason: `daemon binary not found: ${daemonPath}` };
  }
  for (const name of ["settings.json", "settings.local.json"]) {
    const candidate = path.join(host.workspaceRoot, ".claude", name);
    if (fs.existsSync(candidate) && fs.readFileSync(candidate, "utf-8").includes("history-record")) {
      return { ...base, status: "skipped", reason: `${name} already records history with history-record` };
    }
  }

  try {
    const text = fs.existsSync(file) ? fs.readFileSync(file, "utf-8") : null;
    const { doc, stop } = loadHookSettings(text);
    const command = host.historyHookCommand(daemonPath);
    const existing = recordTurnHooks(stop);

    if (existing.length === 1 && existing[0]["command"] === command) {
      return { ...base, status: "skipped", reason: "the record-turn hook is already installed" };
    }
    if (existing.length > 0) {
      existing[0]["command"] = command;
      // Extra copies would record every turn more than once.
      for (const group of stop) {
        const inner = (group as Record<string, unknown> | null)?.["hooks"];
        if (Array.isArray(inner)) {
          (group as Record<string, unknown>)["hooks"] = inner.filter(
            (h) => h === existing[0] || !existing.includes(h as Record<string, unknown>)
          );
        }
      }
    } else {
      stop.push({ hooks: [{ type: "command", command, timeout: HOOK_TIMEOUT_SECONDS }] });
    }

    const backupPath = host.backupIfExists(file);
    host.atomicWrite(file, JSON.stringify(doc, null, 2) + "\n");
    return { ...base, status: "written", ...(backupPath ? { backupPath } : {}) };
  } catch (error) {
    return { ...base, status: "failed", reason: describeError(error) };
  }
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
export function historyHookInstalled(host: AgentSetupManager): boolean {
  const read = (name: string): string | null => {
    const file = path.join(host.workspaceRoot, ".claude", name);
    try {
      return fs.existsSync(file) ? fs.readFileSync(file, "utf-8") : null;
    } catch {
      return null;
    }
  };
  const shared = read("settings.json");
  const local = read("settings.local.json");
  if ([shared, local].some((text) => text !== null && text.includes("history-record"))) {
    return true;
  }
  if (local === null) {
    return false;
  }
  try {
    return recordTurnHooks(loadHookSettings(local).stop).length > 0;
  } catch {
    return false;
  }
}

/**
 * Remove comP's record-turn Stop hook from `.claude/settings.local.json`.
 * Backs up first, keeps every other hook and key, never touches settings.json.
 */
export function removeHistoryHook(host: AgentSetupManager): WriteOutcome {
  const file = host.historyHookFile();
  const base = { path: file, scope: "workspace" as const };
  if (!fs.existsSync(file)) {
    return { ...base, status: "skipped", reason: "no .claude/settings.local.json" };
  }
  try {
    const { doc, stop } = loadHookSettings(fs.readFileSync(file, "utf-8"));
    const targets = recordTurnHooks(stop);
    if (targets.length === 0) {
      return { ...base, status: "skipped", reason: "no record-turn hook" };
    }

    const remaining = stop.flatMap((group) => {
      const inner = (group as Record<string, unknown> | null)?.["hooks"];
      if (!Array.isArray(inner)) return [group];
      const kept = inner.filter((h) => !targets.includes(h as Record<string, unknown>));
      // Drop only groups that this removal emptied; an already-empty group is the user's.
      if (kept.length === 0 && inner.length > 0) return [];
      return [{ ...(group as Record<string, unknown>), hooks: kept }];
    });
    const hooks = doc["hooks"] as Record<string, unknown>;
    if (remaining.length > 0) {
      hooks["Stop"] = remaining;
    } else {
      delete hooks["Stop"];
    }
    if (Object.keys(hooks).length === 0) {
      delete doc["hooks"];
    }

    const backupPath = host.backupIfExists(file);
    host.atomicWrite(file, JSON.stringify(doc, null, 2) + "\n");
    return { ...base, status: "written", ...(backupPath ? { backupPath } : {}) };
  } catch (error) {
    return { ...base, status: "failed", reason: describeError(error) };
  }
}

export function repairHistoryHook(host: AgentSetupManager): RepairEntry {
  const file = host.historyHookFile();
  if (!fs.existsSync(file)) {
    return { file, status: "missing" };
  }

  let loaded: { doc: Record<string, unknown>; stop: unknown[] };
  try {
    loaded = loadHookSettings(fs.readFileSync(file, "utf-8"));
  } catch (error) {
    return { file, status: "failed", reason: `invalid settings: ${describeError(error)}` };
  }

  const hook = recordTurnHooks(loaded.stop)[0];
  if (!hook) {
    return { file, status: "skipped", reason: "no record-turn hook" };
  }
  const command = hook["command"] as string;
  const match = /^"([^"]+)" record-turn(?: "([^"]*)")?\s*$/.exec(command);
  if (!match) {
    return { file, status: "skipped", reason: "record-turn command is not in the form setup writes" };
  }

  const exeBroken = !fs.existsSync(match[1]);
  const available = host.resolveRepairPath("workspace");
  // An older installed extension's binary outlives the upgrade for a while.
  const exeOutdated = available !== null &&
    host.isOlderInstalledExtensionBinary(match[1]) &&
    !samePath(match[1], available);
  const wsStale = match[2] !== host.workspaceRoot.replace(/\\/g, "/");
  if (!exeBroken && !exeOutdated && !wsStale) {
    return { file, status: "healthy" };
  }
  const exe = exeBroken || exeOutdated ? available : match[1];
  if (!exe) {
    return { file, status: "skipped", reason: "no replacement binary available" };
  }
  if (HOOK_UNSAFE_CHARACTERS.test(exe) || HOOK_UNSAFE_CHARACTERS.test(host.workspaceRoot)) {
    return { file, status: "skipped", reason: "the daemon or workspace path contains shell metacharacters" };
  }

  const replacement = host.historyHookCommand(exe);
  hook["command"] = replacement;
  host.atomicWrite(file, JSON.stringify(loaded.doc, null, 2) + "\n");
  return { file, status: "repaired", from: command, to: replacement };
}
