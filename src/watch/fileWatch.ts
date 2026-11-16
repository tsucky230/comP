// Turns file-system events into index updates for the comP daemon.
//
// Kept free of the vscode module so the scheduling can be tested with fake
// timers; extension.ts passes in the real FileSystemWatcher and the daemon calls.

export interface WatchedUri {
  readonly fsPath: string;
}

export interface Subscription {
  dispose(): unknown;
}

/** The three events of a vscode.FileSystemWatcher. */
export interface WatchEvents<U extends WatchedUri> {
  onDidCreate(listener: (uri: U) => unknown): Subscription;
  onDidChange(listener: (uri: U) => unknown): Subscription;
  onDidDelete(listener: (uri: U) => unknown): Subscription;
}

export interface WatchActions<U extends WatchedUri> {
  /** Re-index one file. */
  index(uri: U): Promise<void>;
  /** Drop one file from the index. */
  remove(uri: U): Promise<void>;
  /** Receives whatever `index` or `remove` rejected with, and the file it was for. */
  onError(error: unknown, uri: U): void;
}

export interface WatchOptions<U extends WatchedUri> {
  /** Quiet period before a created or changed file is indexed. Default 500 ms. */
  delayMs?: number;
  /** Events for files this returns true for are ignored. */
  isExcluded?: (uri: U) => boolean;
}

export interface IndexingWatch extends Subscription {
  /**
   * Drop every pending timer and every queued file; an `index` already running
   * finishes. Later events are handled as usual.
   */
  cancelPending(): void;
}

/**
 * Subscribe to `events` and keep the index in step with them.
 *
 * Per file (keyed by `fsPath`):
 * - create or change starts a `delayMs` timer; another create or change of the
 *   same file before it fires restarts it. When it fires, the file joins the
 *   index queue.
 * - delete cancels that file's pending timer, takes it out of the queue, and
 *   runs `remove` right away, so a deleted file is never indexed afterwards. A
 *   create after the delete (an atomic save) schedules a fresh `index`.
 * - Files never share a timer: events for one file do not cancel or delay
 *   another file's pending timer.
 *
 * The queue runs `index` one file at a time, in the order the files joined it;
 * the next starts when the previous one settles. A file already waiting in the
 * queue is not added again.
 * WHY one at a time: the daemon answers requests in order and the extension
 * times each request from the moment it is sent, so a burst (a checkout, a
 * format-all) sent at once makes the tail of it — and any request behind it —
 * time out.
 *
 * A rejected `index` or `remove` goes to `onError`; other files are unaffected.
 *
 * `dispose()` cancels every pending timer, empties the queue and ends the three
 * subscriptions; no action starts after it.
 */
export function watchForIndexing<U extends WatchedUri>(
  events: WatchEvents<U>,
  actions: WatchActions<U>,
  options: WatchOptions<U> = {}
): IndexingWatch {
  const delayMs = options.delayMs ?? 500;
  const pending = new Map<string, ReturnType<typeof setTimeout>>();
  // Insertion-ordered: the first entry is the next file to index.
  const queued = new Map<string, U>();
  let running = false;
  let disposed = false;

  // Never rejects: every failure goes to onError.
  const settle = async (action: () => Promise<void>, uri: U): Promise<void> => {
    try {
      await action();
    } catch (error) {
      try {
        actions.onError(error, uri);
      } catch {
        // An error thrown by onError must not become an unhandled rejection.
      }
    }
  };

  const runQueue = (): void => {
    if (running || disposed) {
      return;
    }
    const next = queued.entries().next();
    if (next.done) {
      return;
    }
    const [path, uri] = next.value;
    queued.delete(path);
    running = true;
    void settle(() => actions.index(uri), uri).then(() => {
      running = false;
      runQueue();
    });
  };

  const clearPending = (): void => {
    for (const handle of pending.values()) {
      clearTimeout(handle);
    }
    pending.clear();
    queued.clear();
  };

  const scheduleIndex = (uri: U): void => {
    if (disposed || options.isExcluded?.(uri)) {
      return;
    }
    const path = uri.fsPath;
    const existing = pending.get(path);
    if (existing !== undefined) {
      clearTimeout(existing);
    }
    pending.set(path, setTimeout(() => {
      pending.delete(path);
      // A file already waiting keeps its place (Map.set does not move a key).
      queued.set(path, uri);
      runQueue();
    }, delayMs));
  };

  const handleDelete = (uri: U): void => {
    if (disposed || options.isExcluded?.(uri)) {
      return;
    }
    const path = uri.fsPath;
    const existing = pending.get(path);
    if (existing !== undefined) {
      clearTimeout(existing);
      pending.delete(path);
    }
    queued.delete(path);
    void settle(() => actions.remove(uri), uri);
  };

  const subscriptions = [
    events.onDidCreate(scheduleIndex),
    events.onDidChange(scheduleIndex),
    events.onDidDelete(handleDelete),
  ];

  return {
    cancelPending: clearPending,
    dispose() {
      if (disposed) {
        return;
      }
      disposed = true;
      clearPending();
      subscriptions.forEach((s) => s.dispose());
    },
  };
}
