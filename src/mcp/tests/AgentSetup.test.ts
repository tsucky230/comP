// AgentSetup Tests
//
// Coverage:
// - Config generation for each agent
// - File path resolution
// - Error handling for unsupported agents
// - Directory creation for config files
// - Repair of daemon paths left stale by an extension upgrade

import { expect } from "chai";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { AgentSetupManager, RepairEntry } from "../AgentSetup";
import { DaemonManager } from "../../daemon/DaemonManager";

// Mock DaemonManager
class MockDaemonManager implements Partial<DaemonManager> {
  async request(_method: string, _params?: unknown): Promise<unknown> {
    return {};
  }
}

describe("AgentSetupManager", () => {
  let manager: AgentSetupManager;
  let mockDaemon: MockDaemonManager;
  const testWorkspace = "/tmp/test-workspace";

  beforeEach(() => {
    mockDaemon = new MockDaemonManager();
    manager = new AgentSetupManager(mockDaemon as any, testWorkspace);
  });

  describe("getAgentConfig", () => {
    it("should return Claude Code config", () => {
      const config = manager.getAgentConfig("Claude Code");

      expect(config).to.exist;
      expect(config?.name).to.equal("Claude Code");
      expect(config?.configPath).to.equal(".mcp.json");
    });

    it("should return Cursor config", () => {
      const config = manager.getAgentConfig("Cursor");

      expect(config).to.exist;
      expect(config?.name).to.equal("Cursor");
      expect(config?.configPath).to.include("cursor_config.json");
    });

    it("should return Cline config", () => {
      const config = manager.getAgentConfig("Cline");

      expect(config).to.exist;
      expect(config?.name).to.equal("Cline");
      expect(config?.configPath).to.include("cline_config.json");
    });

    it("should return Windsurf config", () => {
      const config = manager.getAgentConfig("Windsurf");

      expect(config).to.exist;
      expect(config?.name).to.equal("Windsurf");
      expect(config?.configPath).to.include("windsurf_config.json");
    });

    it("should return Continue config", () => {
      const config = manager.getAgentConfig("Continue");

      expect(config).to.exist;
      expect(config?.name).to.equal("Continue");
      expect(config?.configPath).to.include("continue_config.py");
    });

    it("should return GitHub Copilot config", () => {
      const config = manager.getAgentConfig("GitHub Copilot");

      expect(config).to.exist;
      expect(config?.name).to.equal("GitHub Copilot");
      expect(config?.configPath).to.include("mcp.json");
    });

    it("should return Aider config", () => {
      const config = manager.getAgentConfig("Aider");

      expect(config).to.exist;
      expect(config?.name).to.equal("Aider");
      expect(config?.configPath).to.include(".aider.conf.yml");
    });

    it("should return null for unsupported agent", () => {
      const config = manager.getAgentConfig("UnsupportedAgent");

      expect(config).to.be.null;
    });
  });

  describe("generateConfig", () => {
    it("should generate Claude Code MCP configuration", async () => {
      const result = await manager.generateConfig("Claude Code");

      expect(result.success).to.be.true;
      expect(result.configPath).to.include(".mcp.json");
      expect(result.message).to.include("Claude Code");
      expect(result.command).to.exist;
      expect(result.command).to.include("claude mcp add comp");
      expect(result.command).to.include(`COMP_WORKSPACE_ROOT="${testWorkspace}"`);
      expect(result.llmPrompt).to.exist;
      expect(result.llmPrompt).to.include(".mcp.json");

      // Verify content is valid JSON
      const content = fs.readFileSync(result.configPath, "utf-8");
      const config = JSON.parse(content);
      expect(config.mcpServers).to.exist;
      expect(config.mcpServers.comp).to.exist;
    });

    it("should generate Cursor MCP configuration", async () => {
      const result = await manager.generateConfig("Cursor");

      expect(result.success).to.be.true;
      expect(result.configPath).to.include("cursor_config.json");
    });

    it("should generate Cline MCP configuration", async () => {
      const result = await manager.generateConfig("Cline");

      expect(result.success).to.be.true;
      expect(result.configPath).to.include("cline_config.json");
    });

    it("should generate Windsurf MCP configuration", async () => {
      const result = await manager.generateConfig("Windsurf");

      expect(result.success).to.be.true;
      expect(result.configPath).to.include("windsurf_config.json");
    });

    it("should generate GitHub Copilot MCP configuration", async () => {
      const result = await manager.generateConfig("GitHub Copilot");

      expect(result.success).to.be.true;
      expect(result.configPath).to.include("mcp.json");

      // Verify content
      const content = fs.readFileSync(result.configPath, "utf-8");
      const config = JSON.parse(content);
      expect(config.servers).to.exist;
      expect(config.servers.comp).to.exist;
    });

    it("should fail for unsupported agent", async () => {
      const result = await manager.generateConfig("UnsupportedAgent");

      expect(result.success).to.be.false;
      expect(result.message).to.include("not supported");
    });

    it("should create directory if it does not exist", async () => {
      const result = await manager.generateConfig("Claude Code");

      expect(result.success).to.be.true;
      const dir = path.dirname(result.configPath);
      expect(fs.existsSync(dir)).to.be.true;
    });
  });

  describe("config content validation", () => {
    it("Claude Code config should contain MCP servers object", async () => {
      const result = await manager.generateConfig("Claude Code");
      const content = fs.readFileSync(result.configPath, "utf-8");
      const config = JSON.parse(content);

      expect(config.mcpServers.comp.command).to.exist;
      expect(config.mcpServers.comp.env).to.exist;
      expect(config.mcpServers.comp.env.COMP_WORKSPACE_ROOT).to.equal(testWorkspace);
    });

    it("Cline config should contain MCP servers object", async () => {
      const result = await manager.generateConfig("Cline");
      const content = fs.readFileSync(result.configPath, "utf-8");
      const config = JSON.parse(content);

      expect(config.mcpServers).to.exist;
      expect(config.mcpServers.comp).to.exist;
    });

    it("Continue config should be Python-compatible", async () => {
      const result = await manager.generateConfig("Continue");
      const content = fs.readFileSync(result.configPath, "utf-8");

      // Should contain Python-like syntax
      expect(content).to.include("mcp_servers");
      expect(content).to.include("COMP_WORKSPACE_ROOT");
    });

    it("Aider config should contain mcp-servers YAML block", async () => {
      const result = await manager.generateConfig("Aider");
      expect(result.success).to.be.true;
      expect(result.configPath).to.include(".aider.conf.yml");

      const content = fs.readFileSync(result.configPath, "utf-8");
      expect(content).to.include("mcp-servers:");
      expect(content).to.include("comp:");
      expect(content).to.include("COMP_WORKSPACE_ROOT");
    });

    it("Aider config should warn when mcp-servers block already exists", async () => {
      // Pre-create a config file with an existing mcp-servers block
      const configPath = path.join(testWorkspace, ".aider.conf.yml");
      fs.writeFileSync(configPath, "mcp-servers:\n  other-server:\n    command: /usr/bin/other\n");

      const result = await manager.generateConfig("Aider");
      expect(result.success).to.be.true;

      const content = fs.readFileSync(result.configPath, "utf-8");
      expect(content).to.include("WARNING");
      expect(content).to.include("existing config below");
    });
  });

  describe("repairStaleConfigs", () => {
    const tmpRoot = path.join(os.tmpdir(), `comp-repair-${process.pid}`);
    const devBinaryName = process.platform === "win32" ? "comp-daemon.exe" : "comp-daemon";
    const bundledBinaryName =
      process.platform === "win32"
        ? "comp-daemon-win.exe"
        : process.platform === "darwin"
          ? "comp-daemon-macos"
          : "comp-daemon-linux";

    let caseIndex = 0;
    let ws: string;
    let extDir: string;
    let globalConfig: string;
    let devBinary: string;
    let bundledBinary: string;
    let stalePath: string;
    let repairManager: AgentSetupManager;

    const writeJson = (file: string, value: unknown): void => {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, JSON.stringify(value, null, 2), "utf-8");
    };
    const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));
    const entryFor = (entries: RepairEntry[], file: string): RepairEntry | undefined =>
      entries.find((e) => e.file === file);

    beforeEach(() => {
      // Each case gets a private tree so repairs in one test cannot leak into another
      const caseDir = path.join(tmpRoot, `case-${caseIndex++}`);
      ws = path.join(caseDir, "workspace");
      extDir = path.join(caseDir, "extension");
      globalConfig = path.join(caseDir, "global", "mcp_config.json");
      devBinary = path.join(ws, "daemon", "target", "release", devBinaryName);
      bundledBinary = path.join(extDir, "daemon", "target", "release", bundledBinaryName);
      stalePath = path.join(caseDir, "comp-vscode-0.9.2", "daemon", "target", "release", bundledBinaryName);

      fs.mkdirSync(path.dirname(devBinary), { recursive: true });
      fs.writeFileSync(devBinary, "");
      fs.mkdirSync(path.dirname(bundledBinary), { recursive: true });
      fs.writeFileSync(bundledBinary, "");
      fs.mkdirSync(path.dirname(globalConfig), { recursive: true });

      repairManager = new AgentSetupManager(mockDaemon as any, ws, extDir);
      // WHY: the global target defaults to the real ~/.gemini config. Redirect it so
      // running the suite never edits the developer's own machine-wide MCP setup.
      (repairManager as any).antigravityConfigPath = () => globalConfig;
    });

    after(() => {
      fs.rmSync(tmpRoot, { recursive: true, force: true });
    });

    it("B1: rewrites a .vscode/mcp.json command that no longer exists", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, {
        servers: { comp: { command: stalePath, args: [], env: { COMP_WORKSPACE_ROOT: ws, RUST_LOG: "info" } } },
      });

      const entries = repairManager.repairStaleConfigs();

      const entry = entryFor(entries, cfg);
      expect(entry?.status).to.equal("repaired");
      expect(entry?.from).to.equal(stalePath);
      expect(entry?.to).to.equal(devBinary);
      expect(readJson(cfg).servers.comp.command).to.equal(devBinary);
    });

    it("B2: leaves a command that resolves to an existing file untouched", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, { servers: { comp: { command: devBinary, env: { COMP_WORKSPACE_ROOT: ws } } } });
      const before = fs.readFileSync(cfg, "utf-8");

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("healthy");
      expect(fs.readFileSync(cfg, "utf-8")).to.equal(before);
    });

    it("B3: leaves a relative command untouched", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      const relative = path.join("daemon", "target", "release", devBinaryName);
      writeJson(cfg, { servers: { comp: { command: relative, env: { COMP_WORKSPACE_ROOT: "." } } } });
      const before = fs.readFileSync(cfg, "utf-8");

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("skipped");
      expect(fs.readFileSync(cfg, "utf-8")).to.equal(before);
    });

    it("B4: leaves a command containing a ${...} variable untouched", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, {
        servers: { comp: { command: "${workspaceFolder}/daemon/target/release/comp-daemon", env: { COMP_WORKSPACE_ROOT: "${workspaceFolder}" } } },
      });
      const before = fs.readFileSync(cfg, "utf-8");

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("skipped");
      expect(fs.readFileSync(cfg, "utf-8")).to.equal(before);
    });

    it("B5: reports missing files without throwing", () => {
      const entries = repairManager.repairStaleConfigs();

      expect(entries.length).to.be.greaterThan(0);
      expect(entries.every((e) => e.status === "missing")).to.be.true;
    });

    it("B6: reports unparseable JSON as failed and leaves the file alone", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      fs.mkdirSync(path.dirname(cfg), { recursive: true });
      fs.writeFileSync(cfg, "{ this is not json", "utf-8");

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("failed");
      expect(fs.readFileSync(cfg, "utf-8")).to.equal("{ this is not json");
    });

    it("B7: preserves other MCP server entries while repairing comp", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, {
        servers: {
          comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: ws } },
          other: { command: "/usr/bin/other-server", args: ["--flag"] },
        },
      });

      repairManager.repairStaleConfigs();

      const written = readJson(cfg);
      expect(written.servers.comp.command).to.equal(devBinary);
      expect(written.servers.other.command).to.equal("/usr/bin/other-server");
      expect(written.servers.other.args).to.deep.equal(["--flag"]);
    });

    it("B8: preserves unknown top-level keys while repairing comp", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, {
        inputs: [{ id: "api-key", type: "promptString" }],
        servers: { comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: ws } } },
      });

      repairManager.repairStaleConfigs();

      const written = readJson(cfg);
      expect(written.inputs).to.deep.equal([{ id: "api-key", type: "promptString" }]);
      expect(written.servers.comp.command).to.equal(devBinary);
    });

    it("B9: does not rewrite when no replacement binary exists", () => {
      fs.rmSync(devBinary);
      fs.rmSync(bundledBinary);
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, { servers: { comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: ws } } } });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("skipped");
      expect(readJson(cfg).servers.comp.command).to.equal(stalePath);
    });

    it("B10: repairs .mcp.json under the mcpServers key", () => {
      const cfg = path.join(ws, ".mcp.json");
      writeJson(cfg, { mcpServers: { comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: ws } } } });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("repaired");
      expect(readJson(cfg).mcpServers.comp.command).to.equal(devBinary);
    });

    it("repairs the Cursor config whose comp entry sits at the document root", () => {
      const cfg = path.join(ws, ".comp", "config", "cursor_config.json");
      writeJson(cfg, { comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: ws } } });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("repaired");
      expect(readJson(cfg).comp.command).to.equal(devBinary);
    });

    it("refreshes a stale COMP_WORKSPACE_ROOT even when the command is healthy", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      const otherMachineRoot = path.join(tmpRoot, "c-home-project");
      writeJson(cfg, {
        servers: { comp: { command: devBinary, env: { COMP_WORKSPACE_ROOT: otherMachineRoot, RUST_LOG: "info" } } },
      });

      const entries = repairManager.repairStaleConfigs();

      const entry = entryFor(entries, cfg);
      expect(entry?.status).to.equal("repaired");
      expect(entry?.envRepaired).to.be.true;
      const written = readJson(cfg);
      expect(written.servers.comp.env.COMP_WORKSPACE_ROOT).to.equal(ws);
      expect(written.servers.comp.env.RUST_LOG).to.equal("info");
    });

    it("repairs the global config with the bundled binary, never the dev build", () => {
      writeJson(globalConfig, {
        mcpServers: { comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: path.join(tmpRoot, "other-project") } } },
      });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, globalConfig)?.status).to.equal("repaired");
      const written = readJson(globalConfig);
      expect(written.mcpServers.comp.command).to.equal(bundledBinary);
      expect(written.mcpServers.comp.command).to.not.equal(devBinary);
    });

    it("never rewrites COMP_WORKSPACE_ROOT in the global config", () => {
      const otherProject = path.join(tmpRoot, "other-project");
      writeJson(globalConfig, {
        mcpServers: { comp: { command: stalePath, env: { COMP_WORKSPACE_ROOT: otherProject } } },
      });

      repairManager.repairStaleConfigs();

      expect(readJson(globalConfig).mcpServers.comp.env.COMP_WORKSPACE_ROOT).to.equal(otherProject);
    });

    it("skips a file that has no comp server entry", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, { servers: { other: { command: "/usr/bin/other-server" } } });
      const before = fs.readFileSync(cfg, "utf-8");

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("skipped");
      expect(fs.readFileSync(cfg, "utf-8")).to.equal(before);
    });

    it("leaves a ${...} COMP_WORKSPACE_ROOT alone when the command is healthy", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, {
        servers: { comp: { command: devBinary, env: { COMP_WORKSPACE_ROOT: "${workspaceFolder}" } } },
      });
      const before = fs.readFileSync(cfg, "utf-8");

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("healthy");
      expect(fs.readFileSync(cfg, "utf-8")).to.equal(before);
    });

    it("skips a comp entry that carries no command key", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, { servers: { comp: { args: [], env: { COMP_WORKSPACE_ROOT: ws } } } });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("skipped");
    });

    it("treats a comp entry without an env object as healthy", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, { servers: { comp: { command: devBinary, args: [] } } });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("healthy");
    });

    it("treats an env without COMP_WORKSPACE_ROOT as healthy", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, { servers: { comp: { command: devBinary, env: { RUST_LOG: "info" } } } });

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("healthy");
    });

    it("skips a config whose document root is not an object", () => {
      const cfg = path.join(ws, ".vscode", "mcp.json");
      writeJson(cfg, ["not", "an", "object"]);

      const entries = repairManager.repairStaleConfigs();

      expect(entryFor(entries, cfg)?.status).to.equal("skipped");
    });

    it("cannot repair a global config when the extension path is unknown", () => {
      const noExtManager = new AgentSetupManager(mockDaemon as any, ws, undefined);
      (noExtManager as any).antigravityConfigPath = () => globalConfig;
      writeJson(globalConfig, { mcpServers: { comp: { command: stalePath } } });

      const entries = noExtManager.repairStaleConfigs();

      expect(entryFor(entries, globalConfig)?.status).to.equal("skipped");
      expect(readJson(globalConfig).mcpServers.comp.command).to.equal(stalePath);
    });
  });
});

