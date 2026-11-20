// comP が MAGATAMA の起動登録を管理する（tasks/magatama_sync_plan.md の 1）。設定ファイルへの書き込みの結合テスト。
// Owned by the designer role; implementers must not edit.
import { expect } from "chai";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { AgentSetupManager } from "../AgentSetup";
import { managedEntry } from "../../config/magatama";

class MockDaemonManager {
  async request(): Promise<unknown> {
    return {};
  }
}

describe("AgentSetupManager: MAGATAMA registration", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-magatama-${process.pid}`);
  let caseIndex = 0;
  let ws: string;
  let home: string;
  let codexHome: string;
  let storage: string;

  const make = (magatama?: { manage: boolean; uvAvailable: boolean; pin?: string }): AgentSetupManager =>
    new AgentSetupManager(new MockDaemonManager() as any, ws, undefined, {
      homeDir: home,
      globalStorageDir: storage,
      codexHome,
      ...(magatama ? { magatama } : {}),
    });
  const mcpJson = (): string => path.join(ws, ".mcp.json");
  const codexGlobal = (): string => path.join(codexHome, "config.toml");
  const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));
  const writeText = (file: string, text: string): void => {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text, "utf-8");
  };
  const writeJson = (file: string, value: unknown): void => writeText(file, JSON.stringify(value, null, 2));
  const managedOld = { command: "uvx", args: ["magatama@0.7.1", "serve"], env: { COMP_MANAGED_BY: "comP", X: "1" } };
  const userEntry = { command: "magatama", args: ["serve"] };

  beforeEach(() => {
    const dir = path.join(tmpRoot, `case-${caseIndex++}`);
    ws = path.join(dir, "ws");
    home = path.join(dir, "home");
    codexHome = path.join(dir, "codex");
    storage = path.join(dir, "globalStorage", "tsucky230.comp-vscode");
    for (const d of [ws, home, codexHome, storage]) fs.mkdirSync(d, { recursive: true });
  });
  after(() => fs.rmSync(tmpRoot, { recursive: true, force: true }));

  describe("JSON configs (Claude Code .mcp.json)", () => {
    it("adds the managed entry next to comp when asked", async () => {
      const result = await make({ manage: true, uvAvailable: true, pin: "0.7.2" }).generateConfig("Claude Code");
      const doc = readJson(mcpJson());
      expect(doc.mcpServers.magatama).to.deep.equal(managedEntry("0.7.2"));
      expect(doc.mcpServers.comp).to.be.an("object");
      expect(result.magatama).to.deep.include({ path: mcpJson(), scope: "workspace", outcome: "added" });
    });

    it("writes nothing for MAGATAMA when not asked", async () => {
      const result = await make().generateConfig("Claude Code");
      expect(readJson(mcpJson()).mcpServers).to.not.have.property("magatama");
      expect(result.magatama?.map((m) => m.outcome)).to.deep.equal(["kept"]);
    });

    it("does not write an entry that cannot start without uv", async () => {
      const result = await make({ manage: true, uvAvailable: false, pin: "0.7.2" }).generateConfig("Claude Code");
      expect(readJson(mcpJson()).mcpServers).to.not.have.property("magatama");
      expect(result.magatama).to.deep.include({
        path: mcpJson(),
        scope: "workspace",
        outcome: "kept",
        reason: "uv-missing",
      });
    });

    it("leaves the user's own entry exactly as it was", async () => {
      writeJson(mcpJson(), { mcpServers: { magatama: userEntry } });
      const result = await make({ manage: true, uvAvailable: true, pin: "0.7.2" }).generateConfig("Claude Code");
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal(userEntry);
      expect(result.magatama).to.deep.include({
        path: mcpJson(),
        scope: "workspace",
        outcome: "kept",
        reason: "user-managed",
      });
    });

    it("upgrades an older managed entry and keeps the user's extra env", async () => {
      writeJson(mcpJson(), { mcpServers: { magatama: managedOld } });
      const result = await make({ manage: false, uvAvailable: true, pin: "0.7.2" }).generateConfig("Claude Code");
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal({
        command: "uvx",
        args: ["magatama@0.7.2", "serve"],
        env: { COMP_MANAGED_BY: "comP", X: "1" },
      });
      expect(result.magatama).to.deep.include({ path: mcpJson(), scope: "workspace", outcome: "updated" });
    });

    it("never downgrades an entry a newer comP wrote", async () => {
      const newer = { command: "uvx", args: ["magatama@0.8.0", "serve"], env: { COMP_MANAGED_BY: "comP" } };
      writeJson(mcpJson(), { mcpServers: { magatama: newer } });
      await make({ manage: true, uvAvailable: true, pin: "0.7.2" }).generateConfig("Claude Code");
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal(newer);
    });
  });

  describe("Codex config.toml", () => {
    it("adds one magatama table with the marker, and adding again does not duplicate it", async () => {
      const m = make({ manage: true, uvAvailable: true, pin: "0.7.2" });
      await m.generateConfig("Codex");
      await m.generateConfig("Codex");
      const text = fs.readFileSync(codexGlobal(), "utf-8");
      expect(text.match(/^\[mcp_servers\.magatama\]/gm)).to.have.length(1);
      expect(text.match(/^\[mcp_servers\.magatama\.env\]/gm)).to.have.length(1);
      expect(text).to.match(/magatama@0\.7\.2/);
      expect(text).to.match(/COMP_MANAGED_BY\s*=\s*["']comP["']/);
      expect(text).to.match(/^\[mcp_servers\.comp\]/m);
    });

    it("leaves the user's own magatama table byte-for-byte", async () => {
      const user = '[mcp_servers.magatama]\ncommand = "magatama"\nargs = ["serve"]\n';
      writeText(codexGlobal(), user);
      await make({ manage: true, uvAvailable: true, pin: "0.7.2" }).generateConfig("Codex");
      const text = fs.readFileSync(codexGlobal(), "utf-8");
      expect(text.startsWith(user)).to.equal(true);
      expect(text.match(/^\[mcp_servers\.magatama\]/gm)).to.have.length(1);
    });

    it("upgrades an older managed table and keeps the user's other lines in it", async () => {
      writeText(
        codexGlobal(),
        [
          "[mcp_servers.magatama]",
          'command = "uvx"',
          'args = ["magatama@0.7.1", "serve"]',
          "startup_timeout_sec = 60",
          "",
          "[mcp_servers.magatama.env]",
          'COMP_MANAGED_BY = "comP"',
          "",
        ].join("\n")
      );
      const result = await make({ manage: false, uvAvailable: true, pin: "0.7.2" }).generateConfig("Codex");
      const text = fs.readFileSync(codexGlobal(), "utf-8");
      expect(text).to.match(/magatama@0\.7\.2/);
      expect(text).to.not.match(/magatama@0\.7\.1/);
      expect(text).to.match(/startup_timeout_sec = 60/);
      expect(text.match(/^\[mcp_servers\.magatama\]/gm)).to.have.length(1);
      expect(result.magatama).to.deep.include({ path: codexGlobal(), scope: "global", outcome: "updated" });
    });
  });

  describe("updateManagedMagatama (startup repair)", () => {
    it("upgrades managed entries only, and creates no files", async () => {
      writeJson(mcpJson(), { mcpServers: { magatama: managedOld } });
      const cursorHome = path.join(home, ".cursor", "mcp.json");
      writeJson(cursorHome, { mcpServers: { magatama: userEntry } });
      const outcomes = await make({ manage: false, uvAvailable: false, pin: "0.7.2" }).updateManagedMagatama();
      expect(readJson(mcpJson()).mcpServers.magatama.args).to.deep.equal(["magatama@0.7.2", "serve"]);
      expect(readJson(cursorHome).mcpServers.magatama).to.deep.equal(userEntry);
      expect(fs.existsSync(codexGlobal())).to.equal(false);
      expect(outcomes.filter((o) => o.outcome === "updated").map((o) => o.path)).to.deep.equal([mcpJson()]);
    });

    it("never adds an entry, even when management is on", async () => {
      writeJson(mcpJson(), { mcpServers: {} });
      await make({ manage: true, uvAvailable: true, pin: "0.7.2" }).updateManagedMagatama();
      expect(readJson(mcpJson()).mcpServers).to.not.have.property("magatama");
    });
  });

  describe("findUserManagedMagatama / manageMagatamaAt (the notification's button)", () => {
    it("lists the user's own entries with their scope, and not comP's", async () => {
      writeJson(mcpJson(), { mcpServers: { magatama: userEntry } });
      writeText(codexGlobal(), '[mcp_servers.magatama]\ncommand = "uv"\nargs = ["run", "magatama", "serve"]\n');
      writeJson(path.join(home, ".cursor", "mcp.json"), { mcpServers: { magatama: managedEntry("0.7.2") } });
      const found = await make().findUserManagedMagatama();
      const byPath = new Map(found.map((f) => [f.path, f]));
      expect(byPath.get(mcpJson())).to.deep.equal({ path: mcpJson(), scope: "workspace", entry: userEntry });
      expect(byPath.get(codexGlobal())).to.deep.equal({
        path: codexGlobal(),
        scope: "global",
        entry: { command: "uv", args: ["run", "magatama", "serve"] },
      });
      expect(found).to.have.length(2);
    });

    it("replaces the user's entry with the managed one only where asked", async () => {
      writeJson(mcpJson(), { mcpServers: { magatama: userEntry, other: { command: "x" } } });
      const cursorHome = path.join(home, ".cursor", "mcp.json");
      writeJson(cursorHome, { mcpServers: { magatama: userEntry } });
      const outcomes = await make({ manage: false, uvAvailable: true, pin: "0.7.2" }).manageMagatamaAt([mcpJson()]);
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal(managedEntry("0.7.2"));
      expect(readJson(mcpJson()).mcpServers.other).to.deep.equal({ command: "x" });
      expect(readJson(cursorHome).mcpServers.magatama).to.deep.equal(userEntry);
      expect(outcomes).to.deep.include({ path: mcpJson(), scope: "workspace", outcome: "updated" });
    });

    it("refuses to replace when uv is missing", async () => {
      writeJson(mcpJson(), { mcpServers: { magatama: userEntry } });
      const outcomes = await make({ manage: true, uvAvailable: false, pin: "0.7.2" }).manageMagatamaAt([mcpJson()]);
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal(userEntry);
      expect(outcomes).to.deep.include({ path: mcpJson(), scope: "workspace", outcome: "failed", reason: "uv-missing" });
    });
  });
});
