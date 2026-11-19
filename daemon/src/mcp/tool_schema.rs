use super::*;

impl MCPServer {
    pub(crate) fn tool_definitions() -> Value {
        json!({
            "tools": [
                {
                    "name": "run_pipeline",
                    "description": "ALWAYS call this tool FIRST at the start of every task — before reading files, running grep/find/Bash searches, or exploring the codebase manually. Covers coding tasks (bug fix, feature, refactor) and documentation tasks (writing/editing Markdown). Do NOT use Read, Bash, or get_context to locate relevant files — this tool replaces all of that. Returns pivot files and related symbols ranked by relevance. Tracked files with uncommitted changes (`git diff HEAD`: staged or unstaged edits) are ranked first and marked with `git_diff: true` in the response; changes already committed on the branch and new untracked files are not included. IMPORTANT: The 'task' parameter MUST be in English. Translate queries from other languages (e.g. Japanese) to English before calling.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "task": {
                                "type": "string",
                                "description": "One sentence describing what you are about to implement, fix, or write. IMPORTANT: The task description MUST be in English. Translate to English if needed. Examples: 'fix JWT token expiry bug in auth middleware', 'write installation section in README.md'"
                            },
                            "max_tokens": {
                                "type": "integer",
                                "description": "Token budget for returned context. Default: 8000"
                            },
                            "include_tests": {
                                "type": "boolean",
                                "description": "Include test files in results. Default: false"
                            },
                            "include_content": {
                                "type": "boolean",
                                "description": "If true, include compressed file content in each pivot_file entry. Default: false"
                            },
                            "compression_level": {
                                "type": "integer",
                                "enum": [0, 1, 2],
                                "description": "Omit for auto-adjustment (comP picks the lowest level that fits the budget). Set explicitly to disable auto-adjustment: 0=full source, 1=compact (comments removed, ~30% smaller), 2=skeleton (signatures only, ~75% smaller)"
                            }
                        },
                        "required": ["task"]
                    }
                },
                {
                    "name": "get_context",
                    "description": "Search for specific symbols (functions, classes, types) by name or keyword. Use when you know the exact name to look up. Each result carries the symbol's `id`, which get_impact_graph takes. Do NOT use for starting a new task — use run_pipeline instead. IMPORTANT: The 'query' parameter MUST be in English or match exact symbol names.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Symbol name or keyword. IMPORTANT: The query MUST be in English or match exact symbol names in the code. Example: 'authenticate' or 'UserRepository'"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max results to return. Default: 10"
                            },
                            "kind_filter": {
                                "type": "string",
                                "description": "Filter by symbol kind: 'function', 'class', 'type', or 'variable'"
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "get_impact_graph",
                    "description": "Show which files and symbols are affected when a specific symbol changes. Call before modifying a function or class to understand the blast radius. Requires a numeric symbol_id: call get_context with the symbol's name first and take the `id` of the matching result.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol_id": {
                                "type": "integer",
                                "description": "Numeric ID of the symbol to analyze: the `id` of a get_context result"
                            },
                            "symbol_name": {
                                "type": "string",
                                "description": "Human-readable name for display purposes only"
                            },
                            "max_depth": {
                                "type": "integer",
                                "default": 0,
                                "description": "Maximum traversal depth for transitive impact (0 = unlimited, 1 = direct dependents only, N = N hops)"
                            }
                        },
                        "required": ["symbol_id"]
                    }
                },
                {
                    "name": "list_indexed_files",
                    "description": "List all indexed files with symbol counts and language breakdown. Use to understand overall codebase structure. Do NOT use to find relevant files for a task — use run_pipeline for that.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "session_recall",
                    "description": "Recall past MCP tool invocations (queries, symbols, files, tokens) across ALL sessions, newest first, each tagged with its date/time. Survives daemon restarts and session breaks — call this when resuming work to reconstruct what was previously asked and done. Returns a Markdown list with stale status. If query is provided, substring matches come first (newest first), then other records ranked by relevance (BM25 over character bigrams, so Japanese and multi-word queries work) weighted by recency. Turns list the delegations they ran; delegations name the turn that ran them.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Optional search query: case-insensitive substring matches first, then relevance-ranked matches"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max number of past invocations to return (default: 20)"
                            }
                        }
                    }
                },
                {
                    "name": "check_rule_conflicts",
                    "description": "Rule sharing (beta, off unless comp.ruleSharing.enabled is on): list pairs of sections from different agents' instruction files in this repository (CLAUDE.md, AGENTS.md, GEMINI.md, .cursor/rules, ... — git-tracked only) that may contradict. comP only finds candidates; you judge each pair and report real contradictions to the user. Do not edit instruction files unless asked.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "max_pairs": {
                                "type": "integer",
                                "description": "Maximum number of pairs to return (1-100). Default: 20"
                            }
                        }
                    }
                },
                {
                    "name": "session_log",
                    "description": "Record an interaction — a user request and what was done in response — so it can be recalled in later sessions (survives daemon restarts). Call this after completing a task to persist intent and outcome. Stored in .comp/history and indexed so run_pipeline can also surface it.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "request": {
                                "type": "string",
                                "description": "What the user asked for (their request/instruction)"
                            },
                            "outcome": {
                                "type": "string",
                                "description": "What was done in response (summary of the action and result)"
                            },
                            "files": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional list of files touched"
                            }
                        },
                        "required": ["request"]
                    }
                },
                {
                    "name": "get_symbol",
                    "description": "Get source code, outbound dependencies, and inbound dependents for a specific symbol by name. Useful when you need to inspect a function, class, or type definition.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Symbol name (e.g. 'authenticate' or 'MCPServer')"
                            },
                            "file_path": {
                                "type": "string",
                                "description": "Optional file path relative to workspace root to narrow search"
                            },
                            "compression_level": {
                                "type": "integer",
                                "enum": [0, 1, 2],
                                "default": 0,
                                "description": "0=full source (default), 1=compact (comments+blank lines removed, 20-35% smaller), 2=skeleton (signatures only, 50-70% smaller)"
                            }
                        },
                        "required": ["name"]
                    }
                },
                {
                    "name": "get_dependencies",
                    "description": "Retrieve dependencies of a symbol. Direction 'out' returns symbols that this symbol depends on. Direction 'in' returns symbols that depend on this symbol.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Symbol name"
                            },
                            "direction": {
                                "type": "string",
                                "enum": ["in", "out"],
                                "description": "Dependency direction. 'in' for incoming dependencies (dependents), 'out' for outgoing dependencies."
                            }
                        },
                        "required": ["name", "direction"]
                    }
                },
                {
                    "name": "get_file_summary",
                    "description": "List all symbol names, their kinds (function, class, etc.), and signatures inside a specific file. Excludes body content.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "File path relative to workspace root (e.g., 'src/extension.ts')"
                            }
                        },
                        "required": ["file_path"]
                    }
                },
                {
                    "name": "get_project_overview",
                    "description": "Get a high-level summary of the workspace: total file count, symbol count, language distribution, top files by symbol count, and lists of exported symbols.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "get_git_diff_context",
                    "description": "Get context for files changed in a git diff. Useful for PR review or understanding recent changes. Returns a table of changed files with symbol counts.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "base_ref": {
                                "type": "string",
                                "description": "Git ref to diff against. Default: HEAD~1. Use 'main' or 'master' for branch diffs."
                            }
                        }
                    }
                },
                {
                    "name": "compress_file",
                    "description": "Compress a file using AST-based compression. Removes comments (level 1) or extracts signatures only (level 2). Useful for reading large files with fewer tokens. Requires an absolute path within the workspace.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Absolute path to the file within the workspace"
                            },
                            "compression_level": {
                                "type": "integer",
                                "description": "0=full (no-op), 1=compact (comments removed), 2=skeleton (signatures only). Default: 1"
                            }
                        },
                        "required": ["path"]
                    }
                }
            ]
        })
    }
}
