#!/usr/bin/env node

/**
 * Pre-push check: docs / GitHub Pages must not contradict or omit what the
 * daemon actually implements.
 *
 * WHY this exists: the older pre-push checks only prove a version string is
 * present. They cannot see that a tool is undocumented, that a documented
 * tool does not exist, or that a behaviour change never reached the docs.
 * Every check here is a pure function over text so it can be unit tested
 * without a git repo; only main() touches the filesystem and git.
 *
 * Limit: this catches structural omissions and contradictions, not whether
 * prose is accurate. That part is the manual list in docs/dev/PRE_PUSH_CHECKLIST.md.
 */

'use strict';

/**
 * Tools the daemon exposes, read from daemon/src/mcp/mod.rs.
 *
 * @param {string} rustSrc contents of daemon/src/mcp/mod.rs
 * @returns {{listed: string[], callable: string[], required: Record<string, string[]>}}
 *   listed   - names in the tools/list response (what a client can discover)
 *   callable - names accepted by tools/call (a superset: may include hidden tools)
 *   required - tool -> argument names its handler rejects with "Missing '<arg>' parameter"
 */
function fnBody(src, fnName) {
  const start = src.search(new RegExp(`fn ${fnName}\\(`));
  if (start === -1) return '';
  const rest = src.slice(start + 1);
  // The next line opening an impl-level fn (4-space indent) ends this method.
  const next = rest.search(/\n {4}(?:pub )?(?:async )?fn \w/);
  return next === -1 ? rest : rest.slice(0, next);
}

function extractCodeTools(rustSrc) {
  // WHY both bodies: the daemon's handle_tools_list only calls tool_definitions(),
  // which holds the names; reading handle_tools_list alone found no tool at all,
  // so an undocumented tool went unreported.
  const listBodies = fnBody(rustSrc, 'handle_tools_list') + fnBody(rustSrc, 'tool_definitions');
  const listed = [...new Set([...listBodies.matchAll(/"name":\s*"([a-z_]+)"/g)].map((m) => m[1]))];

  const callable = [];
  const handlers = {};
  for (const m of fnBody(rustSrc, 'handle_tools_call').matchAll(/"([a-z_]+)"\s*=>\s*self\.(handle_\w+)/g)) {
    callable.push(m[1]);
    handlers[m[1]] = m[2];
  }

  const required = {};
  for (const [tool, handler] of Object.entries(handlers)) {
    const args = [...fnBody(rustSrc, handler).matchAll(/Missing '([a-z_]+)' parameter/g)].map((m) => m[1]);
    if (args.length > 0) required[tool] = [...new Set(args)];
  }
  return { listed, callable, required };
}

/**
 * Tool names documented in MCP_TOOLS.md as `### \`name\`` headings.
 *
 * @param {string} mcpToolsMd
 * @returns {string[]}
 */
function extractDocumentedTools(mcpToolsMd) {
  return [...mcpToolsMd.matchAll(/^### `([a-z_]+)`\s*$/gm)].map((m) => m[1]);
}

/**
 * Every tool in tools/list must have a section in MCP_TOOLS.md, and every
 * section must be a tool the daemon really exposes (callable).
 *
 * @returns {string[]} human readable problems, empty when consistent
 */
function checkToolCoverage(codeTools, mcpToolsMd) {
  const documented = extractDocumentedTools(mcpToolsMd);
  const problems = [];
  for (const tool of codeTools.listed) {
    if (!documented.includes(tool)) {
      problems.push(`MCP_TOOLS.md has no "### \`${tool}\`" section, but the daemon lists that tool`);
    }
  }
  for (const tool of documented) {
    if (!codeTools.callable.includes(tool)) {
      problems.push(`MCP_TOOLS.md documents \`${tool}\`, but the daemon does not expose it`);
    }
  }
  return problems;
}

/**
 * The Errors table in MCP_TOOLS.md must name, for each tool, every argument
 * the code requires. A wrong argument name surfaces to the user only as a
 * failed call, so the table is what tells them the right name.
 *
 * @returns {string[]}
 */
function checkRequiredArgs(codeTools, mcpToolsMd) {
  const rows = {};
  for (const m of mcpToolsMd.matchAll(/^\|\s*`([a-z_]+)`\s*\|(.*)\|\s*$/gm)) {
    rows[m[1]] = new Set([...m[2].matchAll(/`([a-z_]+)`/g)].map((a) => a[1]));
  }
  const problems = [];
  for (const [tool, args] of Object.entries(codeTools.required)) {
    if (!rows[tool]) {
      problems.push(`Errors table in MCP_TOOLS.md has no row for \`${tool}\` (requires: ${args.join(', ')})`);
      continue;
    }
    const missing = args.filter((a) => !rows[tool].has(a));
    if (missing.length > 0) {
      problems.push(`Errors table row for \`${tool}\` is missing required argument(s): ${missing.join(', ')}`);
    }
  }
  return problems;
}

/**
 * A tool's own section in MCP_TOOLS.md must name each argument the code
 * requires. Catches a section that documents an old or wrong argument name.
 * A tool with no section is skipped; checkToolCoverage reports that.
 *
 * @returns {string[]}
 */
function checkSectionArgs(codeTools, mcpToolsMd) {
  const problems = [];
  for (const [tool, args] of Object.entries(codeTools.required)) {
    const start = mcpToolsMd.search(new RegExp(`^### \`${tool}\`\\s*$`, 'm'));
    if (start === -1) continue;
    const after = mcpToolsMd.slice(start + 1);
    const end = after.search(/\n#{2,3} /);
    const section = end === -1 ? after : after.slice(0, end);
    const missing = args.filter((a) => !section.includes(`\`${a}\``));
    if (missing.length > 0) {
      problems.push(`MCP_TOOLS.md section for \`${tool}\` does not name required argument(s): ${missing.join(', ')}`);
    }
  }
  return problems;
}

/**
 * Regression tests named in the CHANGELOG section for `version` must exist as
 * `fn <name>` in the given Rust sources. Only tokens shaped `x::tests::name`
 * are checked; anything else in the section is prose.
 *
 * @param {string} changelogMd
 * @param {string} version e.g. "0.11.6"
 * @param {string} rustSources concatenated Rust sources to search
 * @returns {string[]}
 */
function checkChangelogTests(changelogMd, version, rustSources) {
  const start = changelogMd.indexOf(`## [${version}]`);
  if (start === -1) return [`CHANGELOG.md has no "## [${version}]" section`];
  const after = changelogMd.slice(start + 1);
  const end = after.search(/\n## \[/);
  const section = end === -1 ? after : after.slice(0, end);

  const names = new Set([...section.matchAll(/`[\w:]*::tests::(\w+)`/g)].map((m) => m[1]));
  const problems = [];
  for (const name of names) {
    if (!new RegExp(`fn ${name}\\b`).test(rustSources)) {
      problems.push(`CHANGELOG.md [${version}] names test \`${name}\`, but no such fn exists`);
    }
  }
  return problems;
}

/**
 * Implementation changes in the pushed range must come with a docs change
 * (CHANGELOG, README, docs/). Heuristic by design: it cannot tell whether the
 * docs change is the right one, only that there is none at all.
 *
 * @param {string[]} changedFiles repo-relative paths, forward slashes
 * @returns {string[]}
 */
function checkDocsTouched(changedFiles) {
  const isImplementation = (f) =>
    /^(daemon\/src|src)\//.test(f) && !/(^|\/)tests?\//.test(f) && !/\.test\.ts$/.test(f);
  const isDocs = (f) => f === 'CHANGELOG.md' || /^README(_ja)?\.md$/.test(f) || f.startsWith('docs/');
  if (changedFiles.some(isImplementation) && !changedFiles.some(isDocs)) {
    return ['Implementation files changed but no CHANGELOG / README / docs file did'];
  }
  return [];
}

/**
 * README / Pages / docs must not mention a tool the daemon does not expose.
 * Only backticked or <code> tokens are considered, to avoid matching prose.
 *
 * @param {string} text contents of one documentation file
 * @param {string[]} callable tool names from extractCodeTools
 * @param {string[]} ignore names that look like tools but are not (e.g. get_started)
 * @returns {string[]} unknown tool names found
 */
function findUnknownToolMentions(text, callable, ignore) {
  const name = '((?:get|list|session|compress|run)_[a-z_]+)';
  const found = [
    ...text.matchAll(new RegExp('`' + name + '`', 'g')),
    ...text.matchAll(new RegExp('<code>' + name + '</code>', 'g')),
  ].map((m) => m[1]);
  return [...new Set(found)].filter((n) => !callable.includes(n) && !ignore.includes(n));
}

// Words that look like tool names in docs but are not (none needed so far).
const NOT_TOOLS = [];

const DOC_FILES_FOR_TOOL_MENTIONS = ['README.md', 'README_ja.md', 'docs/index.html', 'docs/index_ja.html'];

function listRustFiles(dir, out = []) {
  const fs = require('fs');
  const path = require('path');
  if (!fs.existsSync(dir)) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === 'target') continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) listRustFiles(full, out);
    else if (entry.name.endsWith('.rs')) out.push(full);
  }
  return out;
}

/**
 * Files changed in the range about to be pushed. Falls back to origin/main
 * when the branch has no upstream; returns null when git cannot answer, in
 * which case the docs-touched check is skipped rather than guessed.
 */
function changedFilesInPushRange() {
  const { execSync } = require('child_process');
  for (const range of ['@{upstream}..HEAD', 'origin/main..HEAD']) {
    try {
      const out = execSync(`git diff --name-only ${range}`, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
      return out.split('\n').map((l) => l.trim()).filter(Boolean);
    } catch {
      // try the next range
    }
  }
  return null;
}

function main() {
  const fs = require('fs');
  const path = require('path');
  const root = path.resolve(__dirname, '..');
  const read = (rel) => fs.readFileSync(path.join(root, rel), 'utf8');

  const problems = [];
  const codeTools = extractCodeTools(read('daemon/src/mcp/mod.rs'));
  const mcpToolsMd = read('docs/user/MCP_TOOLS.md');
  problems.push(...checkToolCoverage(codeTools, mcpToolsMd));
  problems.push(...checkRequiredArgs(codeTools, mcpToolsMd));
  problems.push(...checkSectionArgs(codeTools, mcpToolsMd));

  const version = JSON.parse(read('package.json')).version;
  const rustSources = [...listRustFiles(path.join(root, 'daemon/src')), ...listRustFiles(path.join(root, 'daemon/tests'))]
    .map((f) => fs.readFileSync(f, 'utf8'))
    .join('\n');
  problems.push(...checkChangelogTests(read('CHANGELOG.md'), version, rustSources));

  const docFiles = [...DOC_FILES_FOR_TOOL_MENTIONS];
  for (const dir of ['docs/user']) {
    for (const f of fs.readdirSync(path.join(root, dir))) {
      if (f.endsWith('.md')) docFiles.push(`${dir}/${f}`);
    }
  }
  for (const f of docFiles) {
    for (const name of findUnknownToolMentions(read(f), codeTools.callable, NOT_TOOLS)) {
      problems.push(`${f} mentions \`${name}\`, which the daemon does not expose`);
    }
  }

  const changed = changedFilesInPushRange();
  if (changed === null) console.log('[comP] docs check: could not determine the push range, skipping the docs-touched check');
  else problems.push(...checkDocsTouched(changed));

  if (problems.length > 0) {
    console.error('[comP] ❌ docs / implementation consistency problems:');
    for (const p of problems) console.error(`[comP]    - ${p}`);
    console.error('[comP] Fix the docs (or the code), then push again. Manual list: docs/dev/PRE_PUSH_CHECKLIST.md');
    return 1;
  }
  console.log('[comP] ✅ docs / implementation consistency checks passed');
  console.log('[comP] Not machine-checkable, confirm by hand: docs/dev/PRE_PUSH_CHECKLIST.md');
  return 0;
}

module.exports = {
  extractCodeTools,
  extractDocumentedTools,
  checkToolCoverage,
  checkRequiredArgs,
  checkSectionArgs,
  checkChangelogTests,
  checkDocsTouched,
  findUnknownToolMentions,
  main,
};

if (require.main === module) {
  process.exit(main());
}
