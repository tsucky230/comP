// Unit tests for repairMcpConfigs — the activate() wiring around
// AgentSetupManager.repairStaleConfigs()
//
// Coverage:
// - No workspace open: returns silently
// - Stale daemon path: rewritten and the user is notified once
// - Healthy config: file untouched, no notification
// - Failure inside the repair: swallowed so activation is never blocked

import { expect } from "chai";
import * as sinon from "sinon";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import * as vscode from "vscode";
import { repairMcpConfigs } from "../extension";

describe("repairMcpConfigs", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-activate-repair-${process.pid}`);
  const devBinaryName = process.platform === "win32" ? "comp-daemon.exe" : "comp-daemon";

  let caseIndex = 0;
  let ws: string;
  let extDir: string;
  let devBinary: string;
  let stalePath: string;
  let configPath: string;
  let mockContext: any;
  let infoStub: sinon.SinonStub;
  let originalHome: string | undefined;
  let originalUserProfile: string | undefined;

  const writeJson = (file: string, value: unknown): void => {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify(value, null, 2), "utf-8");
  };

  beforeEach(() => {
    const caseDir = path.join(tmpRoot, `case-${caseIndex++}`);
    ws = path.join(caseDir, "workspace");
    extDir = path.join(caseDir, "extension");
    devBinary = path.join(ws, "daemon", "target", "release", devBinaryName);
    stalePath = path.join(caseDir, "comp-vscode-0.9.2", devBinaryName);
    configPath = path.join(ws, ".vscode", "mcp.json");

    fs.mkdirSync(path.dirname(devBinary), { recursive: true });
    fs.writeFileSync(devBinary, "");

    // WHY: repairMcpConfigs builds its own AgentSetupManager, so the Antigravity
    // target cannot be stubbed from here. Point the home directory at the case
    // directory instead — otherwise a test run would inspect, and possibly
    // rewrite, the developer's real ~/.gemini MCP config.
    originalHome = process.env.HOME;
    originalUserProfile = process.env.USERPROFILE;
    process.env.HOME = caseDir;
    process.env.USERPROFILE = caseDir;

    mockContext = { extensionPath: extDir };
    infoStub = sinon.stub().resolves(undefined);
    (vscode.window as any).showInformationMessage = infoStub;
    (vscode.workspace as any).workspaceFolders = [{ uri: { fsPath: ws } }];
  });

  afterEach(() => {
    process.env.HOME = originalHome;
    process.env.USERPROFILE = originalUserProfile;
    if (originalHome === undefined) delete process.env.HOME;
    if (originalUserProfile === undefined) delete process.env.USERPROFILE;
    (vscode.workspace as any).workspaceFolders = undefined;
  });

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  it("does nothing when no workspace folder is open", () => {
    (vscode.workspace as any).workspaceFolders = undefined;
    writeJson(configPath, { servers: { comp: { command: stalePath } } });

    expect(() => repairMcpConfigs(mockContext)).to.not.throw();

    expect(infoStub.called).to.be.false;
    expect(JSON.parse(fs.readFileSync(configPath, "utf-8")).servers.comp.command).to.equal(stalePath);
  });

  it("rewrites a stale daemon path and notifies the user once", () => {
    writeJson(configPath, {
      servers: { comp: { command: stalePath, args: [], env: { COMP_WORKSPACE_ROOT: ws } } },
    });

    repairMcpConfigs(mockContext);

    expect(JSON.parse(fs.readFileSync(configPath, "utf-8")).servers.comp.command).to.equal(devBinary);
    expect(infoStub.calledOnce).to.be.true;
    expect(infoStub.firstCall.args[0]).to.contain("MCP");
  });

  it("leaves a healthy config untouched and stays quiet", () => {
    writeJson(configPath, {
      servers: { comp: { command: devBinary, env: { COMP_WORKSPACE_ROOT: ws } } },
    });
    const before = fs.readFileSync(configPath, "utf-8");

    repairMcpConfigs(mockContext);

    expect(fs.readFileSync(configPath, "utf-8")).to.equal(before);
    expect(infoStub.called).to.be.false;
  });

  it("swallows a failure so activation is never blocked", () => {
    // A non-string fsPath is truthy but makes path.join throw inside the manager
    (vscode.workspace as any).workspaceFolders = [{ uri: { fsPath: 42 } }];

    expect(() => repairMcpConfigs(mockContext)).to.not.throw();

    expect(infoStub.called).to.be.false;
  });
});
