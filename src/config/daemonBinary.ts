import * as fs from "fs";
import * as path from "path";

/** VS Code setting (under `comp.`) that lets a comP checkout run its own cargo build. */
export const USE_WORKSPACE_DAEMON_BUILD_SETTING = "useWorkspaceDaemonBuild";

/** Name of the binary the VSIX ships for a platform (one VSIX carries all three). */
export function bundledDaemonBinaryName(platform: NodeJS.Platform = process.platform): string {
  if (platform === "win32") return "comp-daemon-win.exe";
  if (platform === "darwin") return "comp-daemon-macos";
  return "comp-daemon-linux";
}

export function bundledDaemonBinaryPath(extensionPath: string, platform: NodeJS.Platform = process.platform): string {
  return path.join(extensionPath, "daemon", "target", "release", bundledDaemonBinaryName(platform));
}

/** cargo's output for this workspace: release first, then debug. */
export function workspaceDaemonBuildPaths(workspaceRoot: string, platform: NodeJS.Platform = process.platform): string[] {
  const name = platform === "win32" ? "comp-daemon.exe" : "comp-daemon";
  return ["release", "debug"].map((profile) => path.join(workspaceRoot, "daemon", "target", profile, name));
}

export interface ResolveDaemonBinaryOptions {
  extensionPath?: string;
  workspaceRoot?: string;
  /**
   * `comp.useWorkspaceDaemonBuild`. Off by default because the workspace build
   * is whatever file sits at daemon/target/…: in a comP checkout that is a
   * possibly stale local build (an upgraded extension kept running 0.11.11),
   * and in a cloned repository it is an executable the repository supplied.
   */
  useWorkspaceBuild: boolean;
  platform?: NodeJS.Platform;
  exists?: (file: string) => boolean;
}

/** The daemon binary to run, or null when none of the candidates exists. */
export function resolveDaemonBinary(options: ResolveDaemonBinaryOptions): string | null {
  const platform = options.platform ?? process.platform;
  const exists = options.exists ?? ((file: string) => fs.existsSync(file));
  if (options.useWorkspaceBuild && options.workspaceRoot) {
    const build = workspaceDaemonBuildPaths(options.workspaceRoot, platform).find(exists);
    if (build) return build;
  }
  if (options.extensionPath) {
    const bundled = bundledDaemonBinaryPath(options.extensionPath, platform);
    if (exists(bundled)) return bundled;
  }
  return null;
}

/**
 * Warning text when the running daemon is not the extension's own version, else null.
 *
 * A daemon without `daemon_version` predates 0.9.2, so it is reported too: an
 * unknown version cannot be assumed to understand the extension's requests
 * (a 0.11.11 daemon answers 0.12.0's `syncFolders` with "unknown method").
 */
export function describeDaemonVersionMismatch(
  extensionVersion: string,
  daemonVersion: string | undefined
): string | null {
  if (daemonVersion === extensionVersion) return null;
  return `daemon ${daemonVersion ?? "(version unknown)"} / extension ${extensionVersion}`;
}
