// daemonEnv - the environment of the daemon the extension starts
//
// The daemon refuses additional roots in the home directory's hidden folders
// (`~/.ssh`, `~/AppData`, …) unless COMP_ALLOW_HIDDEN_HOME_ROOTS is `1`/`true`
// (daemon/src/indexer/roots.rs::HomePolicy). `comp.blockHiddenHomeFolders`
// is the VS Code face of that switch, for the extension's own daemon only:
// daemons that agents start read their MCP config's env instead.

/** Read by the daemon; `1` or `true` turns the protection off. */
export const ALLOW_HIDDEN_HOME_ROOTS_ENV = "COMP_ALLOW_HIDDEN_HOME_ROOTS";

/**
 * Environment for the extension's daemon. Never mutates `base`.
 *
 * - Copies `base`, then sets `COMP_WORKSPACE_ROOT` to `workspaceRoot` and
 *   `RUST_LOG` to `info`.
 * - `blockHiddenHomeFolders` true: every key equal to
 *   `COMP_ALLOW_HIDDEN_HOME_ROOTS` ignoring case is removed, so a value the
 *   editor inherited cannot override the setting (Windows looks environment
 *   names up ignoring case).
 * - false: those keys are removed as well, then the variable is set to `"1"`.
 */
export function buildDaemonEnv(
  base: NodeJS.ProcessEnv,
  workspaceRoot: string,
  blockHiddenHomeFolders: boolean
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [key, value] of Object.entries(base)) {
    if (key.toUpperCase() !== ALLOW_HIDDEN_HOME_ROOTS_ENV) {
      env[key] = value;
    }
  }
  env["COMP_WORKSPACE_ROOT"] = workspaceRoot;
  env["RUST_LOG"] = "info";
  if (!blockHiddenHomeFolders) {
    env[ALLOW_HIDDEN_HOME_ROOTS_ENV] = "1";
  }
  return env;
}
