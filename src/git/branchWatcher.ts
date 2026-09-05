// Branch-switch detection for comP's auto-indexer.
//
// comP has no git awareness otherwise: the FileSystemWatcher-driven incremental
// indexer only reacts to individual file save/delete events, so a `git checkout`
// (hundreds of files changing at once, some appearing, some disappearing) is
// invisible to it as a distinct event. This module polls the repository's
// current ref and reports when it changes, so the caller can trigger a full
// re-index instead.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

/**
 * Read the workspace's current git ref, if any.
 *
 * Reads `.git/HEAD` directly rather than shelling out to `git` — this runs on
 * every poll tick, and a plain file read is far cheaper than spawning a process.
 *
 * Returns:
 * - the branch name, for an attached HEAD (`ref: refs/heads/<name>`)
 * - the raw commit SHA, for a detached HEAD — this is still a valid "did
 *   anything change" identity even though it isn't a branch name
 * - `null` when there is nothing to read: not a git repository, or `.git` is a
 *   worktree pointer file (`gitdir: ...`) rather than a real git directory.
 *   Resolving worktrees is out of scope here — the caller simply never sees a
 *   change reported in that case.
 */
export function readCurrentBranchRef(workspaceRoot: string): string | null {
  const headPath = path.join(workspaceRoot, ".git", "HEAD");
  let content: string;
  try {
    content = fs.readFileSync(headPath, "utf-8").trim();
  } catch {
    return null;
  }

  const refMatch = /^ref:\s*refs\/heads\/(.+)$/.exec(content);
  if (refMatch) {
    return refMatch[1];
  }
  // Detached HEAD: HEAD holds a raw commit SHA rather than a `ref:` line.
  return content.length > 0 ? content : null;
}

/**
 * Poll for a change in the workspace's current branch/ref and report it.
 *
 * WHY polling instead of a FileSystemWatcher on `.git/HEAD`: this mirrors
 * SidebarPanel's `startStatsRefresh` interval rather than the file-watcher
 * pattern used elsewhere in the extension, at the caller's request.
 *
 * The first tick never fires `onBranchChanged` — it only establishes the
 * baseline — so activation on an already-open workspace doesn't look like a
 * spurious branch switch.
 */
export function startBranchWatch(
  workspaceRoot: string,
  onBranchChanged: (newRef: string) => void,
  intervalMs = 3000
): vscode.Disposable {
  let lastRef = readCurrentBranchRef(workspaceRoot);

  const timer = setInterval(() => {
    const currentRef = readCurrentBranchRef(workspaceRoot);
    if (currentRef !== null && currentRef !== lastRef) {
      lastRef = currentRef;
      onBranchChanged(currentRef);
    } else if (currentRef !== null) {
      lastRef = currentRef;
    }
    // A momentarily unreadable HEAD (mid-checkout) is not treated as a
    // change — `lastRef` is left as-is so the next good read is compared
    // against the last known-good value, not against `null`.
  }, intervalMs);

  return {
    dispose: () => clearInterval(timer),
  };
}
