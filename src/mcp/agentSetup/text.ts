import * as path from "path";

/**
 * Same file on this machine. Windows paths compare without case, as its file
 * system does: Claude Code writes `c:\Users\…` where VS Code gives `C:\Users\…`,
 * and treating those as different rewrote healthy configs on every upgrade check.
 */
export function samePath(a: string, b: string): boolean {
  const left = path.resolve(a);
  const right = path.resolve(b);
  return process.platform === "win32" ? left.toLowerCase() === right.toLowerCase() : left === right;
}

/** [major, minor, patch] from a `tsucky230.comp-vscode-X.Y.Z…` folder, else null. */
export function extensionFolderVersion(folder: string): [number, number, number] | null {
  const m = /^tsucky230\.comp-vscode-(\d+)\.(\d+)\.(\d+)(?:[-.][^\\/]*)?$/.exec(path.basename(folder));
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

/**
 * Walk a key chain and return the object at the end, or null if any hop is
 * missing or is not a plain object.
 */
export function readObjectPath(doc: unknown, keys: string[]): Record<string, unknown> | null {
  let node: unknown = doc;
  for (const key of keys) {
    if (node === null || typeof node !== "object" || Array.isArray(node)) {
      return null;
    }
    node = (node as Record<string, unknown>)[key];
  }
  if (node === null || typeof node !== "object" || Array.isArray(node)) {
    return null;
  }
  return node as Record<string, unknown>;
}

/** Message of an unknown throwable, for reporting into a WriteOutcome. */
export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Read the value out of a quoted TOML scalar.
 *
 * Only the two single-line forms are handled, which is all comP writes and all
 * a daemon path can be: a literal string takes its content verbatim, a basic
 * string has its backslash escapes undone.
 */
export function decodeTomlString(raw: string): string {
  if (raw.startsWith("'")) {
    return raw.slice(1, -1);
  }
  // \uXXXX has to be tried before the single-char escapes below: it is the
  // counterpart of the \uXXXX form tomlQuote emits for control characters, and
  // without it a value containing one would decode as the literal text "u001b"
  // instead of the control character, permanently disagreeing with what was
  // written. Since the write and the read of the same value would then never
  // match, the workspace-root repair pass would treat the file as stale and
  // rewrite it on every single activation.
  return raw
    .slice(1, -1)
    .replace(/\\u([0-9a-fA-F]{4})/g, (_match, hex: string) => String.fromCharCode(parseInt(hex, 16)))
    .replace(/\\(.)/g, (_match, char: string) => {
      switch (char) {
        case "n":
          return "\n";
        case "t":
          return "\t";
        case "r":
          return "\r";
        default:
          return char;
      }
    });
}

/**
 * Quote one token for cmd.exe.
 *
 * Anything outside the plain path alphabet gets wrapped in double quotes, which
 * covers the case that actually bites — spaces in a Windows user directory.
 * Callers reject SHELL_METACHARACTERS first, so no escaping beyond this is
 * needed.
 */
export function quoteForShell(value: string): string {
  return /^[A-Za-z0-9_.:\\/=-]+$/.test(value) ? value : `"${value}"`;
}

/**
 * Quote a value as a YAML single-quoted scalar.
 *
 * WHY single quotes: a Windows daemon path is full of backslashes, and YAML
 * reads a backslash inside double quotes as an escape character, so a path
 * like E:\dev would lose the \d. Inside single quotes every character is
 * literal and only the quote itself needs doubling.
 */
export function yamlQuote(value: string): string {
  return "'" + value.replace(/'/g, "''") + "'";
}

/**
 * Quote a value as a TOML string.
 *
 * WHY a literal string by default: a Windows daemon path is full of
 * backslashes, and TOML reads a backslash inside a basic string as an escape,
 * so `C:\Users` would be rejected as an unknown escape sequence. A literal
 * string takes every character as-is. It cannot contain a single quote or a
 * control character, so those rare values fall back to a basic string.
 *
 * The fallback escapes the control characters too: a directory name may
 * legally contain a newline on macOS and Linux, and emitting it raw would
 * split the value across lines and leave the whole config unparseable.
 */
export function tomlQuote(value: string): string {
  // eslint-disable-next-line no-control-regex
  if (!/['\u0000-\u001f\u007f]/.test(value)) {
    return "'" + value + "'";
  }
  const escaped = value
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t")
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f]/g, (char) => {
      const code = char.charCodeAt(0).toString(16).padStart(4, "0");
      return `\\u${code}`;
    });
  return '"' + escaped + '"';
}

/**
 * Locate comP's own table in a Codex config.
 *
 * Returns the character range covering `[mcp_servers.comp]`, everything under
 * it, and any `[mcp_servers.comp.*]` sub-tables that follow — a config written
 * by `codex mcp add` uses a `[mcp_servers.comp.env]` sub-table, and leaving it
 * behind after replacing the parent would resurrect a stale environment.
 *
 * Shared by the writer and the repair pass so that "where comP's block is" has
 * exactly one definition.
 */
export function findCompSection(text: string): { start: number; end: number } | null {
  return findServerSection(text, "comp");
}

/**
 * `findCompSection` for any server name: the range of `[mcp_servers.<name>]`
 * and its `[mcp_servers.<name>.*]` sub-tables. `name` must be a plain bare key.
 */
export function findServerSection(
  text: string,
  name: string
): { start: number; end: number } | null {
  const lines = text.split("\n");
  // Header of the table, or of one of its sub-tables. Bare and quoted key
  // forms both occur: `codex mcp add` writes the bare one, hand edits vary.
  const header = new RegExp(
    String.raw`^\s*\[\s*mcp_servers\s*\.\s*("${name}"|'${name}'|${name})\s*(\.[^\]]*)?\]`
  );
  const isCompHeader = (line: string): boolean => header.test(line);
  const isAnyHeader = (line: string): boolean => /^\s*\[/.test(line);

  let start = -1;
  let end = -1;
  let offset = 0;
  for (const line of lines) {
    const lineEnd = offset + line.length + 1;
    if (start === -1) {
      if (isCompHeader(line)) {
        start = offset;
        end = Math.min(lineEnd, text.length);
      }
    } else if (isAnyHeader(line)) {
      // A sub-table still belongs to comP; anything else ends the section.
      if (!isCompHeader(line)) {
        break;
      }
      end = Math.min(lineEnd, text.length);
    } else {
      end = Math.min(lineEnd, text.length);
    }
    offset = lineEnd;
  }

  return start === -1 ? null : { start, end };
}

/**
 * Whether a located section can be replaced as a unit.
 *
 * findCompSection ends the section at the next line that opens a table, and a
 * nested array element written on its own line — `["--flag"],` inside a
 * multi-line `args` — looks exactly like one. Splicing there would strip the
 * rest of the collection and leave the file unparseable, taking down the
 * user's whole Codex configuration rather than just comP.
 *
 * A truncated section always ends with brackets still open, so that is what is
 * checked. Anything this cannot account for — a multi-line basic string, say —
 * also comes back unbalanced, which is the safe direction to be wrong in.
 */
export function isSelfContained(section: string): boolean {
  let depth = 0;
  let quote: '"' | "'" | null = null;

  for (let i = 0; i < section.length; i++) {
    const char = section[i];

    if (quote !== null) {
      // Only basic strings have escapes; in a literal string a backslash is
      // just a backslash, which is the whole reason paths are written that way.
      if (char === "\\" && quote === '"') {
        i++;
      } else if (char === quote) {
        quote = null;
      } else if (char === "\n") {
        return false;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      quote = char;
    } else if (char === "#") {
      while (i < section.length && section[i] !== "\n") {
        i++;
      }
    } else if (char === "[" || char === "{") {
      depth++;
    } else if (char === "]" || char === "}") {
      depth--;
      if (depth < 0) {
        return false;
      }
    }
  }

  return depth === 0 && quote === null;
}

/**
 * Servers written in a shape this line-oriented splice cannot rewrite.
 *
 * Both checks are scoped to the table they matter in: an unrelated key called
 * `comp` under `[tui]` is none of comP's business, and refusing to configure
 * Codex because of one would be a plain false alarm.
 *
 * Returns the reason to refuse, or null when the file can be edited.
 */
export function unsupportedServerShape(text: string): string | null {
  let table = "";

  for (const rawLine of text.split("\n")) {
    const line = rawLine.trim();

    const header = /^\[\s*([^\]]*?)\s*\]/.exec(line);
    if (header) {
      table = header[1].replace(/\s+/g, "");
      continue;
    }
    // `mcp_servers = { ... }` — the entire server table written inline.
    if (table === "" && /^mcp_servers\s*=/.test(line)) {
      return "mcp_servers is written as an inline table; merge comP by hand";
    }
    // `comp = { ... }` or `comp.command = ...` under `[mcp_servers]`. TOML
    // allows whitespace on both sides of a dotted-key separator, so the dot
    // itself — not just the key after it — needs `\s*` around it too;
    // otherwise `comp . command = …` (legal TOML) slips through undetected
    // and a later `[mcp_servers.comp]` table becomes a duplicate-key error
    // that Codex refuses to parse at all.
    if (
      table === "mcp_servers" &&
      /^("comp"|'comp'|comp)\s*(\.\s*[A-Za-z0-9_-]+\s*)*=/.test(line)
    ) {
      return "a comp entry is written in inline/dotted form; merge comP by hand";
    }
  }

  return null;
}

/**
 * The lines of an existing comp section that the user or Codex wrote, to be
 * carried over below a freshly generated block.
 *
 * WHY carry rather than replace the whole section: Codex saves "always allow"
 * as `[mcp_servers.comp.tools.<tool>] approval_mode = "approve"`, and users add
 * keys such as `default_tools_approval_mode` or `startup_timeout_sec`. Dropping
 * them on every setup silently put each comP call back behind Codex's approval
 * prompt and auto-review.
 *
 * Dropped: the table header, `command` / `args` / `env` in any key spelling
 * (bare, quoted, dotted — a second definition next to the generated ones would
 * make the file unparseable) and the whole `[mcp_servers.comp.env]` sub-table
 * (a stale environment must not survive a replace). Everything else, comments
 * included, is kept in order.
 */
export function userLinesInCompSection(sectionText: string): string {
  const comp = `\\s*mcp_servers\\s*\\.\\s*(?:"comp"|'comp'|comp)\\s*`;
  const isMainHeader = new RegExp(`^\\s*\\[${comp}\\]`);
  const isEnvHeader = new RegExp(`^\\s*\\[${comp}\\.\\s*(?:"env"|'env'|env)\\s*\\]`);
  const isHeader = (line: string): boolean => /^\s*\[/.test(line);
  const firstKeySegment = /^\s*(?:"([^"]*)"|'([^']*)'|([A-Za-z0-9_-]+))\s*[.=]/;
  const managedKeys = new Set(["command", "args", "env"]);

  let table: "main" | "env" | "sub" = "main";
  const kept: string[] = [];
  for (const line of sectionText.replace(/\n$/, "").split("\n")) {
    if (isHeader(line)) {
      table = isMainHeader.test(line) ? "main" : isEnvHeader.test(line) ? "env" : "sub";
      if (table === "sub") {
        kept.push(line);
      }
      continue;
    }
    if (table === "env") {
      continue;
    }
    if (table === "main") {
      const key = firstKeySegment.exec(line);
      if (key && managedKeys.has(key[1] ?? key[2] ?? key[3])) {
        continue;
      }
    }
    kept.push(line);
  }

  while (kept.length > 0 && kept[0].trim() === "") {
    kept.shift();
  }
  while (kept.length > 0 && kept[kept.length - 1].trim() === "") {
    kept.pop();
  }
  if (kept.length === 0) {
    return "";
  }
  // A sub-table right after the generated env line reads better set apart.
  const lead = isHeader(kept[0]) ? "\n" : "";
  return lead + kept.join("\n") + "\n";
}

/**
 * Blank out TOML comments while keeping every other character — including
 * newlines — exactly in place.
 *
 * WHY: the caller matches a key like COMP_WORKSPACE_ROOT against the raw
 * text. Unlike the command match elsewhere, that key can sit in the middle
 * of a line (it does in the very block this writer generates, packed into an
 * inline `env = { … }` table), so anchoring the pattern to the start of a
 * line cannot rule out a comment the way `^\s*command` does. A user comment
 * such as `# old value: COMP_WORKSPACE_ROOT = '/old/path'` placed above the
 * real line would otherwise be found first — regex search returns the
 * leftmost match — and get "repaired" instead of the value that actually
 * takes effect, leaving the real one stale forever.
 *
 * Preserving length and line breaks keeps every match index valid against
 * the original, unmasked text: nothing meaningful ever lives inside a
 * comment, so a match found here always falls in a region this function left
 * untouched, and slicing the original at that index yields identical bytes.
 */
export function maskComments(text: string): string {
  let out = "";
  let quote: '"' | "'" | null = null;
  let i = 0;
  while (i < text.length) {
    const char = text[i];
    if (quote !== null) {
      out += char;
      if (char === "\\" && quote === '"' && i + 1 < text.length) {
        out += text[i + 1];
        i += 2;
        continue;
      }
      if (char === quote) {
        quote = null;
      }
      i++;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      out += char;
      i++;
      continue;
    }
    if (char === "#") {
      while (i < text.length && text[i] !== "\n") {
        out += " ";
        i++;
      }
      continue;
    }
    out += char;
    i++;
  }
  return out;
}
