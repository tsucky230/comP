// CodeLens - Display dependency information as inline annotations
//
// Responsibilities:
// 1. Query daemon for symbols in active document
// 2. Calculate dependent count for each exported symbol
// 3. Render CodeLens inline showing "X references" for each symbol

import * as vscode from "vscode";
import { DaemonManager } from "../daemon/DaemonManager";

interface SymbolInfo {
  id: number;
  name: string;
  kind: string;
  line: number;
  column: number;
  dependentCount: number;
  scope?: string;
}

interface DaemonSymbol {
  id: number;
  name: string;
  kind: string;
  line: number;
  column: number;
  scope?: string;
  dependents?: number;
}

/**
 * CodeLens provider for displaying symbol dependency information
 *
 * # Responsibilities
 * - Language support check: TypeScript, Python, Go, Rust, Java, C#, etc.
 * - Retrieve symbol information from the daemon
 * - Display only exported/public symbols
 * - Show dependency count for each symbol
 */
export class DependencyCodeLensProvider implements vscode.CodeLensProvider {
  private daemonManager: DaemonManager;
  private onDidChangeCodeLensesEmitter = new vscode.EventEmitter<void>();
  private cache = new Map<string, DaemonSymbol[]>();

  readonly onDidChangeCodeLenses = this.onDidChangeCodeLensesEmitter.event;

  constructor(daemonManager: DaemonManager) {
    this.daemonManager = daemonManager;
  }

  /**
   * Provide code lenses for document
   *
   * # Inputs
   * - document: VSCode document
   * - token: Cancellation token
   *
   * # Outputs
   * - Array of CodeLens (dependency info for exported symbols)
   *
   * # Prerequisites
   * - The daemon must be running
   */
  async provideCodeLenses(
    document: vscode.TextDocument,
    _token: vscode.CancellationToken
  ): Promise<vscode.CodeLens[]> {
    // Language filter: only process supported languages
    if (!this.isSupportedLanguage(document.languageId)) {
      return [];
    }
    // Read on every call so that turning the setting off takes effect on the next refresh.
    if (!vscode.workspace.getConfiguration("comp").get<boolean>("enableCodeLens", true)) {
      return [];
    }

    const codeLenses: vscode.CodeLens[] = [];

    try {
      const filePath = document.uri.fsPath;

      // Get from cache, otherwise query the daemon
      let symbols = this.cache.get(filePath);
      if (!symbols) {
        try {
          symbols = (await this.daemonManager.getSymbols(filePath)) || [];
          this.cache.set(filePath, symbols);
        } catch (error) {
          // Return empty array if daemon query fails
          console.debug("[comP] Failed to query symbols:", error);
          return [];
        }
      }

      // Filter for exported/public symbols and generate CodeLenses
      for (const symbol of symbols) {
        // Display only exported or public symbols
        if (!this.isExportedSymbol(symbol)) {
          continue;
        }

        // Verify if line number is within document bounds
        if (symbol.line >= document.lineCount) {
          continue;
        }

        const line = document.lineAt(symbol.line);
        const range = new vscode.Range(
          new vscode.Position(symbol.line, symbol.column || 0),
          new vscode.Position(
            symbol.line,
            Math.min(
              symbol.column + (symbol.name?.length || 0),
              line.range.end.character
            )
          )
        );

        const symbolInfo: SymbolInfo = {
          id: symbol.id,
          name: symbol.name,
          kind: symbol.kind,
          line: symbol.line,
          column: symbol.column || 0,
          dependentCount: symbol.dependents || 0,
          scope: symbol.scope,
        };

        const codeLens = new vscode.CodeLens(range, undefined);
        (codeLens as any).data = symbolInfo;
        codeLenses.push(codeLens);
      }

      return codeLenses;
    } catch (error) {
      console.error("[comP] CodeLens error:", error);
      return [];
    }
  }

  /**
   * Resolve code lens command
   *
   * # Inputs
   * - codeLens: VSCode CodeLens object
   * - token: Cancellation token
   *
   * # Outputs
   * - CodeLens with command configured
   *
   * # Behavior
   * - Dependent count > 0: Display "X references", click starts reference provider
   * - Dependent count = 0: Display "No references" (no-op command)
   */
  resolveCodeLens(
    codeLens: vscode.CodeLens,
    _token: vscode.CancellationToken
  ): vscode.CodeLens {
    const symbolInfo = (codeLens as any).data as SymbolInfo;

    if (symbolInfo.dependentCount > 0) {
      const refText = `${symbolInfo.dependentCount} reference${
        symbolInfo.dependentCount === 1 ? "" : "s"
      }`;

      codeLens.command = {
        title: refText,
        command: "vscode.executeReferenceProvider",
        arguments: [codeLens.range.start],
      };
    } else {
      codeLens.command = {
        title: "No references",
        command: "",
      };
    }

    return codeLens;
  }

  /**
   * Check if symbol is exported/public
   *
   * # Inputs
   * - symbol: Symbol returned from the daemon
   *
   * # Outputs
   * - true if symbol is exported/public and should be displayed
   *
   * # Filter Conditions
   * - scope contains "export" or "public"
   * - kind is function, class, interface, type, etc. (variables excluded)
   */
  private isExportedSymbol(symbol: DaemonSymbol): boolean {
    // Kinds to display
    const displayKinds = [
      "function",
      "class",
      "interface",
      "type",
      "enum",
      "struct",
      "trait",
      "module",
    ];

    if (!displayKinds.includes(symbol.kind.toLowerCase())) {
      return false;
    }

    // Display only if scope contains export/public
    if (!symbol.scope) {
      return false;
    }

    const scopeLower = symbol.scope.toLowerCase();
    return (
      scopeLower.includes("export") ||
      scopeLower.includes("public") ||
      scopeLower.includes("default")
    );
  }

  /**
   * Check if language is supported by comP
   *
   * # Inputs
   * - languageId: VSCode language ID
   *
   * # Outputs
   * - true if supported, false otherwise
   *
   * # Supported Languages
   * - TypeScript, JavaScript, Python, Go, Rust, Java, C#, C++, Ruby, PHP, SQL, JSON, YAML
   */
  private isSupportedLanguage(languageId: string): boolean {
    const supported = [
      "typescript",
      "javascript",
      "python",
      "go",
      "rust",
      "java",
      "csharp",
      "cpp",
      "ruby",
      "php",
      "sql",
      "json",
      "yaml",
    ];
    return supported.includes(languageId.toLowerCase());
  }

  /**
   * Clear cache for file (called when file is re-indexed)
   */
  invalidateFile(filePath: string): void {
    this.cache.delete(filePath);
  }

  /**
   * Clear all cache
   */
  clearCache(): void {
    this.cache.clear();
  }

  /**
   * Signal that code lenses should be refreshed
   */
  refresh(): void {
    this.onDidChangeCodeLensesEmitter.fire();
  }

  /**
   * Dispose provider resources
   */
  dispose(): void {
    this.onDidChangeCodeLensesEmitter.dispose();
    this.cache.clear();
  }
}
