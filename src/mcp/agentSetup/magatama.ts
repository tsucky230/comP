// MAGATAMA の登録を、JSON と Codex TOML の設定ファイルへ書く（判断は config/magatama.ts の純粋な関数）。
// 計画: tasks/magatama_sync_plan.md
import * as fs from "fs";
import {
  AgentSetupManager,
  MagatamaOutcome,
  MagatamaSettings,
  WriteTarget,
} from "../AgentSetup";
import {
  MAGATAMA_PIN,
  MANAGED_MARKER_KEY,
  MANAGED_MARKER_VALUE,
  MagatamaPlan,
  compareVersions,
  managedEntry,
  parseManagedEntry,
  planMagatamaEntry,
} from "../../config/magatama";
import {
  decodeTomlString,
  describeError,
  findServerSection,
  isSelfContained,
  maskComments,
  readObjectPath,
  samePath,
  tomlQuote,
} from "./text";

const SERVER = "magatama";

/** 設定ファイル（の文字列）に MAGATAMA の登録があるか、あれば何か。 */
type Found =
  | { kind: "absent" }
  | { kind: "unreadable" }
  | { kind: "entry"; entry: Record<string, unknown> };

export function resolveSettings(settings: MagatamaSettings | undefined): Required<MagatamaSettings> {
  return {
    manage: settings?.manage ?? false,
    uvAvailable: settings?.uvAvailable ?? false,
    pin: settings?.pin ?? MAGATAMA_PIN,
  };
}

// ---- JSON ----------------------------------------------------------------

function jsonContainer(doc: unknown, serverKeys: string[]): Record<string, unknown> | null {
  return readObjectPath(doc, serverKeys);
}

/** JSON の設定の文字列に、計画を適用した新しい文字列（変更なしなら null）を返す。 */
function applyToJson(
  text: string,
  serverKeys: string[],
  plan: (existing: unknown) => MagatamaPlan
): { text: string | null; plan: MagatamaPlan } | null {
  const doc: unknown = JSON.parse(text);
  const container = jsonContainer(doc, serverKeys);
  if (!container) return null;
  const decided = plan(container[SERVER]);
  if (decided.action === "keep") return { text: null, plan: decided };
  container[SERVER] = decided.entry;
  return { text: JSON.stringify(doc, null, 2) + "\n", plan: decided };
}

function readJsonEntry(text: string, serverKeys: string[]): Found {
  let doc: unknown;
  try {
    doc = JSON.parse(text);
  } catch {
    return { kind: "unreadable" };
  }
  const container = jsonContainer(doc, serverKeys);
  if (!container || !(SERVER in container)) return { kind: "absent" };
  const value = container[SERVER];
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? { kind: "entry", entry: value as Record<string, unknown> }
    : { kind: "unreadable" };
}

// ---- Codex TOML ----------------------------------------------------------

const STRING_TOKEN = /"(?:[^"\\]|\\.)*"|'[^']*'/g;

function tomlStrings(raw: string): string[] | null {
  const tokens = raw.match(STRING_TOKEN) ?? [];
  const rest = raw.replace(STRING_TOKEN, "").replace(/[\s,]/g, "");
  return rest === "" ? tokens.map(decodeTomlString) : null;
}

function tomlArray(raw: string): string[] | null {
  const m = /^\[(.*)\]$/.exec(raw.trim());
  return m ? tomlStrings(m[1]) : null;
}

function tomlInlineEnv(raw: string): Record<string, string> | null {
  const m = /^\{(.*)\}$/.exec(raw.trim());
  if (!m) return null;
  const out: Record<string, string> = {};
  const pair = /(?:^|,)\s*(?:"([^"]*)"|'([^']*)'|([A-Za-z0-9_-]+))\s*=\s*("(?:[^"\\]|\\.)*"|'[^']*')\s*(?=,|$)/y;
  const body = m[1].trim();
  let at = 0;
  while (at < body.length) {
    pair.lastIndex = at;
    const p = pair.exec(body);
    if (!p) return null;
    out[p[1] ?? p[2] ?? p[3]] = decodeTomlString(p[4]);
    at = pair.lastIndex;
  }
  return out;
}

/**
 * `[mcp_servers.magatama]` の節から entry（command・args・env）を読む。1行に書かれた形だけを読み、
 * それ以外（複数行の値、ドット付きのキー）は unreadable としてユーザー管理に倒す。
 */
function readTomlSection(sectionText: string): Found {
  const lines = maskComments(sectionText).split("\n");
  let table: "main" | "env" | "other" = "main";
  const entry: Record<string, unknown> = {};
  let env: Record<string, string> | undefined;
  for (const line of lines) {
    if (line.trim() === "") continue;
    if (/^\s*\[/.test(line)) {
      const header = /^\s*\[\s*mcp_servers\s*\.\s*(?:"magatama"|'magatama'|magatama)\s*(?:\.\s*(.*?))?\s*\]/.exec(line);
      const sub = header?.[1];
      table = header && sub === undefined ? "main" : sub !== undefined && /^("env"|'env'|env)$/.test(sub) ? "env" : "other";
      if (table === "env") env = env ?? {};
      continue;
    }
    if (table === "other") continue;
    const kv = /^\s*(?:"([^"]*)"|'([^']*)'|([A-Za-z0-9_-]+))\s*=\s*(.*?)\s*$/.exec(line);
    if (!kv) return { kind: "unreadable" };
    const key = kv[1] ?? kv[2] ?? kv[3];
    const raw = kv[4];
    if (table === "env") {
      const value = tomlStrings(raw);
      if (value === null || value.length !== 1 || !/^["']/.test(raw)) return { kind: "unreadable" };
      (env as Record<string, string>)[key] = value[0];
    } else if (key === "command") {
      const value = tomlStrings(raw);
      if (value === null || value.length !== 1) return { kind: "unreadable" };
      entry.command = value[0];
    } else if (key === "args") {
      const value = tomlArray(raw);
      if (value === null) return { kind: "unreadable" };
      entry.args = value;
    } else if (key === "env") {
      const value = tomlInlineEnv(raw);
      if (value === null) return { kind: "unreadable" };
      env = { ...(env ?? {}), ...value };
    }
  }
  if (env) entry.env = env;
  return { kind: "entry", entry };
}

/** `[mcp_servers]` の下に `magatama = ...` / `magatama.x = ...` と書かれていないか（足すと重複キーになる）。 */
function hasInlineMagatama(text: string): boolean {
  let table = "";
  for (const raw of maskComments(text).split("\n")) {
    const line = raw.trim();
    const header = /^\[\s*([^\]]*?)\s*\]/.exec(line);
    if (header) {
      table = header[1].replace(/\s+/g, "");
      continue;
    }
    if (table === "mcp_servers" && /^("magatama"|'magatama'|magatama)\s*(\.|=)/.test(line)) return true;
    if (table === "" && /^mcp_servers\s*=/.test(line)) return true;
    if (table === "" && /^mcp_servers\s*\.\s*("magatama"|'magatama'|magatama)\s*(\.|=)/.test(line)) return true;
  }
  return false;
}

function readTomlEntry(text: string): Found {
  if (hasInlineMagatama(text)) return { kind: "unreadable" };
  const section = findServerSection(text, SERVER);
  if (!section) return { kind: "absent" };
  return readTomlSection(text.slice(section.start, section.end));
}

function tomlBlock(entry: { args: string[]; env: Record<string, string> } & Record<string, unknown>): string {
  const q = tomlQuote;
  const lines = [
    "[mcp_servers.magatama]",
    `command = ${q(String(entry.command))}`,
    `args = [${entry.args.map(q).join(", ")}]`,
    "",
    "[mcp_servers.magatama.env]",
    ...Object.entries(entry.env).map(([k, v]) => `${k} = ${q(v)}`),
  ];
  return lines.join("\n") + "\n";
}

/** 節の中の main 表の args の行だけを書き換える（ほかの行は1バイトも変えない）。 */
function rewriteArgsLine(sectionText: string, pin: string): string | null {
  const masked = maskComments(sectionText);
  let table: "main" | "other" = "main";
  let offset = 0;
  for (const line of masked.split("\n")) {
    const lineEnd = offset + line.length + 1;
    if (/^\s*\[/.test(line)) {
      table = /^\s*\[\s*mcp_servers\s*\.\s*(?:"magatama"|'magatama'|magatama)\s*\]/.test(line) ? "main" : "other";
    } else if (table === "main" && /^\s*(?:"args"|'args'|args)\s*=/.test(line)) {
      const original = sectionText.slice(offset, offset + line.length);
      const changed = original.replace(/(["'])magatama@[^"'\s]*\1/, (_m, quote: string) => `${quote}magatama@${pin}${quote}`);
      if (changed === original) return null;
      return sectionText.slice(0, offset) + changed + sectionText.slice(offset + line.length);
    }
    offset = lineEnd;
  }
  return null;
}

const MAGATAMA_MAIN_HEADER = /^\s*\[\s*mcp_servers\s*\.\s*(?:"magatama"|'magatama'|magatama)\s*\]/;
const MAGATAMA_ENV_HEADER =
  /^\s*\[\s*mcp_servers\s*\.\s*(?:"magatama"|'magatama'|magatama)\s*\.\s*(?:"env"|'env'|env)\s*\]/;

/**
 * ユーザーの登録を管理登録に替えた JSON の値。ユーザーのほかのキーと env は残し、command・args を管理の値にして
 * 目印を足す。
 */
function adoptedJsonEntry(existing: unknown, pin: string): Record<string, unknown> {
  const base =
    existing !== null && typeof existing === "object" && !Array.isArray(existing)
      ? (existing as Record<string, unknown>)
      : {};
  const env =
    base.env !== null && typeof base.env === "object" && !Array.isArray(base.env)
      ? (base.env as Record<string, unknown>)
      : {};
  const managed = managedEntry(pin);
  return { ...base, command: managed.command, args: managed.args, env: { ...env, ...managed.env } };
}

/**
 * ユーザーの `[mcp_servers.magatama]` の節を管理登録に替える。主表の command と args の行だけを置き換え、env 副表に
 * 目印を足す（なければ作る）。ほかのキー・副表、節の末尾にある次の節のコメントと空行はそのまま残す。
 * 節が自己完結でない（複数行の値を含む）か、読めない形なら null。
 */
function adoptTomlSection(sectionText: string, pin: string): string | null {
  if (!isSelfContained(sectionText)) return null;
  const q = tomlQuote;
  const managed = managedEntry(pin);
  const markerLine = `COMP_MANAGED_BY = ${q("comP")}`;
  const commandLine = `command = ${q(managed.command)}`;
  const argsLine = `args = [${managed.args.map(q).join(", ")}]`;
  const lines = sectionText.split("\n");
  const masked = maskComments(sectionText).split("\n");
  // 末尾の空行・コメント行は次の節のものとして扱い、範囲から外す
  let last = masked.length - 1;
  while (last >= 0 && masked[last].trim() === "") last--;
  if (last < 0) return null;

  let table: "main" | "env" | "other" = "main";
  let sawCommand = false;
  let sawArgs = false;
  let envHeaderAt = -1;
  let envMarkerSeen = false;
  let mainHeaderAt = -1;
  const out: string[] = [];
  for (let i = 0; i <= last; i++) {
    const line = lines[i];
    const m = masked[i];
    if (/^\s*\[/.test(m)) {
      table = MAGATAMA_MAIN_HEADER.test(m) ? "main" : MAGATAMA_ENV_HEADER.test(m) ? "env" : "other";
      if (table === "main") mainHeaderAt = out.length;
      if (table === "env") envHeaderAt = out.length;
      out.push(line);
      continue;
    }
    const key = /^\s*(?:"([^"]*)"|'([^']*)'|([A-Za-z0-9_-]+))\s*=/.exec(m);
    const name = key ? (key[1] ?? key[2] ?? key[3]) : undefined;
    if (table === "main" && name === "command") {
      out.push(commandLine);
      sawCommand = true;
    } else if (table === "main" && name === "args") {
      out.push(argsLine);
      sawArgs = true;
    } else if (table === "main" && name === "env") {
      // env = { ... } の1行形式: 目印を足す（あれば値を直す）
      out.push(
        /COMP_MANAGED_BY/.test(m)
          ? line.replace(/(COMP_MANAGED_BY\s*=\s*)("(?:[^"\\]|\\.)*"|'[^']*')/, (_x, k: string) => `${k}${q("comP")}`)
          : line.replace(/\{\s*/, `{ ${markerLine}, `).replace(/,\s*\}\s*$/, " }")
      );
      envMarkerSeen = true;
    } else if (table === "env" && name === "COMP_MANAGED_BY") {
      out.push(markerLine);
      envMarkerSeen = true;
    } else {
      out.push(line);
    }
  }
  if (mainHeaderAt === -1) return null;
  const missing = [...(sawCommand ? [] : [commandLine]), ...(sawArgs ? [] : [argsLine])];
  out.splice(mainHeaderAt + 1, 0, ...missing);
  if (!envMarkerSeen) {
    if (envHeaderAt !== -1) {
      out.splice(envHeaderAt + missing.length + 1, 0, markerLine);
    } else {
      out.push("", "[mcp_servers.magatama.env]", markerLine);
    }
  }
  return [...out, ...lines.slice(last + 1)].join("\n");
}

function applyToToml(
  text: string,
  decide: (found: Found) => MagatamaPlan | "replace" | "unreadable",
  pin: string
): { text: string | null; plan: MagatamaPlan | "replace" | "unreadable" } {
  const found = readTomlEntry(text);
  const plan = decide(found);
  if (plan === "unreadable" || (plan !== "replace" && plan.action === "keep")) return { text: null, plan };
  const section = findServerSection(text, SERVER);
  if (plan !== "replace" && plan.action === "add") {
    return { text: text.trimEnd() + (text.trim() === "" ? "" : "\n\n") + tomlBlock(managedEntry(pin)), plan };
  }
  if (!section) return { text: null, plan: "unreadable" };
  const sectionText = text.slice(section.start, section.end);
  if (plan === "replace") {
    const adopted = adoptTomlSection(sectionText, pin);
    if (adopted === null) return { text: null, plan: "unreadable" };
    return { text: text.slice(0, section.start) + adopted + text.slice(section.end), plan };
  }
  if (!isSelfContained(sectionText)) return { text: null, plan: "unreadable" };
  const rewritten = rewriteArgsLine(sectionText, pin);
  if (rewritten === null) return { text: null, plan: "unreadable" };
  return { text: text.slice(0, section.start) + rewritten + text.slice(section.end), plan };
}

// ---- generateConfig への組み込み ------------------------------------------

function outcomeOf(
  target: { path: string; scope: "workspace" | "global" },
  plan: MagatamaPlan | "replace" | "unreadable"
): MagatamaOutcome {
  const base = { path: target.path, scope: target.scope };
  if (plan === "unreadable") return { ...base, outcome: "kept", reason: "user-managed" };
  if (plan === "replace") return { ...base, outcome: "updated" };
  if (plan.action === "keep") return { ...base, outcome: "kept", reason: plan.reason };
  return { ...base, outcome: plan.action === "add" ? "added" : "updated" };
}

/**
 * comP を書き終えた設定ファイルの文字列に、MAGATAMA の登録の計画を適用する。
 * 対象外の形式（Continue・Aider）は null。keep なら content をそのまま返す。
 */
export function applyMagatamaToContent(
  content: string,
  target: WriteTarget,
  settings: Required<MagatamaSettings>
): { content: string; outcome: MagatamaOutcome } | null {
  const opts = { manage: settings.manage, uvAvailable: settings.uvAvailable };
  try {
    if (target.format.kind === "json") {
      const result = applyToJson(content, target.format.serverKeys, (e) => planMagatamaEntry(e, settings.pin, opts));
      if (!result) return null;
      return { content: result.text ?? content, outcome: outcomeOf(target, result.plan) };
    }
    if (target.format.kind === "toml") {
      const result = applyToToml(
        content,
        (found) => (found.kind === "unreadable" ? "unreadable" : planMagatamaEntry(found.kind === "entry" ? found.entry : undefined, settings.pin, opts)),
        settings.pin
      );
      return { content: result.text ?? content, outcome: outcomeOf(target, result.plan) };
    }
  } catch (error) {
    return { content, outcome: { path: target.path, scope: target.scope, outcome: "failed", reason: describeError(error) } };
  }
  return null;
}

// ---- 既にあるファイルへの操作（起動時の修復・通知のボタン） -------------------

/** JSON と TOML の対象を、パスで重複なく（全エージェント分）集める。 */
function existingTargets(host: AgentSetupManager): WriteTarget[] {
  const seen: WriteTarget[] = [];
  for (const agent of AgentSetupManager.AGENTS) {
    for (const target of host.targetsForAgent(agent)) {
      if (target.format.kind !== "json" && target.format.kind !== "toml") continue;
      if (seen.some((t) => samePath(t.path, target.path))) continue;
      if (fs.existsSync(target.path)) seen.push(target);
    }
  }
  return seen;
}

function writeBack(host: AgentSetupManager, filePath: string, text: string): void {
  host.backupIfExists(filePath);
  host.atomicWrite(filePath, text);
}

/** 既にあるファイルの magatama を 1 件ずつ見て、decide が書き換えを求めたものだけ書く。登録のないファイルは結果に出さない。 */
function visit(
  host: AgentSetupManager,
  only: string[] | null,
  decide: (found: Found) => MagatamaPlan | "replace" | "unreadable",
  pin: string
): MagatamaOutcome[] {
  const outcomes: MagatamaOutcome[] = [];
  for (const target of existingTargets(host)) {
    if (only && !only.some((p) => samePath(p, target.path))) continue;
    try {
      const text = fs.readFileSync(target.path, "utf-8");
      let newText: string | null;
      let plan: MagatamaPlan | "replace" | "unreadable";
      if (target.format.kind === "json") {
        const found = readJsonEntry(text, target.format.serverKeys);
        if (found.kind === "absent" && only === null) continue;
        const decided = decide(found);
        plan = decided;
        newText = null;
        if (decided !== "unreadable" && (decided === "replace" || decided.action !== "keep")) {
          const doc = JSON.parse(text) as unknown;
          const container = jsonContainer(doc, target.format.serverKeys);
          if (container) {
            container[SERVER] = decided === "replace" ? adoptedJsonEntry(container[SERVER], pin) : decided.entry;
            newText = JSON.stringify(doc, null, 2) + "\n";
          }
        }
      } else {
        if (readTomlEntry(text).kind === "absent" && only === null) continue;
        const result = applyToToml(text, decide, pin);
        newText = result.text;
        plan = result.plan;
      }
      if (newText !== null) writeBack(host, target.path, newText);
      outcomes.push(outcomeOf(target, plan));
    } catch (error) {
      outcomes.push({ path: target.path, scope: target.scope, outcome: "failed", reason: describeError(error) });
    }
  }
  return outcomes;
}

/** 起動時の修復: 管理登録の更新だけ（追加しない）。 */
export function updateManaged(host: AgentSetupManager, settings: Required<MagatamaSettings>): MagatamaOutcome[] {
  return visit(
    host,
    null,
    (found) =>
      found.kind === "entry"
        ? planMagatamaEntry(found.entry, settings.pin, { manage: false, uvAvailable: settings.uvAvailable })
        : { action: "keep", reason: "user-managed" },
    settings.pin
  ).filter((o) => o.outcome !== "kept" || o.reason !== "user-managed");
}

/** ユーザーが自分で書いた登録（comP 管理でないもの）。 */
export function findUserManaged(host: AgentSetupManager): { path: string; scope: "workspace" | "global"; entry: unknown }[] {
  const found: { path: string; scope: "workspace" | "global"; entry: unknown }[] = [];
  for (const target of existingTargets(host)) {
    try {
      const text = fs.readFileSync(target.path, "utf-8");
      const read = target.format.kind === "json" ? readJsonEntry(text, target.format.serverKeys) : readTomlEntry(text);
      if (read.kind === "entry" && !parseManagedEntry(read.entry).managed) {
        found.push({ path: target.path, scope: target.scope, entry: read.entry });
      }
    } catch {
      // 読めないファイルはユーザーの登録として数えない
    }
  }
  return found;
}

/** 通知のボタン: 指定したファイルの登録を管理登録に置き換える。 */
export function manageAt(host: AgentSetupManager, paths: string[], settings: Required<MagatamaSettings>): MagatamaOutcome[] {
  const known = existingTargets(host);
  const unknown = paths
    .filter((p) => !known.some((t) => samePath(t.path, p)))
    .map((p): MagatamaOutcome => ({ path: p, scope: "workspace", outcome: "failed", reason: "not-found" }));
  if (!settings.uvAvailable) {
    const failed = known
      .filter((t) => paths.some((p) => samePath(t.path, p)))
      .map((t): MagatamaOutcome => ({ path: t.path, scope: t.scope, outcome: "failed", reason: "uv-missing" }));
    return [...failed, ...unknown];
  }
  const done = visit(
    host,
    paths,
    (found) => {
      if (found.kind === "unreadable") return "unreadable";
      if (found.kind === "absent") return { action: "keep", reason: "not-managed-yet" };
      if (parseManagedEntry(found.entry).managed) {
        return planMagatamaEntry(found.entry, settings.pin, { manage: true, uvAvailable: true });
      }
      // 目印のない uvx の登録でも、版を下げる置き換え・版が読めない置き換えはしない
      const pinned = parseManagedEntry({ ...found.entry, env: { [MANAGED_MARKER_KEY]: MANAGED_MARKER_VALUE } });
      if (pinned.managed) {
        const order = compareVersions(pinned.version, settings.pin);
        if (order === null) return { action: "keep", reason: "unparseable-version" };
        if (order > 0) return { action: "keep", reason: "newer" };
      }
      return "replace";
    },
    settings.pin
  ).map((o): MagatamaOutcome =>
    o.outcome === "kept" && o.reason === "user-managed"
      ? { ...o, outcome: "failed", reason: "unreadable" }
      : o.outcome === "kept" && o.reason === "not-managed-yet"
        ? { ...o, outcome: "failed", reason: "not-found" }
        : o
  );
  return [...done, ...unknown];
}
