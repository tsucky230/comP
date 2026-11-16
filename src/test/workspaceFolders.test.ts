// Multi-root workspaces — .comp/config.json `workspace_folders` sync
//
// Coverage:
// - package.json declares comp.indexWorkspaceFolders: on by default, window scope
// - otherFolders(): self exclusion and de-duplication, per platform
// - syncWorkspaceFolders(): each folder that uses comP lists the others; folders
//   without .comp/ are never touched; other keys survive; off clears the list

import { expect } from "chai";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { otherFolders, syncWorkspaceFolders } from "../config/workspaceFolders";

const repoRoot = path.join(__dirname, "..", "..");
const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));

describe("multi-root setting (package.json)", () => {
  const pkg = readJson(path.join(repoRoot, "package.json"));
  const prop = pkg.contributes.configuration.properties["comp.indexWorkspaceFolders"];
  const nlsEn = readJson(path.join(repoRoot, "package.nls.json"));
  const nlsJa = readJson(path.join(repoRoot, "package.nls.ja.json"));
  const resolve = (value: string, nls: Record<string, string>): string => {
    const m = /^%(.+)%$/.exec(value);
    return m ? nls[m[1]] : value;
  };

  it("is a boolean, on by default, window scope", () => {
    expect(prop).to.not.equal(undefined);
    expect(prop.type).to.equal("boolean");
    expect(prop.default).to.equal(true);
    expect(prop.scope).to.equal("window");
  });

  it("names the config key and the cost in both languages", () => {
    const en = resolve(prop.markdownDescription, nlsEn);
    const ja = resolve(prop.markdownDescription, nlsJa);
    for (const text of [en, ja]) {
      expect(text).to.be.a("string");
      expect(text).to.include("workspace_folders");
      expect(text).to.include(".comp/config.json");
    }
    expect(en).to.match(/each folder|every folder/i);
    expect(ja).to.match(/それぞれ|各フォルダ/);
  });
});

describe("otherFolders", () => {
  const cases: Array<{ platform: NodeJS.Platform; folders: string[]; self: string; want: string[] }> = [
    { platform: "linux", folders: ["/w/a", "/w/b", "/w/c"], self: "/w/b", want: ["/w/a", "/w/c"] },
    { platform: "linux", folders: ["/w/a"], self: "/w/a", want: [] },
    { platform: "linux", folders: [], self: "/w/a", want: [] },
    // duplicates of another folder collapse to the first spelling
    { platform: "linux", folders: ["/w/a", "/w/b", "/w/b/", "/w/x/../b"], self: "/w/a", want: ["/w/b"] },
    // self is excluded however it is spelled
    { platform: "linux", folders: ["/w/a/", "/w/x/../a", "/w/b"], self: "/w/a", want: ["/w/b"] },
    // case matters outside Windows
    { platform: "linux", folders: ["/w/A", "/w/a"], self: "/w/a", want: ["/w/A"] },
    { platform: "darwin", folders: ["/w/A", "/w/b"], self: "/w/a", want: ["/w/A", "/w/b"] },
    // a sibling that only shares a prefix is a different folder
    { platform: "linux", folders: ["/w/app", "/w/app2"], self: "/w/app", want: ["/w/app2"] },
  ];
  for (const c of cases) {
    it(`${c.platform}: ${JSON.stringify(c.folders)} minus ${c.self}`, () => {
      expect(otherFolders(c.folders, c.self, c.platform)).to.deep.equal(c.want);
    });
  }

  it("win32: ignores case and separator style when excluding self and removing duplicates", () => {
    const folders = ["e:\\dev\\app", "E:\\Dev\\Lib", "e:/dev/lib/", "E:\\dev\\app2"];
    expect(otherFolders(folders, "E:\\DEV\\APP", "win32")).to.deep.equal(["E:\\Dev\\Lib", "E:\\dev\\app2"]);
  });
});

describe("syncWorkspaceFolders", () => {
  const tmpRoot = path.join(os.tmpdir(), `comp-wsfolders-${process.pid}`);
  let caseIndex = 0;
  let a: string;
  let b: string;
  let c: string;

  beforeEach(() => {
    const base = path.join(tmpRoot, `case-${caseIndex++}`);
    [a, b, c] = ["a", "b", "c"].map((name) => path.join(base, name));
    for (const dir of [a, b, c]) fs.mkdirSync(dir, { recursive: true });
  });

  after(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  });

  const configOf = (folder: string): string => path.join(folder, ".comp", "config.json");
  const useComp = (folder: string, config?: unknown | string): void => {
    fs.mkdirSync(path.join(folder, ".comp"), { recursive: true });
    if (config !== undefined) {
      fs.writeFileSync(configOf(folder), typeof config === "string" ? config : JSON.stringify(config), "utf-8");
    }
  };
  const statuses = (results: Array<{ status: string }>): string[] => results.map((r) => r.status);

  it("lists the other folders in every folder that uses comP", () => {
    useComp(a);
    useComp(b, { exclude: ["dist"] });
    useComp(c);

    const results = syncWorkspaceFolders([a, b, c], true);

    expect(statuses(results)).to.deep.equal(["written", "written", "written"]);
    expect(results.map((r) => r.path)).to.deep.equal([configOf(a), configOf(b), configOf(c)]);
    expect(readJson(configOf(a))).to.deep.equal({ workspace_folders: [b, c] });
    expect(readJson(configOf(b))).to.deep.equal({ exclude: ["dist"], workspace_folders: [a, c] });
    expect(readJson(configOf(c))).to.deep.equal({ workspace_folders: [a, b] });
  });

  it("never creates .comp/ in a folder that does not use comP, but still lists that folder", () => {
    useComp(a);

    const results = syncWorkspaceFolders([a, b], true);

    expect(statuses(results)).to.deep.equal(["written", "skipped"]);
    expect(readJson(configOf(a))).to.deep.equal({ workspace_folders: [b] });
    expect(fs.existsSync(path.join(b, ".comp"))).to.equal(false);
  });

  it("keeps every other key, including hand-written additional_paths", () => {
    const others = {
      exclude: ["dist"],
      additional_paths: ["../shared-lib"],
      max_nodes: 1000,
      ruleSharing: { enabled: true },
    };
    useComp(a, { ...others, workspace_folders: ["/stale/one", "/stale/two"] });

    expect(statuses(syncWorkspaceFolders([a, b], true))).to.deep.equal(["written", "skipped"]);
    expect(readJson(configOf(a))).to.deep.equal({ ...others, workspace_folders: [b] });
  });

  it("does not rewrite a file that already holds the wanted list", () => {
    const text = JSON.stringify({ workspace_folders: [b], exclude: [] }, null, 4);
    useComp(a, text);

    expect(statuses(syncWorkspaceFolders([a, b], true))).to.deep.equal(["unchanged", "skipped"]);
    expect(fs.readFileSync(configOf(a), "utf-8")).to.equal(text);
  });

  it("rewrites when the order of the other folders changed", () => {
    useComp(a, { workspace_folders: [c, b] });

    expect(syncWorkspaceFolders([a, b, c], true)[0].status).to.equal("written");
    expect(readJson(configOf(a))).to.deep.equal({ workspace_folders: [b, c] });
  });

  for (const scenario of ["switched off", "single folder"] as const) {
    it(`clears a stale list when ${scenario}`, () => {
      useComp(a, { exclude: ["dist"], workspace_folders: [b] });

      const results = scenario === "switched off" ? syncWorkspaceFolders([a, b], false) : syncWorkspaceFolders([a], true);

      expect(results[0].status).to.equal("written");
      expect(readJson(configOf(a))).to.deep.equal({ exclude: ["dist"], workspace_folders: [] });
    });

    it(`adds nothing when ${scenario} and there is no list`, () => {
      const text = JSON.stringify({ exclude: ["dist"] });
      useComp(a, text);
      useComp(b);

      const results = scenario === "switched off" ? syncWorkspaceFolders([a, b], false) : syncWorkspaceFolders([a], true);

      expect(statuses(results)).to.deep.equal(scenario === "switched off" ? ["unchanged", "unchanged"] : ["unchanged"]);
      expect(fs.readFileSync(configOf(a), "utf-8")).to.equal(text);
      expect(fs.existsSync(configOf(b))).to.equal(false);
    });
  }

  for (const bad of ["{", "", "[1]", "null", "\"x\""]) {
    it(`leaves an unreadable config.json untouched: ${JSON.stringify(bad)}`, () => {
      useComp(a, bad);
      useComp(b);

      const results = syncWorkspaceFolders([a, b], true);

      expect(statuses(results)).to.deep.equal(["invalid", "written"]);
      expect(fs.readFileSync(configOf(a), "utf-8")).to.equal(bad);
      expect(readJson(configOf(b))).to.deep.equal({ workspace_folders: [a] });
    });
  }

  for (const odd of ["/one/path", { a: 1 }, null, [1, 2]]) {
    it(`replaces a malformed list: ${JSON.stringify(odd)}`, () => {
      useComp(a, { exclude: ["x"], workspace_folders: odd });

      expect(syncWorkspaceFolders([a, b], true)[0].status).to.equal("written");
      expect(readJson(configOf(a))).to.deep.equal({ exclude: ["x"], workspace_folders: [b] });
    });
  }

  it("returns no results for no folders", () => {
    expect(syncWorkspaceFolders([], true)).to.deep.equal([]);
  });
});
