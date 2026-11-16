// syncBudget: mirroring comp.maxContextTokens into .comp/config.json.

import { expect } from "chai";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { syncBudget } from "../config/budget";

describe("syncBudget", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-budget-${process.pid}`);
  let n = 0;
  let root: string;
  const configPath = () => path.join(root, ".comp", "config.json");
  const writeConfig = (text: string) => {
    fs.mkdirSync(path.join(root, ".comp"), { recursive: true });
    fs.writeFileSync(configPath(), text, "utf-8");
  };
  const readConfig = () => JSON.parse(fs.readFileSync(configPath(), "utf-8"));

  beforeEach(() => {
    root = path.join(tmpRoot, `case-${n++}`);
    fs.mkdirSync(root, { recursive: true });
  });
  after(() => fs.rmSync(tmpRoot, { recursive: true, force: true }));

  it("creates nothing in a folder without .comp/", () => {
    const r = syncBudget(root, 12000, 7000);
    expect(r.status).to.equal("unchanged");
    expect(r.synced).to.equal(7000);
    expect(fs.existsSync(path.join(root, ".comp"))).to.be.false;
  });

  it("creates config.json in an existing .comp/ when the setting is set", () => {
    fs.mkdirSync(path.join(root, ".comp"));
    const r = syncBudget(root, 12000, undefined);
    expect(r).to.include({ status: "written", synced: 12000 });
    expect(readConfig()).to.deep.equal({ default_budget_tokens: 12000 });
  });

  it("does not create config.json while the setting is unset", () => {
    fs.mkdirSync(path.join(root, ".comp"));
    const r = syncBudget(root, undefined, undefined);
    expect(r).to.include({ status: "unchanged", synced: undefined });
    expect(fs.existsSync(configPath())).to.be.false;
  });

  it("writes the value rounded down and keeps the other keys", () => {
    writeConfig(JSON.stringify({ exclude: ["env"], default_budget_tokens: 8000 }));
    const r = syncBudget(root, 16000.7, undefined);
    expect(r).to.include({ status: "written", synced: 16000 });
    expect(readConfig()).to.deep.equal({ exclude: ["env"], default_budget_tokens: 16000 });
  });

  it("leaves the file alone when the value is already there", () => {
    const text = '{"default_budget_tokens":16000}';
    writeConfig(text);
    const r = syncBudget(root, 16000, undefined);
    expect(r).to.include({ status: "unchanged", synced: 16000 });
    expect(fs.readFileSync(configPath(), "utf-8")).to.equal(text);
  });

  it("removes the value comP wrote once the setting is cleared", () => {
    writeConfig(JSON.stringify({ exclude: ["env"], default_budget_tokens: 16000 }));
    const r = syncBudget(root, undefined, 16000);
    expect(r).to.include({ status: "written", synced: undefined });
    expect(readConfig()).to.deep.equal({ exclude: ["env"] });
  });

  it("keeps a value written by hand when the setting is cleared", () => {
    writeConfig(JSON.stringify({ default_budget_tokens: 9000 }));
    const r = syncBudget(root, undefined, 16000);
    expect(r).to.include({ status: "unchanged", synced: undefined });
    expect(readConfig()).to.deep.equal({ default_budget_tokens: 9000 });
  });

  it("keeps a value written by hand when comP never wrote one", () => {
    writeConfig(JSON.stringify({ default_budget_tokens: 9000 }));
    const r = syncBudget(root, undefined, undefined);
    expect(r).to.include({ status: "unchanged", synced: undefined });
    expect(readConfig()).to.deep.equal({ default_budget_tokens: 9000 });
  });

  for (const unusable of [0, -5, Number.NaN, Number.POSITIVE_INFINITY]) {
    it(`treats ${unusable} as unset`, () => {
      writeConfig(JSON.stringify({ default_budget_tokens: 16000 }));
      const r = syncBudget(root, unusable, 16000);
      expect(r).to.include({ status: "written", synced: undefined });
      expect(readConfig()).to.deep.equal({});
    });
  }

  it("reports a file that is not JSON and never replaces it", () => {
    writeConfig("{ broken");
    const r = syncBudget(root, 12000, 7000);
    expect(r.status).to.equal("invalid");
    expect(r.synced).to.equal(7000);
    expect(fs.readFileSync(configPath(), "utf-8")).to.equal("{ broken");
  });

  it("reports a file that is not a JSON object", () => {
    writeConfig("[1, 2]");
    const r = syncBudget(root, 12000, undefined);
    expect(r).to.include({ status: "invalid", reason: "not a JSON object" });
    expect(fs.readFileSync(configPath(), "utf-8")).to.equal("[1, 2]");
  });
});
