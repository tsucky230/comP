// Commands Unit Tests
//
// Coverage:
// - registerCommands() registers all 5 expected commands
// - comp.showStats calls getStats and shows message
// - comp.forceReindex with "Yes" triggers daemon request
// - comp.forceReindex with "Cancel" skips reindex
// - comp.setupAgents with no selection does nothing
// - comp.generateContext with no input does nothing

import { expect } from "chai";
import * as sinon from "sinon";
import * as vscode from "vscode";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { registerCommands } from "../commands";

describe("registerCommands", () => {
  let mockContext: any;
  let mockDaemon: any;
  let mockStatusBar: any;
  let handlers: Map<string, () => Promise<void>>;

  // WHY a real temp directory: setupAgents writes config and instruction files
  // relative to the workspace root. Left undefined, the root falls back to "."
  // and a test run would rewrite this repository's own CLAUDE.md.
  const tmpRoot = path.join(os.tmpdir(), `comp-commands-${process.pid}`);
  let caseIndex = 0;
  let setupWorkspace: string;

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  beforeEach(() => {
    handlers = new Map();
    (vscode.commands as any).registerCommand = (name: string, handler: () => Promise<void>) => {
      handlers.set(name, handler);
      return { dispose: () => {} };
    };

    setupWorkspace = path.join(tmpRoot, `case-${caseIndex++}`);
    fs.mkdirSync(setupWorkspace, { recursive: true });

    mockContext = {
      subscriptions: { push: sinon.stub() },
      globalStorageUri: { fsPath: path.join(setupWorkspace, "globalStorage", "comp") },
    };

    mockDaemon = {
      request: sinon.stub().resolves({}),
      getStats: sinon.stub().resolves({ total_nodes: 10, total_files: 5, total_edges: 20 }),
      isRunning: sinon.stub().returns(true),
      compressFile: sinon.stub().resolves({ text: "compressed_output", compressionRate: "38%" }),
    };

    mockStatusBar = {
      show: sinon.stub(),
      updateStats: sinon.stub(),
    };

    (vscode.window as any).showQuickPick = sinon.stub().resolves(undefined);
    (vscode.window as any).showInputBox = sinon.stub().resolves(undefined);
    (vscode.window as any).showInformationMessage = sinon.stub().resolves(undefined);
    (vscode.window as any).showErrorMessage = sinon.stub().resolves(undefined);
    (vscode.window as any).showWarningMessage = sinon.stub().resolves("Cancel");
    (vscode.window as any).activeTextEditor = undefined;
    (vscode.window as any).withProgress = async (_options: any, task: () => Promise<any>) => {
      return await task();
    };
    (vscode.workspace as any).workspaceFolders = [{ uri: { fsPath: setupWorkspace } }];
    (vscode.env.clipboard as any).writeText = sinon.stub().resolves();
  });

  it("registers all 7 commands", () => {
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    expect(handlers.size).to.equal(7);
    expect(handlers.has("comp.setupAgents")).to.be.true;
    expect(handlers.has("comp.forceReindex")).to.be.true;
    expect(handlers.has("comp.generateContext")).to.be.true;
    expect(handlers.has("comp.showImpactGraph")).to.be.true;
    expect(handlers.has("comp.showStats")).to.be.true;
    expect(handlers.has("comp.copyActiveFileCompressed")).to.be.true;
    expect(handlers.has("comp.exportDebugLog")).to.be.true;
  });

  it("comp.showStats calls getStats and shows information message", async () => {
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.showStats")!();
    expect(mockDaemon.getStats.calledOnce).to.be.true;
    expect((vscode.window as any).showInformationMessage.calledOnce).to.be.true;
  });

  it('comp.forceReindex with "Yes" calls request("forceReindex")', async () => {
    (vscode.window as any).showWarningMessage = sinon.stub().resolves("Yes");
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.forceReindex")!();
    expect(mockDaemon.request.calledWith("forceReindex")).to.be.true;
  });

  it('comp.forceReindex with "Cancel" skips request', async () => {
    (vscode.window as any).showWarningMessage = sinon.stub().resolves("Cancel");
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.forceReindex")!();
    expect(mockDaemon.request.called).to.be.false;
  });

  it("comp.forceReindex updates status bar on success", async () => {
    (vscode.window as any).showWarningMessage = sinon.stub().resolves("Yes");
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.forceReindex")!();
    expect(mockStatusBar.updateStats.calledOnce).to.be.true;
  });

  it("comp.setupAgents with no selection does nothing", async () => {
    (vscode.window as any).showQuickPick = sinon.stub().resolves(undefined);
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.setupAgents")!();
    expect((vscode.window as any).showInformationMessage.called).to.be.false;
  });

  it("comp.setupAgents shows an English QuickPick placeholder by default", async () => {
    // The shared vscode mock defaults env.language to "en" (see src/test/setup.ts);
    // this is the state every other test in this file already runs under.
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);

    await handlers.get("comp.setupAgents")!();

    const placeHolder = (vscode.window as any).showQuickPick.getCall(0).args[1].placeHolder;
    expect(placeHolder).to.equal("Select the agents to configure comP for (detected ones are pre-selected)");
  });

  it("comp.setupAgents switches the QuickPick placeholder to Japanese under a Japanese VS Code locale", async () => {
    const originalLanguage = (vscode.env as any).language;
    (vscode.env as any).language = "ja";
    try {
      registerCommands(mockContext, () => mockDaemon, mockStatusBar);
      await handlers.get("comp.setupAgents")!();

      const placeHolder = (vscode.window as any).showQuickPick.getCall(0).args[1].placeHolder;
      expect(placeHolder).to.equal("comP を設定するエージェントを選んでください（検出済みは選択済み）");
    } finally {
      // The vscode mock is one shared instance across every test file in the
      // run, so leaving this changed would bleed into unrelated tests.
      (vscode.env as any).language = originalLanguage;
    }
  });

  it("comp.setupAgents with an empty multi-select does nothing", async () => {
    (vscode.window as any).showQuickPick = sinon.stub().resolves([]);
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.setupAgents")!();
    expect((vscode.window as any).showInformationMessage.called).to.be.false;
  });

  it("comp.setupAgents writes the config for every picked agent", async () => {
    (vscode.window as any).showQuickPick = sinon
      .stub()
      .resolves([{ label: "GitHub Copilot" }, { label: "Aider" }]);
    (vscode.window as any).showInformationMessage = sinon.stub().resolves("Done");
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);

    await handlers.get("comp.setupAgents")!();

    expect((vscode.window as any).showErrorMessage.called).to.be.false;
    expect(fs.existsSync(path.join(setupWorkspace, ".vscode", "mcp.json"))).to.be.true;
    expect(fs.existsSync(path.join(setupWorkspace, ".aider.conf.yml"))).to.be.true;
    // The rules land in the instruction file instead of a prompt to paste them.
    expect(fs.readFileSync(path.join(setupWorkspace, "CLAUDE.md"), "utf-8")).to.include(
      "comP MCP Tool Usage"
    );
    expect((vscode.window as any).showInformationMessage.called).to.be.true;
  });

  it("comp.setupAgents offers user-scope registration only for Claude Code", async () => {
    (vscode.window as any).showQuickPick = sinon.stub().resolves([{ label: "GitHub Copilot" }]);
    (vscode.window as any).showInformationMessage = sinon.stub().resolves(undefined);
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);

    await handlers.get("comp.setupAgents")!();

    // The user-scope confirmation only fires alongside a Claude Code pick.
    // With just GitHub Copilot selected, the only call left is the final
    // summary — so exactly one call, not the two a Claude Code pick produces.
    expect((vscode.window as any).showInformationMessage.callCount).to.equal(1);
  });

  it("comp.generateContext with no input does nothing", async () => {
    (vscode.window as any).showInputBox = sinon.stub().resolves(undefined);
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.generateContext")!();
    expect(mockDaemon.request.called).to.be.false;
  });

  it("comp.showImpactGraph with no active editor shows error", async () => {
    (vscode.window as any).activeTextEditor = undefined;
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.showImpactGraph")!();
    expect((vscode.window as any).showErrorMessage.calledOnce).to.be.true;
  });

  it("comp.copyActiveFileCompressed with no active editor shows error", async () => {
    (vscode.window as any).activeTextEditor = undefined;
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.copyActiveFileCompressed")!();
    expect((vscode.window as any).showErrorMessage.calledOnce).to.be.true;
  });

  it("comp.showImpactGraph with active editor calls get_symbol and opens the result", async () => {
    (vscode.window as any).activeTextEditor = {
      document: {
        fileName: "/path/file.ts",
        getWordRangeAtPosition: sinon.stub().returns({}),
        getText: sinon.stub().returns("mySymbol"),
      },
      selection: { active: { line: 5, character: 3 } },
    };
    mockDaemon.request = sinon.stub().resolves("## mySymbol\n\n- dependents: ...");
    (vscode.workspace as any).openTextDocument = sinon.stub().resolves({});
    (vscode.window as any).showTextDocument = sinon.stub().resolves(undefined);
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.showImpactGraph")!();
    expect(mockDaemon.request.calledWith("get_symbol", sinon.match({ name: "mySymbol" }))).to.be.true;
    expect((vscode.window as any).showTextDocument.calledOnce).to.be.true;
  });

  it("comp.showImpactGraph with no symbol at cursor shows error", async () => {
    (vscode.window as any).activeTextEditor = {
      document: {
        fileName: "/path/file.ts",
        getWordRangeAtPosition: sinon.stub().returns(undefined),
        getText: sinon.stub(),
      },
      selection: { active: { line: 5, character: 3 } },
    };
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.showImpactGraph")!();
    expect((vscode.window as any).showErrorMessage.calledOnce).to.be.true;
    expect(mockDaemon.request.called).to.be.false;
  });

  it("comp.generateContext with task input calls run_pipeline and opens the result", async () => {
    (vscode.window as any).showInputBox = sinon.stub().resolves("add user auth");
    mockDaemon.request = sinon.stub().resolves({ pivot_files: [] });
    (vscode.workspace as any).openTextDocument = sinon.stub().resolves({});
    (vscode.window as any).showTextDocument = sinon.stub().resolves(undefined);
    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.generateContext")!();
    expect(mockDaemon.request.calledWith("run_pipeline", sinon.match({ task: "add user auth" }))).to.be.true;
    expect((vscode.window as any).showTextDocument.calledOnce).to.be.true;
  });

  it("comp.showStats shows error when daemon is not running", async () => {
    const notRunning = { ...mockDaemon, isRunning: sinon.stub().returns(false) };
    registerCommands(mockContext, () => notRunning, mockStatusBar);
    await handlers.get("comp.showStats")!();
    expect((vscode.window as any).showErrorMessage.calledOnce).to.be.true;
  });

  it("comp.forceReindex shows error when daemon is not running", async () => {
    (vscode.window as any).showWarningMessage = sinon.stub().resolves("Yes");
    const notRunning = { ...mockDaemon, isRunning: sinon.stub().returns(false) };
    registerCommands(mockContext, () => notRunning, mockStatusBar);
    await handlers.get("comp.forceReindex")!();
    expect((vscode.window as any).showErrorMessage.calledOnce).to.be.true;
  });

  it("comp.exportDebugLog shows warning when session-memory.json not found", async () => {
    const fs = require("fs");
    const existsStub = sinon.stub(fs, "existsSync").returns(false);
    try {
      registerCommands(mockContext, () => mockDaemon, mockStatusBar);
      await handlers.get("comp.exportDebugLog")!();
      expect((vscode.window as any).showWarningMessage.calledOnce).to.be.true;
    } finally {
      existsStub.restore();
    }
  });

  it("comp.copyActiveFileCompressed shows error when daemon is not running", async () => {
    (vscode.window as any).activeTextEditor = {
      document: { uri: vscode.Uri.file("/workspace/test.rs") },
    };
    const notRunning = { ...mockDaemon, isRunning: sinon.stub().returns(false) };
    registerCommands(mockContext, () => notRunning, mockStatusBar);
    await handlers.get("comp.copyActiveFileCompressed")!();
    expect((vscode.window as any).showErrorMessage.calledOnce).to.be.true;
  });

  it("comp.copyActiveFileCompressed with active editor and selection calls compressFile and writes clipboard", async () => {
    // Setup active editor
    (vscode.window as any).activeTextEditor = {
      document: {
        uri: vscode.Uri.file("/workspace/test_file.rs"),
        languageId: "rust"
      }
    };
    // Mock QuickPick selection: Compact level
    const selection = { label: "Compact", value: 1 };
    (vscode.window as any).showQuickPick = sinon.stub().resolves(selection);

    registerCommands(mockContext, () => mockDaemon, mockStatusBar);
    await handlers.get("comp.copyActiveFileCompressed")!();

    expect(mockDaemon.compressFile.calledWith("/workspace/test_file.rs", 1)).to.be.true;
    expect((vscode.env as any).clipboard.writeText.calledWith("compressed_output")).to.be.true;
  });
});
