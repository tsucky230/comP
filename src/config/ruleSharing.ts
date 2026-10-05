// ruleSharing - the "rule sharing (beta)" switch shared with the daemon
//
// Mirrors `comp.ruleSharing.enabled` into `<workspace>/.comp/config.json`
// under `ruleSharing`, which the daemon checks on every run_pipeline and
// check_rule_conflicts call (daemon/src/rules/mod.rs::rule_sharing_enabled).
// Same merge rules as conversationRecording.ts.

import { SyncResult, syncConfigSection } from "./conversationRecording";

export interface RuleSharingSettings {
  /** Off by default: rule sharing is beta and opt-in. */
  enabled: boolean;
}

export const DEFAULT_RULE_SHARING: RuleSharingSettings = { enabled: false };

/** Mirror the setting into `.comp/config.json`, keeping every other key. */
export function syncRuleSharing(workspaceRoot: string, settings: RuleSharingSettings): SyncResult {
  return syncConfigSection(workspaceRoot, "ruleSharing", settings.enabled, (current) => ({
    ...current,
    enabled: settings.enabled,
  }));
}
