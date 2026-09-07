# Contributing to comP

Thank you for your interest in contributing to comP! This document provides guidelines and instructions for reporting bugs, suggesting features, and submitting code.

---

## Code of Conduct

comP is committed to providing a welcoming and inclusive community. Please treat all contributors and users with respect. Discrimination, harassment, or any form of abuse will not be tolerated.

---

## Reporting Bugs

If you discover a bug, please report it by creating a GitHub Issue:

1. Go to [GitHub Issues](https://github.com/tsucky230/comP/issues)
2. Click **New Issue** → **Bug Report**
3. Provide:
   - **Title**: Brief description of the bug
   - **Environment**: OS, VSCode version, comP version, Rust version (if applicable)
   - **Steps to reproduce**: Clear, step-by-step instructions
   - **Expected behavior**: What should happen
   - **Actual behavior**: What actually happened
   - **Screenshots/logs**: Attach VSCode Output panel logs if relevant

### Where to Find Logs

- **VSCode Output Panel**: View → Output → "comP"
- **Daemon Logs**: `.comp/daemon.log` (if enabled)

---

## Suggesting Features

Have a feature idea? Create a Feature Request:

1. Go to [GitHub Discussions](https://github.com/tsucky230/comP/discussions)
2. Create a **New Discussion** with category "Ideas"
3. Describe:
   - **What problem does this solve?**
   - **How would a user interact with this feature?**
   - **Are there any alternatives?**
   - **Any implementation notes?**

Feature requests are reviewed by maintainers and may be added to the Roadmap.

---

## Setting Up the Development Environment

### Prerequisites

- **OS**: Windows, macOS, or Linux
- **Node.js**: 18+
- **Rust**: 1.70+
- **Git**: Latest version

### Clone and Install

```bash
# Clone the repository
git clone https://github.com/tsucky230/comP.git
cd comP

# Install npm dependencies
npm install

# Verify Rust is installed
rustc --version
cargo --version
```

### Build

```bash
# Compile TypeScript extension
npm run compile

# Build Rust daemon
npm run daemon:build
```

### Packaging a Local VSIX (for testing)

Plain `vsce package` only bundles the daemon binary for your current platform if
it already has the CI's platform-specific filename (`comp-daemon-win.exe`,
`comp-daemon-linux`, `comp-daemon-macos`). A local `cargo build --release`
produces `comp-daemon`/`comp-daemon.exe` instead, so `.vscodeignore`'s allowlist
won't match it and the resulting VSIX will silently ship without a daemon
binary — the extension will fail to start on any workspace that doesn't
already have a matching dev build.

To build and package a VSIX that actually works when installed:

```bash
npm run package:local
```

This runs `daemon:build`, copies the binary to the platform-specific filename
via `scripts/package-daemon-binary.js`, then runs `vsce package`. Always use
this (not a bare `vsce package`) when testing a locally built VSIX.

### Watch Mode (Recommended for Development)

```bash
# Terminal 1: Watch TypeScript changes
npm run watch

# Terminal 2: Watch Rust daemon changes (in another terminal)
npm run daemon:build -- --watch
```

### Testing

```bash
# Run Rust tests
cargo test --all --manifest-path daemon/Cargo.toml

# Run npm tests
npm test

# Run both
npm run daemon:test && npm test
```

### Linting

```bash
# Check Markdown
npm run lint:md

# Auto-fix Markdown
npm run lint:md:fix
```

### Debug in VSCode

1. Open the project in VSCode
2. Press **F5** to start debugging (launches Extension Development Host)
3. Open a test folder in the new VSCode window
4. Test commands via Command Palette (Ctrl+Shift+P)

---

## Submitting Changes

### Branch Naming

Use descriptive branch names following this pattern:

```bash
feature/description-of-feature
bugfix/description-of-bug
docs/description-of-docs-change
```

Examples:

- `feature/impact-graph-visualization`
- `bugfix/mcp-connection-timeout`
- `docs/add-api-examples`

### Commit Messages

Follow conventional commits format:

```text
type(scope): subject

Body paragraph explaining the change...

Resolves #123
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

Examples:

```text
feat(indexer): add support for Kotlin language

Add tree-sitter-kotlin parser to support indexing Kotlin files.
Includes unit tests for symbol extraction.

Resolves #42
```

```text
fix(daemon): handle IPC timeout gracefully

Improve error handling when daemon doesn't respond within 5 seconds.
Add retry logic with exponential backoff.
```

### Pull Request Process

1. **Before starting**: Check if an issue already exists. If not, create one to discuss the change first.
2. **Create a branch**: `git checkout -b feature/your-feature`
3. **Implement your changes**:
   - Write code following the project's style and patterns
   - Add tests (aim for 80%+ code coverage)
   - Update documentation if needed
4. **Pass checks**:
   - `npm run lint:md:fix` (Markdown linting)
   - `npm run compile` (TypeScript compilation)
   - `npm run daemon:build` (Rust compilation)
   - `npm test && npm run daemon:test` (all tests pass)
5. **Push and open PR**:
   - Push to your fork: `git push origin feature/your-feature`
   - Open a PR against `main` branch
   - Fill in the PR template with:
     - Summary of changes
     - Why this change is needed
     - Testing steps
     - Checklist of completed items

### Pull Request Checklist

Before submitting your PR, ensure:

- [ ] Code compiles without warnings/errors
- [ ] All tests pass (`npm test && npm run daemon:test`)
- [ ] Code coverage is 80%+ for new code
- [ ] Markdown is linted (`npm run lint:md`)
- [ ] Commits follow conventional format
- [ ] PR description is clear and references any related issues
- [ ] Documentation is updated (README, docs/, code comments)
- [ ] No breaking changes (or breaking changes are clearly noted)

---

## Code Style

### TypeScript

- Use `strict` TypeScript mode (enabled in `tsconfig.json`)
- Use 2-space indentation
- Prefer `const` over `let`, avoid `var`
- Use interfaces over type aliases for external APIs
- Document public APIs with JSDoc comments

### Rust

- Format code with `rustfmt` (run before commit)
- Use meaningful variable and function names
- Add comments for non-obvious logic
- Aim for 80%+ test coverage with boundary value testing

### Comments

- Write comments for the **why**, not the **what**
- Keep comments concise and clear
- Update comments when code changes

---

## Documentation

All contributions should include documentation updates:

- **API changes**: Update `docs/API.md`
- **Architecture changes**: Update `docs/ARCHITECTURE.md`
- **User-facing features**: Update `README.md` and `README_ja.md`
- **Code changes**: Add/update code comments
- **Bugs fixed**: Add entry to `CHANGELOG.md` under "Fixed"

---

## Release Process (Maintainers Only)

1. Update `package.json` version following semantic versioning
2. Update `CHANGELOG.md` with changes
3. Commit: `git commit -m "chore: bump version to X.Y.Z"`
4. Tag: `git tag -a vX.Y.Z -m "Release version X.Y.Z"`
5. Push: `git push origin main && git push origin vX.Y.Z`
6. GitHub Actions automatically:
   - Builds VSIX package
   - Generates SBOM
   - Creates GitHub Release with artifacts

---

## MCP Server Development

comP implements the **Model Context Protocol (MCP)** to expose tools to AI agents. If you're adding new MCP tools or modifying the protocol:

### MCP Tool Development Checklist

1. **Define the tool** in `daemon/src/mcp/mod.rs`:
   - Implement the tool handler function
   - Update the tool list in `handle_tools_list()`
   - Add JSDoc-style documentation

2. **Test the tool**:
   - Add integration tests in `daemon/tests/`
   - Verify with Claude Code or another MCP client
   - Check `.comp/daemon.log` for errors

3. **Document the tool**:
   - Update [docs/user/MCP_TOOLS.md](docs/user/MCP_TOOLS.md) with:
     - Tool name and description
     - Input parameters and types
     - Output format and examples
     - Example usage in Claude Code chat

4. **Multi-agent compatibility**:
   - Test with at least 2 agents (Claude Code + Cursor/Copilot)
   - Verify parameter marshaling (JSON serialization)
   - Check timeout handling (default 30s)

### Example: Adding a New MCP Tool

```rust
// daemon/src/mcp/mod.rs
pub async fn handle_new_tool(params: Value) -> Result<Value> {
    let query = params["query"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing 'query' parameter"))?;
    
    // Implementation...
    
    Ok(json!({
        "result": "...",
        "metadata": { "tokens": 1234 }
    }))
}
```

Then add to `handle_tools_list()`:

```rust
tools.push(json!({
    "name": "new_tool",
    "description": "Does something useful",
    "inputSchema": {
        "type": "object",
        "properties": {
            "query": { "type": "string" }
        },
        "required": ["query"]
    }
}));
```

### Testing MCP Servers

```bash
# Test the daemon directly (Unix socket / named pipe)
cargo test --all --manifest-path daemon/Cargo.toml

# Test integration with VSCode extension
npm run compile && F5  # Opens Extension Development Host
```

---

## Getting Help

- 📖 **Documentation**: [docs/](docs/)
- 💬 **Discussions**: [GitHub Discussions](https://github.com/tsucky230/comP/discussions)
- 🐛 **Issues**: [GitHub Issues](https://github.com/tsucky230/comP/issues)

---

## Recognition

All contributors will be listed in the `CONTRIBUTORS.md` file. Thank you for making comP better! 🚀
