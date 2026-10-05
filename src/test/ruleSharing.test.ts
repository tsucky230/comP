// Rule sharing (beta) — settings contribution and .comp/config.json sync
//
// Coverage:
// - package.json declares comp.ruleSharing.enabled: off by default, beta, resource scope,
//   and the description names its boundaries (git-tracked files only, no user settings)
// - syncRuleSharing(): same merge rules as conversation recording, without touching
//   the conversationRecording section or other keys

import { expect } from "chai";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { DEFAULT_RULE_SHARING, syncRuleSharing } from "../config/ruleSharing";

const repoRoot = path.join(__dirname, "..", "..");
const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));

describe("rule sharing setting (package.json)", () => {
  const pkg = readJson(path.join(repoRoot, "package.json"));
  const prop = pkg.contributes.configuration.properties["comp.ruleSharing.enabled"];
  const nlsEn = readJson(path.join(repoRoot, "package.nls.json"));
  const nlsJa = readJson(path.join(repoRoot, "package.nls.ja.json"));
  const resolve = (value: string, nls: Record<string, string>): string => {
    const m = /^%(.+)%$/.exec(value);
    return m ? nls[m[1]] : value;
  };

  it("is a boolean, off by default, resource scope", () => {
    expect(prop).to.not.equal(undefined);
    expect(prop.type).to.equal("boolean");
    expect(prop.default).to.equal(false);
    expect(prop.scope).to.equal("resource");
  });

  it("is marked beta and states its boundaries in both languages", () => {
    const en = resolve(prop.markdownDescription, nlsEn);
    const ja = resolve(prop.markdownDescription, nlsJa);
    expect(en).to.be.a("string").and.match(/beta/i);
    expect(ja).to.be.a("string").and.match(/β版/);
    for (const text of [en, ja]) {
      expect(text).to.include("git");
      expect(text).to.include("check_rule_conflicts");
      expect(text).to.include(".comp/rules");
      expect(text).to.include("run_pipeline");
    }
    expect(en).to.match(/user settings|home directory/i);
    expect(ja).to.match(/ユーザー設定|ホーム/);
  });
});

describe("syncRuleSharing", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-rulesync-${process.pid}`);
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
    expect(DEFAULT_RULE_SHARING).to.deep.equal({ enabled: false });
  });

  it("writes the section the daemon reads", () => {
    expect(syncRuleSharing(ws, { enabled: true }).status).to.equal("written");
    expect(readJson(configPath)).to.deep.equal({ ruleSharing: { enabled: true } });
  });

  for (const enabled of [true, false]) {
    it(`keeps other keys, the conversationRecording section and extra ruleSharing keys (enabled=${enabled})`, () => {
      const others = {
        exclude: ["dist"],
        conversationRecording: { enabled: true, agents: { "claude-code": true } },
      };
      writeConfig(JSON.stringify({ ...others, ruleSharing: { enabled: !enabled, maxTokens: 500 } }));
      expect(syncRuleSharing(ws, { enabled }).status).to.equal("written");
      expect(readJson(configPath)).to.deep.equal({ ...others, ruleSharing: { enabled, maxTokens: 500 } });
    });
  }

  it("does not rewrite the file when nothing changed", () => {
    const text = JSON.stringify({ ruleSharing: { enabled: true }, exclude: [] }, null, 4);
    writeConfig(text);
    expect(syncRuleSharing(ws, { enabled: true }).status).to.equal("unchanged");
    expect(fs.readFileSync(configPath, "utf-8")).to.equal(text);
  });

  it("does not create .comp/ just to record that sharing is off", () => {
    expect(syncRuleSharing(ws, { enabled: false }).status).to.equal("unchanged");
    expect(fs.existsSync(path.join(ws, ".comp"))).to.equal(false);
  });

  for (const bad of ["{", "", "[1]", "null", "\"x\""]) {
    it(`leaves an unreadable config.json untouched: ${JSON.stringify(bad)}`, () => {
      writeConfig(bad);
      expect(syncRuleSharing(ws, { enabled: true }).status).to.equal("invalid");
      expect(fs.readFileSync(configPath, "utf-8")).to.equal(bad);
    });
  }

  for (const odd of [{ ruleSharing: true }, { ruleSharing: "on" }, { ruleSharing: [1] }]) {
    it(`replaces a malformed section: ${JSON.stringify(odd)}`, () => {
      writeConfig(JSON.stringify({ exclude: ["x"], ...odd }));
      expect(syncRuleSharing(ws, { enabled: true }).status).to.equal("written");
      expect(readJson(configPath)).to.deep.equal({ exclude: ["x"], ruleSharing: { enabled: true } });
    });
  }
});
