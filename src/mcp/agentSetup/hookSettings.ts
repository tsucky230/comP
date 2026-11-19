/**
 * Read settings.local.json for a hook edit: the parsed document plus its
 * hooks.Stop array, created empty when absent. Throws on invalid JSON or a
 * shape Claude Code itself would not accept, so the caller never rewrites a
 * file it does not understand.
 */
export function loadHookSettings(text: string | null): { doc: Record<string, unknown>; stop: unknown[] } {
  const parsed: unknown = text === null ? {} : JSON.parse(text);
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("settings file is not a JSON object");
  }
  const doc = parsed as Record<string, unknown>;
  const hooks = doc["hooks"] ?? {};
  if (hooks === null || typeof hooks !== "object" || Array.isArray(hooks)) {
    throw new Error("\"hooks\" is not an object");
  }
  const stop = (hooks as Record<string, unknown>)["Stop"] ?? [];
  if (!Array.isArray(stop)) {
    throw new Error("\"hooks.Stop\" is not an array");
  }
  doc["hooks"] = hooks;
  (hooks as Record<string, unknown>)["Stop"] = stop;
  return { doc, stop };
}

/** Every `{type: "command", command}` object under hooks.Stop that runs record-turn. */
export function recordTurnHooks(stop: unknown[]): Record<string, unknown>[] {
  const found: Record<string, unknown>[] = [];
  for (const group of stop) {
    const inner = (group as Record<string, unknown> | null)?.["hooks"];
    if (!Array.isArray(inner)) continue;
    for (const hook of inner) {
      const command = (hook as Record<string, unknown> | null)?.["command"];
      if (typeof command === "string" && /(^|\s)record-turn(\s|$)/.test(command)) {
        found.push(hook as Record<string, unknown>);
      }
    }
  }
  return found;
}
