// scripts/check-docs-consistency.js - Unit tests for the pre-push docs/implementation consistency check
//
// Test coverage:
// 1. Reading tool names and required arguments out of the daemon source
// 2. Tool coverage between daemon and MCP_TOOLS.md (both directions)
// 3. Required-argument table in MCP_TOOLS.md
// 4. CHANGELOG regression tests actually exist
// 5. Implementation change without any docs change
// 6. Tools mentioned in README / Pages that the daemon does not expose

import { expect } from 'chai';

// @ts-expect-error scripts/ is plain JS outside rootDir and ships no type declarations
import check from '../../scripts/check-docs-consistency.js';

const RUST = `
    pub async fn handle_tools_list(&self) -> Result<Value> {
        Ok(json!({ "tools": [ { "name": "alpha" }, { "name": "beta" } ] }))
    }

    pub async fn handle_tools_call(&self, params: Value) -> Result<Value> {
        let result = match name {
            "alpha" => self.handle_alpha(args).await?,
            "beta" => self.handle_beta(args).await?,
            "hidden" => self.handle_hidden().await?,
            _ => return Err(anyhow!("Unknown tool: {}", name)),
        };
    }

    pub async fn handle_alpha(&self, params: Value) -> Result<Value> {
        let t = params["task"].as_str().ok_or_else(|| anyhow!("Missing 'task' parameter"))?;
    }

    pub async fn handle_beta(&self, params: Value) -> Result<Value> {
        let n = params["name"].as_str().ok_or_else(|| anyhow!("Missing 'name' parameter"))?;
        let d = params["direction"].as_str().ok_or_else(|| anyhow!("Missing 'direction' parameter"))?;
    }

    pub async fn handle_hidden(&self) -> Result<Value> {
        Ok(json!({}))
    }

    pub async fn handle_index_file(&self, params: Value) -> Result<Value> {
        let p = params["path"].as_str().ok_or_else(|| anyhow!("Missing 'path' parameter"))?;
    }
`;

const DOCS_OK = `
# MCP Tools Reference

## Tools

### \`alpha\`

Does alpha.

- \`task\` (string, required)

### \`beta\`

Does beta.

- \`name\` (string, required)
- \`direction\` (string, required)

## Errors

| Tool | Required arguments |
| --- | --- |
| \`alpha\` | \`task\` |
| \`beta\` | \`name\`, \`direction\` (\`in\` or \`out\`) |
`;

describe('check-docs-consistency', () => {
  describe('extractCodeTools', () => {
    const tools = check.extractCodeTools(RUST);

    it('reads the tools/list names', () => {
      expect(tools.listed).to.deep.equal(['alpha', 'beta']);
    });

    it('reads tools/call names, including tools that are not listed', () => {
      expect(tools.callable).to.have.members(['alpha', 'beta', 'hidden']);
    });

    it('maps each tool to the arguments its handler requires', () => {
      expect(tools.required.alpha).to.deep.equal(['task']);
      expect(tools.required.beta).to.have.members(['name', 'direction']);
    });

    it('does not attribute another handler\'s arguments to a tool', () => {
      expect(tools.required.hidden ?? []).to.deep.equal([]);
      expect(tools.required.alpha).to.not.include('path');
    });

    // The daemon's handle_tools_list only calls tool_definitions(), where the names live.
    const SPLIT = `
    pub async fn handle_tools_list(&self) -> Result<Value> {
        let mut list = Self::tool_definitions();
        Ok(list)
    }

    fn tool_definitions() -> Value {
        json!({ "tools": [
            { "name": "alpha", "inputSchema": { "properties": { "name": { "type": "string" } } } },
            { "name": "beta" }
        ] })
    }

    pub async fn handle_tools_call(&self, params: Value) -> Result<Value> {
        let result = match name {
            "alpha" => self.handle_alpha(args).await?,
            "beta" => self.handle_beta(args).await?,
            _ => return Err(anyhow!("Unknown tool: {}", name)),
        };
    }
`;

    it('reads the tools/list names from tool_definitions() when handle_tools_list delegates to it', () => {
      expect(check.extractCodeTools(SPLIT).listed).to.deep.equal(['alpha', 'beta']);
    });

    it('reports a tool listed through tool_definitions() that has no section', () => {
      const problems = check.checkToolCoverage(check.extractCodeTools(SPLIT), '### `alpha`\n');
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('beta');
    });
  });

  describe('extractDocumentedTools', () => {
    it('reads ### `name` headings only', () => {
      const md = '### `alpha`\n\ntext\n\n## `not-a-tool-heading`\n\n### plain heading\n\n### `beta`\n';
      expect(check.extractDocumentedTools(md)).to.deep.equal(['alpha', 'beta']);
    });
  });

  describe('checkToolCoverage', () => {
    const tools = check.extractCodeTools(RUST);

    it('passes when every listed tool is documented', () => {
      expect(check.checkToolCoverage(tools, DOCS_OK)).to.deep.equal([]);
    });

    it('reports a listed tool with no section', () => {
      const problems = check.checkToolCoverage(tools, DOCS_OK.replace('### `beta`', '### beta'));
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('beta');
    });

    it('reports a documented tool the daemon does not expose', () => {
      const problems = check.checkToolCoverage(tools, DOCS_OK + '\n### `ghost`\n');
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('ghost');
    });

    it('accepts documenting a callable tool that is not listed', () => {
      expect(check.checkToolCoverage(tools, DOCS_OK + '\n### `hidden`\n')).to.deep.equal([]);
    });
  });

  describe('checkRequiredArgs', () => {
    const tools = check.extractCodeTools(RUST);

    it('passes when the table names every required argument', () => {
      expect(check.checkRequiredArgs(tools, DOCS_OK)).to.deep.equal([]);
    });

    it('reports an argument missing from the table row', () => {
      const problems = check.checkRequiredArgs(tools, DOCS_OK.replace('`name`, `direction`', '`name`'));
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('beta').and.to.include('direction');
    });

    it('reports a tool that has no row at all', () => {
      const problems = check.checkRequiredArgs(tools, DOCS_OK.replace(/\| `alpha` \|.*\n/, ''));
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('alpha');
    });

    it('does not require a row for a tool with no required arguments', () => {
      const noArgs = { listed: ['solo'], callable: ['solo'], required: {} };
      expect(check.checkRequiredArgs(noArgs, DOCS_OK)).to.deep.equal([]);
    });
  });

  describe('checkSectionArgs', () => {
    const tools = check.extractCodeTools(RUST);

    it('passes when each tool section names its required arguments', () => {
      expect(check.checkSectionArgs(tools, DOCS_OK)).to.deep.equal([]);
    });

    it('reports a section that documents a different argument name', () => {
      const problems = check.checkSectionArgs(tools, DOCS_OK.replace('- `task` (string, required)', '- `query` (string, required)'));
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('alpha').and.to.include('task');
    });

    it('does not read arguments from the next tool\'s section', () => {
      const problems = check.checkSectionArgs(tools, DOCS_OK.replace('- `task` (string, required)\n', ''));
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('alpha');
    });

    it('skips a tool with no section (checkToolCoverage reports that)', () => {
      expect(check.checkSectionArgs(tools, '# Doc\n\n## Errors\n')).to.deep.equal([]);
    });
  });

  describe('checkChangelogTests', () => {
    const changelog = [
      '## [0.2.0] - 2026-01-02',
      '- fix. tests: `mcp::tests::test_present`, `mcp::tests::test_absent`',
      '## [0.1.0] - 2026-01-01',
      '- old. tests: `mcp::tests::test_from_old_release`',
    ].join('\n');
    const rust = 'fn test_present() {}\nfn other() {}';

    it('reports a named test that has no fn', () => {
      const problems = check.checkChangelogTests(changelog, '0.2.0', rust);
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('test_absent');
    });

    it('ignores tests named under other versions', () => {
      const problems = check.checkChangelogTests(changelog, '0.2.0', rust);
      expect(problems.join(' ')).to.not.include('test_from_old_release');
    });

    it('passes when every named test exists', () => {
      expect(check.checkChangelogTests(changelog, '0.2.0', rust + '\nfn test_absent() {}')).to.deep.equal([]);
    });

    it('reports a version that has no section', () => {
      const problems = check.checkChangelogTests(changelog, '9.9.9', rust);
      expect(problems).to.have.length(1);
      expect(problems[0]).to.include('9.9.9');
    });
  });

  describe('checkDocsTouched', () => {
    it('reports implementation changes with no docs change', () => {
      expect(check.checkDocsTouched(['daemon/src/mcp/mod.rs'])).to.have.length(1);
    });

    it('passes when CHANGELOG changed too', () => {
      expect(check.checkDocsTouched(['daemon/src/mcp/mod.rs', 'CHANGELOG.md'])).to.deep.equal([]);
    });

    it('passes when a docs file changed too', () => {
      expect(check.checkDocsTouched(['src/extension.ts', 'docs/user/MCP_TOOLS.md'])).to.deep.equal([]);
    });

    it('does not require docs for test-only, script-only or docs-only changes', () => {
      expect(check.checkDocsTouched(['src/test/foo.test.ts'])).to.deep.equal([]);
      expect(check.checkDocsTouched(['scripts/x.js'])).to.deep.equal([]);
      expect(check.checkDocsTouched(['docs/dev/CODING.md'])).to.deep.equal([]);
      expect(check.checkDocsTouched([])).to.deep.equal([]);
    });
  });

  describe('findUnknownToolMentions', () => {
    const callable = ['run_pipeline', 'get_symbol'];

    it('reports a backticked tool name the daemon lacks', () => {
      expect(check.findUnknownToolMentions('use `get_ghost` here', callable, [])).to.deep.equal(['get_ghost']);
    });

    it('reports a <code> tool name the daemon lacks', () => {
      expect(check.findUnknownToolMentions('<code>get_ghost</code>', callable, [])).to.deep.equal(['get_ghost']);
    });

    it('accepts known tools', () => {
      expect(check.findUnknownToolMentions('`run_pipeline` and `get_symbol`', callable, [])).to.deep.equal([]);
    });

    it('ignores tool-like words in plain prose', () => {
      expect(check.findUnknownToolMentions('you can get_ghost later', callable, [])).to.deep.equal([]);
    });

    it('honours the ignore list', () => {
      expect(check.findUnknownToolMentions('`get_started`', callable, ['get_started'])).to.deep.equal([]);
    });

    it('reports each unknown name once', () => {
      expect(check.findUnknownToolMentions('`get_ghost` `get_ghost`', callable, [])).to.deep.equal(['get_ghost']);
    });
  });
});
