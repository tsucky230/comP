// Which daemon binary the extension runs, and whether it matches the extension
//
// Coverage:
// - package.json declares comp.useWorkspaceDaemonBuild: off by default, machine
//   scope (a cloned repository's .vscode/settings.json cannot turn it on and make
//   the extension run an executable the repository ships)
// - resolveDaemonBinary(): the bundled binary wins unless the setting is on; with
//   the setting on, the workspace release build, then the debug build
// - describeDaemonVersionMismatch(): a daemon older or newer than the extension,
//   or one that does not report a version, yields a warning; an equal one does not

import { expect } from "chai";
import * as fs from "fs";
import * as path from "path";
import * as sinon from "sinon";
import * as vscode from "vscode";
import { warnOnDaemonVersionMismatch } from "../extension";
import {
  bundledDaemonBinaryPath,
  describeDaemonVersionMismatch,
  resolveDaemonBinary,
  workspaceDaemonBuildPaths,
} from "../config/daemonBinary";

const repoRoot = path.join(__dirname, "..", "..");
const readJson = (file: string): any => JSON.parse(fs.readFileSync(file, "utf-8"));

describe("workspace daemon build setting (package.json)", () => {
  const pkg = readJson(path.join(repoRoot, "package.json"));
  const prop = pkg.contributes.configuration.properties["comp.useWorkspaceDaemonBuild"];

  it("is a boolean, off by default, machine scope, described in both languages", () => {
    expect(prop).to.not.equal(undefined);
    expect(prop.type).to.equal("boolean");
    expect(prop.default).to.equal(false);
    expect(prop.scope).to.equal("machine");
    const key = /^%(.+)%$/.exec(prop.markdownDescription)?.[1];
    expect(key).to.be.a("string");
    expect(readJson(path.join(repoRoot, "package.nls.json"))[key as string]).to.be.a("string");
    expect(readJson(path.join(repoRoot, "package.nls.ja.json"))[key as string]).to.be.a("string");
  });
});

describe("resolveDaemonBinary", () => {
  const ext = path.join("X:", "ext", "tsucky230.comp-vscode-0.12.0");
  const ws = path.join("X:", "work", "comP");

  for (const platform of ["win32", "darwin", "linux"] as const) {
    const bundled = bundledDaemonBinaryPath(ext, platform);
    const [release, debug] = workspaceDaemonBuildPaths(ws, platform);
    const resolve = (useWorkspaceBuild: boolean, present: string[]): string | null =>
      resolveDaemonBinary({
        extensionPath: ext,
        workspaceRoot: ws,
        useWorkspaceBuild,
        platform,
        exists: (p: string) => present.includes(p),
      });

    it(`${platform}: uses the bundled binary while the setting is off, even with a workspace build`, () => {
      expect(resolve(false, [bundled, release, debug])).to.equal(bundled);
    });

    it(`${platform}: uses the workspace release build, then the debug build, when the setting is on`, () => {
      expect(resolve(true, [bundled, release, debug])).to.equal(release);
      expect(resolve(true, [bundled, debug])).to.equal(debug);
      expect(resolve(true, [bundled])).to.equal(bundled);
    });

    it(`${platform}: returns null when nothing usable exists`, () => {
      expect(resolve(false, [release])).to.equal(null);
      expect(resolve(true, [])).to.equal(null);
    });
  }

  it("names the platform binaries the VSIX ships and cargo builds", () => {
    expect(path.basename(bundledDaemonBinaryPath(ext, "win32"))).to.equal("comp-daemon-win.exe");
    expect(path.basename(bundledDaemonBinaryPath(ext, "darwin"))).to.equal("comp-daemon-macos");
    expect(path.basename(bundledDaemonBinaryPath(ext, "linux"))).to.equal("comp-daemon-linux");
    expect(workspaceDaemonBuildPaths(ws, "win32")).to.deep.equal([
      path.join(ws, "daemon", "target", "release", "comp-daemon.exe"),
      path.join(ws, "daemon", "target", "debug", "comp-daemon.exe"),
    ]);
  });

  it("ignores the workspace when there is none", () => {
    expect(
      resolveDaemonBinary({ extensionPath: ext, useWorkspaceBuild: true, platform: "linux", exists: () => true })
    ).to.equal(bundledDaemonBinaryPath(ext, "linux"));
  });
});

describe("warnOnDaemonVersionMismatch", () => {
  let warn: sinon.SinonStub;
  let original: unknown;
  const context = (version?: string): any => ({ extension: { packageJSON: { version } } });

  beforeEach(() => {
    original = (vscode.window as any).showWarningMessage;
    warn = sinon.stub().resolves(undefined);
    (vscode.window as any).showWarningMessage = warn;
  });

  afterEach(() => {
    (vscode.window as any).showWarningMessage = original;
  });

  it("shows one warning naming both versions when they differ", () => {
    warnOnDaemonVersionMismatch(context("0.12.0"), "0.11.11");
    expect(warn.calledOnce).to.equal(true);
    expect(warn.firstCall.args[0]).to.include("0.11.11");
    expect(warn.firstCall.args[0]).to.include("0.12.0");
  });

  it("stays quiet when the versions match or the extension version is unknown", () => {
    warnOnDaemonVersionMismatch(context("0.12.0"), "0.12.0");
    warnOnDaemonVersionMismatch(context(undefined), "0.11.11");
    warnOnDaemonVersionMismatch({} as any, "0.11.11");
    expect(warn.called).to.equal(false);
  });
});

describe("describeDaemonVersionMismatch", () => {
  it("is null when the daemon reports the extension's version", () => {
    expect(describeDaemonVersionMismatch("0.12.0", "0.12.0")).to.equal(null);
  });

  it("names both versions when they differ, in either direction", () => {
    const older = describeDaemonVersionMismatch("0.12.0", "0.11.11");
    expect(older).to.include("0.11.11");
    expect(older).to.include("0.12.0");
    expect(describeDaemonVersionMismatch("0.12.0", "0.13.0")).to.include("0.13.0");
  });

  it("warns when the daemon does not report a version (older than 0.9.2)", () => {
    const missing = describeDaemonVersionMismatch("0.12.0", undefined);
    expect(missing).to.be.a("string");
    expect(missing).to.include("0.12.0");
  });
});
