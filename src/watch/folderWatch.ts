// Turns folder-level create/delete events into syncFolders requests.
//
// VS Code reports a deleted, moved or renamed folder as one event for the
// folder itself, and the per-file watcher (fileWatch.ts) only listens for
// source-file extensions, so without this the files inside such a folder stay
// in the index (or never enter it) until the next full pass. Kept free of the
// vscode module so the scheduling can be tested with fake timers.

import type { Subscription, WatchedUri } from "./fileWatch";
export type { Subscription, WatchedUri };

/** The create and delete events of a vscode.FileSystemWatcher (changes are not needed). */
export interface FolderEvents<U extends WatchedUri> {
  onDidCreate(listener: (uri: U) => unknown): Subscription;
  onDidDelete(listener: (uri: U) => unknown): Subscription;
}

export interface FolderWatchOptions<U extends WatchedUri> {
  /** Quiet period after the first candidate before one sync is sent. Default 1000 ms. */
  delayMs?: number;
  /** Paths the index never holds (hidden folders, excluded names). Checked before any I/O. */
  isExcluded: (uri: U) => boolean;
  /** Files the per-file watcher already handles. Checked before any I/O. */
  isSourceFile: (uri: U) => boolean;
  /**
   * Whether a created path is a directory. Called only for create events that
   * pass the two checks above; a rejection counts as "not a directory".
   */
  isDirectory: (uri: U) => Promise<boolean>;
  /** Receives the candidate paths (`fsPath`, in arrival order, without duplicates). */
  sync: (paths: string[]) => Promise<void>;
  /** Receives whatever `sync` rejected with. */
  onError: (error: unknown) => void;
}

/**
 * Collect folder-level changes and hand them to `sync` in batches.
 *
 * - A delete that passes `isExcluded` and `isSourceFile` is a candidate (no
 *   I/O: the path is gone, so it cannot be inspected).
 * - Such a create is a candidate only when `isDirectory` resolves to true.
 * - The first candidate starts a `delayMs` timer; when it fires, every
 *   candidate collected so far goes to one `sync` call.
 * - While a `sync` is running, new candidates wait; when it settles, waiting
 *   candidates start a new `delayMs` timer.
 * - A rejected `sync` goes to `onError`; later candidates are still sent.
 *
 * WHY batches: a folder rename arrives as a delete and a create, and a checkout
 * can touch many folders at once; the daemon answers with at most one full
 * pass per call.
 *
 * `dispose()` cancels the timer, drops the candidates and ends both
 * subscriptions; no `sync` starts after it.
 */
export function watchFolders<U extends WatchedUri>(
  events: FolderEvents<U>,
  options: FolderWatchOptions<U>
): Subscription {
  const delay = options.delayMs ?? 1000;
  const candidates = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | null = null;
  let syncing = false;
  let disposed = false;

  const clearTimer = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  const schedule = () => {
    if (disposed || syncing || timer !== null || candidates.size === 0) return;
    timer = setTimeout(onTimer, delay);
  };

  const onTimer = () => {
    timer = null;
    if (disposed) return;
    const paths = Array.from(candidates);
    candidates.clear();
    syncing = true;
    // WHY Promise.resolve().then: a sync that throws instead of rejecting must
    // not leave `syncing` stuck at true.
    void Promise.resolve()
      .then(() => options.sync(paths))
      .catch((err) => {
        try {
          options.onError(err);
        } catch {
          // An error thrown by onError must not become an unhandled rejection.
        }
      })
      .then(() => {
        syncing = false;
        schedule();
      });
  };

  const addCandidate = (path: string) => {
    if (disposed) return;
    candidates.add(path);
    schedule();
  };

  const handleDelete = (uri: U) => {
    if (disposed) return;
    if (options.isExcluded(uri) || options.isSourceFile(uri)) return;
    addCandidate(uri.fsPath);
  };

  const handleCreate = (uri: U) => {
    if (disposed) return;
    if (options.isExcluded(uri) || options.isSourceFile(uri)) return;
    void Promise.resolve()
      .then(() => options.isDirectory(uri))
      .then((isDir) => {
        // addCandidate ignores the result if dispose() ran in the meantime.
        if (isDir) {
          addCandidate(uri.fsPath);
        }
      })
      .catch(() => {
        // rejection counts as "not a directory", ignore
      });
  };

  const subCreate = events.onDidCreate(handleCreate);
  const subDelete = events.onDidDelete(handleDelete);

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    clearTimer();
    candidates.clear();
    subCreate.dispose();
    subDelete.dispose();
  };

  return { dispose };
}
