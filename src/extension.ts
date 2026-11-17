// VSCode Extension Entry Point - comP
//
// Responsibilities:
// 1. Activate on startup
// 2. Start Rust daemon (or connect if already running)
// 3. Register commands (Setup Agents, Force Re-index, Generate Context, Show Impact)
// 4. Set up status bar
// 5. Set up event listeners for auto-indexing

import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import { DaemonManager } from "./daemon/DaemonManager";
import { StatusBar } from "./ui/StatusBar";
import { SidebarPanel } from "./ui/SidebarPanel";
import { DependencyCodeLensProvider } from "./ui/CodeLens";
import { currentRecordingSettings, registerCommands, useWorkspaceDaemonBuild } from "./ui/commands";
import { describeDaemonVersionMismatch } from "./config/daemonBinary";
import { syncConversationRecording } from "./config/conversationRecording";
import { syncRuleSharing } from "./config/ruleSharing";
import { syncWorkspaceFolders } from "./config/workspaceFolders";
import { registerChatParticipant } from "./mcp/chatParticipant";
import { AgentSetupManager } from "./mcp/AgentSetup";
import { isJapaneseLocale, t } from "./i18n";
import { startBranchWatch } from "./git/branchWatcher";
import { IndexingWatch, watchForIndexing } from "./watch/fileWatch";
import { watchFolders } from "./watch/folderWatch";
import { syncBudget } from "./config/budget";

/** Global context */
let daemonManager: DaemonManager | null = null;
let statusBar: StatusBar | null = null;
let sidebarPanel: SidebarPanel | null = null;
let codeLensProvider: DependencyCodeLensProvider | null = null;

/**
 * Merge `comp.exclude` values into a `.comp/config.json` object.
 *
 * WHY pure function: enables unit testing without VS Code API or filesystem I/O.
 *
 * @param existing - Current parsed config.json content (any type; gracefully handles non-objects).
 * @param exclude - New exclude array from `comp.exclude` setting.
 * @returns Merged config object. Returns existing unchanged when the exclude value is already equal.
 */
export function mergeExcludeIntoConfig(
  existing: unknown,
  exclude: string[]
): Record<string, unknown> {
  const base: Record<string, unknown> =
    existing !== null && typeof existing === "object" && !Array.isArray(existing)
      ? { ...(existing as Record<string, unknown>) }
      : {};

  const current = Array.isArray(base["exclude"]) ? (base["exclude"] as unknown[]) : null;
  const same =
    current !== null &&
    current.length === exclude.length &&
    exclude.every((v, i) => v === current[i]);

  if (same) {
    return base;
  }

  return { ...base, exclude };
}

// Check if .comp directory exists in workspace root
function hasCompDirectory(): boolean {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!workspaceRoot) return false;

  const compPath = path.join(workspaceRoot, ".comp");
  return fs.existsSync(compPath);
}

let watcherDisposable: IndexingWatch | null = null;
let codeLensDisposable: vscode.Disposable | null = null;
let branchWatcherDisposable: vscode.Disposable | null = null;

/**
 * Re-point MCP config files whose recorded daemon path no longer exists.
 *
 * WHY: VS Code installs extensions into `<publisher>.<name>-<version>` and removes
 * the previous directory on upgrade. Config files written by "comP: Setup Agents"
 * keep the old absolute path, so MCP clients fail to spawn the daemon while the
 * extension itself keeps working — it resolves its own binary at runtime.
 *
 * WHY unconditional rather than gated on a stored version: globalState is shared
 * across the whole VS Code installation, not per workspace. A version gate repairs
 * whichever workspace is opened first after an upgrade and leaves the rest broken.
 * The check is a handful of existsSync calls on files that usually do not exist.
 *
 * Exported for unit testing; activate() is the only production caller.
 */
export function repairMcpConfigs(context: vscode.ExtensionContext): void {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!workspaceRoot) return;

  try {
    // daemonManager is unused by AgentSetupManager (reserved for future expansion)
    const agentSetup = new AgentSetupManager(
      null as unknown as DaemonManager,
      workspaceRoot,
      context.extensionPath,
      { locale: isJapaneseLocale() ? "ja" : "en", useWorkspaceDaemonBuild: useWorkspaceDaemonBuild() }
    );
    const repaired = agentSetup.repairStaleConfigs().filter((entry) => entry.status === "repaired");

    if (repaired.length === 0) return;

    for (const entry of repaired) {
      console.log(`[comP] Repaired MCP config: ${entry.file}${entry.to ? ` -> ${entry.to}` : " (workspace root)"}`);
    }
    vscode.window.showInformationMessage(
      t(
        `comP: repaired ${repaired.length} MCP config(s) to point at the current daemon path. Restart the agent(s) to pick it up.`,
        `comP: ${repaired.length} 件の MCP 設定を現在の daemon パスへ修復しました。エージェントを再起動してください。`
      )
    );
  } catch (error) {
    console.warn("[comP] MCP config repair failed:", error);
  }
}

/**
 * Warn when the daemon the extension started is not the extension's version.
 *
 * WHY: requests added in a newer extension (e.g. 0.12.0's `syncFolders`) fail
 * on an older daemon, and those failures are logged, not shown — an upgraded
 * extension ran a stale workspace build of 0.11.11 without anyone noticing.
 * The bundled binary always matches, so this fires only for a workspace build.
 *
 * Exported for unit testing; startDaemonStack() is the only production caller.
 */
export function warnOnDaemonVersionMismatch(
  context: vscode.ExtensionContext,
  daemonVersion: string | undefined
): void {
  const extensionVersion: unknown = context.extension?.packageJSON?.version;
  if (typeof extensionVersion !== "string") return;
  const mismatch = describeDaemonVersionMismatch(extensionVersion, daemonVersion);
  if (!mismatch) return;
  console.warn(`[comP] Daemon version mismatch: ${mismatch}`);
  void vscode.window.showWarningMessage(
    t(
      `comP: the running daemon is not this extension's version (${mismatch}), so some features may not work. If you use comp.useWorkspaceDaemonBuild, rebuild the daemon (cargo build --release) and reload the window.`,
      `comP: 動いているデーモンが拡張機能と別の版です（${mismatch}）。一部の機能が動かない場合があります。comp.useWorkspaceDaemonBuild を使っている場合は、デーモンを作り直して（cargo build --release）ウィンドウを再読み込みしてください。`
    )
  );
}

/**
 * Sync `comp.exclude` VS Code setting into `.comp/config.json`.
 *
 * WHY: The daemon reads exclude patterns from .comp/config.json at indexer
 * creation time. By writing the VS Code setting here (before startDaemonStack),
 * both initial indexing and forceReindex pick up the user's exclude list.
 * Skips the write when the value is already equal to avoid unnecessary diffs.
 */
function syncExcludeToConfig(): void {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!workspaceRoot) return;

  const exclude = vscode.workspace
    .getConfiguration("comp")
    .get<string[]>("exclude", []);

  const configPath = path.join(workspaceRoot, ".comp", "config.json");

  let existing: unknown = {};
  if (fs.existsSync(configPath)) {
    try {
      existing = JSON.parse(fs.readFileSync(configPath, "utf8"));
    } catch {
      existing = {};
    }
  }

  const merged = mergeExcludeIntoConfig(existing, exclude);

  // Skip write when exclude is already equal (avoid diff noise)
  const currentExclude = Array.isArray((existing as Record<string, unknown>)?.["exclude"])
    ? ((existing as Record<string, unknown>)["exclude"] as unknown[])
    : null;
  const alreadySynced =
    currentExclude !== null &&
    currentExclude.length === exclude.length &&
    exclude.every((v, i) => v === currentExclude[i]);

  if (alreadySynced) {
    return;
  }

  try {
    const compDir = path.join(workspaceRoot, ".comp");
    if (!fs.existsSync(compDir)) {
      fs.mkdirSync(compDir, { recursive: true });
    }
    fs.writeFileSync(configPath, JSON.stringify(merged, null, 2) + "\n", "utf8");
    console.log(`[comP] Synced comp.exclude to ${configPath}`);
  } catch (error) {
    console.warn("[comP] Failed to sync comp.exclude:", error);
  }
}


/** Activation: called when extension is loaded */
/**
 * Keep `.comp/config.json`'s conversationRecording section in step with the
 * `comp.conversationRecording.*` settings, which record-turn reads.
 *
 * WHY sync on change as well as on activation: turning recording off must stop
 * it at once (the hook stays installed and keeps being called). The setup
 * prompt fires only on an off-to-on transition, never on activation, so a user
 * who removed the hook on purpose is not nagged every time VS Code starts.
 */
function watchConversationRecording(context: vscode.ExtensionContext): void {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!workspaceRoot) return;

  const isOn = (s: { enabled: boolean; claudeCode: boolean }) => s.enabled && s.claudeCode;
  const sync = (): void => {
    try {
      const result = syncConversationRecording(workspaceRoot, currentRecordingSettings());
      if (result.status === "invalid") {
        vscode.window.showWarningMessage(
          t(
            `comP: .comp/config.json is not valid JSON (${result.reason}), so conversation recording (beta) stays off. Fix the file to apply the setting.`,
            `comP: .comp/config.json が正しい JSON ではないため（${result.reason}）、会話の記録（β版）はOFFのままです。ファイルを直すと設定が反映されます。`
          )
        );
      }
    } catch (error) {
      console.warn(`[comP] failed to sync conversation recording setting: ${error}`);
    }
  };

  let previous = currentRecordingSettings();
  sync();

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (!event.affectsConfiguration("comp.conversationRecording")) return;
      const now = currentRecordingSettings();
      sync();
      const turnedOn = !isOn(previous) && isOn(now);
      previous = now;
      if (!turnedOn) return;

      const probe = new AgentSetupManager(null as unknown as DaemonManager, workspaceRoot, context.extensionPath, {
        useWorkspaceDaemonBuild: useWorkspaceDaemonBuild(),
      });
      if (probe.historyHookInstalled()) return;
      const runSetup = t("Run Setup Agents", "Setup Agents を実行");
      const answer = await vscode.window.showInformationMessage(
        t(
          "Conversation recording (beta) is on. Run comP: Setup Agents and pick Claude Code to install the recording hook.",
          "会話の記録（β版）をONにしました。comP: Setup Agents で Claude Code を選ぶと、記録用のフックが入ります。"
        ),
        runSetup
      );
      if (answer === runSetup) {
        await vscode.commands.executeCommand("comp.setupAgents");
      }
    })
  );
}

/**
 * Keep `.comp/config.json`'s ruleSharing section in step with
 * `comp.ruleSharing.enabled`, which the daemon reads on every call.
 */
function watchRuleSharing(context: vscode.ExtensionContext): void {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) return;
  const sync = (): void => {
    const enabled = vscode.workspace
      .getConfiguration("comp.ruleSharing", folder.uri)
      .get<boolean>("enabled", false);
    try {
      const result = syncRuleSharing(folder.uri.fsPath, { enabled });
      if (result.status === "invalid") {
        vscode.window.showWarningMessage(
          t(
            `comP: .comp/config.json is not valid JSON (${result.reason}), so rule sharing (beta) stays off. Fix the file to apply the setting.`,
            `comP: .comp/config.json が正しい JSON ではないため（${result.reason}）、ルール共有（β版）はOFFのままです。ファイルを直すと設定が反映されます。`
          )
        );
      }
    } catch (error) {
      console.warn(`[comP] failed to sync rule sharing setting: ${error}`);
    }
  };
  sync();
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("comp.ruleSharing")) sync();
    })
  );
}

// WHY: a daemon started by hand creates `.comp/` only then, so the budget that
// could not be written at activation has to be written after the start.
let syncBudgetAfterStart: (() => void) | null = null;

/**
 * Keep `.comp/config.json`'s `default_budget_tokens` in step with an explicitly
 * set `comp.maxContextTokens`, and redraw CodeLens when `comp.enableCodeLens`
 * changes (the provider reads that setting on every call).
 */
function watchBudgetAndCodeLens(context: vscode.ExtensionContext): void {
  const folder = vscode.workspace.workspaceFolders?.[0];
  const STATE_KEY = "comp.syncedBudgetTokens";
  const sync = (): void => {
    if (!folder) return;
    try {
      const inspected = vscode.workspace.getConfiguration("comp", folder.uri).inspect<number>("maxContextTokens");
      const explicit = inspected?.workspaceFolderValue ?? inspected?.workspaceValue ?? inspected?.globalValue;
      const result = syncBudget(folder.uri.fsPath, explicit, context.workspaceState.get<number>(STATE_KEY));
      void context.workspaceState.update(STATE_KEY, result.synced);
      if (result.status === "invalid") {
        vscode.window.showWarningMessage(
          t(
            `comP: .comp/config.json is not valid JSON (${result.reason}), so comp.maxContextTokens cannot be applied. Fix the file to apply the setting.`,
            `comP: .comp/config.json が正しい JSON ではないため（${result.reason}）、comp.maxContextTokens を反映できません。ファイルを直すと設定が反映されます。`
          )
        );
      }
    } catch (error) {
      console.warn(`[comP] failed to sync comp.maxContextTokens: ${error}`);
    }
  };
  sync();
  syncBudgetAfterStart = sync;
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("comp.maxContextTokens")) sync();
      if (event.affectsConfiguration("comp.enableCodeLens")) codeLensProvider?.refresh();
    })
  );
}

// WHY: a daemon started by hand creates `.comp/` only then, so the list that
// could not be written at activation has to be written after the start.
let syncWorkspaceFoldersAfterStart: (() => void) | null = null;

/**
 * Keep every workspace folder's `.comp/config.json` `workspace_folders` in step
 * with the open folders and `comp.indexWorkspaceFolders`.
 */
function watchWorkspaceFolders(context: vscode.ExtensionContext): void {
  const sync = (reindex: boolean): void => {
    try {
      const folders = (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.fsPath);
      const enabled = vscode.workspace.getConfiguration("comp").get<boolean>("indexWorkspaceFolders", true);
      const results = syncWorkspaceFolders(folders, enabled);
      for (const result of results) {
        if (result.status === "invalid") {
          vscode.window.showWarningMessage(
            t(
              `comP: ${result.path} is not valid JSON (${result.reason}), so the other workspace folders cannot be added to the index. Fix the file to apply the setting.`,
              `comP: ${result.path} が正しい JSON ではないため（${result.reason}）、ほかのワークスペースフォルダを索引に含められません。ファイルを直すと設定が反映されます。`
            )
          );
        }
      }
      // WHY: the daemon reads the list when it indexes, so a changed list needs a re-index.
      if (reindex && daemonManager?.isRunning() && results.some((r) => r.status === "written")) {
        daemonManager.request("forceReindex").catch((error) => {
          console.warn(`[comP] failed to reindex after workspace folders changed: ${error}`);
        });
      }
    } catch (error) {
      console.warn(`[comP] failed to sync workspace folders: ${error}`);
    }
  };
  sync(false);
  syncWorkspaceFoldersAfterStart = () => sync(true);
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => sync(true)),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("comp.indexWorkspaceFolders")) sync(true);
    })
  );
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  console.log("[comP] Extension activating...");

  try {
    // Before anything else: an upgrade may have left MCP configs pointing at a
    // binary that no longer exists. Runs in both auto and manual mode.
    repairMcpConfigs(context);

    const autoStartDaemon = hasCompDirectory();
    console.log(`[comP] .comp directory exists: ${autoStartDaemon}`);

    // 1. Sidebar panel (Always active so that start button is available even in manual mode)
    sidebarPanel = SidebarPanel.createOrShow(context.extensionPath, null, context);
    context.subscriptions.push(
      vscode.window.registerWebviewViewProvider(SidebarPanel.viewType, sidebarPanel, {
        webviewOptions: { retainContextWhenHidden: true },
      })
    );

    // 2. Always register the StatusBar and commands.
    // WHY: To prevent "command not found" errors on comp.showStats when daemon fails to start,
    // we register them exactly once in activate() regardless of startDaemonStack success.
    statusBar = new StatusBar();
    statusBar.show("Stopped");
    context.subscriptions.push({ dispose: () => statusBar?.dispose() });
    registerCommands(context, () => daemonManager, statusBar);
    registerChatParticipant(context, () => daemonManager);
    watchConversationRecording(context);
    watchRuleSharing(context);
    watchWorkspaceFolders(context);
    watchBudgetAndCodeLens(context);

    // 3. Inject lifecycle callbacks into SidebarPanel.
    // WHY: Prevent duplicate DaemonManager creation within SidebarPanel.
    // The actual start/stop execution is centralized in extension.ts.
    sidebarPanel.setLifecycleCallbacks({
      onStartRequest: async () => {
        await startDaemonStack(context);
        return daemonManager;
      },
      onStopRequest: async () => {
        await stopDaemonStack();
      },
    });

    // 4. Auto mode: immediately start if .comp directory exists
    if (autoStartDaemon) {
      // Sync comp.exclude to .comp/config.json before daemon starts so the
      // initial indexer picks up the user's exclude list without a Force Re-index.
      syncExcludeToConfig();
      await startDaemonStack(context);
      console.log("[comP] Extension activated successfully (auto-mode)");
    } else {
      console.log("[comP] No .comp directory found - sidebar will show startup controls");
      console.log("[comP] Extension activated successfully (manual-mode)");
    }
  } catch (error) {
    console.error("[comP] Activation failed:", error);
    vscode.window.showErrorMessage(
      `comP failed to activate: ${error instanceof Error ? error.message : String(error)}`
    );
  }
}

/**
 * Starts the daemon and helper services.
 * Skips starting if a running instance already exists to prevent duplicate processes.
 */
async function startDaemonStack(context: vscode.ExtensionContext): Promise<void> {
  if (daemonManager?.isRunning()) {
    console.log("[comP] Daemon already running, skipping startDaemonStack");
    return;
  }

  daemonManager = new DaemonManager(context);

  statusBar?.show("Initializing...");

  try {
    await daemonManager.start();
  } catch (error) {
    statusBar?.show("Error");
    throw error;
  }

  sidebarPanel?.setDaemonManager(daemonManager);
  syncWorkspaceFoldersAfterStart?.();
  syncBudgetAfterStart?.();

  // Update status bar immediately with token stats (SidebarPanel also polls every 5s,
  // but showing efficiency right away avoids a blank display on startup)
  try {
    const stats = await daemonManager.getStats();
    statusBar?.updateStats(stats.total_nodes || 0, stats.total_files || 0, "Ready", stats.efficiency || "0%");
    warnOnDaemonVersionMismatch(context, stats.daemon_version);
  } catch {
    statusBar?.show("Ready");
  }

  // CodeLens and FileWatcher need to be re-created on restart -> manage via dispose
  if (codeLensDisposable) {
    codeLensDisposable.dispose();
  }
  codeLensProvider = new DependencyCodeLensProvider(daemonManager);
  codeLensDisposable = vscode.languages.registerCodeLensProvider(
    ["typescript", "javascript", "python", "go", "rust", "java", "csharp"],
    codeLensProvider
  );
  // WHY: Do not push to context.subscriptions since startDaemonStack is called on restarts.
  // Pushing would accumulate obsolete entries, leading to duplicate dispose errors during deactivation.
  // Instead, dispose manually during deactivate().

  if (watcherDisposable) {
    watcherDisposable.dispose();
    watcherDisposable = null;
  }
  watcherDisposable = setupFileWatchers(context, daemonManager, codeLensProvider);

  if (branchWatcherDisposable) {
    branchWatcherDisposable.dispose();
    branchWatcherDisposable = null;
  }
  branchWatcherDisposable = setupBranchWatch(daemonManager);
}

/**
 * Watch for git branch switches and trigger a full re-index automatically.
 *
 * WHY a full re-index rather than the incremental per-file path: a branch
 * switch can change hundreds of files at once, and the FileSystemWatcher path
 * (setupFileWatchers) isn't built for that — it sends one indexFile request per
 * file, each re-resolving only that file's edges. A full re-index pass
 * (index_all_roots, what `forceReindex` runs) re-resolves every file's edges in the same
 * pass, so a stale dependency edge into a changed file never has a chance to
 * appear — see the "syncIndex" idea rejected in the design notes: an
 * incremental re-resolve only recomputes edges *from* files in the changed
 * set, so edges *into* a changed file from unrelated, unchanged files would go
 * missing until those files were reparsed for some other reason.
 *
 * WHY no confirmation dialog and no error toast: this is an automatic
 * background reaction to something the user already did (switching branches),
 * not a user-initiated action — popping "This will re-index everything,
 * continue?" or an error toast on every switch would be pure noise. Failures
 * are still logged and reflected in the status bar.
 *
 * Gated behind `comp.autoIndex` for the same reason setupFileWatchers is: a
 * user who disabled auto-indexing does not want an automatic reindex from
 * this path either.
 */
function setupBranchWatch(dm: DaemonManager): vscode.Disposable | null {
  const autoIndex = vscode.workspace.getConfiguration("comp").get<boolean>("autoIndex", true);
  if (!autoIndex) {
    return null;
  }

  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!workspaceRoot) {
    return null;
  }

  return startBranchWatch(workspaceRoot, async () => {
    // WHY drop the file watcher's pending work first: those files would be
    // re-indexed one by one after the full pass, and each such update deletes
    // the edges pointing into its file — undoing what the full pass rebuilt.
    watcherDisposable?.cancelPending();
    statusBar?.show("Indexing...");
    try {
      await dm.request("forceReindex");
      const stats = await dm.getStats();
      statusBar?.updateStats(stats.total_nodes || 0, stats.total_files || 0, "Ready", stats.efficiency || "0%");
    } catch (error) {
      statusBar?.show("Error");
      console.error("[comP] Branch-switch re-index failed:", error);
    }
  });
}

/**
 * Stops the daemon and helper services.
 * Commands registered in activate() are not disposed here to keep them registered.
 */
async function stopDaemonStack(): Promise<void> {
  if (watcherDisposable) {
    watcherDisposable.dispose();
    watcherDisposable = null;
  }
  if (branchWatcherDisposable) {
    branchWatcherDisposable.dispose();
    branchWatcherDisposable = null;
  }
  if (codeLensDisposable) {
    codeLensDisposable.dispose();
    codeLensDisposable = null;
  }
  codeLensProvider = null;

  if (daemonManager) {
    await daemonManager.stop();
    daemonManager = null;
  }

  sidebarPanel?.setDaemonManager(null);
  statusBar?.show("Stopped");
}

/** Deactivation: called when extension is unloaded */
export async function deactivate(): Promise<void> {
  console.log("[comP] Extension deactivating...");

  sidebarPanel?.dispose();

  if (daemonManager) {
    await daemonManager.stop();
  }

  if (statusBar) {
    statusBar.dispose();
  }

  if (codeLensDisposable) {
    codeLensDisposable.dispose();
  }

  if (codeLensProvider) {
    codeLensProvider.dispose();
  }
}

/** Setup file system watchers for auto-indexing */
function setupFileWatchers(
  context: vscode.ExtensionContext,
  daemonManager: DaemonManager,
  codeLensProvider: DependencyCodeLensProvider
): IndexingWatch | null {
  const config = vscode.workspace.getConfiguration("comp");
  const autoIndex = config.get<boolean>("autoIndex", true);

  if (!autoIndex) {
    console.log("[comP] Auto-indexing disabled");
    return null;
  }

  // WHY: Mirror the daemon-side exclusion list so we skip the IPC round-trip
  //      entirely for paths inside excluded directories (e.g. pip-installed .venv).
  //      The daemon also guards against these, but dropping them here saves overhead.
  const EXCLUDED_SEGMENTS = new Set([
    "node_modules", "venv", "__pycache__", "coverage", "vendor", "out",
  ]);
  function isExcludedPath(uri: vscode.Uri): boolean {
    const rel = vscode.workspace.asRelativePath(uri, false).replace(/\\/g, "/");
    return rel.split("/").some(seg => seg.startsWith(".") || EXCLUDED_SEGMENTS.has(seg));
  }

  // Watch for file changes (only code files)
  const SOURCE_EXTENSIONS = ["ts", "tsx", "js", "jsx", "py", "go", "rs", "java", "cs", "rb", "php", "sql", "json", "yaml", "xml", "md"];
  const sourcePattern = `**/*.{${SOURCE_EXTENSIONS.join(",")}}`;
  const sourceFile = new RegExp(`\\.(${SOURCE_EXTENSIONS.join("|")})$`, "i");
  const watcher = vscode.workspace.createFileSystemWatcher(sourcePattern, false, false, false);

  // WHY markStale gets the absolute path: in a multi-root workspace, a path
  // relative to the second folder would name a different file (or none) in the
  // daemon's session memory; the daemon converts it to the stored form.
  const afterIndexChange = async (uri: vscode.Uri): Promise<void> => {
    codeLensProvider.invalidateFile(uri.fsPath);
    codeLensProvider.refresh();
    await daemonManager.markStale(uri.fsPath);
  };
  const scheduling = watchForIndexing(
    watcher,
    {
      index: async (uri) => {
        await daemonManager.indexFile(uri.fsPath);
        await afterIndexChange(uri);
      },
      remove: async (uri) => {
        await daemonManager.removeFile(uri.fsPath);
        await afterIndexChange(uri);
      },
      onError: (error, uri) => console.error(`[comP] Error updating the index for ${uri.fsPath}:`, error),
    },
    { isExcluded: isExcludedPath }
  );

  // WHY a second watcher: VS Code reports a deleted, moved or renamed folder as
  // one event for the folder, which the source-extension pattern above never
  // matches, so the files inside it would stay in (or never enter) the index.
  // Change events are not needed; create/delete are filtered before any I/O.
  const folderWatcher = vscode.workspace.createFileSystemWatcher("**/*", false, true, false);
  const folders = watchFolders(folderWatcher, {
    isExcluded: isExcludedPath,
    isSourceFile: (uri) => sourceFile.test(uri.fsPath),
    isDirectory: async (uri) => ((await vscode.workspace.fs.stat(uri)).type & vscode.FileType.Directory) !== 0,
    sync: async (paths) => {
      const result = (await daemonManager.request("syncFolders", { paths })) as { reindexed?: boolean } | null;
      if (!result?.reindexed) return;
      codeLensProvider.clearCache();
      codeLensProvider.refresh();
      const stats = await daemonManager.getStats();
      statusBar?.updateStats(stats.total_nodes || 0, stats.total_files || 0, "Ready", stats.efficiency || "0%");
    },
    onError: (error) => console.error("[comP] Error syncing folder changes:", error),
  });

  const handle: IndexingWatch = {
    cancelPending: () => scheduling.cancelPending(),
    dispose: () => {
      scheduling.dispose();
      watcher.dispose();
      folders.dispose();
      folderWatcher.dispose();
    },
  };
  context.subscriptions.push(handle);
  console.log("[comP] Auto-indexing enabled");
  return handle;
}
