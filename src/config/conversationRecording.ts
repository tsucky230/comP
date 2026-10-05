// conversationRecording - the "conversation recording (beta)" switch shared with the daemon
//
// The VS Code settings `comp.conversationRecording.*` are the source of truth;
// this module mirrors them into `<workspace>/.comp/config.json` under
// `conversationRecording`, which `comp-daemon record-turn` reads before it
// records anything (daemon/src/mcp/record_turn.rs).
//
// WHY a vscode-free module: the merge rules are what can go wrong (losing the
// user's other keys, clobbering a file that failed to parse), and keeping them
// out of extension.ts lets mocha test them directly.

import * as fs from "fs";
import * as path from "path";

export interface ConversationRecordingSettings {
  /** Master switch. Off by default: recording is beta and opt-in. */
  enabled: boolean;
  /** Claude Code's Stop hook; only matters while `enabled` is true. */
  claudeCode: boolean;
}

export const DEFAULT_CONVERSATION_RECORDING: ConversationRecordingSettings = {
  enabled: false,
  claudeCode: true,
};

/**
 * - `written`   — config.json now holds the given settings
 * - `unchanged` — it already did (or recording is off and there is no file), nothing written
 * - `invalid`   — config.json exists but is not a JSON object; left untouched
 */
export type SyncResult =
  | { status: "written"; path: string }
  | { status: "unchanged"; path: string }
  | { status: "invalid"; path: string; reason: string };

/**
 * Mirror the settings into `.comp/config.json`, keeping every other key.
 *
 * Never throws for an unreadable file — it reports `invalid` so the caller can
 * warn, because silently replacing it would wipe the user's other settings
 * (exclude, max_nodes, ...). Write errors do throw.
 */
export function syncConversationRecording(
  workspaceRoot: string,
  settings: ConversationRecordingSettings
): SyncResult {
  return syncConfigSection(workspaceRoot, "conversationRecording", settings.enabled, (current) =>
    section(current, settings)
  );
}

/**
 * Merge one top-level section of `.comp/config.json`, shared by every beta
 * switch so they all follow the same rules: other keys are kept, nothing is
 * written when the section is already equal, a missing file is created only
 * to switch something on, and an unreadable file is reported, never replaced.
 *
 * `build` receives the current section (or `{}` when absent or malformed).
 */
export function syncConfigSection(
  workspaceRoot: string,
  key: string,
  enabled: boolean,
  build: (current: Record<string, unknown>) => Record<string, unknown>
): SyncResult {
  const configPath = path.join(workspaceRoot, ".comp", "config.json");

  if (!fs.existsSync(configPath)) {
    // A missing file already reads as "off" to the daemon; creating .comp/
    // just to say so would litter workspaces that never use comP.
    if (!enabled) {
      return { status: "unchanged", path: configPath };
    }
    fs.mkdirSync(path.dirname(configPath), { recursive: true });
    fs.writeFileSync(configPath, JSON.stringify({ [key]: build({}) }, null, 2) + "\n", "utf-8");
    return { status: "written", path: configPath };
  }

  let doc: unknown;
  try {
    doc = JSON.parse(fs.readFileSync(configPath, "utf-8"));
  } catch (error) {
    return { status: "invalid", path: configPath, reason: error instanceof Error ? error.message : String(error) };
  }
  if (!isPlainObject(doc)) {
    return { status: "invalid", path: configPath, reason: "not a JSON object" };
  }

  const current = doc[key];
  const next = build(isPlainObject(current) ? current : {});
  if (JSON.stringify(current) === JSON.stringify(next)) {
    return { status: "unchanged", path: configPath };
  }
  doc[key] = next;
  fs.writeFileSync(configPath, JSON.stringify(doc, null, 2) + "\n", "utf-8");
  return { status: "written", path: configPath };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/**
 * The section record-turn reads. Other agents' entries survive so that a
 * future version's settings are not wiped by this one.
 */
function section(current: Record<string, unknown>, settings: ConversationRecordingSettings): Record<string, unknown> {
  const agents = isPlainObject(current["agents"]) ? { ...current["agents"] } : {};
  agents["claude-code"] = settings.claudeCode;
  return { ...current, enabled: settings.enabled, agents };
}
