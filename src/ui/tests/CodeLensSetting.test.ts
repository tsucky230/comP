// comp.enableCodeLens: the provider shows nothing while the setting is off.

import { expect } from "chai";
import * as vscode from "vscode";
import { DependencyCodeLensProvider } from "../CodeLens";

describe("DependencyCodeLensProvider and comp.enableCodeLens", () => {
  const workspace = vscode.workspace as unknown as { getConfiguration: unknown };
  const original = workspace.getConfiguration;
  const daemon = {
    getSymbols: async (_p: string) => [
      { id: 1, name: "MyClass", kind: "class", line: 0, column: 0, scope: "export", dependents: 2 },
    ],
    isRunning: () => true,
  };
  const doc = {
    languageId: "typescript",
    uri: { fsPath: "/path/file.ts" },
    lineCount: 10,
    lineAt: (_line: number) => ({ range: { end: { character: 80 } } }),
  } as unknown as vscode.TextDocument;

  const setEnabled = (value: boolean | undefined) => {
    workspace.getConfiguration = (section?: string) => ({
      get: (key: string, fallback?: unknown) =>
        section === "comp" && key === "enableCodeLens" && value !== undefined ? value : fallback,
    });
  };

  afterEach(() => {
    workspace.getConfiguration = original;
  });

  it("shows no CodeLens while the setting is off", async () => {
    setEnabled(false);
    const p = new DependencyCodeLensProvider(daemon as never);
    expect(await p.provideCodeLenses(doc, null as never)).to.deep.equal([]);
    p.dispose();
  });

  it("shows CodeLens while the setting is on", async () => {
    setEnabled(true);
    const p = new DependencyCodeLensProvider(daemon as never);
    expect(await p.provideCodeLenses(doc, null as never)).to.have.length(1);
    p.dispose();
  });

  it("shows CodeLens when the setting is left at its default", async () => {
    setEnabled(undefined);
    const p = new DependencyCodeLensProvider(daemon as never);
    expect(await p.provideCodeLenses(doc, null as never)).to.have.length(1);
    p.dispose();
  });
});
