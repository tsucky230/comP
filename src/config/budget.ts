// budget - `comp.maxContextTokens` shared with the daemon
//
// run_pipeline falls back to `.comp/config.json`'s `default_budget_tokens` when
// the agent passes no `max_tokens` (daemon/src/mcp/mod.rs::load_default_budget).
// Every agent spawns its own daemon and all of them read that file, so the
// setting is mirrored there instead of being handed to the extension's daemon only.

import * as fs from "fs";
import * as path from "path";

export const BUDGET_KEY = "default_budget_tokens";

export interface BudgetSyncResult {
  status: "written" | "unchanged" | "invalid";
  path: string;
  /** Why the file could not be used; only for `invalid`. */
  reason?: string;
  /** The value comP now owns in the file — pass it back as `lastSynced` next time. */
  synced: number | undefined;
}

/**
 * Mirror the setting into `.comp/config.json`, keeping every other key.
 *
 * - `explicit`: the value the user set in any scope, `undefined` while only the
 *   default applies. A number below 1 (or not finite) counts as unset.
 * - `lastSynced`: what this function wrote last time for this workspace.
 *
 * Rules:
 * - No `.comp/` folder: nothing is created, because a user-level setting would
 *   otherwise put `.comp/` into every folder VS Code opens.
 * - Unreadable file or not a JSON object: `invalid`, never replaced.
 * - Usable `explicit`: written as an integer (rounded down) unless already equal.
 * - Otherwise the setting was cleared: the value is removed only when it is the
 *   one comP wrote (`lastSynced`); a value written by hand stays.
 */
export function syncBudget(
  workspaceRoot: string,
  explicit: number | undefined,
  lastSynced: number | undefined
): BudgetSyncResult {
  const compDir = path.join(workspaceRoot, ".comp");
  const configPath = path.join(compDir, "config.json");
  const wanted =
    typeof explicit === "number" && Number.isFinite(explicit) && explicit >= 1 ? Math.floor(explicit) : undefined;

  if (!fs.existsSync(compDir)) {
    return { status: "unchanged", path: configPath, synced: lastSynced };
  }

  let doc: Record<string, unknown> = {};
  if (fs.existsSync(configPath)) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(fs.readFileSync(configPath, "utf-8"));
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      return { status: "invalid", path: configPath, reason, synced: lastSynced };
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { status: "invalid", path: configPath, reason: "not a JSON object", synced: lastSynced };
    }
    doc = parsed as Record<string, unknown>;
  } else if (wanted === undefined) {
    return { status: "unchanged", path: configPath, synced: undefined };
  }

  const current = doc[BUDGET_KEY];
  if (wanted !== undefined) {
    if (current === wanted) {
      return { status: "unchanged", path: configPath, synced: wanted };
    }
    doc[BUDGET_KEY] = wanted;
  } else {
    if (lastSynced === undefined || current !== lastSynced) {
      return { status: "unchanged", path: configPath, synced: undefined };
    }
    delete doc[BUDGET_KEY];
  }
  fs.writeFileSync(configPath, JSON.stringify(doc, null, 2) + "\n", "utf-8");
  return { status: "written", path: configPath, synced: wanted };
}
