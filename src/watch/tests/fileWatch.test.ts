// watchForIndexing: per-file scheduling of index / remove calls.
// Owned by the designer role; implementers must not edit.
//
// Transition table per file F (test-side copy, written independently):
//   idle    + create/change -> pending  (timer starts)
//   pending + create/change -> pending  (timer restarts, no index yet)
//   pending + timer fires   -> idle     (index F once)
//   idle    + delete        -> idle     (remove F now)
//   pending + delete        -> idle     (timer cancelled, remove F now, never indexed)
//   any     + excluded path -> unchanged
//   any     + dispose       -> disposed (all timers cancelled, later events ignored)
// Invariant: files are independent — G's events never cancel or delay F.

import { expect } from "chai";
import * as sinon from "sinon";
import { watchForIndexing, WatchedUri } from "../fileWatch";

type Listener = (uri: WatchedUri) => unknown;

class FakeWatcher {
  private listeners: Record<"create" | "change" | "delete", Listener[]> = { create: [], change: [], delete: [] };
  disposed = 0;

  private on(kind: "create" | "change" | "delete", l: Listener) {
    this.listeners[kind].push(l);
    return { dispose: () => { this.disposed++; this.listeners[kind] = this.listeners[kind].filter((x) => x !== l); } };
  }
  onDidCreate = (l: Listener) => this.on("create", l);
  onDidChange = (l: Listener) => this.on("change", l);
  onDidDelete = (l: Listener) => this.on("delete", l);

  create(p: string) { this.listeners.create.forEach((l) => l({ fsPath: p })); }
  change(p: string) { this.listeners.change.forEach((l) => l({ fsPath: p })); }
  delete(p: string) { this.listeners.delete.forEach((l) => l({ fsPath: p })); }
}

describe("watchForIndexing", () => {
  let clock: sinon.SinonFakeTimers;
  let watcher: FakeWatcher;
  let log: string[];
  let errors: Array<{ error: unknown; path: string }>;
  let failIndex: Set<string>;
  let failRemove: Set<string>;

  const actions = () => ({
    index: async (uri: WatchedUri) => {
      if (failIndex.has(uri.fsPath)) throw new Error(`index failed: ${uri.fsPath}`);
      log.push(`index ${uri.fsPath}`);
    },
    remove: async (uri: WatchedUri) => {
      if (failRemove.has(uri.fsPath)) throw new Error(`remove failed: ${uri.fsPath}`);
      log.push(`remove ${uri.fsPath}`);
    },
    onError: (error: unknown, uri: WatchedUri) => { errors.push({ error, path: uri.fsPath }); },
  });

  beforeEach(() => {
    clock = sinon.useFakeTimers();
    watcher = new FakeWatcher();
    log = [];
    errors = [];
    failIndex = new Set();
    failRemove = new Set();
  });

  afterEach(() => clock.restore());

  it("indexes a changed file once the delay has passed, not before", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/a.ts");

    await clock.tickAsync(99);
    expect(log).to.deep.equal([]);
    await clock.tickAsync(1);
    expect(log).to.deep.equal(["index /w/a.ts"]);
  });

  it("indexes a newly created file", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.create("/w/new.ts");

    await clock.tickAsync(100);
    expect(log).to.deep.equal(["index /w/new.ts"]);
  });

  it("waits 500 ms when no delay is given", async () => {
    watchForIndexing(watcher, actions());
    watcher.change("/w/a.ts");

    await clock.tickAsync(499);
    expect(log).to.deep.equal([]);
    await clock.tickAsync(1);
    expect(log).to.deep.equal(["index /w/a.ts"]);
  });

  it("restarts the timer on a repeated change and indexes the file only once", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/a.ts");
    await clock.tickAsync(60);
    watcher.change("/w/a.ts");

    await clock.tickAsync(99);
    expect(log).to.deep.equal([]);
    await clock.tickAsync(1);
    expect(log).to.deep.equal(["index /w/a.ts"]);
    await clock.tickAsync(1000);
    expect(log).to.deep.equal(["index /w/a.ts"]);
  });

  it("indexes every file of a burst, each on its own timer", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/a.ts");
    await clock.tickAsync(60);
    watcher.change("/w/b.ts");
    watcher.create("/w/c.ts");

    await clock.tickAsync(40);
    expect(log).to.deep.equal(["index /w/a.ts"]);
    await clock.tickAsync(60);
    expect(log).to.have.members(["index /w/a.ts", "index /w/b.ts", "index /w/c.ts"]);
    expect(log).to.have.length(3);
  });

  it("removes a deleted file right away", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.delete("/w/a.ts");

    await clock.tickAsync(0);
    expect(log).to.deep.equal(["remove /w/a.ts"]);
  });

  it("never indexes a file deleted while its index was pending", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/a.ts");
    await clock.tickAsync(50);
    watcher.delete("/w/a.ts");

    await clock.tickAsync(1000);
    expect(log).to.deep.equal(["remove /w/a.ts"]);
  });

  it("does not let a delete of one file cancel another file's pending index", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/a.ts");
    watcher.delete("/w/b.ts");

    await clock.tickAsync(100);
    expect(log).to.deep.equal(["remove /w/b.ts", "index /w/a.ts"]);
  });

  it("re-indexes a file deleted and created again (atomic save)", async () => {
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.delete("/w/a.ts");
    watcher.create("/w/a.ts");

    await clock.tickAsync(0);
    expect(log).to.deep.equal(["remove /w/a.ts"]);
    await clock.tickAsync(100);
    expect(log).to.deep.equal(["remove /w/a.ts", "index /w/a.ts"]);
  });

  it("ignores create, change and delete of excluded files", async () => {
    watchForIndexing(watcher, actions(), {
      delayMs: 100,
      isExcluded: (uri) => uri.fsPath.includes("/node_modules/"),
    });
    watcher.create("/w/node_modules/x.js");
    watcher.change("/w/node_modules/y.js");
    watcher.delete("/w/node_modules/z.js");
    watcher.change("/w/a.ts");

    await clock.tickAsync(1000);
    expect(log).to.deep.equal(["index /w/a.ts"]);
  });

  it("cancels pending work on dispose and ignores later events", async () => {
    const sub = watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/a.ts");
    await clock.tickAsync(50);

    sub.dispose();
    expect(watcher.disposed).to.equal(3);
    watcher.change("/w/b.ts");
    watcher.delete("/w/c.ts");

    await clock.tickAsync(1000);
    expect(log).to.deep.equal([]);
  });

  it("reports a failed index to onError and still indexes the other files", async () => {
    failIndex.add("/w/bad.ts");
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.change("/w/bad.ts");
    watcher.change("/w/good.ts");

    await clock.tickAsync(100);
    expect(log).to.deep.equal(["index /w/good.ts"]);
    expect(errors).to.have.length(1);
    expect(errors[0].path).to.equal("/w/bad.ts");
    expect(String(errors[0].error)).to.include("index failed: /w/bad.ts");
  });

  describe("index queue", () => {
    // index logs "start X", then waits until the test calls finish(X).
    let gates: Map<string, () => void>;
    const gated = () => ({
      ...actions(),
      index: (uri: WatchedUri) =>
        new Promise<void>((resolve) => {
          log.push(`start ${uri.fsPath}`);
          gates.set(uri.fsPath, () => { log.push(`done ${uri.fsPath}`); resolve(); });
        }),
    });
    const finish = async (p: string) => {
      gates.get(p)!();
      gates.delete(p);
      await clock.tickAsync(0);
    };

    beforeEach(() => { gates = new Map(); });

    it("runs one index at a time, in the order the files became due", async () => {
      watchForIndexing(watcher, gated(), { delayMs: 100 });
      watcher.change("/w/a.ts");
      await clock.tickAsync(10);
      watcher.change("/w/b.ts");
      await clock.tickAsync(10);
      watcher.change("/w/c.ts");

      await clock.tickAsync(200);
      expect(log).to.deep.equal(["start /w/a.ts"]);
      await finish("/w/a.ts");
      expect(log).to.deep.equal(["start /w/a.ts", "done /w/a.ts", "start /w/b.ts"]);
      await finish("/w/b.ts");
      await finish("/w/c.ts");
      expect(log).to.deep.equal([
        "start /w/a.ts", "done /w/a.ts", "start /w/b.ts", "done /w/b.ts", "start /w/c.ts", "done /w/c.ts",
      ]);
    });

    it("keeps a file waiting in the queue only once", async () => {
      watchForIndexing(watcher, gated(), { delayMs: 100 });
      watcher.change("/w/a.ts");
      watcher.change("/w/b.ts");
      await clock.tickAsync(100);
      watcher.change("/w/b.ts");
      await clock.tickAsync(100);

      await finish("/w/a.ts");
      await finish("/w/b.ts");
      await clock.tickAsync(1000);
      expect(log.filter((l) => l === "start /w/b.ts")).to.have.length(1);
    });

    it("takes a deleted file out of the queue", async () => {
      watchForIndexing(watcher, gated(), { delayMs: 100 });
      watcher.change("/w/a.ts");
      watcher.change("/w/b.ts");
      await clock.tickAsync(100);
      watcher.delete("/w/b.ts");
      await clock.tickAsync(0);

      await finish("/w/a.ts");
      await clock.tickAsync(1000);
      expect(log).to.deep.equal(["start /w/a.ts", "remove /w/b.ts", "done /w/a.ts"]);
    });

    it("cancelPending drops timers and queued files, lets the running index finish, and keeps watching", async () => {
      const sub = watchForIndexing(watcher, gated(), { delayMs: 100 });
      watcher.change("/w/a.ts");
      watcher.change("/w/b.ts");
      await clock.tickAsync(100);
      watcher.change("/w/c.ts");
      await clock.tickAsync(50);

      sub.cancelPending();
      await finish("/w/a.ts");
      await clock.tickAsync(1000);
      expect(log).to.deep.equal(["start /w/a.ts", "done /w/a.ts"]);

      watcher.change("/w/d.ts");
      await clock.tickAsync(100);
      expect(log).to.deep.equal(["start /w/a.ts", "done /w/a.ts", "start /w/d.ts"]);
    });

    it("starts nothing from the queue after dispose", async () => {
      const sub = watchForIndexing(watcher, gated(), { delayMs: 100 });
      watcher.change("/w/a.ts");
      watcher.change("/w/b.ts");
      await clock.tickAsync(100);

      sub.dispose();
      await finish("/w/a.ts");
      await clock.tickAsync(1000);
      expect(log).to.deep.equal(["start /w/a.ts", "done /w/a.ts"]);
    });
  });

  it("reports a failed remove to onError", async () => {
    failRemove.add("/w/a.ts");
    watchForIndexing(watcher, actions(), { delayMs: 100 });
    watcher.delete("/w/a.ts");

    await clock.tickAsync(0);
    expect(log).to.deep.equal([]);
    expect(errors.map((e) => e.path)).to.deep.equal(["/w/a.ts"]);
    expect(String(errors[0].error)).to.include("remove failed: /w/a.ts");
  });
});
