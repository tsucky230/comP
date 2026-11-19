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
export function compRuleSnippet(): string {
  return [
    "## comP MCP Tool Usage",
    "",
    "At the start of every coding or documentation task, ALWAYS call `run_pipeline` FIRST.",
    "Do NOT read files, run grep/find/Bash searches, or explore the codebase manually before calling run_pipeline.",
    "run_pipeline indexes the entire codebase and returns exactly the relevant files — use it every time.",
  ].join("\n");
}
