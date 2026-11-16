// workspaceFolders - tells each workspace folder's index about the others
//
// In a multi-root workspace every folder that uses comP has its own
// `.comp/index.db`, and an agent started in one folder only reaches that one.
// This module lists the *other* folders in `<folder>/.comp/config.json` under
// `workspace_folders`; the daemon indexes them into the same database
// (daemon/src/indexer/roots.rs). `additional_paths`, the hand-written
// equivalent, is never touched.

import * as fs from "fs";
import * as path from "path";
import { SyncResult } from "./conversationRecording";

const KEY = "workspace_folders";

/** `skipped`: the folder has no `.comp/` directory, so comP is not in use there. */
export type FolderSyncResult = SyncResult | { status: "skipped"; path: string };

/**
 * The folders other than `self`, in the given order, without duplicates.
 *
 * Two paths are the same folder when they are equal after `resolve` from
 * that platform's path module (`path.win32` for `win32`, `path.posix`
 * otherwise); on `win32` the comparison also ignores case (`e:\dev\app` is
 * `E:\Dev\App`). `platform` exists so both rules can be tested on any OS.
 */
export function otherFolders(
  folders: readonly string[],
  self: string,
  platform: NodeJS.Platform = process.platform
): string[] {
  const pathModule = platform === "win32" ? path.win32 : path.posix;
  const keyOf = (p: string): string => {
    const resolved = pathModule.resolve(p);
    return platform === "win32" ? resolved.toLowerCase() : resolved;
  };
  const seen = new Set<string>([keyOf(self)]);
  const result: string[] = [];
  for (const folder of folders) {
    const key = keyOf(folder);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(folder);
  }
  return result;
}

/**
 * Write `workspace_folders` into every folder's `.comp/config.json`.
 *
 * For each folder, the wanted list is `otherFolders(folders, folder)` when
 * `enabled`, and `[]` otherwise. One result per folder, in the given order:
 *
 * - `skipped`   — no `.comp/` directory; nothing is created.
 * - `written`   — the key now holds the wanted list. Every other key keeps
 *   its value. A missing config.json is created only for a non-empty list.
 * - `unchanged` — the key already holds the wanted list, or the wanted list
 *   is empty and the key (or the whole file) is absent. The file is not
 *   rewritten, so its formatting survives.
 * - `invalid`   — config.json exists but is not a JSON object; left untouched.
 *
 * A key holding anything other than the wanted list (including a non-array)
 * is replaced. Paths are written as given. Write errors throw.
 */
export function syncWorkspaceFolders(folders: readonly string[], enabled: boolean): FolderSyncResult[] {
  return folders.map((folder) => syncOne(folder, enabled ? otherFolders(folders, folder) : []));
}

function syncOne(folder: string, wanted: string[]): FolderSyncResult {
  const compDir = path.join(folder, ".comp");
  if (!fs.existsSync(compDir) || !fs.statSync(compDir).isDirectory()) {
    return { status: "skipped", path: folder };
  }
  const configPath = path.join(compDir, "config.json");

  if (!fs.existsSync(configPath)) {
    if (wanted.length === 0) return { status: "unchanged", path: configPath };
    fs.writeFileSync(configPath, JSON.stringify({ [KEY]: wanted }, null, 2) + "\n", "utf-8");
    return { status: "written", path: configPath };
  }

  let doc: unknown;
  try {
    doc = JSON.parse(fs.readFileSync(configPath, "utf-8"));
  } catch (error) {
    return { status: "invalid", path: configPath, reason: error instanceof Error ? error.message : String(error) };
  }
  if (doc === null || typeof doc !== "object" || Array.isArray(doc)) {
    return { status: "invalid", path: configPath, reason: "not a JSON object" };
  }
  const obj = doc as Record<string, unknown>;
  const current = obj[KEY];
  if (current === undefined ? wanted.length === 0 : JSON.stringify(current) === JSON.stringify(wanted)) {
    return { status: "unchanged", path: configPath };
  }
  obj[KEY] = wanted;
  fs.writeFileSync(configPath, JSON.stringify(obj, null, 2) + "\n", "utf-8");
  return { status: "written", path: configPath };
}
