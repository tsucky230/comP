// Test setup - mock vscode module for unit tests
import Module from 'module';

const originalRequire = Module.prototype.require;

// WHY: Without caching, require('vscode') creates a new object each time, leading to different
//      references between test files and source files. We share the same instance across all modules
//      so that property overrides on the mock propagate to the source files.
let vscodeMock: any = null;

Module.prototype.require = function (id: string) {
  if (id === 'vscode') {
    if (vscodeMock) return vscodeMock;
    // VSCode EventEmitter: notifies listeners via fire(), and subscribes via the event property
    class MockVSCodeEventEmitter<T = void> {
      private listeners: Array<(e: T) => any> = [];

      // .event returns a "listener registration function" (VSCode API specification)
      get event() {
        return (listener: (e: T) => any): { dispose: () => void } => {
          this.listeners.push(listener);
          return { dispose: () => this.fire = this.fire };
        };
      }

      fire(data?: T): void {
        for (const listener of this.listeners) {
          listener(data as T);
        }
      }

      dispose(): void {
        this.listeners = [];
      }
    }

    // VSCode Position: Value object representing line and character positions
    class MockPosition {
      line: number;
      character: number;
      constructor(line: number, character: number) {
        this.line = line;
        this.character = character;
      }
    }

    // VSCode Range: A pair of start/end Positions
    class MockRange {
      start: MockPosition;
      end: MockPosition;
      constructor(start: MockPosition, end: MockPosition) {
        this.start = start;
        this.end = end;
      }
    }

    // VSCode CodeLens: Has a Range and an optional Command
    class MockCodeLens {
      range: MockRange;
      command: any;
      isResolved: boolean;
      constructor(range: MockRange, command?: any) {
        this.range = range;
        this.command = command;
        this.isResolved = false;
      }
    }

    vscodeMock = {
      ExtensionContext: class {},
      EventEmitter: MockVSCodeEventEmitter,
      Position: MockPosition,
      Range: MockRange,
      CodeLens: MockCodeLens,
      Uri: {
        file: (p: string) => ({ fsPath: p, path: p }),
        parse: (val: string) => ({ fsPath: val, path: val }),
      },
      workspace: {
        workspaceFolders: undefined,
        openTextDocument: () => Promise.resolve({ uri: { fsPath: '/mock/doc' } }),
      },
      ProgressLocation: { SourceControl: 1, Window: 10, Notification: 15 },
      StatusBarAlignment: { Left: 1, Right: 2 },
      ThemeColor: class {
        id: string;
        constructor(id: string) { this.id = id; }
      },
      commands: {
        registerCommand: () => ({ dispose: () => {} }),
        executeCommand: () => Promise.resolve(),
      },
      window: {
        createWebviewPanel: () => {},
        createStatusBarItem: () => ({
          text: "",
          tooltip: "",
          command: "",
          backgroundColor: undefined,
          show: () => {},
          dispose: () => {},
        }),
        createOutputChannel: () => ({
          name: "",
          append: () => {},
          appendLine: () => {},
          clear: () => {},
          show: () => {},
          hide: () => {},
          replace: () => {},
          dispose: () => {},
        }),
        showInformationMessage: () => Promise.resolve(undefined),
        showErrorMessage: () => Promise.resolve(undefined),
        showWarningMessage: () => Promise.resolve(undefined),
        showQuickPick: () => Promise.resolve(undefined),
        showInputBox: () => Promise.resolve(undefined),
        showTextDocument: () => Promise.resolve(undefined),
        activeTextEditor: undefined,
      },
      env: {
        clipboard: {
          writeText: () => Promise.resolve(),
        },
      },
    };
    return vscodeMock;
  }
  return originalRequire.apply(this, [id] as any);
};
