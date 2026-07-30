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
import { registerCommands } from "./ui/commands";
import { SessionMemoryManager } from "./mcp/sessionMemory";
import { registerChatParticipant } from "./mcp/chatParticipant";
import { AgentSetupManager } from "./mcp/AgentSetup";

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

let watcherDisposable: vscode.Disposable | null = null;
let codeLensDisposable: vscode.Disposable | null = null;

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
 */
function repairMcpConfigs(context: vscode.ExtensionContext): void {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!workspaceRoot) return;

  try {
    // daemonManager is unused by AgentSetupManager (reserved for future expansion)
    const agentSetup = new AgentSetupManager(
      null as unknown as DaemonManager,
      workspaceRoot,
      context.extensionPath
    );
    const repaired = agentSetup.repairStaleConfigs().filter((entry) => entry.status === "repaired");

    if (repaired.length === 0) return;

    for (const entry of repaired) {
      console.log(`[comP] Repaired MCP config: ${entry.file}${entry.to ? ` -> ${entry.to}` : " (workspace root)"}`);
    }
    vscode.window.showInformationMessage(
      `comP: ${repaired.length} 件の MCP 設定を現在の daemon パスへ修復しました。エージェントを再起動してください。`
    );
  } catch (error) {
    console.warn("[comP] MCP config repair failed:", error);
  }
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

  // Update status bar immediately with token stats (SidebarPanel also polls every 5s,
  // but showing efficiency right away avoids a blank display on startup)
  try {
    const stats = await daemonManager.getStats();
    statusBar?.updateStats(stats.total_nodes || 0, stats.total_files || 0, "Ready", stats.efficiency || "0%");
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
): vscode.Disposable | null {
  const config = vscode.workspace.getConfiguration("comp");
  const autoIndex = config.get<boolean>("autoIndex", true);

  if (!autoIndex) {
    console.log("[comP] Auto-indexing disabled");
    return null;
  }

  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const sessionMemoryManager = workspaceRoot ? new SessionMemoryManager(workspaceRoot) : null;

  // Debounce timer for rapid file changes
  let debounceTimer: NodeJS.Timeout | null = null;

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
  const sourcePattern = "**/*.{ts,tsx,js,jsx,py,go,rs,java,cs,rb,php,sql,json,yaml,xml,md}";
  const watcher = vscode.workspace.createFileSystemWatcher(sourcePattern, false, false, false);

  watcher.onDidChange(async (uri) => {
    if (isExcludedPath(uri)) { return; }

    // Debounce rapid changes (wait 500ms after last change)
    if (debounceTimer) {
      clearTimeout(debounceTimer);
    }

    debounceTimer = setTimeout(async () => {
      debounceTimer = null;
      try {
        await daemonManager.indexFile(uri.fsPath);
        // Invalidate CodeLens cache for this file
        codeLensProvider.invalidateFile(uri.fsPath);
        codeLensProvider.refresh();

        // Mark session memory entries as stale if they depend on this file
        if (sessionMemoryManager) {
          const relativePath = vscode.workspace.asRelativePath(uri, false);
          sessionMemoryManager.markStaleForFile(relativePath);
        }
      } catch (error) {
        console.error("[comP] Error indexing file:", error);
      }
    }, 500);
  });

  watcher.onDidDelete(async (uri) => {
    if (isExcludedPath(uri)) { return; }
    try {
      // Notify daemon that file was deleted
      await daemonManager.removeFile(uri.fsPath);
      // Invalidate CodeLens cache for deleted file
      codeLensProvider.invalidateFile(uri.fsPath);
      codeLensProvider.refresh();

      // Mark session memory entries as stale if they depend on this file
      if (sessionMemoryManager) {
        const relativePath = vscode.workspace.asRelativePath(uri, false);
        sessionMemoryManager.markStaleForFile(relativePath);
      }
    } catch (error) {
      console.error("[comP] Error removing file from index:", error);
    }
  });

  // WHY: Return a composite disposable to ensure pending debounce timers are cleared when watcher is disposed.
  const disposable: vscode.Disposable = {
    dispose: () => {
      if (debounceTimer) {
        clearTimeout(debounceTimer);
        debounceTimer = null;
      }
      watcher.dispose();
    },
  };
  context.subscriptions.push(disposable);
  console.log("[comP] Auto-indexing enabled");
  return disposable;
}
