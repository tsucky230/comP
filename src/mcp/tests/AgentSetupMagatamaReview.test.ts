// reviewer の指摘（2026-11-21）の再現テスト: 置き換えでユーザーの設定を失う・版を下げる・TOML を壊す。
// Owned by the designer role; implementers must not edit.
import { expect } from "chai";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { AgentSetupManager } from "../AgentSetup";
import { probeCoversEntry } from "../../config/magatama";

class MockDaemonManager {
  async request(): Promise<unknown> {
    return {};
  }
}

describe("AgentSetupManager: MAGATAMA review regressions", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-magatama-review-${process.pid}`);
  let caseIndex = 0;
  let ws: string;
  let home: string;
  let codexHome: string;
  let storage: string;

  const make = (magatama: { manage: boolean; uvAvailable: boolean; pin?: string }): AgentSetupManager =>
    new AgentSetupManager(new MockDaemonManager() as any, ws, undefined, {
      homeDir: home,
      globalStorageDir: storage,
      codexHome,
      magatama,
    });
  const mcpJson = (): string => path.join(ws, ".mcp.json");
  const codexGlobal = (): string => path.join(codexHome, "config.toml");
  const writeText = (file: string, text: string): void => {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text, "utf-8");
  };
  const readText = (file: string): string => fs.readFileSync(file, "utf-8");
  const readJson = (file: string): any => JSON.parse(readText(file));
  const on = { manage: true, uvAvailable: true, pin: "0.7.2" };

  beforeEach(() => {
    const dir = path.join(tmpRoot, `case-${caseIndex++}`);
    ws = path.join(dir, "ws");
    home = path.join(dir, "home");
    codexHome = path.join(dir, "codex");
    storage = path.join(dir, "globalStorage", "tsucky230.comp-vscode");
    for (const d of [ws, home, codexHome, storage]) fs.mkdirSync(d, { recursive: true });
  });
  after(() => fs.rmSync(tmpRoot, { recursive: true, force: true }));

  describe("manageMagatamaAt keeps what the user configured", () => {
    it("TOML: keeps the user's other keys, sub-tables and the next table's comment", async () => {
      writeText(
        codexGlobal(),
        [
          "[mcp_servers.magatama]",
          'command = "magatama"',
          'args = ["serve"]',
          "startup_timeout_sec = 60",
          "",
          "[mcp_servers.magatama.tools.analyze]",
          'approval_mode = "approve"',
          "",
          "# --- GitHub server (keep) ---",
          "[mcp_servers.github]",
          'command = "gh"',
          "",
        ].join("\n")
      );
      const outcomes = await make(on).manageMagatamaAt([codexGlobal()]);
      const text = readText(codexGlobal());
      expect(outcomes).to.deep.include({ path: codexGlobal(), scope: "global", outcome: "updated" });
      expect(text).to.match(/magatama@0\.7\.2/);
      expect(text).to.match(/COMP_MANAGED_BY\s*=\s*["']comP["']/);
      expect(text).to.include("startup_timeout_sec = 60");
      expect(text).to.include("[mcp_servers.magatama.tools.analyze]");
      expect(text).to.include('approval_mode = "approve"');
      expect(text).to.include("# --- GitHub server (keep) ---\n[mcp_servers.github]");
      expect(text).to.include('command = "gh"');
      expect(text.match(/^\[mcp_servers\.magatama\]/gm)).to.have.length(1);
      expect(text.match(/^\[mcp_servers\.magatama\.env\]/gm)).to.have.length(1);
      expect(text).to.not.include('command = "magatama"');
      expect(text).to.not.include('args = ["serve"]');
    });

    it("JSON: keeps the user's other keys and env, adding comP's marker", async () => {
      const user = { command: "magatama", args: ["serve"], cwd: "C:\\work", env: { A: "1" }, timeout: 30 };
      writeText(mcpJson(), JSON.stringify({ mcpServers: { magatama: user } }, null, 2));
      await make(on).manageMagatamaAt([mcpJson()]);
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal({
        command: "uvx",
        args: ["magatama@0.7.2", "serve"],
        cwd: "C:\\work",
        env: { A: "1", COMP_MANAGED_BY: "comP" },
        timeout: 30,
      });
    });
  });

  describe("manageMagatamaAt never moves MAGATAMA to an older version", () => {
    it("leaves a user's uvx entry pinned to a newer release alone", async () => {
      const newer = { command: "uvx", args: ["magatama@0.9.0", "serve"] };
      writeText(mcpJson(), JSON.stringify({ mcpServers: { magatama: newer } }, null, 2));
      const outcomes = await make(on).manageMagatamaAt([mcpJson()]);
      expect(readJson(mcpJson()).mcpServers.magatama).to.deep.equal(newer);
      expect(outcomes).to.deep.include({ path: mcpJson(), scope: "workspace", outcome: "kept", reason: "newer" });
    });

    it("still adopts a user's uvx entry on the same release (adds the marker)", async () => {
      writeText(
        mcpJson(),
        JSON.stringify({ mcpServers: { magatama: { command: "uvx", args: ["magatama@0.7.2", "serve"] } } })
      );
      await make(on).manageMagatamaAt([mcpJson()]);
      expect(readJson(mcpJson()).mcpServers.magatama.env).to.deep.equal({ COMP_MANAGED_BY: "comP" });
    });
  });

  describe("Codex TOML shapes comP cannot safely edit are left untouched", () => {
    const shapes: Array<[string, string]> = [
      ["dotted keys at the root", 'mcp_servers.magatama.command = "magatama"\nmcp_servers.magatama.args = ["serve"]\n'],
      ["dotted keys under [mcp_servers]", '[mcp_servers]\nmagatama.command = "magatama"\nmagatama.args = ["serve"]\n'],
      ["inline table under [mcp_servers]", '[mcp_servers]\nmagatama = { command = "magatama", args = ["serve"] }\n'],
    ];
    for (const [name, text] of shapes) {
      it(`Setup Agents does not add a second magatama table: ${name}`, async () => {
        writeText(codexGlobal(), text);
        const result = await make(on).generateConfig("Codex");
        const after = readText(codexGlobal());
        expect(after.match(/^\[mcp_servers\.magatama\]/gm)).to.equal(null);
        const mine = result.magatama?.find((m) => m.path === codexGlobal());
        expect(mine?.outcome).to.not.equal("added");
      });
    }

    it("does not replace a table whose multi-line string contains a line starting with [", async () => {
      const text = [
        "[mcp_servers.magatama]",
        'command = "magatama"',
        'args = ["serve"]',
        'description = """',
        "[note] keep this",
        '"""',
        "",
      ].join("\n");
      writeText(codexGlobal(), text);
      const outcomes = await make(on).manageMagatamaAt([codexGlobal()]);
      expect(readText(codexGlobal())).to.equal(text);
      expect(outcomes.find((o) => o.path === codexGlobal())?.outcome).to.not.equal("updated");
    });
  });
});

describe("magatama: probeCoversEntry (which entries a version probe speaks for)", () => {
  // ワークスペース側の確認は PATH の `magatama --version` なので、その結果が当てはまるのは
  // 実際に PATH の magatama を起動する登録だけ。ホーム側は登録自身のコマンドで確かめるので常に当てはまる。
  const cases: Array<[string, unknown, "workspace" | "global", boolean]> = [
    ["home entry, any command", { command: "uv", args: ["run", "--directory", "C:\\M", "magatama", "serve"] }, "global", true],
    ["workspace entry running magatama", { command: "magatama", args: ["serve"] }, "workspace", true],
    ["workspace entry running magatama.exe by path", { command: "C:\\py\\Scripts\\MAGATAMA.EXE", args: ["serve"] }, "workspace", true],
    ["workspace uv run --directory", { command: "uv", args: ["run", "--directory", "C:\\M", "magatama", "serve"] }, "workspace", false],
    ["workspace uvx pinned", { command: "uvx", args: ["magatama@0.9.0", "serve"] }, "workspace", false],
    ["workspace entry without a string command", { command: 3, args: ["serve"] }, "workspace", false],
    ["not an object", null, "global", false],
  ];
  for (const [name, entry, scope, expected] of cases) {
    it(name, () => expect(probeCoversEntry(entry, scope)).to.equal(expected));
  }
});
