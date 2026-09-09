// DaemonManager - Manages Rust daemon lifecycle and IPC communication
//
// Responsibilities:
// - Start daemon process
// - Stop daemon gracefully
// - Send JSON-RPC requests to daemon via stdio
// - Handle reconnection on failures
// - Log daemon output for debugging

import * as vscode from "vscode";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";
import { readFileSync } from "fs";

interface JSONRPCRequest {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown;
}

interface JSONRPCResponse {
  jsonrpc: "2.0";
  id: number | string;
  result?: unknown;
  error?: { code: number; message: string };
}

export class DaemonManager {
  private context: vscode.ExtensionContext;
  private process: ChildProcess | null = null;
  private requestId = 0;
  private pendingRequests = new Map<number, (response: JSONRPCResponse) => void>();
  private isReady = false;
  // Process is spawned (stdin/stdout available) but handshake not yet complete.
  // Flag to allow getStats requests during waitForReady.
  private isProcessSpawned = false;
  private responseBuffer = "";
  // WHY: Prevent the buffer from growing indefinitely if the daemon sends a massive burst of malformed data without newlines
  private readonly MAX_BUFFER_SIZE = 10 * 1024 * 1024; // 10 MB

  constructor(context: vscode.ExtensionContext) {
    this.context = context;
  }

  /**
   * Start the Rust daemon
   *
   * Process:
   * 1. Find daemon binary
   * 2. Spawn process with environment variables
   * 3. Attach stdout/stderr handlers
   * 4. Wait for ready signal
   */
  async start(): Promise<void> {
    const daemonPath = this.getDaemonPath();
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || ".";

    console.log("[comP] Starting daemon from:", daemonPath);

    try {
      this.process = spawn(daemonPath, [], {
        env: {
          ...process.env,
          COMP_WORKSPACE_ROOT: workspaceRoot,
          RUST_LOG: "info",
        },
        cwd: workspaceRoot,
      });

      // Handle daemon output - parse JSON-RPC responses
      this.process.stdout?.on("data", (data) => {
        const chunk = data.toString('utf8');
        console.debug(`[comP] Received ${chunk.length} bytes from daemon`);
        this.responseBuffer += chunk;

        if (this.responseBuffer.length > this.MAX_BUFFER_SIZE) {
          console.error('[comP] Response buffer exceeded 10MB limit — clearing (daemon may be misbehaving)');
          this.responseBuffer = '';
          return;
        }

        // Process complete lines (JSON-RPC responses are newline-separated)
        const lines = this.responseBuffer.split("\n");

        // Process all complete lines except the last one (which may be incomplete)
        for (let i = 0; i < lines.length - 1; i++) {
          const line = lines[i].trim();
          if (line.length === 0) {
            continue;
          }

          try {
            const response: JSONRPCResponse = JSON.parse(line);
            console.log(`[comP] Parsed JSON-RPC response (id: ${response.id})`);
            const handler = this.pendingRequests.get(response.id as number);

            if (handler) {
              console.log(`[comP] Found handler for request id: ${response.id}`);
              this.pendingRequests.delete(response.id as number);
              handler(response);
            } else {
              console.warn(`[comP] No handler for response id: ${response.id}`);
            }
          } catch (error) {
            // Not a JSON response - might be debug output from daemon
            console.debug(`[comP daemon output] ${line}`);
          }
        }

        // Keep the incomplete line in buffer for next data chunk
        this.responseBuffer = lines[lines.length - 1];
      });

      this.process.stderr?.on("data", (data) => {
        console.error("[comP daemon err]", data.toString('utf8').trim());
      });

      this.process.on("error", (err) => {
        console.error("[comP] Failed to spawn daemon:", err.message);
        // reject all pending requests immediately
        for (const [id, handler] of this.pendingRequests) {
          handler({ jsonrpc: "2.0", id, error: { code: -32000, message: `Daemon spawn failed: ${err.message}` } });
        }
        this.pendingRequests.clear();
        this.isReady = false;
        this.isProcessSpawned = false;
      });

      this.process.on("exit", (code) => {
        console.log("[comP] Daemon exited with code:", code);
        // reject pending requests so they don't hang until timeout
        for (const [id, handler] of this.pendingRequests) {
          handler({ jsonrpc: "2.0", id, error: { code: -32000, message: `Daemon exited with code ${code}` } });
        }
        this.pendingRequests.clear();
        this.isReady = false;
        this.isProcessSpawned = false;
      });

      // WHY: stdin might not be ready immediately after spawn.
      // We keep isReady=false until connectivity is confirmed via waitForReady to prevent permanent pending states.
      // Since waitForReady queries getStats, we use the internal isProcessSpawned flag to temporarily bypass the check.
      this.isProcessSpawned = true;
      console.log("[comP] Daemon process spawned, awaiting readiness...");

      // Wait for daemon to be ready by checking connectivity
      await this.waitForReady();
      this.isReady = true;
      console.log("[comP] Daemon ready");
    } catch (error) {
      console.error("[comP] Failed to start daemon:", error);
      throw new Error(`Failed to start comP daemon: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /**
   * Wait for daemon to be ready
   *
   * Pings the daemon with a getStats request until it responds
   */
  private async waitForReady(): Promise<void> {
    const maxRetries = 10;
    const retryDelayMs = 500;

    for (let i = 0; i < maxRetries; i++) {
      try {
        await this.getStats();
        console.log("[comP] Daemon is ready");
        return;
      } catch (error) {
        if (i < maxRetries - 1) {
          await new Promise((resolve) => setTimeout(resolve, retryDelayMs));
        }
      }
    }

    throw new Error("Daemon did not respond after 10 retries — binary may have crashed or workspace is too large");
  }

  /**
   * Stop the daemon gracefully
   */
  async stop(): Promise<void> {
    if (!this.process) {
      return;
    }

    console.log("[comP] Stopping daemon...");
    const proc = this.process;

    // WHY: Clear references beforehand to prevent duplicate kill calls from concurrent stop() or deactivate() invocations.
    this.process = null;
    this.isReady = false;
    this.isProcessSpawned = false;

    await new Promise<void>((resolve) => {
      const timeout = setTimeout(() => {
        console.log("[comP] Daemon did not exit in time, sending SIGKILL");
        proc.kill("SIGKILL");
        resolve();
      }, 3000);

      // WHY: Register the exit handler before sending the kill signal.
      // If registered after, we might miss the exit event if it dies immediately, causing a 3-second timeout wait.
      proc.once("exit", () => {
        clearTimeout(timeout);
        resolve();
      });

      // WHY: Windows native binaries often ignore SIGTERM, so send SIGKILL directly on Windows to ensure termination.
      const signal = process.platform === "win32" ? "SIGKILL" : "SIGTERM";
      proc.kill(signal);
    });
  }

  /**
   * Send a request to daemon and wait for response
   *
   * # Protocol:
   * Request: { "jsonrpc": "2.0", "id": 1, "method": "...", "params": {...} }
   * Response: { "jsonrpc": "2.0", "id": 1, "result": {...} }
   */
  async request(method: string, params?: unknown): Promise<unknown> {
    // Allow sending getStats during waitForReady where isReady is still false, to establish connectivity
    const allowedDuringHandshake = method === "getStats" && this.isProcessSpawned;
    if (!this.process || (!this.isReady && !allowedDuringHandshake)) {
      throw new Error("Daemon is not running");
    }

    const id = ++this.requestId;
    const request: JSONRPCRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    console.log(`[comP] Sending request: ${method} (id: ${id})`);

    return new Promise((resolve, reject) => {
      const METHOD_TIMEOUTS: Record<string, number> = {
        getStats: 5000,
        forceReindex: 120_000,
      };
      const timeoutMs = METHOD_TIMEOUTS[method] ?? 3000;
      const timeout = setTimeout(() => {
        this.pendingRequests.delete(id);
        console.error(`[comP] Request timeout: ${method} (id: ${id}) after ${timeoutMs}ms`);
        reject(new Error(`Request timeout for method: ${method}`));
      }, timeoutMs);

      // Wait for response
      this.pendingRequests.set(id, (response) => {
        clearTimeout(timeout);
        console.log(`[comP] Received response: ${method} (id: ${id})`);
        if (response.error) {
          reject(new Error(response.error.message));
        } else {
          resolve(response.result);
        }
      });

      // Send request to daemon
      console.debug(`[comP] Writing request to daemon: ${JSON.stringify(request)}`);
      this.process!.stdin?.write(JSON.stringify(request) + "\n");
    });
  }

  /**
   * Index a single file (incremental update)
   */
  async indexFile(filePath: string): Promise<void> {
    await this.request("indexFile", { path: filePath });
  }

  /**
   * Remove file from index
   */
  async removeFile(filePath: string): Promise<void> {
    await this.request("removeFile", { path: filePath });
  }

  /**
   * Mark every recorded session-memory call that touched `filePath` as stale.
   *
   * WHY this is an RPC rather than local file I/O: it used to be
   * SessionMemoryManager.markStaleForFile, which read and wrote
   * `.comp/session-memory.json` directly — the single file the daemon's
   * per-agent split (v0.11.1) stopped reading entirely, so every call was a
   * silent no-op (Phase 3, A7). The daemon owns the real per-agent files and
   * the locking/atomic-write discipline around them; routing through it here
   * avoids a second, divergent implementation touching the same files.
   */
  async markStale(filePath: string): Promise<void> {
    await this.request("mark_stale", { path: filePath });
  }

  /**
   * Check if daemon is running
   */
  isRunning(): boolean {
    return this.isReady && this.process !== null;
  }

  /**
   * Get current index statistics
   *
   * Returns current state of the index
   */
  async getStats(): Promise<{
    total_files: number;
    total_nodes: number;
    total_edges: number;
    efficiency?: string;
    tokens_sent?: number;
    tokens_saved?: number;
    queries_count?: number;
    avg_tokens_per_query?: number;
    last_activity_at?: number;
  }> {
    const result = await this.request("getStats", {});
    if (!result || typeof result !== "object") {
      throw new Error("Invalid stats response from daemon");
    }
    const stats = result as Record<string, unknown>;
    return {
      total_files: Number(stats["total_files"]) || 0,
      total_nodes: Number(stats["total_nodes"]) || 0,
      total_edges: Number(stats["total_edges"]) || 0,
      efficiency: typeof stats["efficiency"] === "string" ? stats["efficiency"] : undefined,
      tokens_sent: stats["tokens_sent"] !== undefined ? Number(stats["tokens_sent"]) : undefined,
      tokens_saved: stats["tokens_saved"] !== undefined ? Number(stats["tokens_saved"]) : undefined,
      queries_count: stats["queries_count"] !== undefined ? Number(stats["queries_count"]) : undefined,
      avg_tokens_per_query: stats["avg_tokens_per_query"] !== undefined ? Number(stats["avg_tokens_per_query"]) : undefined,
      // WHY: lets SidebarPanel show "last agent connection" without reading any
      // file itself — see daemon/src/mcp/mod.rs::latest_activity_timestamp's doc.
      last_activity_at: stats["last_activity_at"] !== undefined ? Number(stats["last_activity_at"]) : undefined,
    };
  }

  /**
   * Get symbols in a file for CodeLens display
   *
   * Returns array of symbols with dependent counts
   */
  async getSymbols(filePath: string): Promise<any[]> {
    return (await this.request("getSymbols", { path: filePath })) as any[];
  }

  /**
   * Compress a single file using AST compression
   */
  async compressFile(filePath: string, compressionLevel: number): Promise<{ text: string; compressionRate: string }> {
    const result = await this.request("compressFile", {
      path: filePath,
      compression_level: compressionLevel,
    });
    if (!result || typeof result !== "object") {
      throw new Error("Invalid response from daemon for compressFile");
    }
    const res = result as Record<string, unknown>;
    if (typeof res["compressed_text"] !== "string") {
      throw new Error("Missing compressed_text in response");
    }
    return {
      text: res["compressed_text"],
      compressionRate: typeof res["compression_rate"] === "string" ? res["compression_rate"] : "0%",
    };
  }

  /**
   * Get path to daemon binary
   *
   * Checks:
   * 1. Bundled release binary (production)
   * 2. Cargo build output (development)
   */
  private getDaemonPath(): string {
    // Production: bundled binary (different names per platform to support packaging all platforms in one VSIX)
    let bundledBinaryName = "comp-daemon-linux";
    if (process.platform === "win32") {
      bundledBinaryName = "comp-daemon-win.exe";
    } else if (process.platform === "darwin") {
      bundledBinaryName = "comp-daemon-macos";
    }

    const bundledPath = path.join(
      this.context.extensionPath,
      "daemon",
      "target",
      "release",
      bundledBinaryName
    );

    // Development: check workspace daemon build (first release, then debug)
    // In development, the local cargo build outputs default to comp-daemon or comp-daemon.exe
    const workspacePath = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (workspacePath) {
      const devBinaryName = process.platform === "win32" ? "comp-daemon.exe" : "comp-daemon";
      const devPath = path.join(workspacePath, "daemon", "target", "release", devBinaryName);
      try {
        readFileSync(devPath);
        return devPath;
      } catch {
        // Fallback to debug build if release build is not found
        const debugPath = path.join(workspacePath, "daemon", "target", "debug", devBinaryName);
        try {
          readFileSync(debugPath);
          return debugPath;
        } catch {
          // File doesn't exist, fall through
        }
      }
    }

    return bundledPath;
  }
}
