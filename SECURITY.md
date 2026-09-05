# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 0.8.x   | ✅        |
| 0.7.x   | ✅        |
| 0.6.x   | ⚠️ 非推奨 |
| 0.5.x   | ❌        |
| 0.4.x以前 | ❌      |

## Reporting a Vulnerability

**Do not report security vulnerabilities via GitHub Issues.**

Please report security vulnerabilities using:

- **GitHub Private Vulnerability Reporting**: Please use the
  [Security tab](https://github.com/tsucky230/comP/security/advisories/new).

### What to include

- Description of the vulnerability and its potential impact
- Steps to reproduce
- Affected version(s)
- Any suggested fix (optional)

### Response timeline

| Stage | Target |
|---|---|
| Acknowledgement | Within 72 hours |
| Initial assessment | Within 7 days |
| Fix or mitigation | Within 90 days |

We follow coordinated disclosure: please allow us to release a fix before
publishing details publicly.

## Scope

In scope:

- comP VSCode extension (`src/`)
- Rust daemon (`daemon/`)
- MCP protocol handling

Out of scope:

- VSCode itself
- Dependencies (report to upstream maintainers)
- Issues requiring physical access to the machine
