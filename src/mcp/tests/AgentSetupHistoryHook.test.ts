// AgentSetup — Claude Code Stop hook (`comp-daemon record-turn`) tests
//
// Coverage:
// - generateConfig("Claude Code") installs the hook into .claude/settings.local.json
// - merging with existing settings, dedup, replacement of an outdated command
// - refusal cases: invalid JSON, project already has history-record, binary missing,
//   shell metacharacters in the path
// - repairStaleConfigs() rewrites a stale hook command after an extension upgrade
// - the Session Continuity snippet no longer claims prompt auto-injection

import { expect } from "chai";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { AgentSetupManager, RepairEntry } from "../AgentSetup";

class MockDaemonManager {
  async request(): Promise<unknown> {
    return {};
  }
}

const fwd = (p: string): string => p.replace(/\\/g, "/");
const expectedCommand = (daemon: string, ws: string): string =>
  `"${fwd(daemon)}" record-turn "${fwd(ws)}"`;

const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));
const writeText = (file: string, value: string): void => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, value, "utf-8");
};
const writeJson = (file: string, value: unknown): void =>
  writeText(file, JSON.stringify(value, null, 2));

/** Every command string under hooks.Stop[*].hooks[*]. */
const stopCommands = (doc: any): string[] =>
  ((doc?.hooks?.Stop ?? []) as any[]).flatMap((m) =>
    ((m?.hooks ?? []) as any[]).map((h) => h?.command)
  );

describe("AgentSetupManager — Claude Code history hook", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-historyhook-${process.pid}`);
  let caseIndex = 0;
  let ws: string;
  let fakeHome: string;
  let daemon: string;
  let manager: AgentSetupManager;
  let localSettings: string;

  const makeManager = (workspace: string, daemonPath: string, locale?: "en" | "ja") => {
    const m = new AgentSetupManager(new MockDaemonManager() as any, workspace, undefined, {
      homeDir: fakeHome,
      codexHome: path.join(fakeHome, ".codex"),
      ...(locale ? { locale } : {}),
    });
    (m as any).getDaemonPath = () => daemonPath;
    return m;
  };

  beforeEach(() => {
    const caseDir = path.join(tmpRoot, `case-${caseIndex++}`);
    ws = path.join(caseDir, "workspace");
    fakeHome = path.join(caseDir, "home");
    daemon = path.join(caseDir, "ext", "daemon", "target", "release", "comp-daemon-win.exe");
    fs.mkdirSync(ws, { recursive: true });
    fs.mkdirSync(fakeHome, { recursive: true });
    writeText(daemon, "");
    localSettings = path.join(ws, ".claude", "settings.local.json");
    manager = makeManager(ws, daemon);
  });

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  describe("installation", () => {
    it("creates settings.local.json with one Stop hook in Claude Code's format", async () => {
      const result = await manager.generateConfig("Claude Code");
      expect(result.historyHook).to.deep.include({ path: localSettings, scope: "workspace", status: "written" });
      const doc = readJson(localSettings);
      expect(doc.hooks.Stop).to.have.length(1);
      expect(doc.hooks.Stop[0].hooks).to.deep.equal([
        { type: "command", command: expectedCommand(daemon, ws), timeout: 10 },
      ]);
    });

    it("uses forward slashes and quotes both paths, including paths with spaces", async () => {
      const spaced = path.join(path.dirname(ws), "my work space");
      fs.mkdirSync(spaced, { recursive: true });
      const m = makeManager(spaced, daemon);
      await m.generateConfig("Claude Code");
      const cmd = stopCommands(readJson(path.join(spaced, ".claude", "settings.local.json")))[0];
      expect(cmd).to.equal(expectedCommand(daemon, spaced));
      expect(cmd).to.not.include("\\");
    });

    for (const agent of ["Cursor", "Codex", "GitHub Copilot", "Gemini CLI"]) {
      it(`does not install the hook for ${agent}`, async () => {
        const result = await manager.generateConfig(agent);
        expect(result.historyHook).to.equal(undefined);
        expect(fs.existsSync(localSettings)).to.equal(false);
      });
    }

    it("does not change success, configPath or writes", async () => {
      const result = await manager.generateConfig("Claude Code");
      expect(result.success).to.equal(true);
      expect(result.configPath).to.equal(path.join(ws, ".mcp.json"));
      expect(result.writes.map((w) => w.path)).to.not.include(localSettings);
    });

    it("still reports the MCP write as success when the hook fails", async () => {
      writeText(localSettings, "{ not json");
      const result = await manager.generateConfig("Claude Code");
      expect(result.success).to.equal(true);
      expect(result.historyHook?.status).to.equal("failed");
    });
  });

  describe("merging", () => {
    it("keeps unrelated keys, other events and other Stop hooks", async () => {
      writeJson(localSettings, {
        permissions: { allow: ["Bash(ls)"] },
        hooks: {
          PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: "guard.sh" }] }],
          Stop: [{ hooks: [{ type: "command", command: "notify.sh" }] }],
        },
      });
      const result = await manager.generateConfig("Claude Code");
      expect(result.historyHook?.status).to.equal("written");
      const doc = readJson(localSettings);
      expect(doc.permissions).to.deep.equal({ allow: ["Bash(ls)"] });
      expect(doc.hooks.PreToolUse).to.deep.equal([
        { matcher: "Bash", hooks: [{ type: "command", command: "guard.sh" }] },
      ]);
      expect(stopCommands(doc)).to.deep.equal(["notify.sh", expectedCommand(daemon, ws)]);
    });

    it("backs up an existing file before rewriting it", async () => {
      writeJson(localSettings, { model: "x" });
      const result = await manager.generateConfig("Claude Code");
      expect(result.historyHook?.backupPath).to.equal(localSettings + ".bak");
      expect(readJson(localSettings + ".bak")).to.deep.equal({ model: "x" });
    });

    it("is idempotent: a second run adds nothing and reports skipped", async () => {
      await manager.generateConfig("Claude Code");
      const before = fs.readFileSync(localSettings, "utf-8");
      const second = await manager.generateConfig("Claude Code");
      expect(second.historyHook?.status).to.equal("skipped");
      expect(second.historyHook?.reason).to.match(/already/i);
      expect(fs.readFileSync(localSettings, "utf-8")).to.equal(before);
    });

    for (const old of [
      `"C:/Users/u/.vscode/extensions/tsucky230.comp-vscode-0.11.6/daemon/target/release/comp-daemon-win.exe" record-turn "C:/old/ws"`,
      `"/gone/comp-daemon" record-turn`,
      `comp-daemon record-turn`,
    ]) {
      it(`replaces an outdated record-turn command instead of adding a second one: ${old}`, async () => {
        writeJson(localSettings, {
          hooks: { Stop: [{ hooks: [{ type: "command", command: old, timeout: 10 }] }, { hooks: [{ type: "command", command: "keep.sh" }] }] },
        });
        const result = await manager.generateConfig("Claude Code");
        expect(result.historyHook?.status).to.equal("written");
        expect(stopCommands(readJson(localSettings))).to.deep.equal([expectedCommand(daemon, ws), "keep.sh"]);
      });
    }

    for (const [label, content] of [
      ["array", "[]"],
      ["string", "\"x\""],
      ["hooks is an array", JSON.stringify({ hooks: [] })],
      ["Stop is an object", JSON.stringify({ hooks: { Stop: {} } })],
    ] as const) {
      it(`refuses to rewrite a settings file of the wrong shape (${label})`, async () => {
        writeText(localSettings, content);
        const result = await manager.generateConfig("Claude Code");
        expect(result.historyHook?.status).to.equal("failed");
        expect(fs.readFileSync(localSettings, "utf-8")).to.equal(content);
      });
    }
  });

  describe("refusals", () => {
    for (const bad of ["{ not json", "", "{\"hooks\": {\"Stop\": [}"]) {
      it(`leaves invalid JSON untouched and takes no backup: ${JSON.stringify(bad)}`, async () => {
        writeText(localSettings, bad);
        const result = await manager.generateConfig("Claude Code");
        expect(result.historyHook?.status).to.equal("failed");
        expect(fs.readFileSync(localSettings, "utf-8")).to.equal(bad);
        expect(fs.existsSync(localSettings + ".bak")).to.equal(false);
      });
    }

    for (const where of ["settings.json", "settings.local.json"]) {
      it(`skips when ${where} already runs history-record (no double recording)`, async () => {
        const file = path.join(ws, ".claude", where);
        writeJson(file, {
          hooks: { Stop: [{ hooks: [{ type: "command", command: "$CLAUDE_PROJECT_DIR/.claude/hooks/history-record.sh" }] }] },
        });
        const before = fs.readFileSync(file, "utf-8");
        const result = await manager.generateConfig("Claude Code");
        expect(result.historyHook?.status).to.equal("skipped");
        expect(result.historyHook?.reason).to.match(/history-record/);
        expect(fs.readFileSync(file, "utf-8")).to.equal(before);
        if (where === "settings.json") {
          expect(fs.existsSync(localSettings)).to.equal(false);
        }
      });
    }

    it("skips when the daemon binary does not exist", async () => {
      const m = makeManager(ws, path.join(ws, ".comp", "bin", "comp-daemon.exe"));
      const result = await m.generateConfig("Claude Code");
      expect(result.historyHook?.status).to.equal("skipped");
      expect(fs.existsSync(localSettings)).to.equal(false);
    });

    // The metacharacter check runs before the existence check, so these paths
    // need not exist ('"' and '|' cannot even be created on Windows).
    for (const ch of ['"', "&", "%", "|", "$", "`"]) {
      it(`skips when the daemon path contains the shell metacharacter ${ch}`, async () => {
        const m = makeManager(ws, path.join(path.dirname(daemon), `a${ch}b.exe`));
        const result = await m.generateConfig("Claude Code");
        expect(result.historyHook?.status).to.equal("skipped");
        expect(fs.existsSync(localSettings)).to.equal(false);
      });
      it(`skips when the workspace path contains the shell metacharacter ${ch}`, async () => {
        const m = makeManager(path.join(path.dirname(ws), `w${ch}s`), daemon);
        (m as any).workspaceRoot = path.join(path.dirname(ws), `w${ch}s`);
        const result = await m.generateConfig("Claude Code");
        expect(result.historyHook?.status).to.equal("skipped");
      });
    }
  });

  describe("repairStaleConfigs", () => {
    const hookEntry = (entries: RepairEntry[]): RepairEntry | undefined =>
      entries.find((e) => e.file === localSettings);

    it("rewrites a hook whose daemon path disappeared", () => {
      const stale = `"C:/gone/comp-vscode-0.11.6/daemon/target/release/comp-daemon-win.exe" record-turn "${fwd(ws)}"`;
      writeJson(localSettings, {
        hooks: { Stop: [{ hooks: [{ type: "command", command: "keep.sh" }] }, { hooks: [{ type: "command", command: stale, timeout: 10 }] }] },
      });
      (manager as any).resolveRepairPath = () => daemon;
      const entry = hookEntry(manager.repairStaleConfigs());
      expect(entry).to.deep.include({ status: "repaired", from: stale, to: expectedCommand(daemon, ws) });
      expect(stopCommands(readJson(localSettings))).to.deep.equal(["keep.sh", expectedCommand(daemon, ws)]);
    });

    it("rewrites the workspace argument when the project moved", () => {
      const moved = `"${fwd(daemon)}" record-turn "C:/old/location"`;
      writeJson(localSettings, { hooks: { Stop: [{ hooks: [{ type: "command", command: moved }] }] } });
      (manager as any).resolveRepairPath = () => daemon;
      const entry = hookEntry(manager.repairStaleConfigs());
      expect(entry?.status).to.equal("repaired");
      expect(stopCommands(readJson(localSettings))).to.deep.equal([expectedCommand(daemon, ws)]);
    });

    it("reports healthy and leaves the file byte-identical when nothing is stale", async () => {
      await manager.generateConfig("Claude Code");
      const before = fs.readFileSync(localSettings, "utf-8");
      const entry = hookEntry(manager.repairStaleConfigs());
      expect(entry?.status).to.equal("healthy");
      expect(fs.readFileSync(localSettings, "utf-8")).to.equal(before);
    });

    it("reports missing when there is no settings.local.json", () => {
      expect(hookEntry(manager.repairStaleConfigs())?.status).to.equal("missing");
    });

    it("skips a settings file without a record-turn hook", () => {
      writeJson(localSettings, { hooks: { Stop: [{ hooks: [{ type: "command", command: "notify.sh" }] }] } });
      expect(hookEntry(manager.repairStaleConfigs())?.status).to.equal("skipped");
    });

    it("skips rather than writing a broken path when no replacement binary exists", () => {
      const stale = `"C:/gone/comp-daemon-win.exe" record-turn "${fwd(ws)}"`;
      writeJson(localSettings, { hooks: { Stop: [{ hooks: [{ type: "command", command: stale }] }] } });
      (manager as any).resolveRepairPath = () => null;
      const entry = hookEntry(manager.repairStaleConfigs());
      expect(entry?.status).to.equal("skipped");
      expect(stopCommands(readJson(localSettings))).to.deep.equal([stale]);
    });

    it("reports failed for invalid JSON without touching it", () => {
      writeText(localSettings, "{ nope");
      expect(hookEntry(manager.repairStaleConfigs())?.status).to.equal("failed");
      expect(fs.readFileSync(localSettings, "utf-8")).to.equal("{ nope");
    });
  });

  describe("Session Continuity snippet", () => {
    for (const locale of ["en", "ja"] as const) {
      it(`does not claim prompt auto-injection and names the Stop hook (${locale})`, async () => {
        const m = makeManager(ws, daemon, locale);
        await m.generateConfig("Claude Code");
        const text = fs.readFileSync(path.join(ws, "CLAUDE.md"), "utf-8");
        expect(text).to.not.include("auto-injects");
        expect(text).to.not.include("自動的に注入");
        expect(text).to.include("record-turn");
        expect(text).to.include(".comp/history");
      });
    }
  });
});
