// compress.rs - AST-based code compression for LLM context optimization
//
// WHY: run_pipeline returns raw source text which is often 60-80% comments and blank lines.
//      Compressing before returning reduces token cost without losing semantic content.

use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompressionLevel {
    Full = 0,
    Compact = 1,
    Skeleton = 2,
}

impl CompressionLevel {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Compact,
            2 => Self::Skeleton,
            _ => Self::Full,
        }
    }
}

pub fn compress(source: &str, language: &str, level: CompressionLevel) -> String {
    match level {
        CompressionLevel::Full => source.to_string(),
        CompressionLevel::Compact => compact(source, language),
        CompressionLevel::Skeleton => {
            if matches!(language, "md" | "markdown") {
                markdown_compress(source)
            } else {
                skeleton(source, language)
            }
        }
    }
}

fn make_parser(language: &str) -> Option<Parser> {
    let lang: tree_sitter::Language = match language {
        "rs" | "rust" => tree_sitter_rust::LANGUAGE.into(),
        "ts" | "tsx" | "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "js" | "jsx" | "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "py" | "python" => tree_sitter_python::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "html" | "htm" => tree_sitter_html::LANGUAGE.into(),
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" | "cc" | "cxx" | "c++" => tree_sitter_cpp::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    Some(parser)
}

fn compact(source: &str, language: &str) -> String {
    let tree = make_parser(language).and_then(|mut p| p.parse(source, None));
    let tree = match tree {
        Some(t) => t,
        None => return collapse_blank_lines(source),
    };

    let mut comment_ranges: Vec<(usize, usize)> = Vec::new();
    collect_comment_ranges(&tree.root_node(), &mut comment_ranges);
    comment_ranges.sort_by_key(|r| r.0);

    let bytes = source.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut pos = 0usize;
    for (start, end) in &comment_ranges {
        if pos < *start {
            out.extend_from_slice(&bytes[pos..*start]);
        }
        pos = *end;
    }
    if pos < bytes.len() {
        out.extend_from_slice(&bytes[pos..]);
    }

    let without_comments = String::from_utf8_lossy(&out).into_owned();
    collapse_blank_lines(&without_comments)
}

fn collect_comment_ranges(node: &Node, out: &mut Vec<(usize, usize)>) {
    if is_comment(node.kind()) {
        out.push((node.start_byte(), node.end_byte()));
        return;
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        collect_comment_ranges(&child, out);
    }
}

fn is_comment(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment" | "comment" | "doc_comment")
}

fn collapse_blank_lines(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut prev_blank = false;
    for line in src.lines() {
        if line.trim().is_empty() {
            if !prev_blank {
                out.push('\n');
            }
            prev_blank = true;
        } else {
            out.push_str(line);
            out.push('\n');
            prev_blank = false;
        }
    }
    out.trim_end().to_string()
}

fn skeleton(source: &str, language: &str) -> String {
    let tree = match make_parser(language).and_then(|mut p| p.parse(source, None)) {
        Some(t) => t,
        None => return source.lines().next().unwrap_or("").to_string(),
    };
    let bytes = source.as_bytes();
    let mut out = String::new();
    emit_children(&tree.root_node(), bytes, language, &mut out);
    out.trim_end().to_string()
}

fn markdown_compress(source: &str) -> String {
    let mut kept_lines = Vec::new();
    let mut in_code_block = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            kept_lines.push(line);
            continue;
        }

        if in_code_block {
            kept_lines.push(line);
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        let is_keep = trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || trimmed.starts_with("> ")
            || trimmed == "---"
            || trimmed == "***"
            || trimmed == "___"
            || trimmed.chars().next().is_some_and(|c| c.is_numeric());

        if is_keep {
            kept_lines.push(line);
        }
    }

    let out = kept_lines.join("\n");
    collapse_blank_lines(&out)
}

fn emit_children(node: &Node, bytes: &[u8], lang: &str, out: &mut String) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        emit_node(&child, bytes, lang, out);
    }
}

fn emit_node(node: &Node, bytes: &[u8], lang: &str, out: &mut String) {
    if !node.is_named() || is_comment(node.kind()) {
        return;
    }
    if is_container(node.kind()) {
        let body_kinds = body_kinds_for(lang);
        let mut cur = node.walk();
        let body = node.children(&mut cur).find(|c| body_kinds.contains(&c.kind()));
        if let Some(body) = body {
            if let Ok(sig) = std::str::from_utf8(&bytes[node.start_byte()..body.start_byte()]) {
                let sig = sig.trim_end();
                if lang == "py" || lang == "python" {
                    out.push_str(sig);
                    out.push_str(" ...\n");
                } else {
                    out.push_str(sig);
                    out.push_str(" { ... }\n");
                }
            }
        } else if let Ok(text) = std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()]) {
            out.push_str(text.lines().next().unwrap_or(""));
            out.push('\n');
        }
    } else if let Ok(text) = std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()]) {
        let t = text.trim_end();
        if !t.is_empty() {
            out.push_str(t);
            out.push('\n');
        }
    }
}

fn is_container(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "impl_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "mod_item"
            | "function_declaration"
            | "class_declaration"
            | "method_definition"
            | "function_expression"
            | "function_definition"
            | "class_definition"
            | "method_declaration"
    )
}

fn body_kinds_for(lang: &str) -> &'static [&'static str] {
    match lang {
        "rs" | "rust" => &[
            "block",
            "declaration_list",
            "field_declaration_list",
            "enum_variant_list",
        ],
        "ts" | "tsx" | "typescript" | "js" | "jsx" | "javascript" => {
            &["statement_block", "class_body"]
        }
        "py" | "python" => &["block"],
        "go" => &["block"],
        _ => &["block"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_is_noop() {
        let src = "fn foo() { let x = 1; }";
        assert_eq!(compress(src, "rust", CompressionLevel::Full), src);
    }

    #[test]
    fn test_compact_rust_removes_line_comment() {
        let src = "// top comment\nfn foo() {\n    // inner\n    let x = 1;\n}\n";
        let result = compress(src, "rust", CompressionLevel::Compact);
        assert!(!result.contains("top comment"), "line comment should be removed");
        assert!(!result.contains("inner"), "inner comment should be removed");
        assert!(result.contains("fn foo()"));
        assert!(result.contains("let x = 1;"));
    }

    #[test]
    fn test_compact_rust_removes_block_comment() {
        let src = "/* block */\nfn foo() {}\n";
        let result = compress(src, "rust", CompressionLevel::Compact);
        assert!(!result.contains("block"), "block comment should be removed");
        assert!(result.contains("fn foo()"));
    }

    #[test]
    fn test_compact_collapses_blank_lines() {
        let src = "fn a() {}\n\n\n\nfn b() {}\n";
        let result = compress(src, "rust", CompressionLevel::Compact);
        assert!(!result.contains("\n\n\n"), "3+ blank lines should collapse to 1");
    }

    #[test]
    fn test_skeleton_rust_function() {
        let src = "pub fn calc(x: i32) -> i32 {\n    let y = x * 2;\n    y + 1\n}\n";
        let result = compress(src, "rust", CompressionLevel::Skeleton);
        assert!(result.contains("pub fn calc(x: i32) -> i32"));
        assert!(result.contains("{ ... }"));
        assert!(!result.contains("let y = x * 2;"), "body should be replaced");
    }

    #[test]
    fn test_skeleton_typescript_function() {
        let src = "function greet(name: string): string {\n    return `Hello ${name}`;\n}\n";
        let result = compress(src, "typescript", CompressionLevel::Skeleton);
        assert!(result.contains("greet(name: string)"));
        assert!(result.contains("{ ... }"));
        assert!(!result.contains("Hello"), "body should be replaced");
    }

    #[test]
    fn test_skeleton_python_function() {
        let src = "def add(a, b):\n    return a + b\n";
        let result = compress(src, "python", CompressionLevel::Skeleton);
        assert!(result.contains("def add(a, b)"));
        assert!(result.contains("..."));
        assert!(!result.contains("return a + b"), "body should be replaced");
    }

    #[test]
    fn test_compression_level_from_i64() {
        assert_eq!(CompressionLevel::from_i64(0), CompressionLevel::Full);
        assert_eq!(CompressionLevel::from_i64(1), CompressionLevel::Compact);
        assert_eq!(CompressionLevel::from_i64(2), CompressionLevel::Skeleton);
        assert_eq!(CompressionLevel::from_i64(99), CompressionLevel::Full);
    }

    #[test]
    fn test_unknown_language_fallback() {
        let src = "SELECT * FROM users WHERE id = 1;";
        let result = compress(src, "sql", CompressionLevel::Compact);
        assert!(!result.is_empty(), "unknown language should return something");
    }
}
