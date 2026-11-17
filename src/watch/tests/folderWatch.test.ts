// watchFolders: batching folder-level create/delete events into syncFolders calls.
// Owned by the designer role; implementers must not edit.

import { expect } from "chai";
import * as sinon from "sinon";
import { watchFolders, WatchedUri } from "../folderWatch";

type Listener = (uri: WatchedUri) => unknown;

class FakeWatcher {
  private listeners: Record<"create" | "delete", Listener[]> = { create: [], delete: [] };
  disposed = 0;

  private on(kind: "create" | "delete", l: Listener) {
    this.listeners[kind].push(l);
    return { dispose: () => { this.disposed++; this.listeners[kind] = this.listeners[kind].filter((x) => x !== l); } };
  }
  onDidCreate = (l: Listener) => this.on("create", l);
  onDidDelete = (l: Listener) => this.on("delete", l);

  create(p: string) { this.listeners.create.forEach((l) => l({ fsPath: p })); }
  delete(p: string) { this.listeners.delete.forEach((l) => l({ fsPath: p })); }
}

describe("watchFolders", () => {
  let clock: sinon.SinonFakeTimers;
  let watcher: FakeWatcher;
  let synced: string[][];
  let statted: string[];
  let errors: unknown[];
  let dirs: Set<string>;
  let statFails: Set<string>;
  let gate: (() => void) | null;
  let failNextSync: boolean;

  const options = (extra: { delayMs?: number; gated?: boolean } = {}) => ({
    delayMs: extra.delayMs,
    isExcluded: (u: WatchedUri) => u.fsPath.includes("/node_modules/") || u.fsPath.includes("/."),
    isSourceFile: (u: WatchedUri) => u.fsPath.endsWith(".ts"),
    isDirectory: async (u: WatchedUri) => {
      statted.push(u.fsPath);
      if (statFails.has(u.fsPath)) throw new Error("stat failed");
      return dirs.has(u.fsPath);
    },
    sync: (paths: string[]) => {
      synced.push(paths);
      if (failNextSync) {
        failNextSync = false;
        return Promise.reject(new Error("daemon down"));
      }
      if (!extra.gated) return Promise.resolve();
      return new Promise<void>((resolve) => { gate = resolve; });
    },
    onError: (e: unknown) => { errors.push(e); },
  });

  beforeEach(() => {
    clock = sinon.useFakeTimers();
    watcher = new FakeWatcher();
    synced = [];
    statted = [];
    errors = [];
    dirs = new Set();
    statFails = new Set();
    gate = null;
    failNextSync = false;
  });

  afterEach(() => clock.restore());

  it("sends a deleted folder after the default 1000 ms, not before", async () => {
    watchFolders(watcher, options());
    watcher.delete("/w/src/old");

    await clock.tickAsync(999);
    expect(synced).to.deep.equal([]);
    await clock.tickAsync(1);
    expect(synced).to.deep.equal([["/w/src/old"]]);
    expect(statted, "a deleted path is not inspected").to.deep.equal([]);
  });

  it("sends a created directory and ignores a created file", async () => {
    dirs.add("/w/src/new");
    watchFolders(watcher, options({ delayMs: 100 }));
    watcher.create("/w/src/new");
    watcher.create("/w/notes.txt");

    await clock.tickAsync(100);
    expect(synced).to.deep.equal([["/w/src/new"]]);
  });

  it("drops excluded paths and source files before any I/O", async () => {
    dirs.add("/w/node_modules/pkg");
    watchFolders(watcher, options({ delayMs: 100 }));
    watcher.create("/w/node_modules/pkg");
    watcher.delete("/w/node_modules/pkg");
    watcher.create("/w/.cache");
    watcher.create("/w/src/a.ts");
    watcher.delete("/w/src/a.ts");

    await clock.tickAsync(1000);
    expect(synced).to.deep.equal([]);
    expect(statted).to.deep.equal([]);
  });

  it("sends a rename (delete + create) and further changes as one batch without duplicates", async () => {
    dirs.add("/w/src/moved");
    watchFolders(watcher, options({ delayMs: 100 }));
    watcher.delete("/w/src/pkg");
    watcher.create("/w/src/moved");
    await clock.tickAsync(50);
    watcher.delete("/w/src/pkg");
    watcher.delete("/w/lib/gone");

    await clock.tickAsync(49);
    expect(synced).to.deep.equal([]);
    await clock.tickAsync(1);
    expect(synced).to.deep.equal([["/w/src/pkg", "/w/src/moved", "/w/lib/gone"]]);
  });

  it("holds new candidates while a sync runs and sends them after it settles", async () => {
    watchFolders(watcher, options({ delayMs: 100, gated: true }));
    watcher.delete("/w/a");
    await clock.tickAsync(100);
    expect(synced).to.deep.equal([["/w/a"]]);

    watcher.delete("/w/b");
    await clock.tickAsync(1000);
    expect(synced, "nothing new while the first sync runs").to.deep.equal([["/w/a"]]);

    gate!();
    await clock.tickAsync(99);
    expect(synced).to.deep.equal([["/w/a"]]);
    await clock.tickAsync(1);
    expect(synced).to.deep.equal([["/w/a"], ["/w/b"]]);
  });

  it("reports a failed sync and still sends later candidates", async () => {
    failNextSync = true;
    watchFolders(watcher, options({ delayMs: 100 }));
    watcher.delete("/w/a");
    await clock.tickAsync(100);
    expect(errors).to.have.length(1);
    expect(String(errors[0])).to.include("daemon down");

    watcher.delete("/w/b");
    await clock.tickAsync(100);
    expect(synced).to.deep.equal([["/w/a"], ["/w/b"]]);
  });

  it("treats a failed stat as not a directory", async () => {
    statFails.add("/w/src/locked");
    watchFolders(watcher, options({ delayMs: 100 }));
    watcher.create("/w/src/locked");

    await clock.tickAsync(1000);
    expect(synced).to.deep.equal([]);
    expect(errors).to.deep.equal([]);
  });

  it("sends nothing after dispose", async () => {
    dirs.add("/w/src/late");
    const sub = watchFolders(watcher, options({ delayMs: 100 }));
    watcher.delete("/w/a");
    watcher.create("/w/src/late");
    sub.dispose();
    expect(watcher.disposed).to.equal(2);

    watcher.delete("/w/b");
    await clock.tickAsync(1000);
    expect(synced).to.deep.equal([]);
  });
});
