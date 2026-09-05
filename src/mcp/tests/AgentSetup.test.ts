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
import { AgentSetupManager, GenerateConfigResult, RepairEntry } from "../AgentSetup";
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

  const tmpRoot = path.join(os.tmpdir(), `comp-agentsetup-${process.pid}`);
  let caseIndex = 0;
  let caseDir: string;
  let testWorkspace: string;
  let fakeHome: string;
  let globalStorageDir: string;
  let codexHome: string;

  beforeEach(() => {
    mockDaemon = new MockDaemonManager();
    // Every case gets a private tree. fakeHome matters most: several agents keep
    // their only config under the home directory, so a real os.homedir() here
    // would rewrite the developer's own Cursor and Windsurf settings.
    caseDir = path.join(tmpRoot, `case-${caseIndex++}`);
    testWorkspace = path.join(caseDir, "workspace");
    fakeHome = path.join(caseDir, "home");
    // Mirrors VS Code's layout: <...>/User/globalStorage/<publisher>.<extension>
    globalStorageDir = path.join(caseDir, "globalStorage", "tsucky230.comp-vscode");
    // Codex honours CODEX_HOME, so without this override a developer who has it
    // set would have their real Codex config rewritten by this suite.
    codexHome = path.join(caseDir, "codex-home");

    fs.mkdirSync(testWorkspace, { recursive: true });
    fs.mkdirSync(fakeHome, { recursive: true });
    fs.mkdirSync(globalStorageDir, { recursive: true });

    manager = new AgentSetupManager(mockDaemon as any, testWorkspace, undefined, {
      homeDir: fakeHome,
      globalStorageDir,
      codexHome,
    });
  });

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));
  const writeJson = (file: string, value: unknown): void => {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify(value, null, 2), "utf-8");
  };
  const writtenPaths = (result: GenerateConfigResult): string[] =>
    result.writes.filter((w) => w.status === "written").map((w) => w.path);
  const readText = (file: string): string => fs.readFileSync(file, "utf-8");
  const writeText = (file: string, value: string): void => {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, value, "utf-8");
  };
  const codexGlobal = (): string => path.join(codexHome, "config.toml");
  const codexProject = (): string => path.join(testWorkspace, ".codex", "config.toml");

  describe("target paths", () => {
    it("Claude Code writes only the project .mcp.json", async () => {
      const result = await manager.generateConfig("Claude Code");

      expect(result.success).to.be.true;
      expect(writtenPaths(result)).to.deep.equal([path.join(testWorkspace, ".mcp.json")]);
      expect(readJson(result.configPath).mcpServers.comp).to.exist;
    });

    it("Codex writes the global config.toml before the project one", async () => {
      const result = await manager.generateConfig("Codex");

      expect(result.success).to.be.true;
      // The global file is always read by Codex; the project one only for a
      // project the user trusts, so the reliable target has to come first.
      expect(writtenPaths(result)).to.deep.equal([codexGlobal(), codexProject()]);
      expect(readText(codexGlobal())).to.include("[mcp_servers.comp]");
    });

    it("Cursor writes both the project and the global mcp.json", async () => {
      const result = await manager.generateConfig("Cursor");

      expect(result.success).to.be.true;
      expect(writtenPaths(result)).to.deep.equal([
        path.join(testWorkspace, ".cursor", "mcp.json"),
        path.join(fakeHome, ".cursor", "mcp.json"),
      ]);
    });

    it("Windsurf writes the global codeium config", async () => {
      const result = await manager.generateConfig("Windsurf");

      expect(writtenPaths(result)).to.deep.equal([
        path.join(fakeHome, ".codeium", "windsurf", "mcp_config.json"),
      ]);
    });

    it("Cline writes into the VS Code globalStorage sibling directory", async () => {
      const result = await manager.generateConfig("Cline");

      expect(result.success).to.be.true;
      expect(result.configPath).to.equal(
        path.join(
          path.dirname(globalStorageDir),
          "saoudrizwan.claude-dev",
          "settings",
          "cline_mcp_settings.json"
        )
      );
      expect(readJson(result.configPath).mcpServers.comp).to.exist;
    });

    it("Cline fails cleanly when the globalStorage location is unknown", async () => {
      const blind = new AgentSetupManager(mockDaemon as any, testWorkspace, undefined, {
        homeDir: fakeHome,
      });

      const result = await blind.generateConfig("Cline");

      expect(result.success).to.be.false;
      expect(result.writes).to.be.empty;
      expect(result.message).to.include("Cline");
    });

    it("Continue writes a block file in both the project and the home directory", async () => {
      const result = await manager.generateConfig("Continue");

      expect(writtenPaths(result)).to.deep.equal([
        path.join(testWorkspace, ".continue", "mcpServers", "comp.yaml"),
        path.join(fakeHome, ".continue", "mcpServers", "comp.yaml"),
      ]);
    });

    it("GitHub Copilot writes .vscode/mcp.json under the servers key", async () => {
      const result = await manager.generateConfig("GitHub Copilot");

      expect(result.configPath).to.equal(path.join(testWorkspace, ".vscode", "mcp.json"));
      expect(readJson(result.configPath).servers.comp).to.exist;
    });

    it("Antigravity writes the global gemini config", async () => {
      const result = await manager.generateConfig("Antigravity");

      expect(writtenPaths(result)).to.deep.equal([
        path.join(fakeHome, ".gemini", "antigravity-ide", "mcp_config.json"),
      ]);
    });

    it("Aider writes .aider.conf.yml", async () => {
      const result = await manager.generateConfig("Aider");

      expect(result.configPath).to.equal(path.join(testWorkspace, ".aider.conf.yml"));
      const content = fs.readFileSync(result.configPath, "utf-8");
      expect(content).to.include("mcp-servers:");
      expect(content).to.include("aider --version");
    });

    it("reports an unsupported agent instead of writing anything", async () => {
      const result = await manager.generateConfig("UnsupportedAgent");

      expect(result.success).to.be.false;
      expect(result.message).to.include("not supported");
      expect(result.writes).to.be.empty;
    });
  });

  describe("workspace root scoping", () => {
    it("workspace-scoped entries carry COMP_WORKSPACE_ROOT", async () => {
      await manager.generateConfig("Cursor");

      const project = readJson(path.join(testWorkspace, ".cursor", "mcp.json"));
      expect(project.mcpServers.comp.env.COMP_WORKSPACE_ROOT).to.equal(testWorkspace);
    });

    it("global entries omit COMP_WORKSPACE_ROOT so other projects are not hijacked", async () => {
      await manager.generateConfig("Cursor");
      await manager.generateConfig("Windsurf");
      await manager.generateConfig("Cline");

      const cursorGlobal = readJson(path.join(fakeHome, ".cursor", "mcp.json"));
      const windsurf = readJson(path.join(fakeHome, ".codeium", "windsurf", "mcp_config.json"));
      const cline = readJson(
        path.join(
          path.dirname(globalStorageDir),
          "saoudrizwan.claude-dev",
          "settings",
          "cline_mcp_settings.json"
        )
      );

      expect(cursorGlobal.mcpServers.comp.env).to.not.have.property("COMP_WORKSPACE_ROOT");
      expect(windsurf.mcpServers.comp.env).to.not.have.property("COMP_WORKSPACE_ROOT");
      expect(cline.mcpServers.comp.env).to.not.have.property("COMP_WORKSPACE_ROOT");
    });

    it("the global Continue block omits COMP_WORKSPACE_ROOT but the project one keeps it", async () => {
      await manager.generateConfig("Continue");

      const project = fs.readFileSync(
        path.join(testWorkspace, ".continue", "mcpServers", "comp.yaml"),
        "utf-8"
      );
      const global = fs.readFileSync(
        path.join(fakeHome, ".continue", "mcpServers", "comp.yaml"),
        "utf-8"
      );

      expect(project).to.include("COMP_WORKSPACE_ROOT");
      expect(global).to.not.include("COMP_WORKSPACE_ROOT");
    });

    it("the global Codex table omits COMP_WORKSPACE_ROOT but the project one keeps it", async () => {
      await manager.generateConfig("Codex");

      expect(readText(codexProject())).to.include(`COMP_WORKSPACE_ROOT = '${testWorkspace}'`);
      expect(readText(codexGlobal())).to.not.include("COMP_WORKSPACE_ROOT");
    });
  });

  describe("merging with existing configuration", () => {
    it("preserves other MCP servers in .mcp.json", async () => {
      const file = path.join(testWorkspace, ".mcp.json");
      writeJson(file, { mcpServers: { other: { command: "/usr/bin/other" } } });

      await manager.generateConfig("Claude Code");

      const merged = readJson(file);
      expect(merged.mcpServers.other.command).to.equal("/usr/bin/other");
      expect(merged.mcpServers.comp).to.exist;
    });

    it("preserves unrelated top-level keys and other servers in .vscode/mcp.json", async () => {
      const file = path.join(testWorkspace, ".vscode", "mcp.json");
      writeJson(file, { inputs: [{ id: "token" }], servers: { other: { command: "x" } } });

      await manager.generateConfig("GitHub Copilot");

      const merged = readJson(file);
      expect(merged.inputs).to.deep.equal([{ id: "token" }]);
      expect(merged.servers.other.command).to.equal("x");
      expect(merged.servers.comp).to.exist;
    });

    it("preserves other servers in the global Cursor config", async () => {
      const file = path.join(fakeHome, ".cursor", "mcp.json");
      writeJson(file, { mcpServers: { other: { command: "x" } } });

      await manager.generateConfig("Cursor");

      expect(readJson(file).mcpServers.other.command).to.equal("x");
      expect(readJson(file).mcpServers.comp).to.exist;
    });

    it("preserves other servers in the Windsurf config", async () => {
      const file = path.join(fakeHome, ".codeium", "windsurf", "mcp_config.json");
      writeJson(file, { mcpServers: { other: { command: "x" } } });

      await manager.generateConfig("Windsurf");

      expect(readJson(file).mcpServers.other.command).to.equal("x");
    });

    it("preserves other servers in the Cline settings", async () => {
      const file = path.join(
        path.dirname(globalStorageDir),
        "saoudrizwan.claude-dev",
        "settings",
        "cline_mcp_settings.json"
      );
      writeJson(file, { mcpServers: { other: { command: "x" } } });

      await manager.generateConfig("Cline");

      expect(readJson(file).mcpServers.other.command).to.equal("x");
    });

    it("replaces its own entry instead of appending a second one", async () => {
      await manager.generateConfig("Claude Code");
      await manager.generateConfig("Claude Code");

      const merged = readJson(path.join(testWorkspace, ".mcp.json"));
      expect(Object.keys(merged.mcpServers)).to.deep.equal(["comp"]);
    });

    it("preserves other Codex servers, sub-tables included", async () => {
      // Shaped like a real config.toml: another server carrying a sub-table of
      // its own, which a naive splice would swallow.
      writeText(
        codexGlobal(),
        [
          "[mcp_servers.vexp]",
          'url = "http://127.0.0.1:9000/mcp"',
          "tool_timeout_sec = 120",
          "",
          "[mcp_servers.vexp.http_headers]",
          'Authorization = "Bearer example-token"',
          "",
        ].join("\n")
      );

      await manager.generateConfig("Codex");

      const merged = readText(codexGlobal());
      expect(merged).to.include("[mcp_servers.vexp]");
      expect(merged).to.include("[mcp_servers.vexp.http_headers]");
      expect(merged).to.include('Authorization = "Bearer example-token"');
      expect(merged).to.include("[mcp_servers.comp]");
    });

    it("replaces its own Codex table instead of appending a second one", async () => {
      await manager.generateConfig("Codex");
      await manager.generateConfig("Codex");

      const merged = readText(codexGlobal());
      expect(merged.split("[mcp_servers.comp]")).to.have.lengthOf(2);
      expect(merged.split("# comP MCP server")).to.have.lengthOf(2);
    });

    it("replaces a Codex entry that used an env sub-table, leaving nothing behind", async () => {
      // The shape `codex mcp add` writes. Replacing only the parent table would
      // leave the old env sub-table attached to the new command.
      writeText(
        codexGlobal(),
        [
          "[mcp_servers.comp]",
          'command = "/gone/comp-daemon"',
          "args = []",
          "",
          "[mcp_servers.comp.env]",
          'RUST_LOG = "trace"',
          "",
          "[mcp_servers.other]",
          'command = "/usr/bin/other"',
          "",
        ].join("\n")
      );

      await manager.generateConfig("Codex");

      const merged = readText(codexGlobal());
      expect(merged).to.not.include("[mcp_servers.comp.env]");
      expect(merged).to.not.include('RUST_LOG = "trace"');
      expect(merged).to.not.include("/gone/comp-daemon");
      expect(merged).to.include("[mcp_servers.other]");
      expect(merged).to.include('command = "/usr/bin/other"');
    });

    it("refuses to guess a merge when Codex uses an inline mcp_servers table", async () => {
      const original = 'mcp_servers = { other = { command = "/usr/bin/other" } }\n';
      writeText(codexGlobal(), original);

      const result = await manager.generateConfig("Codex");

      expect(readText(codexGlobal())).to.equal(original);
      expect(result.writes[0].status).to.equal("failed");
      expect(result.writes[0].reason).to.include("inline table");
    });

    it("refuses to guess a merge when a comp entry is written in dotted form", async () => {
      const original = ["[mcp_servers]", 'comp = { command = "/old/comp-daemon" }', ""].join("\n");
      writeText(codexGlobal(), original);

      const result = await manager.generateConfig("Codex");

      expect(readText(codexGlobal())).to.equal(original);
      expect(result.writes[0].status).to.equal("failed");
    });

    it("is not tripped by an unrelated key called comp in another table", async () => {
      // `[tui] comp = true` says nothing about MCP servers, and refusing to
      // configure Codex over it would be a plain false alarm.
      writeText(codexGlobal(), ["[tui]", "comp = true", ""].join("\n"));

      const result = await manager.generateConfig("Codex");

      expect(result.writes[0].status).to.equal("written");
      const merged = readText(codexGlobal());
      expect(merged).to.include("comp = true");
      expect(merged).to.include("[mcp_servers.comp]");
    });

    it("leaves a server whose name merely starts with comp alone", async () => {
      writeText(
        codexGlobal(),
        ["[mcp_servers.company]", 'command = "/usr/bin/company"', ""].join("\n")
      );

      await manager.generateConfig("Codex");

      const merged = readText(codexGlobal());
      expect(merged).to.include("[mcp_servers.company]");
      expect(merged).to.include('command = "/usr/bin/company"');
      expect(merged).to.include("[mcp_servers.comp]");
    });

    it("refuses a dotted comp entry even with whitespace around the dot", async () => {
      // `comp . command = "…"` is legal TOML — whitespace around a dotted-key
      // separator is explicitly permitted by the spec — and is equivalent to
      // `[mcp_servers.comp]` with `command = "…"`. Missing it here means a
      // later `[mcp_servers.comp]` table redefines the same path and Codex
      // rejects the whole file as a duplicate key.
      const original = ["[mcp_servers]", 'comp . command = "old-daemon"', ""].join("\n");
      writeText(codexGlobal(), original);

      const result = await manager.generateConfig("Codex");

      expect(result.writes[0].status).to.equal("failed");
      expect(readText(codexGlobal())).to.equal(original);
    });

    it("refuses to splice a comp table containing a header-like line inside a multi-line string", async () => {
      // A neighbouring server's multi-line string can legally contain a line
      // that reads exactly like comP's own table header. findCompSection has no
      // notion of TOML string state and would treat it as one, but the
      // bracket-balance check in isSelfContained still catches the resulting
      // truncation and refuses rather than truncating the file.
      const original = [
        "[mcp_servers.aider]",
        'command = "/bin/aider"',
        "args = [\"--system-prompt\", '''",
        "Example TOML you might write:",
        "[mcp_servers.comp]",
        'command = "whatever"',
        "''']",
        "",
        "[mcp_servers.other]",
        'command = "/usr/bin/other"',
        "",
      ].join("\n");
      writeText(codexGlobal(), original);

      const result = await manager.generateConfig("Codex");

      expect(result.writes[0].status).to.equal("failed");
      expect(readText(codexGlobal())).to.equal(original);
    });

    it("refuses to splice a comp table cut short by a nested array element", async () => {
      // `["--flag"],` opens a line the way a table header does, so the section
      // scan stops there. Splicing would strip the rest of the array and leave
      // the file unparseable — taking down all of Codex, not just comP.
      const original = [
        "[mcp_servers.comp]",
        "command = '/old/comp-daemon'",
        "args = [",
        '["--flag"],',
        "]",
        "",
        "[mcp_servers.keepme]",
        'command = "/usr/bin/keepme"',
        "",
      ].join("\n");
      writeText(codexGlobal(), original);

      const result = await manager.generateConfig("Codex");

      expect(result.writes[0].status).to.equal("failed");
      expect(result.writes[0].reason).to.include("multi-line");
      expect(readText(codexGlobal())).to.equal(original);
    });
  });

  describe("backup and failure handling", () => {
    it("writes a .bak whose bytes match the file before the run", async () => {
      const file = path.join(testWorkspace, ".mcp.json");
      writeJson(file, { mcpServers: { other: { command: "x" } } });
      const before = fs.readFileSync(file);

      const result = await manager.generateConfig("Claude Code");

      const backup = result.writes[0].backupPath;
      expect(backup).to.equal(file + ".bak");
      expect(fs.readFileSync(backup!).equals(before)).to.be.true;
    });

    it("does not create a .bak when the file did not exist", async () => {
      const result = await manager.generateConfig("Claude Code");

      expect(result.writes[0].backupPath).to.be.undefined;
      expect(fs.existsSync(path.join(testWorkspace, ".mcp.json.bak"))).to.be.false;
    });

    it("leaves an unparseable config untouched and reports the failure", async () => {
      const file = path.join(testWorkspace, ".mcp.json");
      fs.writeFileSync(file, "{ this is not json", "utf-8");

      const result = await manager.generateConfig("Claude Code");

      expect(result.success).to.be.false;
      expect(fs.readFileSync(file, "utf-8")).to.equal("{ this is not json");
      expect(result.writes[0].status).to.equal("failed");
      expect(fs.existsSync(file + ".bak")).to.be.false;
    });

    it("does not touch the config when the backup cannot be taken", async () => {
      const file = path.join(testWorkspace, ".mcp.json");
      writeJson(file, { mcpServers: { other: { command: "x" } } });
      const before = fs.readFileSync(file, "utf-8");
      (manager as any).backupIfExists = () => {
        throw new Error("disk full");
      };

      const result = await manager.generateConfig("Claude Code");

      expect(result.success).to.be.false;
      expect(result.writes[0].reason).to.include("backup failed");
      expect(fs.readFileSync(file, "utf-8")).to.equal(before);
    });

    it("keeps writing the remaining targets after one fails", async () => {
      const broken = path.join(testWorkspace, ".cursor", "mcp.json");
      fs.mkdirSync(path.dirname(broken), { recursive: true });
      fs.writeFileSync(broken, "}}not json", "utf-8");

      const result = await manager.generateConfig("Cursor");

      expect(result.success).to.be.true;
      expect(result.writes[0].status).to.equal("failed");
      expect(result.writes[1].status).to.equal("written");
      expect(readJson(path.join(fakeHome, ".cursor", "mcp.json")).mcpServers.comp).to.exist;
    });

    it("refuses to guess a merge when Aider already has an mcp-servers block", async () => {
      const file = path.join(testWorkspace, ".aider.conf.yml");
      fs.writeFileSync(file, "mcp-servers:\n  other:\n    command: /usr/bin/other\n", "utf-8");

      const result = await manager.generateConfig("Aider");

      expect(result.success).to.be.false;
      expect(fs.readFileSync(file, "utf-8")).to.include("/usr/bin/other");
      expect(result.writes[0].reason).to.include("mcp-servers");
    });
  });

  describe("manual fallback", () => {
    it("is absent when every target was written", async () => {
      const result = await manager.generateConfig("Cursor");

      expect(result.success).to.be.true;
      expect(result.manualFallback).to.be.undefined;
    });

    it("carries pasteable content for the target that failed", async () => {
      const file = path.join(testWorkspace, ".mcp.json");
      fs.writeFileSync(file, "not json", "utf-8");

      const result = await manager.generateConfig("Claude Code");

      expect(result.manualFallback).to.have.lengthOf(1);
      expect(result.manualFallback![0].path).to.equal(file);
      const parsed = JSON.parse(result.manualFallback![0].content);
      expect(parsed.mcpServers.comp.command).to.be.a("string");
    });

    it("carries a pasteable TOML table when the Codex config cannot be merged", async () => {
      writeText(codexGlobal(), 'mcp_servers = { other = { command = "/usr/bin/other" } }\n');

      const result = await manager.generateConfig("Codex");

      const fallback = result.manualFallback!.find((f) => f.path === codexGlobal())!;
      expect(fallback.content).to.include("[mcp_servers.comp]");
      expect(fallback.content).to.include("command = '");
    });
  });

  describe("Codex TOML format", () => {
    it("writes env as an inline table so the entry stays one contiguous block", async () => {
      await manager.generateConfig("Codex");

      const content = readText(codexProject());
      expect(content).to.include("[mcp_servers.comp]");
      expect(content).to.include("args = []");
      expect(content).to.match(/^env = \{ RUST_LOG = 'info', COMP_WORKSPACE_ROOT = '.+' \}$/m);
      expect(content).to.not.include("[mcp_servers.comp.env]");
    });

    it("quotes TOML scalars so Windows backslashes survive", () => {
      const quote = (AgentSetupManager as any).tomlQuote as (v: string) => string;

      // A literal string takes every character as-is; a basic string would read
      // \U as an escape and reject the path.
      expect(quote("E:\\dev\\comP")).to.equal("'E:\\dev\\comP'");
      expect(quote("it's")).to.equal('"it\'s"');
    });

    it("escapes a newline instead of letting it split the value across lines", () => {
      const quote = (AgentSetupManager as any).tomlQuote as (v: string) => string;

      // A directory name may legally contain a newline on macOS and Linux; a raw
      // one would end the string early and make the whole config unparseable.
      expect(quote("/we\nird/path")).to.equal('"/we\\nird/path"');
      expect(quote("/tab\there")).to.equal('"/tab\\there"');
    });

    it("keeps a daemon path containing $& intact", async () => {
      // Written through String.replace, `$&` would be read as a capture
      // reference and splice the old value into the middle of the new one.
      const tricky = path.join(caseDir, "bin$&dir", "comp-daemon");
      (manager as any).getDaemonPath = () => tricky;

      await manager.generateConfig("Codex");

      expect(readText(codexGlobal())).to.include(`command = '${tricky}'`);
    });

    it("escapes a control character in the daemon path as \\uXXXX", async () => {
      // tomlQuote falls back to a basic string and \uXXXX-escapes control
      // characters (a path may legally contain one on macOS/Linux). This only
      // needs the *string* to contain the character — generateConfig never
      // touches the daemon path on disk while writing the config — so unlike
      // the round-trip test below, this is safe to exercise with a real
      // control byte even though Windows forbids one in an actual filename.
      const tricky = path.join(caseDir, "esc" + String.fromCharCode(0x1b) + "dir", "comp-daemon");
      (manager as any).getDaemonPath = () => tricky;

      await manager.generateConfig("Codex");

      expect(readText(codexGlobal())).to.include("\\u001b");
    });

    it("decodes a \\uXXXX-escaped command value back to a real path during repair", async () => {
      // Same decode branch a real control character would take, but escaping
      // a filesystem-legal character ('x', x) instead — Windows forbids
      // literal control bytes (0x00-0x1F) in filenames, so a test that needed
      // to actually create one on disk would fail there. Writing the escape
      // by hand and pointing it at a real file exercises decodeTomlString's
      // \uXXXX branch identically without relying on that.
      const daemon = path.join(caseDir, "x");
      fs.writeFileSync(daemon, "");
      // Forward slashes only: a raw backslash (e.g. a Windows path's own
      // separators) inside a double-quoted TOML string is itself an escape
      // sequence, which would corrupt the very value this test constructs by
      // hand. fs.existsSync resolves forward slashes fine on every platform.
      const escapedDaemon = daemon.replace(/\\/g, "/").slice(0, -1) + "\\u0078"; // literal backslash-u-0078, not an actual escape
      writeText(
        codexGlobal(),
        ["[mcp_servers.comp]", `command = "${escapedDaemon}"`, "args = []", ""].join("\n")
      );

      const entries = manager.repairStaleConfigs();
      const entry = entries.find((e) => e.file === codexGlobal());

      expect(entry?.status).to.equal("healthy");
    });
  });

  describe("Continue block format", () => {
    it("carries the metadata Continue requires and a list-shaped server entry", async () => {
      await manager.generateConfig("Continue");

      const content = fs.readFileSync(
        path.join(testWorkspace, ".continue", "mcpServers", "comp.yaml"),
        "utf-8"
      );
      expect(content).to.include("schema: v1");
      expect(content).to.include("mcpServers:");
      expect(content).to.include("- name: comp");
      expect(content).to.include("type: stdio");
    });

    it("quotes YAML scalars so Windows backslashes survive", () => {
      const quote = (AgentSetupManager as any).yamlQuote as (v: string) => string;

      expect(quote("E:\\dev\\comP")).to.equal("'E:\\dev\\comP'");
      expect(quote("it's")).to.equal("'it''s'");
    });
  });

  describe("instruction files", () => {
    it("writes the comP usage rules without asking the user to paste them", async () => {
      const result = await manager.generateConfig("Claude Code");

      const claudeMd = path.join(testWorkspace, "CLAUDE.md");
      expect(result.constitutionFiles).to.include(claudeMd);
      expect(fs.readFileSync(claudeMd, "utf-8")).to.include("comP MCP Tool Usage");
    });

    it("does not append a second copy on a repeat run", async () => {
      await manager.generateConfig("Claude Code");
      await manager.generateConfig("Claude Code");

      const content = fs.readFileSync(path.join(testWorkspace, "CLAUDE.md"), "utf-8");
      expect(content.split("comP MCP Tool Usage")).to.have.lengthOf(2);
      expect(content.split("Session Continuity")).to.have.lengthOf(2);
    });

    it("appends to an existing instruction file instead of replacing it", async () => {
      const claudeMd = path.join(testWorkspace, "CLAUDE.md");
      fs.writeFileSync(claudeMd, "# My rules\n\nDo not delete this.\n", "utf-8");

      await manager.generateConfig("Claude Code");

      const content = fs.readFileSync(claudeMd, "utf-8");
      expect(content).to.include("Do not delete this.");
      expect(content).to.include("comP MCP Tool Usage");
    });

    it("writes inside .cursor/rules when it is a directory of rule files", async () => {
      const rulesDir = path.join(testWorkspace, ".cursor", "rules");
      fs.mkdirSync(rulesDir, { recursive: true });
      fs.writeFileSync(path.join(rulesDir, "existing.mdc"), "keep me", "utf-8");

      await manager.generateConfig("Cursor");

      expect(fs.statSync(rulesDir).isDirectory()).to.be.true;
      expect(fs.readFileSync(path.join(rulesDir, "existing.mdc"), "utf-8")).to.equal("keep me");
      expect(fs.readFileSync(path.join(rulesDir, "comp.md"), "utf-8")).to.include(
        "comP MCP Tool Usage"
      );
    });

    it("honours autoGenerateConstitution: false", async () => {
      writeJson(path.join(testWorkspace, ".comp", "config.json"), {
        autoGenerateConstitution: false,
      });

      const result = await manager.generateConfig("Claude Code");

      expect(result.constitutionFiles).to.be.empty;
      expect(fs.existsSync(path.join(testWorkspace, "CLAUDE.md"))).to.be.false;
    });
  });

  describe("Claude Code user scope", () => {
    it("builds a command with the scope, transport and argument separator", async () => {
      const result = await manager.generateConfig("Claude Code");

      expect(result.command).to.include("--scope user");
      expect(result.command).to.include("--transport stdio");
      expect(result.command).to.include(" -- ");
    });

    it("reports the command instead of throwing when the CLI is missing", async () => {
      const runner = async () => ({ ok: false, stdout: "", stderr: "not found" });

      const outcome = await manager.registerClaudeCodeUserScope(runner);

      expect(outcome.registered).to.be.false;
      expect(outcome.reason).to.include("claude CLI");
      expect(outcome.command).to.include("claude mcp add");
    });

    it("passes scope, transport and the -- separator to the CLI", async () => {
      const calls: { file: string; args: string[] }[] = [];
      const runner = async (file: string, args: string[]) => {
        calls.push({ file, args });
        return { ok: true, stdout: "", stderr: "" };
      };

      const outcome = await manager.registerClaudeCodeUserScope(runner);

      expect(outcome.registered).to.be.true;
      const add = calls[1].args;
      expect(add.slice(0, 3)).to.deep.equal(["mcp", "add", "--scope"]);
      expect(add).to.include("user");
      expect(add).to.include("stdio");
      // The daemon path must be the last argument, after the separator.
      expect(add[add.length - 2]).to.equal("--");
    });

    it("omits COMP_WORKSPACE_ROOT from the user-scope registration", async () => {
      const calls: string[][] = [];
      const runner = async (_file: string, args: string[]) => {
        calls.push(args);
        return { ok: true, stdout: "", stderr: "" };
      };

      await manager.registerClaudeCodeUserScope(runner);

      expect(calls[1].join(" ")).to.not.include("COMP_WORKSPACE_ROOT");
    });

    it("skips the CLI when the daemon path would be reinterpreted by cmd.exe", async function () {
      // The Windows runner goes through a shell, so this guard only exists there.
      if (process.platform !== "win32") {
        this.skip();
      }
      (manager as any).getDaemonPath = () => "C:\\dev\\a&b\\comp-daemon.exe";
      let ran = false;
      const runner = async () => {
        ran = true;
        return { ok: true, stdout: "", stderr: "" };
      };

      const outcome = await manager.registerClaudeCodeUserScope(runner);

      expect(ran).to.be.false;
      expect(outcome.registered).to.be.false;
      expect(outcome.reason).to.include("shell metacharacters");
      expect(outcome.command).to.include("claude mcp add");
    });

    it("surfaces the CLI error when registration fails", async () => {
      let call = 0;
      const runner = async () => {
        call += 1;
        return call === 1
          ? { ok: true, stdout: "2.0.0", stderr: "" }
          : { ok: false, stdout: "", stderr: "already exists" };
      };

      const outcome = await manager.registerClaudeCodeUserScope(runner);

      expect(outcome.registered).to.be.false;
      expect(outcome.reason).to.include("already exists");
    });
  });

  describe("detectInstalledAgents", () => {
    it("returns nothing for an untouched machine", () => {
      expect(manager.detectInstalledAgents()).to.be.empty;
    });

    it("detects agents from the traces they leave", () => {
      fs.mkdirSync(path.join(fakeHome, ".cursor"), { recursive: true });
      fs.mkdirSync(path.join(fakeHome, ".codeium", "windsurf"), { recursive: true });
      fs.writeFileSync(path.join(testWorkspace, "CLAUDE.md"), "x", "utf-8");

      const detected = manager.detectInstalledAgents();

      expect(detected).to.include.members(["Cursor", "Windsurf", "Claude Code"]);
      expect(detected).to.not.include("Continue");
      expect(detected).to.not.include("Codex");
    });

    it("detects Codex from its config directory", () => {
      fs.mkdirSync(codexHome, { recursive: true });

      expect(manager.detectInstalledAgents()).to.include("Codex");
    });
  });

  describe("restart guidance", () => {
    it("tells the user how to reload the agent it just configured", async () => {
      const cursor = await manager.generateConfig("Cursor");
      const claude = await manager.generateConfig("Claude Code");

      expect(cursor.restartHint).to.include("Reload Window");
      expect(claude.restartHint).to.include("claude mcp list");
    });

    it("defaults to English when no locale is given", async () => {
      const result = await manager.generateConfig("Codex");

      expect(result.restartHint).to.include("Quit the running Codex");
      expect(result.restartHint).to.not.match(/[぀-ヿ一-鿿]/);
    });

    it("switches every generated string to Japanese when locale: \"ja\" is set", async () => {
      const jaManager = new AgentSetupManager(mockDaemon as any, testWorkspace, undefined, {
        homeDir: fakeHome,
        codexHome,
        locale: "ja",
      });

      const result = await jaManager.generateConfig("Codex");

      expect(result.restartHint).to.include("実行中の Codex を終了し");
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
    let repairCodexHome: string;
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

      // WHY homeDir: several repair targets live under the home directory, so
      // running the suite against a real one would edit the developer's own
      // Cursor and Windsurf configuration.
      repairManager = new AgentSetupManager(mockDaemon as any, ws, extDir, {
        homeDir: path.join(caseDir, "home"),
        // Codex reads CODEX_HOME, which would otherwise point these repairs at
        // the developer's own configuration.
        codexHome: path.join(caseDir, "codex-home"),
      });
      repairCodexHome = path.join(caseDir, "codex-home");
      // The Antigravity target is redirected on top of that so the assertions
      // below can point at one known file.
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
      // homeDir and codexHome are pinned for the same reason as the shared
      // manager above: several targets live under them, and a real one would
      // put this test on the developer's own configuration.
      const noExtManager = new AgentSetupManager(mockDaemon as any, ws, undefined, {
        homeDir: path.join(tmpRoot, "no-ext-home"),
        codexHome: path.join(tmpRoot, "no-ext-codex"),
      });
      (noExtManager as any).antigravityConfigPath = () => globalConfig;
      writeJson(globalConfig, { mcpServers: { comp: { command: stalePath } } });

      const entries = noExtManager.repairStaleConfigs();

      expect(entryFor(entries, globalConfig)?.status).to.equal("skipped");
      expect(readJson(globalConfig).mcpServers.comp.command).to.equal(stalePath);
    });

    describe("Codex config.toml", () => {
      const codexConfig = (): string => path.join(repairCodexHome, "config.toml");
      const writeToml = (file: string, value: string): void => {
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, value, "utf-8");
      };
      const readToml = (file: string): string => fs.readFileSync(file, "utf-8");

      it("does not report a valid TOML config as unparseable JSON", () => {
        // The regression this whole routine exists for: routed through the JSON
        // reader, a healthy Codex config fails on every activation while the
        // stale daemon path inside it is never fixed.
        writeToml(
          codexConfig(),
          ["[mcp_servers.comp]", `command = '${bundledBinary}'`, "args = []", ""].join("\n")
        );

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("healthy");
      });

      it("rewrites a stale command and leaves every other line untouched", () => {
        writeToml(
          codexConfig(),
          [
            "[mcp_servers.vexp]",
            'url = "http://127.0.0.1:9000/mcp"',
            "",
            "[mcp_servers.comp]",
            `command = '${stalePath}'`,
            "args = []",
            "env = { RUST_LOG = 'info' }",
            "",
            "[mcp_servers.other]",
            'command = "/usr/bin/other"',
            "",
          ].join("\n")
        );

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());
        const repaired = readToml(codexConfig());

        expect(entry?.status).to.equal("repaired");
        expect(entry?.from).to.equal(stalePath);
        // A global file gets the bundled binary, never this workspace's dev build.
        expect(entry?.to).to.equal(bundledBinary);
        expect(repaired).to.include(`command = '${bundledBinary}'`);
        expect(repaired).to.include('url = "http://127.0.0.1:9000/mcp"');
        expect(repaired).to.include("[mcp_servers.other]");
        expect(repaired).to.include('command = "/usr/bin/other"');
      });

      it("reads a command written as a basic string", () => {
        // `codex mcp add` writes basic strings, so the repair has to decode them
        // as well as the literal strings comP itself emits.
        const escaped = stalePath.replace(/\\/g, "\\\\");
        writeToml(
          codexConfig(),
          ["[mcp_servers.comp]", `command = "${escaped}"`, "args = []", ""].join("\n")
        );

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("repaired");
        expect(entry?.from).to.equal(stalePath);
      });

      it("leaves a relative command untouched", () => {
        const original = ["[mcp_servers.comp]", "command = 'comp-daemon'", ""].join("\n");
        writeToml(codexConfig(), original);

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("skipped");
        expect(readToml(codexConfig())).to.equal(original);
      });

      it("leaves a command containing a ${...} variable untouched", () => {
        const original = ["[mcp_servers.comp]", "command = '${env:COMP_BIN}'", ""].join("\n");
        writeToml(codexConfig(), original);

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("skipped");
        expect(readToml(codexConfig())).to.equal(original);
      });

      it("skips a config with no comp table", () => {
        writeToml(codexConfig(), '[mcp_servers.vexp]\nurl = "http://localhost/mcp"\n');

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("skipped");
        expect(entry?.reason).to.include("no comp server entry");
      });

      it("reports a missing file without throwing", () => {
        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("missing");
      });

      it("refreshes a stale COMP_WORKSPACE_ROOT in the project config", () => {
        const projectConfig = path.join(ws, ".codex", "config.toml");
        writeToml(
          projectConfig,
          [
            "[mcp_servers.comp]",
            `command = '${devBinary}'`,
            "args = []",
            "env = { RUST_LOG = 'info', COMP_WORKSPACE_ROOT = '/moved/away' }",
            "",
          ].join("\n")
        );

        const entry = entryFor(repairManager.repairStaleConfigs(), projectConfig);

        expect(entry?.status).to.equal("repaired");
        expect(entry?.envRepaired).to.be.true;
        expect(readToml(projectConfig)).to.include(`COMP_WORKSPACE_ROOT = '${ws}'`);
      });

      it("keeps a replacement path containing $& intact", () => {
        const tricky = path.join(extDir, "bin$&dir", "comp-daemon-macos");
        fs.mkdirSync(path.dirname(tricky), { recursive: true });
        fs.writeFileSync(tricky, "");
        (repairManager as any).bundledDaemonPath = () => tricky;
        writeToml(
          codexConfig(),
          ["[mcp_servers.comp]", `command = '${stalePath}'`, "args = []", ""].join("\n")
        );

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("repaired");
        // Through String.replace this became bin'<old path>'dir — a command that
        // is both wrong and unquoted, i.e. an unparseable config.
        expect(readToml(codexConfig())).to.include(`command = '${tricky}'`);
      });

      it("skips a comp table cut short by a nested array element", () => {
        const original = [
          "[mcp_servers.comp]",
          `command = '${stalePath}'`,
          "args = [",
          '["--flag"],',
          "]",
          "",
          "[mcp_servers.keepme]",
          'command = "/usr/bin/keepme"',
          "",
        ].join("\n");
        writeToml(codexConfig(), original);

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("skipped");
        expect(readToml(codexConfig())).to.equal(original);
      });

      it("never rewrites COMP_WORKSPACE_ROOT in the global config", () => {
        writeToml(
          codexConfig(),
          [
            "[mcp_servers.comp]",
            `command = '${bundledBinary}'`,
            "env = { COMP_WORKSPACE_ROOT = '/some/other/project' }",
            "",
          ].join("\n")
        );

        const entry = entryFor(repairManager.repairStaleConfigs(), codexConfig());

        expect(entry?.status).to.equal("healthy");
        expect(readToml(codexConfig())).to.include("'/some/other/project'");
      });
    });
  });
});

