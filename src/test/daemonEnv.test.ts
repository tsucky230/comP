// Hidden home folders — comp.blockHiddenHomeFolders and the daemon environment
//
// Coverage:
// - package.json declares comp.blockHiddenHomeFolders: on by default, machine
//   scope (a repository's .vscode/settings.json cannot turn it off)
// - buildDaemonEnv(): the setting decides COMP_ALLOW_HIDDEN_HOME_ROOTS, and an
//   inherited value in any letter case never wins over it

import { expect } from "chai";
import * as fs from "fs";
import * as path from "path";
import { ALLOW_HIDDEN_HOME_ROOTS_ENV, buildDaemonEnv } from "../config/daemonEnv";

const repoRoot = path.join(__dirname, "..", "..");
const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));

describe("hidden home folder setting (package.json)", () => {
  const pkg = readJson(path.join(repoRoot, "package.json"));
  const prop = pkg.contributes.configuration.properties["comp.blockHiddenHomeFolders"];
  const nlsEn = readJson(path.join(repoRoot, "package.nls.json"));
  const nlsJa = readJson(path.join(repoRoot, "package.nls.ja.json"));
  const resolve = (value: string, nls: Record<string, string>): string => {
    const m = /^%(.+)%$/.exec(value);
    return m ? nls[m[1]] : value;
  };

  it("is a boolean, on by default, machine scope", () => {
    expect(prop).to.not.equal(undefined);
    expect(prop.type).to.equal("boolean");
    expect(prop.default).to.equal(true);
    expect(prop.scope).to.equal("machine");
  });

  it("names what it protects and how agents' daemons are switched, in both languages", () => {
    const en = resolve(prop.markdownDescription, nlsEn);
    const ja = resolve(prop.markdownDescription, nlsJa);
    for (const text of [en, ja]) {
      expect(text).to.be.a("string");
      expect(text).to.include(".ssh");
      expect(text).to.include("AppData");
      expect(text).to.include(ALLOW_HIDDEN_HOME_ROOTS_ENV);
    }
  });
});

describe("buildDaemonEnv", () => {
  const keysLike = (env: NodeJS.ProcessEnv): string[] =>
    Object.keys(env).filter((k) => k.toUpperCase() === ALLOW_HIDDEN_HOME_ROOTS_ENV);

  it("sets the workspace root and log level and keeps the rest", () => {
    const base = { PATH: "/usr/bin", COMP_WORKSPACE_ROOT: "/old", RUST_LOG: "debug" };
    const env = buildDaemonEnv(base, "/ws", true);
    expect(env).to.deep.equal({ PATH: "/usr/bin", COMP_WORKSPACE_ROOT: "/ws", RUST_LOG: "info" });
  });

  it("does not let an inherited switch override the setting when blocking", () => {
    const base = {
      PATH: "/usr/bin",
      COMP_ALLOW_HIDDEN_HOME_ROOTS: "1",
      comp_allow_hidden_home_roots: "true",
      Comp_Allow_Hidden_Home_Roots: "1",
    };
    const env = buildDaemonEnv(base, "/ws", true);
    expect(keysLike(env)).to.deep.equal([]);
    expect(env["PATH"]).to.equal("/usr/bin");
  });

  it("turns the protection off with exactly one variable set to 1", () => {
    const base = { comp_allow_hidden_home_roots: "0", PATH: "/usr/bin" };
    const env = buildDaemonEnv(base, "/ws", false);
    expect(keysLike(env)).to.deep.equal([ALLOW_HIDDEN_HOME_ROOTS_ENV]);
    expect(env[ALLOW_HIDDEN_HOME_ROOTS_ENV]).to.equal("1");
  });

  it("never changes the object it was given", () => {
    const base = { COMP_ALLOW_HIDDEN_HOME_ROOTS: "1", PATH: "/usr/bin" };
    const before = JSON.stringify(base);
    buildDaemonEnv(base, "/ws", true);
    buildDaemonEnv(base, "/ws", false);
    expect(JSON.stringify(base)).to.equal(before);
  });
});
