// Conversation recording (beta) — settings contribution and .comp/config.json sync
//
// Coverage:
// - package.json declares the two settings, off by default, marked beta, resource scope
// - package.nls.json / package.nls.ja.json carry the texts the settings refer to
// - syncConversationRecording(): merge into config.json without losing other keys,
//   no write when unchanged, no file created just to say "off", invalid JSON left untouched

import { expect } from "chai";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import {
  DEFAULT_CONVERSATION_RECORDING,
  syncConversationRecording,
} from "../config/conversationRecording";

const repoRoot = path.join(__dirname, "..", "..");
const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));

describe("conversation recording settings (package.json)", () => {
  const pkg = readJson(path.join(repoRoot, "package.json"));
  const props = pkg.contributes.configuration.properties;
  const nlsEn = readJson(path.join(repoRoot, "package.nls.json"));
  const nlsJa = readJson(path.join(repoRoot, "package.nls.ja.json"));

  const resolve = (value: string, nls: Record<string, string>): string => {
    const m = /^%(.+)%$/.exec(value);
    return m ? nls[m[1]] : value;
  };

  it("declares enabled as a boolean that is off by default", () => {
    const p = props["comp.conversationRecording.enabled"];
    expect(p).to.not.equal(undefined);
    expect(p.type).to.equal("boolean");
    expect(p.default).to.equal(false);
  });

  it("declares claudeCode as a boolean that is on by default (only matters once enabled)", () => {
    const p = props["comp.conversationRecording.claudeCode"];
    expect(p).to.not.equal(undefined);
    expect(p.type).to.equal("boolean");
    expect(p.default).to.equal(true);
  });

  for (const key of ["comp.conversationRecording.enabled", "comp.conversationRecording.claudeCode"]) {
    it(`${key} uses resource scope so a user can turn it on everywhere or per workspace`, () => {
      expect(props[key].scope).to.equal("resource");
    });

    it(`${key} is marked beta in both languages`, () => {
      const desc = props[key].markdownDescription ?? props[key].description;
      expect(desc, "description").to.be.a("string");
      const en = resolve(desc, nlsEn);
      const ja = resolve(desc, nlsJa);
      expect(en, "en text").to.be.a("string").and.match(/beta/i);
      expect(ja, "ja text").to.be.a("string").and.match(/β版/);
    });
  }

  it("explains in the enabled setting where hooks go, where records go, and that off keeps hooks", () => {
    const desc = props["comp.conversationRecording.enabled"].markdownDescription;
    for (const nls of [nlsEn, nlsJa]) {
      const text = resolve(desc, nls);
      expect(text).to.include(".claude/settings.local.json");
      expect(text).to.include(".comp/history");
      expect(text).to.include("comp.removeHistoryHooks");
    }
  });

  it("contributes the remove-hooks command with a localized title", () => {
    const cmd = pkg.contributes.commands.find((c: any) => c.command === "comp.removeHistoryHooks");
    expect(cmd).to.not.equal(undefined);
    expect(resolve(cmd.title, nlsEn)).to.be.a("string").and.not.equal("");
    expect(resolve(cmd.title, nlsJa)).to.be.a("string").and.not.equal("");
  });
});

describe("syncConversationRecording", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-recsync-${process.pid}`);
  let caseIndex = 0;
  let ws: string;
  let configPath: string;

  beforeEach(() => {
    ws = path.join(tmpRoot, `case-${caseIndex++}`);
    fs.mkdirSync(ws, { recursive: true });
    configPath = path.join(ws, ".comp", "config.json");
  });

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  const writeConfig = (text: string) => {
    fs.mkdirSync(path.dirname(configPath), { recursive: true });
    fs.writeFileSync(configPath, text, "utf-8");
  };

  it("defaults to off", () => {
    expect(DEFAULT_CONVERSATION_RECORDING).to.deep.equal({ enabled: false, claudeCode: true });
  });

  it("writes the section in the shape record-turn reads", () => {
    const r = syncConversationRecording(ws, { enabled: true, claudeCode: true });
    expect(r.status).to.equal("written");
    expect(readJson(configPath)).to.deep.equal({
      conversationRecording: { enabled: true, agents: { "claude-code": true } },
    });
  });

  for (const [enabled, claudeCode] of [[true, false], [false, true], [false, false]] as const) {
    it(`keeps other keys and other agents when writing enabled=${enabled} claudeCode=${claudeCode}`, () => {
      writeConfig(JSON.stringify({
        exclude: ["dist"],
        max_nodes: 5,
        conversationRecording: { enabled: !enabled, agents: { "claude-code": !claudeCode, "gemini-cli": true } },
      }));
      const r = syncConversationRecording(ws, { enabled, claudeCode });
      expect(r.status).to.equal("written");
      expect(readJson(configPath)).to.deep.equal({
        exclude: ["dist"],
        max_nodes: 5,
        conversationRecording: { enabled, agents: { "claude-code": claudeCode, "gemini-cli": true } },
      });
    });
  }

  it("does not rewrite the file when nothing changed", () => {
    const text = JSON.stringify({ exclude: [], conversationRecording: { enabled: true, agents: { "claude-code": true } } }, null, 4);
    writeConfig(text);
    const r = syncConversationRecording(ws, { enabled: true, claudeCode: true });
    expect(r.status).to.equal("unchanged");
    expect(fs.readFileSync(configPath, "utf-8")).to.equal(text);
  });

  it("does not create .comp/ just to record that recording is off", () => {
    const r = syncConversationRecording(ws, { enabled: false, claudeCode: true });
    expect(r.status).to.equal("unchanged");
    expect(fs.existsSync(path.join(ws, ".comp"))).to.equal(false);
  });

  it("creates the file when turning recording on in a fresh workspace", () => {
    const r = syncConversationRecording(ws, { enabled: true, claudeCode: false });
    expect(r.status).to.equal("written");
    expect(readJson(configPath).conversationRecording).to.deep.equal({ enabled: true, agents: { "claude-code": false } });
  });

  for (const bad of ["{", "", "not json", "[1,2]", "\"s\"", "null"]) {
    it(`leaves an unreadable config.json untouched and reports it: ${JSON.stringify(bad)}`, () => {
      writeConfig(bad);
      const r = syncConversationRecording(ws, { enabled: true, claudeCode: true });
      expect(r.status).to.equal("invalid");
      expect(fs.readFileSync(configPath, "utf-8")).to.equal(bad);
    });
  }

  for (const odd of [
    { conversationRecording: true },
    { conversationRecording: "on" },
    { conversationRecording: { enabled: true, agents: [] } },
  ]) {
    it(`replaces a malformed section rather than failing: ${JSON.stringify(odd)}`, () => {
      writeConfig(JSON.stringify({ exclude: ["x"], ...odd }));
      const r = syncConversationRecording(ws, { enabled: true, claudeCode: true });
      expect(r.status).to.equal("written");
      expect(readJson(configPath)).to.deep.equal({
        exclude: ["x"],
        conversationRecording: { enabled: true, agents: { "claude-code": true } },
      });
    });
  }
});
