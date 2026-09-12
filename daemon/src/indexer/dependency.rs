// dependency.rs - Dependency analysis for extracted symbols
//
// Responsibilities:
// - Extract import/require statements from source code
// - Map imported module names to symbol node IDs
// - Detect symbol references within the code
// - Record edges in the dependency graph for:
//   - import/require (module dependencies)
//   - function calls (function dependencies)
//   - type references (type dependencies)

use anyhow::Result;
use std::collections::HashMap;

/// Dependency edge type
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum EdgeKind {
    /// import/require statement (file-level dependency)
    Import,
    /// Function call (symbol references function)
    FunctionCall,
    /// Type usage (symbol uses a type/interface)
    TypeReference,
    /// Inheritance (class extends/implements another)
    Inheritance,
}

impl EdgeKind {
    /// Convert to string for database storage
    pub fn as_str(&self) -> &str {
        match self {
            EdgeKind::Import => "import",
            EdgeKind::FunctionCall => "function_call",
            EdgeKind::TypeReference => "type_reference",
            EdgeKind::Inheritance => "inheritance",
        }
    }
}

/// Dependency extraction result
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Dependency {
    /// Source symbol name (or file if module-level)
    pub from: String,
    /// Target symbol name or module name
    pub to: String,
    /// Type of dependency
    pub kind: EdgeKind,
    /// Line number where dependency occurs
    pub line: u32,
}

/// Dependency analyzer
pub struct DependencyAnalyzer;

impl DependencyAnalyzer {
    /// Extract dependencies from source code
    ///
    /// # Arguments
    /// - language: Programming language (rust, typescript, python, etc.)
    /// - source_code: File content
    /// - file_path: Path for module-level imports
    ///
    /// # Returns
    /// - Vec<Dependency>: All dependencies found
    ///
    /// # Process
    /// 1. Parse language-specific import statements
    /// 2. Extract module names and imported symbols
    /// 3. Detect symbol references (function calls, type usage, etc.)
    /// 4. Build dependency list with line numbers
    pub fn extract_dependencies(
        language: &str,
        source_code: &str,
        _file_path: &str,
    ) -> Result<Vec<Dependency>> {
        match language {
            "rust" => Self::extract_rust_dependencies(source_code),
            "typescript" | "javascript" => Self::extract_typescript_dependencies(source_code),
            "python" => Self::extract_python_dependencies(source_code),
            "go" => Self::extract_go_dependencies(source_code),
            _ => Ok(Vec::new()),
        }
    }

    /// Extract Rust dependencies (use statements and function calls)
    ///
    /// # Process:
    /// 1. Parse `use` statements to extract imports
    /// 2. Detect function/method calls (module::function or obj.method)
    /// 3. Detect type usage (variable: Type, Function<T>)
    fn extract_rust_dependencies(source_code: &str) -> Result<Vec<Dependency>> {
        use regex::Regex;

        let mut deps = Vec::new();

        // Pattern 1: Extract `use` statements
        // Matches: use std::collections::HashMap;
        //          use crate::graph::GraphDB;
        let use_pattern = Regex::new(r"^\s*use\s+([\w:]+)(?::\:\{[^}]+\})?;?")?;

        // Pattern 2: Extract associated/function calls (module::function)
        // Matches: GraphDB::new()
        let call_pattern = Regex::new(r"([\w_]+)::([\w_]+)\s*\(")?;

        // Pattern 3: Generic call target — `name(` and `obj.method(`.
        // WHY: The callee name is what we resolve against the global symbol index,
        // so we only need the identifier immediately preceding the `(`.
        let generic_call = Regex::new(r"([A-Za-z_][\w_]*)\s*\(")?;

        for (line_num, line) in source_code.lines().enumerate() {
            let line_no = (line_num + 1) as u32;
            let trimmed = line.trim_start();

            // Extract imports from use statements
            if let Some(caps) = use_pattern.captures(line) {
                if let Some(module) = caps.get(1) {
                    let module_name = module.as_str();
                    // Extract last component (Symbol) from path
                    if let Some(last) = module_name.split("::").last() {
                        deps.push(Dependency {
                            from: "module".to_string(), // File-level import
                            to: last.to_string(),
                            kind: EdgeKind::Import,
                            line: line_no,
                        });
                    }
                }
            }

            // Extract function calls (Type::assoc())
            for caps in call_pattern.captures_iter(line) {
                if let Some(func) = caps.get(2) {
                    deps.push(Dependency {
                        from: "call".to_string(),
                        to: func.as_str().to_string(),
                        kind: EdgeKind::FunctionCall,
                        line: line_no,
                    });
                }
            }

            // Generic calls — skip definition lines so `fn foo()` is not read as a
            // call to `foo`, and skip control-flow keywords like `if (`/`while (`.
            if !is_def_line("rust", trimmed) {
                for caps in generic_call.captures_iter(line) {
                    let name = caps.get(1).unwrap().as_str();
                    if !is_call_keyword("rust", name) {
                        deps.push(Dependency {
                            from: "call".to_string(),
                            to: name.to_string(),
                            kind: EdgeKind::FunctionCall,
                            line: line_no,
                        });
                    }
                }
            }
        }

        Ok(deps)
    }

    /// Extract TypeScript/JavaScript dependencies (import statements and function calls)
    ///
    /// # Process:
    /// 1. Parse import/require statements
    /// 2. Detect function/method calls
    /// 3. Detect type references in type annotations
    fn extract_typescript_dependencies(source_code: &str) -> Result<Vec<Dependency>> {
        use regex::Regex;

        let mut deps = Vec::new();

        // Pattern 1: import { Symbol } from 'module'
        let import_named = Regex::new(r#"import\s+\{\s*([^}]+)\s*\}\s+from\s+['"]([^'"]+)['"]"#)?;

        // Pattern 2: import * as name from 'module'
        let import_star = Regex::new(r#"import\s+\*\s+as\s+(\w+)\s+from\s+['"]([^'"]+)['"]"#)?;

        // Pattern 3: const/var name = require('module')
        let require_pattern = Regex::new(r#"(?:const|var|let)\s+(\w+)\s*=\s*require\(['"](.*?)['"]\)"#)?;

        // Pattern 4: function/method calls (obj.method or Class.static)
        let call_pattern = Regex::new(r"([\w_]+)\.([\w_]+)\s*\(")?;

        // Pattern 5: constructor / type reference (new ClassName())
        let new_pattern = Regex::new(r"new\s+([A-Za-z_][\w_]*)\s*\(")?;

        // Pattern 6: bare function calls (foo())
        let generic_call = Regex::new(r"([A-Za-z_][\w_]*)\s*\(")?;

        for (line_num, line) in source_code.lines().enumerate() {
            let line_no = (line_num + 1) as u32;
            let trimmed = line.trim_start();

            // Extract named imports: import { Symbol } from 'module'
            if let Some(caps) = import_named.captures(line) {
                if let Some(symbols) = caps.get(1) {
                    for symbol in symbols.as_str().split(',') {
                        let sym = symbol.trim();
                        deps.push(Dependency {
                            from: "module".to_string(),
                            to: sym.to_string(),
                            kind: EdgeKind::Import,
                            line: line_no,
                        });
                    }
                }
            }

            // Extract star imports: import * as name
            if let Some(caps) = import_star.captures(line) {
                if let Some(name) = caps.get(1) {
                    deps.push(Dependency {
                        from: "module".to_string(),
                        to: name.as_str().to_string(),
                        kind: EdgeKind::Import,
                        line: line_no,
                    });
                }
            }

            // Extract requires: const name = require('module')
            if let Some(caps) = require_pattern.captures(line) {
                if let Some(name) = caps.get(1) {
                    deps.push(Dependency {
                        from: "module".to_string(),
                        to: name.as_str().to_string(),
                        kind: EdgeKind::Import,
                        line: line_no,
                    });
                }
            }

            // Extract method calls (obj.method())
            for caps in call_pattern.captures_iter(line) {
                if let Some(method) = caps.get(2) {
                    deps.push(Dependency {
                        from: "call".to_string(),
                        to: method.as_str().to_string(),
                        kind: EdgeKind::FunctionCall,
                        line: line_no,
                    });
                }
            }

            // Extract constructor / type references (new ClassName())
            for caps in new_pattern.captures_iter(line) {
                if let Some(class) = caps.get(1) {
                    deps.push(Dependency {
                        from: "call".to_string(),
                        to: class.as_str().to_string(),
                        kind: EdgeKind::TypeReference,
                        line: line_no,
                    });
                }
            }

            // Extract bare calls (foo()), skipping definitions and keywords
            if !is_def_line("typescript", trimmed) {
                for caps in generic_call.captures_iter(line) {
                    let name = caps.get(1).unwrap().as_str();
                    if name != "new" && !is_call_keyword("typescript", name) {
                        deps.push(Dependency {
                            from: "call".to_string(),
                            to: name.to_string(),
                            kind: EdgeKind::FunctionCall,
                            line: line_no,
                        });
                    }
                }
            }
        }

        Ok(deps)
    }

    /// Extract Python dependencies (imports, function and method calls)
    ///
    /// # Process:
    /// 1. `from module import A, B` → import deps for A and B
    /// 2. Function/method calls `foo()` and `obj.method()` → call deps
    ///    (definition lines and control-flow keywords are skipped)
    fn extract_python_dependencies(source_code: &str) -> Result<Vec<Dependency>> {
        use regex::Regex;

        let mut deps = Vec::new();

        // from module import A, B as C
        let from_import = Regex::new(r"^\s*from\s+[\w.]+\s+import\s+(.+)$")?;
        // Generic call target: captures the identifier before `(` (covers obj.method())
        let generic_call = Regex::new(r"([A-Za-z_][\w_]*)\s*\(")?;

        for (line_num, line) in source_code.lines().enumerate() {
            let line_no = (line_num + 1) as u32;
            let trimmed = line.trim_start();

            // from ... import Symbol[, Symbol2]
            if let Some(caps) = from_import.captures(line) {
                if let Some(list) = caps.get(1) {
                    for raw in list.as_str().split(',') {
                        // strip "as alias", parentheses and whitespace
                        let name = raw
                            .split(" as ")
                            .next()
                            .unwrap_or("")
                            .trim()
                            .trim_matches(|c| c == '(' || c == ')')
                            .trim();
                        if !name.is_empty() && name != "*" {
                            deps.push(Dependency {
                                from: "module".to_string(),
                                to: name.to_string(),
                                kind: EdgeKind::Import,
                                line: line_no,
                            });
                        }
                    }
                }
                continue;
            }

            // `import module` lines carry module names, not symbols — skip.
            if trimmed.starts_with("import ") {
                continue;
            }

            // Skip definition lines so `def foo(` / `class Foo(` are not read as calls.
            if is_def_line("python", trimmed) {
                continue;
            }

            for caps in generic_call.captures_iter(line) {
                let name = caps.get(1).unwrap().as_str();
                if !is_call_keyword("python", name) {
                    deps.push(Dependency {
                        from: "call".to_string(),
                        to: name.to_string(),
                        kind: EdgeKind::FunctionCall,
                        line: line_no,
                    });
                }
            }
        }

        Ok(deps)
    }

    /// Extract Go dependencies (function and package calls)
    ///
    /// # Process:
    /// - `pkg.Func()` and `Func()` calls → call deps (captures the callee name)
    /// - definition lines (`func ...`) and control-flow keywords are skipped
    fn extract_go_dependencies(source_code: &str) -> Result<Vec<Dependency>> {
        use regex::Regex;

        let mut deps = Vec::new();
        let generic_call = Regex::new(r"([A-Za-z_][\w_]*)\s*\(")?;

        for (line_num, line) in source_code.lines().enumerate() {
            let line_no = (line_num + 1) as u32;
            let trimmed = line.trim_start();

            if is_def_line("go", trimmed) {
                continue;
            }

            for caps in generic_call.captures_iter(line) {
                let name = caps.get(1).unwrap().as_str();
                if !is_call_keyword("go", name) {
                    deps.push(Dependency {
                        from: "call".to_string(),
                        to: name.to_string(),
                        kind: EdgeKind::FunctionCall,
                        line: line_no,
                    });
                }
            }
        }

        Ok(deps)
    }

    /// Resolve dependencies to node IDs
    ///
    /// # Arguments
    /// - deps: Raw dependencies extracted from code
    /// - symbol_map: Mapping of symbol names to node IDs (current file)
    /// - imported_symbols: Mapping of imported modules to their exported symbols
    ///
    /// # Returns
    /// - Vec<(from_node_id, to_node_id, edge_kind)>: Resolved node pairs for edge creation
    ///
    /// # Process:
    /// 1. For each dependency, look up source node ID in symbol_map
    /// 2. For each dependency, look up target in imported_symbols or symbol_map
    /// 3. Return pairs that can be inserted as edges
    // Retained as the same-file resolver; the indexer uses resolve_global.
    #[allow(dead_code)]
    pub fn resolve_dependencies(
        deps: &[Dependency],
        symbol_map: &HashMap<String, i64>,
        _imported_symbols: &HashMap<String, HashMap<String, i64>>,
    ) -> Vec<(i64, i64, String)> {
        // TODO: Implement node ID resolution
        // - Match symbol names to node IDs from symbol_map
        // - Handle module.symbol notation by looking up in imported_symbols
        // - Return only resolvable dependencies

        let mut edges = Vec::new();

        for dep in deps {
            // Try to find source node ID
            if let Some(&from_id) = symbol_map.get(&dep.from) {
                // Try to find target node ID
                // For now, just check symbol_map (same-file references)
                if let Some(&to_id) = symbol_map.get(&dep.to) {
                    edges.push((from_id, to_id, dep.kind.as_str().to_string()));
                }
            }
        }

        edges
    }

    /// Resolve dependencies into edges using a cross-file global symbol index.
    ///
    /// This is the resolver used by the indexer (the older `resolve_dependencies`
    /// only handles same-file references).
    ///
    /// # Arguments
    /// - deps: Raw dependencies extracted from one file's source
    /// - local_nodes_sorted: `(node_id, name, line)` for this file, ascending by line
    /// - global_index: `name -> [(node_id, file_id, is_exported)]` across the whole repo
    /// - file_id: the file currently being resolved (to prefer/penalise self-file targets)
    ///
    /// # Resolution rules
    /// - **from** = the enclosing symbol, approximated as the nearest preceding
    ///   declaration on or above the dependency line (the schema has no `end_line`,
    ///   so an exact range test is impossible). Deps with no preceding symbol
    ///   (e.g. top-level imports) are skipped.
    /// - **to** = a same-file symbol of that name if present; otherwise a symbol
    ///   from the global index. Cross-file targets are accepted only when a single
    ///   exported (or, failing that, a single total) candidate exists — ambiguous
    ///   names are skipped to avoid false edges (precision over recall).
    /// - Self-edges (`from == to`) are dropped.
    pub fn resolve_global(
        deps: &[Dependency],
        local_nodes_sorted: &[(i64, String, i32)],
        global_index: &crate::graph::GlobalSymbolIndex,
        file_id: i64,
    ) -> Vec<(i64, i64, String)> {
        let mut edges = Vec::new();

        for dep in deps {
            let Some(from_id) = enclosing_symbol(local_nodes_sorted, dep.line) else {
                continue;
            };

            let Some(to_id) =
                resolve_target(&dep.to, from_id, local_nodes_sorted, global_index, file_id)
            else {
                continue;
            };

            if to_id != from_id {
                edges.push((from_id, to_id, dep.kind.as_str().to_string()));
            }
        }

        edges
    }
}

/// Nearest preceding declaration on or above `line` (enclosing-symbol heuristic).
fn enclosing_symbol(local_nodes_sorted: &[(i64, String, i32)], line: u32) -> Option<i64> {
    let line = line as i32;
    let mut best: Option<(i64, i32)> = None;
    for (nid, _name, nline) in local_nodes_sorted {
        if *nline <= line {
            match best {
                Some((_, bl)) if *nline <= bl => {}
                _ => best = Some((*nid, *nline)),
            }
        }
    }
    best.map(|(id, _)| id)
}

/// Resolve a callee/target name to a node id, preferring same-file then a
/// unambiguous cross-file (exported) symbol.
fn resolve_target(
    name: &str,
    from_id: i64,
    local_nodes_sorted: &[(i64, String, i32)],
    global_index: &crate::graph::GlobalSymbolIndex,
    file_id: i64,
) -> Option<i64> {
    // 1. Same-file symbol of this name (excluding the caller itself).
    if let Some((nid, ..)) = local_nodes_sorted
        .iter()
        .find(|(nid, n, _)| n == name && *nid != from_id)
    {
        return Some(*nid);
    }

    // 2. Cross-file: accept only an unambiguous candidate.
    let candidates = global_index.get(name)?;
    let other: Vec<&(i64, i64, bool)> =
        candidates.iter().filter(|(_, fid, _)| *fid != file_id).collect();

    let exported: Vec<&&(i64, i64, bool)> = other.iter().filter(|(_, _, e)| *e).collect();
    if exported.len() == 1 {
        return Some(exported[0].0);
    }
    if exported.is_empty() && other.len() == 1 {
        return Some(other[0].0);
    }

    None
}

/// True when `trimmed` line declares a symbol (so its name is not a call target).
fn is_def_line(language: &str, trimmed: &str) -> bool {
    match language {
        "rust" => {
            trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub async fn ")
        }
        "typescript" | "javascript" => {
            trimmed.starts_with("function ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("export async function ")
        }
        "python" => {
            trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("class ")
        }
        "go" => trimmed.starts_with("func "),
        _ => false,
    }
}

/// True when `name` is a control-flow keyword that may precede `(` but is not a call.
fn is_call_keyword(language: &str, name: &str) -> bool {
    const COMMON: &[&str] = &["if", "for", "while", "switch", "return", "catch"];
    if COMMON.contains(&name) {
        return true;
    }
    match language {
        "rust" => matches!(name, "match" | "fn" | "let" | "loop" | "impl" | "mut"),
        "typescript" | "javascript" => {
            matches!(name, "function" | "await" | "typeof" | "super" | "do" | "else")
        }
        "python" => matches!(
            name,
            "elif" | "with" | "assert" | "del" | "raise" | "except" | "lambda" | "yield" | "print"
        ),
        "go" => matches!(name, "func" | "go" | "defer" | "select" | "range"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rust_dependencies() {
        let code = r#"
use std::collections::HashMap;
use crate::graph::GraphDB;

fn main() {
    let map = HashMap::new();
    let db = GraphDB::new();
}
"#;
        let deps = DependencyAnalyzer::extract_rust_dependencies(code).unwrap();

        // Should extract imports and function calls
        assert!(!deps.is_empty(), "Should extract dependencies");

        // Verify at least some imports are found
        let imports: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::Import)).collect();
        assert!(!imports.is_empty(), "Should find import statements");

        // Verify at least some function calls are found
        let calls: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::FunctionCall)).collect();
        assert!(!calls.is_empty(), "Should find function calls");
    }

    #[test]
    fn test_extract_typescript_dependencies() {
        let code = r#"
import { GraphDB } from './graph';
import * as fs from 'fs';

const db = new GraphDB();
fs.readFile('test.txt', () => {});
"#;
        let deps = DependencyAnalyzer::extract_typescript_dependencies(code).unwrap();

        // Should extract imports and function calls
        assert!(!deps.is_empty(), "Should extract dependencies");

        // Verify imports are found
        let imports: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::Import)).collect();
        assert!(!imports.is_empty(), "Should find import statements");

        // Verify function calls are found
        let calls: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::FunctionCall)).collect();
        assert!(!calls.is_empty(), "Should find method calls");
    }

    #[test]
    fn test_resolve_dependencies() {
        let mut symbol_map = HashMap::new();
        symbol_map.insert("main".to_string(), 1);
        symbol_map.insert("helper".to_string(), 2);

        let deps = vec![Dependency {
            from: "main".to_string(),
            to: "helper".to_string(),
            kind: EdgeKind::FunctionCall,
            line: 5,
        }];

        let edges = DependencyAnalyzer::resolve_dependencies(&deps, &symbol_map, &HashMap::new());

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0], (1, 2, "function_call".to_string()));
    }

    #[test]
    fn test_resolve_dependencies_unresolved() {
        let symbol_map = HashMap::new(); // Empty map

        let deps = vec![Dependency {
            from: "main".to_string(),
            to: "unknown".to_string(),
            kind: EdgeKind::FunctionCall,
            line: 5,
        }];

        let edges = DependencyAnalyzer::resolve_dependencies(&deps, &symbol_map, &HashMap::new());

        // Unresolved dependencies should be skipped
        assert_eq!(edges.len(), 0);
    }

    #[test]
    fn test_extract_python_dependencies() {
        let code = r#"
from utils import helper, other
import os

def main():
    helper()
    obj.process()
"#;
        let deps = DependencyAnalyzer::extract_python_dependencies(code).unwrap();

        let imports: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::Import)).collect();
        assert!(imports.iter().any(|d| d.to == "helper"), "from-import symbol expected");
        assert!(imports.iter().any(|d| d.to == "other"), "second from-import symbol expected");

        let calls: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::FunctionCall)).collect();
        assert!(calls.iter().any(|d| d.to == "helper"), "call to helper expected");
        assert!(calls.iter().any(|d| d.to == "process"), "method call expected");
        // `def main(` must not be treated as a call to `main`
        assert!(!calls.iter().any(|d| d.to == "main"), "definition line must not be a call");
    }

    #[test]
    fn test_extract_go_dependencies() {
        let code = r#"
func main() {
    fmt.Println("hi")
    helper()
}
"#;
        let deps = DependencyAnalyzer::extract_go_dependencies(code).unwrap();
        let calls: Vec<_> = deps.iter().filter(|d| matches!(d.kind, EdgeKind::FunctionCall)).collect();
        assert!(calls.iter().any(|d| d.to == "Println"), "package call expected");
        assert!(calls.iter().any(|d| d.to == "helper"), "bare call expected");
        assert!(!calls.iter().any(|d| d.to == "main"), "func def must not be a call");
    }

    #[test]
    fn test_resolve_global_same_file() {
        // main() at line 1 calls helper() at line 2 (line 5 is inside main's body)
        let local = vec![(1i64, "main".to_string(), 1i32), (2i64, "helper".to_string(), 10i32)];
        let deps = vec![Dependency {
            from: "call".to_string(),
            to: "helper".to_string(),
            kind: EdgeKind::FunctionCall,
            line: 5,
        }];
        let global = HashMap::new();

        let edges = DependencyAnalyzer::resolve_global(&deps, &local, &global, 1);
        assert_eq!(edges, vec![(1, 2, "function_call".to_string())]);
    }

    #[test]
    fn test_resolve_global_cross_file() {
        // Caller file (file_id=1) has `caller` at line 1; callee `target` lives in file 2.
        let local = vec![(10i64, "caller".to_string(), 1i32)];
        let deps = vec![Dependency {
            from: "call".to_string(),
            to: "target".to_string(),
            kind: EdgeKind::FunctionCall,
            line: 3,
        }];
        let mut global = HashMap::new();
        global.insert("target".to_string(), vec![(99i64, 2i64, true)]);

        let edges = DependencyAnalyzer::resolve_global(&deps, &local, &global, 1);
        assert_eq!(edges, vec![(10, 99, "function_call".to_string())]);
    }

    #[test]
    fn test_resolve_global_skips_ambiguous_and_self() {
        let local = vec![(10i64, "caller".to_string(), 1i32)];
        // Two exported `target` definitions in other files → ambiguous → no edge.
        let mut global = HashMap::new();
        global.insert("target".to_string(), vec![(99i64, 2i64, true), (98i64, 3i64, true)]);
        let ambiguous = vec![Dependency {
            from: "call".to_string(),
            to: "target".to_string(),
            kind: EdgeKind::FunctionCall,
            line: 3,
        }];
        assert!(DependencyAnalyzer::resolve_global(&ambiguous, &local, &global, 1).is_empty());

        // Self-reference (caller calls itself) must not create an edge.
        let self_call = vec![Dependency {
            from: "call".to_string(),
            to: "caller".to_string(),
            kind: EdgeKind::FunctionCall,
            line: 3,
        }];
        assert!(DependencyAnalyzer::resolve_global(&self_call, &local, &HashMap::new(), 1).is_empty());
    }
}
