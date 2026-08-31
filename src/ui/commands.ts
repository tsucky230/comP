// Commands - Register VSCode commands for comP
//
// Commands:
// - comp.setupAgents: Configure MCP for AI agents
// - comp.forceReindex: Force complete re-indexing
// - comp.generateContext: Generate optimized context capsule
// - comp.showImpactGraph: Show impact analysis

import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import { DaemonManager } from "../daemon/DaemonManager";
import { StatusBar } from "./StatusBar";
import {
  AgentSetupManager,
  GenerateConfigResult,
  ManualFallback,
  UserScopeResult,
} from "../mcp/AgentSetup";

export function registerCommands(
  context: vscode.ExtensionContext,
  // WHY: A new DaemonManager is created on restart, so we accept a getter to get the latest
  // instance at invocation time instead of a static reference at registration.
  getDaemonManager: () => DaemonManager | null,
  statusBar: StatusBar
): void {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || ".";
  // _daemonManager is unused in AgentSetupManager (reserved for future expansion)
  const agentSetup = new AgentSetupManager(
    null as unknown as DaemonManager,
    workspaceRoot,
    context.extensionPath,
    { globalStorageDir: context.globalStorageUri?.fsPath }
  );

  // WHY a channel rather than a notification: setup writes outside the
  // workspace — into the home directory and VS Code's global storage — and the
  // list of touched files plus their backups is the only record the user gets.
  let setupChannel: vscode.OutputChannel | undefined;
  const reportChannel = (): vscode.OutputChannel => {
    if (!setupChannel) {
      setupChannel = vscode.window.createOutputChannel("comP Setup");
      context.subscriptions.push(setupChannel);
    }
    return setupChannel;
  };

  // Command 1: comp.setupAgents
  // Write comP into every config file the chosen agents actually read
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.setupAgents", async () => {
      const detected = agentSetup.detectInstalledAgents();
      // Detection only preselects: a false negative must never hide an agent
      // the user really has, so every supported agent stays on the list.
      const picked = await vscode.window.showQuickPick(
        AgentSetupManager.AGENTS.map((name) => ({
          label: name,
          description: detected.includes(name) ? "検出済み" : "",
          picked: detected.includes(name),
        })),
        {
          canPickMany: true,
          placeHolder: "comP を設定するエージェントを選んでください（検出済みは選択済み）",
        }
      );

      if (!picked || picked.length === 0) return;

      try {
        const results: { agent: string; result: GenerateConfigResult }[] = [];
        for (const item of picked) {
          results.push({ agent: item.label, result: await agentSetup.generateConfig(item.label) });
        }

        let userScope: UserScopeResult | undefined;
        if (picked.some((item) => item.label === "Claude Code")) {
          const answer = await vscode.window.showInformationMessage(
            "Claude Code に、全プロジェクト共通（ユーザースコープ）でも comP を登録しますか？",
            "登録する",
            "今はしない"
          );
          if (answer === "登録する") {
            userScope = await agentSetup.registerClaudeCodeUserScope();
          }
        }

        const channel = reportChannel();
        channel.clear();
        channel.appendLine(renderSetupReport(results, userScope));
        channel.show(true);

        const failures = results.flatMap((entry) =>
          (entry.result.manualFallback ?? []).map((fallback) => ({ agent: entry.agent, fallback }))
        );
        if (failures.length > 0) {
          await openManualFallback(workspaceRoot, failures);
        }

        const configured = results.filter((entry) => entry.result.success).length;
        vscode.window.showInformationMessage(
          failures.length === 0
            ? `${configured} 件のエージェントを設定しました。エージェントを再起動すると使えます（手順は出力パネルの「comP Setup」）。`
            : `${configured} 件設定しました。${failures.length} 件は手動設定が必要です。開いたタブを確認してください。`
        );
      } catch (error) {
        vscode.window.showErrorMessage(
          `Failed to setup MCP: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    })
  );

  // Command 2: comp.forceReindex
  // Perform complete workspace re-indexing
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.forceReindex", async () => {
      const proceed = await vscode.window.showWarningMessage(
        "This will re-index your entire workspace. Continue?",
        "Yes",
        "Cancel"
      );

      if (proceed !== "Yes") return;

      const dm = getDaemonManager();
      if (!dm?.isRunning()) {
        vscode.window.showErrorMessage("comP daemon is not running. Start it from the comP sidebar first.");
        return;
      }
      statusBar.show("Indexing...");
      try {
        await dm.request("forceReindex");
        const stats = await dm.getStats();
        statusBar.updateStats(stats.total_nodes, stats.total_files, "Ready", stats.efficiency || "0%");
        vscode.window.showInformationMessage(`Re-indexing completed: ${stats.total_nodes} symbols found`);
      } catch (error) {
        statusBar.show("Error");
        vscode.window.showErrorMessage(
          `Re-indexing failed: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    })
  );

  // Command 3: comp.generateContext
  // Generate optimized context for current task
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.generateContext", async () => {
      const task = await vscode.window.showInputBox({
        prompt: "Describe what you're working on",
        placeHolder: "e.g., add user authentication",
      });

      if (!task) return;

      const dm = getDaemonManager();
      if (!dm?.isRunning()) {
        vscode.window.showErrorMessage("comP daemon is not running. Start it from the comP sidebar first.");
        return;
      }
      try {
        const result = await vscode.window.withProgress(
          { location: vscode.ProgressLocation.Notification, title: "comP: Generating optimized context..." },
          () => dm.request("run_pipeline", { task })
        );
        const doc = await vscode.workspace.openTextDocument({
          content: JSON.stringify(result, null, 2),
          language: "json",
        });
        await vscode.window.showTextDocument(doc, { preview: true });
      } catch (error) {
        vscode.window.showErrorMessage(
          `Context generation failed: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    })
  );

  // Command 4: comp.showImpactGraph
  // Show impact analysis for symbol at cursor
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.showImpactGraph", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage("No active editor");
        return;
      }

      const document = editor.document;
      const position = editor.selection.active;
      const wordRange = document.getWordRangeAtPosition(position);
      if (!wordRange) {
        vscode.window.showErrorMessage("No symbol at cursor position");
        return;
      }
      const symbol = document.getText(wordRange);

      const dm = getDaemonManager();
      if (!dm?.isRunning()) {
        vscode.window.showErrorMessage("comP daemon is not running. Start it from the comP sidebar first.");
        return;
      }
      try {
        // get_symbol returns Markdown with the symbol's dependencies and dependents.
        const result = await dm.request("get_symbol", { name: symbol, compression_level: 2 });
        const content = typeof result === "string" ? result : JSON.stringify(result, null, 2);
        const doc = await vscode.workspace.openTextDocument({ content, language: "markdown" });
        await vscode.window.showTextDocument(doc, { preview: true });
      } catch (error) {
        vscode.window.showErrorMessage(
          `Impact analysis failed: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    })
  );

  // Command 5: comp.showStats (internal)
  // Show index statistics dashboard
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.showStats", async () => {
      const dm = getDaemonManager();
      try {
        if (!dm?.isRunning()) throw new Error("Daemon is not running");
        const stats = await dm.getStats();
        const message = `comP Statistics\n\nFiles: ${stats.total_files}\nSymbols: ${stats.total_nodes}\nDependencies: ${stats.total_edges}`;
        vscode.window.showInformationMessage(message);
      } catch (error) {
        vscode.window.showErrorMessage(`Failed to fetch stats: ${error instanceof Error ? error.message : String(error)}`);
      }
    })
  );

  // Command 6: comp.exportDebugLog
  // Export session-memory.json to a user-chosen location
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.exportDebugLog", async () => {
      const sessionMemoryPath = path.join(workspaceRoot, ".comp", "session-memory.json");

      if (!fs.existsSync(sessionMemoryPath)) {
        vscode.window.showWarningMessage(
          "No session memory found. Run a query via MCP first to generate logs."
        );
        return;
      }

      const choice = await vscode.window.showQuickPick(
        [
          { label: "Open in Editor", description: "View session-memory.json in a new tab" },
          { label: "Export to File", description: "Save a copy to a chosen location" },
        ],
        { placeHolder: "How do you want to view the debug log?" }
      );

      if (!choice) return;

      if (choice.label === "Open in Editor") {
        const uri = vscode.Uri.file(sessionMemoryPath);
        const doc = await vscode.workspace.openTextDocument(uri);
        await vscode.window.showTextDocument(doc, { preview: false });
        return;
      }

      // Export to file
      const defaultUri = vscode.Uri.file(
        path.join(workspaceRoot, `comp-debug-${Date.now()}.json`)
      );
      const saveUri = await vscode.window.showSaveDialog({
        defaultUri,
        filters: { "JSON": ["json"] },
        title: "Export comP Debug Log",
      });

      if (!saveUri) return;

      try {
        const content = fs.readFileSync(sessionMemoryPath, "utf-8");
        fs.writeFileSync(saveUri.fsPath, content, "utf-8");
        const openDoc = await vscode.window.showInformationMessage(
          `Debug log exported to ${path.basename(saveUri.fsPath)}`,
          "Open File"
        );
        if (openDoc === "Open File") {
          const doc = await vscode.workspace.openTextDocument(saveUri);
          await vscode.window.showTextDocument(doc, { preview: false });
        }
      } catch (error) {
        vscode.window.showErrorMessage(
          `Export failed: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    })
  );

  // Command 7: comp.copyActiveFileCompressed
  // Copy current active file with AST compression
  context.subscriptions.push(
    vscode.commands.registerCommand("comp.copyActiveFileCompressed", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage("No active editor. Open a file first.");
        return;
      }

      const dm = getDaemonManager();
      if (!dm?.isRunning()) {
        vscode.window.showErrorMessage("comP daemon is not running. Start it from the comP sidebar first.");
        return;
      }

      const levels = [
        { label: "Full Source", description: "No compression, copy full file", value: 0 },
        { label: "Compact", description: "Remove comments and empty lines", value: 1 },
        { label: "Skeleton", description: "Extract declarations only (signatures)", value: 2 }
      ];

      const selected = await vscode.window.showQuickPick(levels, {
        placeHolder: "Select compression level"
      });

      if (selected === undefined) return;

      const filePath = editor.document.uri.fsPath;
      try {
        await vscode.window.withProgress({
          location: vscode.ProgressLocation.Notification,
          title: "comP: Compressing file...",
          cancellable: false
        }, async () => {
          const { text, compressionRate } = await dm.compressFile(filePath, selected.value);
          await vscode.env.clipboard.writeText(text);
          vscode.window.showInformationMessage(`Copied to clipboard (${selected.label} mode, ${compressionRate} reduction).`);
        });
      } catch (error) {
        vscode.window.showErrorMessage(
          `Failed to compress file: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    })
  );
}

/**
 * The setup report written to the "comP Setup" output channel.
 *
 * Every touched path is listed with its scope and its backup, because a setup
 * run writes into the home directory and VS Code's global storage as well as
 * the workspace — files the user would otherwise have no way to find or undo.
 * The restart instructions are part of the report rather than a notification:
 * none of these agents re-read their config while running, so the run is not
 * finished until the user acts on them.
 */
function renderSetupReport(
  results: { agent: string; result: GenerateConfigResult }[],
  userScope?: UserScopeResult
): string {
  const lines: string[] = [];

  for (const { agent, result } of results) {
    lines.push(`=== ${agent} ===`);

    if (result.writes.length === 0) {
      lines.push(`  [FAIL] ${result.message}`);
    }
    for (const write of result.writes) {
      const mark =
        write.status === "written" ? "OK  " : write.status === "skipped" ? "SKIP" : "FAIL";
      const scope = write.scope === "global" ? "  (全プロジェクト共通)" : "";
      lines.push(`  [${mark}] ${write.path}${scope}`);
      if (write.backupPath) {
        lines.push(`         バックアップ: ${write.backupPath}`);
      }
      if (write.reason) {
        lines.push(`         理由: ${write.reason}`);
      }
    }
    for (const file of result.constitutionFiles) {
      lines.push(`  [RULE] ${file} に comP の利用ルールを追記しました`);
    }
    if (result.restartHint) {
      lines.push("  --- 反映するには ---");
      for (const line of result.restartHint.split("\n")) {
        lines.push(`  ${line}`);
      }
    }
    lines.push("");
  }

  if (userScope) {
    lines.push("=== Claude Code (ユーザースコープ) ===");
    if (userScope.registered) {
      lines.push("  [OK  ] claude mcp add --scope user を実行しました");
    } else {
      lines.push(`  [FAIL] ${userScope.reason ?? "登録できませんでした"}`);
      lines.push("  次のコマンドをターミナルで実行してください:");
      lines.push(`    ${userScope.command}`);
    }
    lines.push("");
  }

  return lines.join("\n");
}

/**
 * Open a document holding the entries that could not be written.
 *
 * This is the one path where the user still copies something by hand, so it is
 * shown only for the files that actually failed — a successful run opens
 * nothing.
 */
async function openManualFallback(
  workspaceRoot: string,
  failures: { agent: string; fallback: ManualFallback }[]
): Promise<void> {
  const lines = [
    "# comP: 手動設定が必要な項目",
    "",
    "自動書き込みに失敗したファイルです。既存の内容を確認したうえで、",
    "以下の設定を該当ファイルへマージしてください。",
    "",
  ];

  for (const { agent, fallback } of failures) {
    lines.push(`## ${agent}`);
    lines.push("");
    lines.push("対象ファイル:");
    lines.push("");
    lines.push("```text");
    lines.push(fallback.path);
    lines.push("```");
    lines.push("");
    lines.push("追記する内容:");
    lines.push("");
    lines.push("```");
    lines.push(fallback.content.trimEnd());
    lines.push("```");
    lines.push("");
  }

  const dir = path.join(workspaceRoot, ".comp");
  if (!fs.existsSync(dir)) {
    fs.mkdirSync(dir, { recursive: true });
  }
  const file = path.join(dir, `setup-manual-${Date.now()}.md`);
  fs.writeFileSync(file, lines.join("\n"), "utf-8");

  const doc = await vscode.workspace.openTextDocument(file);
  await vscode.window.showTextDocument(doc, { preview: false });
}
