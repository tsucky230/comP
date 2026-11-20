/**
 * comP が MAGATAMA の起動登録を管理するための判断（純粋な関数だけ。ファイルとプロセスには触れない）。
 *
 * WHY: comP は Marketplace で自動更新されるが、MAGATAMA は PyPI のパッケージで手動更新になる。comP が
 * 検証済みの MAGATAMA の版を持ち、エージェントの設定に `uvx magatama@<版> serve` を書けば、comP の更新だけで
 * MAGATAMA も追随する。書き換えてよいのは comP が書いた登録（env.COMP_MANAGED_BY = "comP"）だけで、版は下げない。
 * 計画: tasks/magatama_sync_plan.md
 */

/** comP がこの版で検証した MAGATAMA。comP のリリースごとに見直す。 */
export const MAGATAMA_PIN = "0.7.3";
/** これより古いユーザー管理の MAGATAMA には通知する。 */
export const MAGATAMA_MIN = "0.7.2";
/** comP が書いた登録の目印（env のキーと値）。 */
export const MANAGED_MARKER_KEY = "COMP_MANAGED_BY";
export const MANAGED_MARKER_VALUE = "comP";
const PACKAGE_PREFIX = "magatama@";

/** "0.7.2" 形式（数字3つ、前後の空白なし）だけを受け付ける。それ以外は null。 */
export function parseVersion(v: string): [number, number, number] | null {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(v);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

/** a<b → -1, a=b → 0, a>b → 1。どちらかが解釈できないなら null。 */
export function compareVersions(a: string, b: string): -1 | 0 | 1 | null {
  const x = parseVersion(a);
  const y = parseVersion(b);
  if (!x || !y) return null;
  for (let i = 0; i < 3; i++) {
    if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
  }
  return 0;
}

/** comP が書く管理登録。 */
export function managedEntry(pin: string): { command: "uvx"; args: string[]; env: Record<string, string> } {
  return {
    command: "uvx",
    args: [`magatama@${pin}`, "serve"],
    env: { [MANAGED_MARKER_KEY]: MANAGED_MARKER_VALUE },
  };
}

/**
 * 既存の登録が comP 管理なら、その版を返す。
 * 管理の条件（すべて）: entry がオブジェクト / env がオブジェクトで env.COMP_MANAGED_BY === "comP" /
 * command が文字列で、基本名（\ と / の後ろ、末尾の .exe を除く、大小無視）が "uvx" /
 * args が文字列の配列で、"serve" を含み、"magatama@" で始まる要素がちょうど1つ。
 * 管理なら { managed: true, version: "@" の後ろ }（版が解釈できなくてもそのまま返す）。それ以外は { managed: false }。
 */
export function parseManagedEntry(entry: unknown): { managed: true; version: string } | { managed: false } {
  if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return { managed: false };
  const { command, args, env } = entry as Record<string, unknown>;
  if (env === null || typeof env !== "object" || Array.isArray(env)) return { managed: false };
  if ((env as Record<string, unknown>)[MANAGED_MARKER_KEY] !== MANAGED_MARKER_VALUE) return { managed: false };
  if (typeof command !== "string") return { managed: false };
  const base = (command.split(/[\\/]/).pop() ?? "").toLowerCase().replace(/\.exe$/, "");
  if (base !== "uvx") return { managed: false };
  if (!Array.isArray(args) || !args.every((a) => typeof a === "string")) return { managed: false };
  if (!args.includes("serve")) return { managed: false };
  const packages = args.filter((a: string) => a.startsWith(PACKAGE_PREFIX));
  if (packages.length !== 1) return { managed: false };
  return { managed: true, version: packages[0].slice(PACKAGE_PREFIX.length) };
}

export type MagatamaKeepReason =
  | "user-managed"
  | "up-to-date"
  | "newer"
  | "unparseable-version"
  | "not-managed-yet"
  | "uv-missing";

export type MagatamaPlan =
  | { action: "add"; entry: Record<string, unknown> }
  | { action: "update"; entry: Record<string, unknown> }
  | { action: "keep"; reason: MagatamaKeepReason };

/**
 * 設定に既にある "magatama" の値（existing、なければ undefined）に対して何をするか。例外は投げない。
 * - existing が undefined: manage=false → keep(not-managed-yet) / manage=true かつ uv なし → keep(uv-missing) /
 *   manage=true かつ uv あり → add(managedEntry(pin))
 * - existing がユーザー管理（parseManagedEntry が managed:false。null・配列・文字列も含む）→ keep(user-managed)
 * - existing が管理登録（manage の値・uv の有無によらない）: 版が解釈できない → keep(unparseable-version) /
 *   版 < pin → update / 版 = pin → keep(up-to-date) / 版 > pin → keep(newer)
 * update の entry は existing の浅いコピーで、args の "magatama@..." の要素だけを "magatama@<pin>" にした新しい配列を持つ。
 * existing そのものは変更しない。
 */
export function planMagatamaEntry(
  existing: unknown,
  pin: string,
  opts: { manage: boolean; uvAvailable: boolean }
): MagatamaPlan {
  if (existing === undefined) {
    if (!opts.manage) return { action: "keep", reason: "not-managed-yet" };
    if (!opts.uvAvailable) return { action: "keep", reason: "uv-missing" };
    return { action: "add", entry: managedEntry(pin) };
  }
  const parsed = parseManagedEntry(existing);
  if (!parsed.managed) return { action: "keep", reason: "user-managed" };
  const order = compareVersions(parsed.version, pin);
  if (order === null) return { action: "keep", reason: "unparseable-version" };
  if (order === 0) return { action: "keep", reason: "up-to-date" };
  if (order > 0) return { action: "keep", reason: "newer" };
  const source = existing as Record<string, unknown>;
  const args = (source.args as string[]).map((a) => (a.startsWith(PACKAGE_PREFIX) ? `${PACKAGE_PREFIX}${pin}` : a));
  return { action: "update", entry: { ...source, args } };
}

/** `magatama --version` の出力（例 "MAGATAMA, version 0.7.2"）から版を取り出す。取り出せなければ null。 */
export function parseMagatamaVersionOutput(output: string | null | undefined): string | null {
  if (!output) return null;
  const m = /\bversion\s+(\d+\.\d+\.\d+)(?![\w.])/i.exec(output);
  return m ? m[1] : null;
}

/** ユーザー管理の MAGATAMA の判定。出力から版が取れない → "unknown"。版 < min → "outdated"。それ以外 → "ok"。 */
export function assessInstalledMagatama(
  versionOutput: string | null | undefined,
  min: string
): "ok" | "outdated" | "unknown" {
  const version = parseMagatamaVersionOutput(versionOutput);
  if (version === null) return "unknown";
  const order = compareVersions(version, min);
  if (order === null) return "unknown";
  return order < 0 ? "outdated" : "ok";
}

/**
 * ユーザー管理の登録から、版を確かめるコマンドを作る。
 * scope が "workspace" なら登録のコマンドは使わず { command: "magatama", args: ["--version"] }
 * （clone したリポジトリが置いたコマンドを、comP が起動時に実行しないため）。
 * "global" なら command と args をそのまま使い、args の最後の "serve" を "--version" に替える。
 * "serve" がない / command が空でない文字列でない / args が文字列の配列でない → null。
 */
export function versionProbeFor(
  entry: unknown,
  scope: "workspace" | "global"
): { command: string; args: string[] } | null {
  if (scope === "workspace") return { command: "magatama", args: ["--version"] };
  if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return null;
  const { command, args } = entry as Record<string, unknown>;
  if (typeof command !== "string" || command === "") return null;
  if (!Array.isArray(args) || !args.every((a) => typeof a === "string")) return null;
  const last = args.lastIndexOf("serve");
  if (last === -1) return null;
  const probe = [...(args as string[])];
  probe[last] = "--version";
  return { command, args: probe };
}

/**
 * 版の確認の結果が、この登録に当てはまるか（通知の「comP に管理を任せる」で置き換える対象を絞る）。
 * global: 登録自身のコマンドで確かめるので true（entry がオブジェクトでなければ false）。
 * workspace: 確認は PATH の `magatama --version` なので、command の基本名（\ と / の後ろ、末尾の .exe を除く、
 * 大小無視）が "magatama" のときだけ true。
 */
export function probeCoversEntry(entry: unknown, scope: "workspace" | "global"): boolean {
  if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return false;
  if (scope === "global") return true;
  const command = (entry as Record<string, unknown>).command;
  if (typeof command !== "string") return false;
  return (command.split(/[\\/]/).pop() ?? "").toLowerCase().replace(/\.exe$/, "") === "magatama";
}
