// VS Code 側の MAGATAMA 連携: uv の検出、先にキャッシュ、通知、起動時の版の確認、管理を任せるコマンド。
// 判断は config/magatama.ts と mcp/agentSetup/magatama.ts にあり、ここは実行と通知だけ（tasks/magatama_sync_plan.md）。
// comP は更新コマンド（uv tool upgrade など）を自動では実行しない。

import * as vscode from "vscode";
import * as os from "os";
import { execFile } from "child_process";
import { t } from "../i18n";
import { AgentSetupManager, MagatamaOutcome } from "../mcp/AgentSetup";
import {
  MAGATAMA_MIN,
  MAGATAMA_PIN,
  assessInstalledMagatama,
  parseMagatamaVersionOutput,
  probeCoversEntry,
  versionProbeFor,
} from "../config/magatama";

export const MAGATAMA_MANAGE_SETTING = "magatama.manage";
export const VERSION_CHECK_DISMISSED_KEY = "comp.magatama.versionCheckDismissed";
const UV_PROBE_TIMEOUT_MS = 5000;
const VERSION_PROBE_TIMEOUT_MS = 5000;
const PREWARM_TIMEOUT_MS = 120000;
const UPGRADE_COMMAND = "uv tool upgrade magatama";

/** `comp.magatama.manage`（machine スコープ）。 */
export function magatamaManageEnabled(): boolean {
  return vscode.workspace.getConfiguration("comp").get<boolean>(MAGATAMA_MANAGE_SETTING, false);
}

function run(
  file: string,
  args: string[],
  timeout: number
): Promise<{ ok: boolean; stdout: string; error?: string }> {
  return new Promise((resolve) => {
    try {
      // shell は使わない（引数をそのまま渡す）
      // cwd と NoDefaultCurrentDirectoryInExePath: Windows が作業フォルダの同名の実行ファイルを先に起動しないため
      const options = {
        timeout,
        windowsHide: true,
        cwd: os.homedir(),
        env: { ...process.env, NoDefaultCurrentDirectoryInExePath: "1" },
      };
      execFile(file, args, options, (error, stdout) => {
        resolve(
          error
            ? { ok: false, stdout: String(stdout ?? ""), error: error.message }
            : { ok: true, stdout: String(stdout ?? "") }
        );
      });
    } catch (error) {
      resolve({ ok: false, stdout: "", error: error instanceof Error ? error.message : String(error) });
    }
  });
}

let uvDetection: Promise<boolean> | undefined;
let uvKnown: boolean | undefined;

/** `uvx --version` が通るか。起動ごとに1回だけ調べて結果を使い回す。 */
export function detectUv(): Promise<boolean> {
  if (!uvDetection) {
    uvDetection = run("uvx", ["--version"], UV_PROBE_TIMEOUT_MS).then((r) => {
      uvKnown = r.ok;
      return r.ok;
    });
  }
  return uvDetection;
}

/** AgentSetupManager に渡す MAGATAMA の設定。uv は検出済みの値（未検出なら false）。 */
export function magatamaOptions(): { manage: boolean; uvAvailable: boolean } {
  return { manage: magatamaManageEnabled(), uvAvailable: uvKnown ?? false };
}

/** uvx で magatama@<pin> を先にダウンロードしておく（エージェントの初回起動のタイムアウトを防ぐ）。裏で実行する。 */
export function prewarmMagatama(pin: string = MAGATAMA_PIN): Promise<void> {
  // WHY --refresh-package: uv は PyPI の索引をキャッシュするので、公開直後の版を「存在しない」と判定して
  // 起動に失敗する（0.7.3 の公開直後に実測）。ここで取り直しておけば、エージェントの起動はキャッシュで動く。
  const args = ["--refresh-package", "magatama", "--refresh-package", "magatama-core", `magatama@${pin}`, "--version"];
  return run("uvx", args, PREWARM_TIMEOUT_MS).then((r) => {
    if (r.ok) return;
    console.warn(`[comP] MAGATAMA prewarm failed: ${r.error}`);
    void vscode.window.showWarningMessage(
      t(
        `comP: could not download MAGATAMA ${pin} ahead of time (${r.error}). The agent's first start may be slow or time out; run \`uvx --refresh-package magatama magatama@${pin} --version\` once to fix it.`,
        `comP: MAGATAMA ${pin} を先にダウンロードできませんでした（${r.error}）。エージェントの初回起動が遅い・タイムアウトすることがあります。\`uvx --refresh-package magatama magatama@${pin} --version\` を一度実行してください。`
      )
    );
  });
}

/** added / updated があれば先にキャッシュする（裏で実行し、待たない）。 */
export function prewarmIfChanged(outcomes: MagatamaOutcome[] | undefined): void {
  if (outcomes?.some((o) => o.outcome === "added" || o.outcome === "updated")) {
    void prewarmMagatama();
  }
}

export function hasUvMissing(outcomes: MagatamaOutcome[] | undefined): boolean {
  return outcomes?.some((o) => o.reason === "uv-missing") ?? false;
}

export function informUvMissing(): void {
  void vscode.window.showInformationMessage(
    t(
      "Install uv and comP can keep MAGATAMA up to date (https://docs.astral.sh/uv/).",
      "uv を入れると comP が MAGATAMA を最新に保てます（https://docs.astral.sh/uv/）。"
    )
  );
}

function describeEntry(entry: unknown): string {
  if (entry === null || typeof entry !== "object") return "(unknown)";
  const { command, args } = entry as { command?: unknown; args?: unknown };
  const parts = [typeof command === "string" ? command : "?", ...(Array.isArray(args) ? args.map(String) : [])];
  return parts.join(" ");
}

/** 「comP: Let comP manage MAGATAMA」。 */
export async function manageMagatamaCommand(makeSetup: () => AgentSetupManager): Promise<void> {
  await vscode.workspace
    .getConfiguration("comp")
    .update(MAGATAMA_MANAGE_SETTING, true, vscode.ConfigurationTarget.Global);

  if (!(await detectUv())) {
    informUvMissing();
    return;
  }

  const setup = makeSetup();
  const found = await setup.findUserManagedMagatama();
  let replaced = 0;
  if (found.length > 0) {
    const replace = t("Replace", "置き換える");
    const list = found.map((f) => `- ${f.path}\n    ${describeEntry(f.entry)}`).join("\n");
    const answer = await vscode.window.showWarningMessage(
      t(
        `${found.length} MAGATAMA registration(s) you wrote yourself were found. Replace them with one comP manages (uvx magatama@${MAGATAMA_PIN} serve)? A .bak backup is taken.\n${list}`,
        `ご自身で登録した MAGATAMA が ${found.length} 件見つかりました。comP が管理する登録（uvx magatama@${MAGATAMA_PIN} serve）に置き換えますか？ .bak にバックアップを取ります。\n${list}`
      ),
      { modal: true },
      replace
    );
    if (answer === replace) {
      const outcomes = await setup.manageMagatamaAt(found.map((f) => f.path));
      replaced = outcomes.filter((o) => o.outcome === "updated").length;
      prewarmIfChanged(outcomes);
    }
  }
  if (replaced === 0) {
    void prewarmMagatama();
  }

  void vscode.window.showInformationMessage(
    t(
      `Switched ${replaced} registration(s) to comP management. Agents without a MAGATAMA registration get one when you run comP: Setup Agents.`,
      `${replaced} 件を comP の管理に切り替えました。登録がないエージェントには comP: Setup Agents で追加されます。`
    )
  );
}

/**
 * 起動時の版の確認。ユーザーが自分で登録した MAGATAMA が古ければ通知する。
 * 判定できないとき（コマンドなし・出力を解釈できない・タイムアウト）は通知しない。
 */
export async function checkUserMagatamaVersions(
  context: vscode.ExtensionContext,
  makeSetup: () => AgentSetupManager
): Promise<void> {
  if (context.globalState.get<boolean>(VERSION_CHECK_DISMISSED_KEY, false)) return;

  const found = await makeSetup().findUserManagedMagatama();
  const verdicts = new Map<string, { verdict: "ok" | "outdated" | "unknown"; version: string | null }>();
  const outdated: { path: string; version: string }[] = [];
  for (const item of found) {
    const probe = versionProbeFor(item.entry, item.scope);
    if (!probe) continue;
    const key = JSON.stringify(probe);
    let result = verdicts.get(key);
    if (!result) {
      const ran = await run(probe.command, probe.args, VERSION_PROBE_TIMEOUT_MS);
      result = {
        verdict: ran.ok ? assessInstalledMagatama(ran.stdout, MAGATAMA_MIN) : "unknown",
        version: ran.ok ? parseMagatamaVersionOutput(ran.stdout) : null,
      };
      verdicts.set(key, result);
      if (result.verdict === "unknown") {
        console.warn(`[comP] could not determine the MAGATAMA version for ${item.path}: ${ran.error ?? "unrecognised output"}`);
      }
    }
    // PATH の magatama の結果は、実際に PATH の magatama を起動する登録にだけ当てはまる
    if (result.verdict === "outdated" && probeCoversEntry(item.entry, item.scope)) {
      outdated.push({ path: item.path, version: result.version ?? "?" });
    }
  }
  if (outdated.length === 0) return;

  const manage = t("Let comP manage it", "comP に管理を任せる");
  const copy = t("Copy update command", "更新コマンドをコピー");
  const dismiss = t("Don't show again", "今後表示しない");
  const answer = await vscode.window.showWarningMessage(
    t(
      `Your MAGATAMA is ${outdated[0].version}, older than the ${MAGATAMA_MIN} that comP was verified with (${outdated.length} config file(s)). Updating is recommended.`,
      `登録されている MAGATAMA は ${outdated[0].version} で、comP が検証した ${MAGATAMA_MIN} より古いです（設定ファイル ${outdated.length} 件）。更新をおすすめします。`
    ),
    manage,
    copy,
    dismiss
  );

  if (answer === manage) {
    if (!(await detectUv())) {
      informUvMissing();
      return;
    }
    const outcomes = await makeSetup().manageMagatamaAt(outdated.map((o) => o.path));
    prewarmIfChanged(outcomes);
    const replaced = outcomes.filter((o) => o.outcome === "updated").length;
    void vscode.window.showInformationMessage(
      t(`Switched ${replaced} registration(s) to comP management.`, `${replaced} 件を comP の管理に切り替えました。`)
    );
  } else if (answer === copy) {
    await vscode.env.clipboard.writeText(UPGRADE_COMMAND);
  } else if (answer === dismiss) {
    await context.globalState.update(VERSION_CHECK_DISMISSED_KEY, true);
  }
}
