// parser.rs - Tree-sitter based code parser
//
// Responsibilities:
// - Initialize tree-sitter parsers for multiple languages
// - Parse source code and extract symbols using AST traversal

use anyhow::{Result, anyhow};
use tree_sitter::{Parser as TreeSitterParser, TreeCursor, Node};

/// Symbol kind extracted from source code
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    Struct,
    Enum,
    Type,
    Variable,
    Constant,
    Module,
    Namespace,
    Method,
    Property,
    Unknown,
}

impl SymbolKind {
    /// Display name for storing in database
    pub fn as_str(&self) -> &str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Type => "type",
            SymbolKind::Variable => "variable",
            SymbolKind::Constant => "constant",
            SymbolKind::Module => "module",
            SymbolKind::Namespace => "namespace",
            SymbolKind::Method => "method",
            SymbolKind::Property => "property",
            SymbolKind::Unknown => "unknown",
        }
    }
}

/// A symbol extracted from source code
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub signature: Option<String>,
    pub is_exported: bool,
    pub scope: Option<String>,
}

/// Parser for extracting symbols from source code
pub struct CodeParser {
    parser: TreeSitterParser,
}

impl CodeParser {
    /// Create a new parser
    pub fn new() -> Result<Self> {
        let parser = TreeSitterParser::new();
        Ok(CodeParser { parser })
    }

    /// Parse a source file and extract symbols with scope tracking
    pub async fn parse_file(&mut self, language: &str, source_code: &str) -> Result<Vec<Symbol>> {
        let lang: tree_sitter::Language = match language {
            "rust" => tree_sitter_rust::LANGUAGE.into(),
            "typescript" | "tsx" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "javascript" | "js" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
            "python" => tree_sitter_python::LANGUAGE.into(),
            "go" => tree_sitter_go::LANGUAGE.into(),
            "html" | "htm" => tree_sitter_html::LANGUAGE.into(),
            _ => return Ok(Vec::new()), // Fallback for unsupported languages
        };

        self.parser.set_language(&lang)?;

        let tree = self.parser.parse(source_code, None).ok_or_else(|| anyhow!("Failed to parse code"))?;
        let root_node = tree.root_node();

        let mut symbols = Vec::new();
        let mut cursor = root_node.walk();

        self.walk_tree(&mut cursor, source_code, language, &mut symbols, None);

        Ok(symbols)
    }

    fn walk_tree(
        &self,
        cursor: &mut TreeCursor,
        source_code: &str,
        language: &str,
        symbols: &mut Vec<Symbol>,
        current_scope: Option<String>,
    ) {
        let node = cursor.node();
        let mut next_scope = current_scope.clone();

        if let Some(symbol) = self.node_to_symbol(node, source_code, language, &current_scope) {
            next_scope = Some(symbol.name.clone());
            symbols.push(symbol);
        }

        if cursor.goto_first_child() {
            loop {
                self.walk_tree(cursor, source_code, language, symbols, next_scope.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
    }

    fn node_to_symbol(
        &self,
        node: Node,
        source_code: &str,
        language: &str,
        scope: &Option<String>,
    ) -> Option<Symbol> {
        let kind_str = node.kind();
        
        let (symbol_kind, name_node) = match (language, kind_str) {
            // Rust
            ("rust", "function_item") => (SymbolKind::Function, node.child_by_field_name("name")),
            ("rust", "struct_item") => (SymbolKind::Struct, node.child_by_field_name("name")),
            ("rust", "enum_item") => (SymbolKind::Enum, node.child_by_field_name("name")),
            ("rust", "impl_item") => (SymbolKind::Class, node.child_by_field_name("type")),
            ("rust", "trait_item") => (SymbolKind::Interface, node.child_by_field_name("name")),
            
            // TypeScript/JavaScript
            ("typescript" | "tsx" | "javascript" | "js" | "jsx", "function_declaration") => 
                (SymbolKind::Function, node.child_by_field_name("name")),
            ("typescript" | "tsx" | "javascript" | "js" | "jsx", "class_declaration") => 
                (SymbolKind::Class, node.child_by_field_name("name")),
            ("typescript" | "tsx" | "javascript" | "js" | "jsx", "method_definition") => 
                (SymbolKind::Method, node.child_by_field_name("name")),
            ("typescript" | "tsx", "interface_declaration") => 
                (SymbolKind::Interface, node.child_by_field_name("name")),
            ("typescript" | "tsx", "type_alias_declaration") => 
                (SymbolKind::Type, node.child_by_field_name("name")),
                
            // Python
            ("python", "function_definition") => (SymbolKind::Function, node.child_by_field_name("name")),
            ("python", "class_definition") => (SymbolKind::Class, node.child_by_field_name("name")),
            
            // Go
            ("go", "function_declaration") => (SymbolKind::Function, node.child_by_field_name("name")),
            ("go", "method_declaration") => (SymbolKind::Method, node.child_by_field_name("name")),
            ("go", "type_spec") => (SymbolKind::Type, node.child_by_field_name("name")),
            
            // HTML
            ("html" | "htm", "element") => {
                let start_tag = node.child(0);
                if let Some(start) = start_tag {
                    if start.kind() == "start_tag" {
                        let tag_name = start.child(1); 
                        if let Some(tn) = tag_name {
                            if tn.kind() == "tag_name" {
                                return self.create_symbol(node, tn, SymbolKind::Module, source_code, scope);
                            }
                        }
                    }
                }
                return None;
            },
            
            _ => return None,
        };

        if let Some(name_node) = name_node {
            self.create_symbol(node, name_node, symbol_kind, source_code, scope)
        } else {
            None
        }
    }

    fn create_symbol(
        &self,
        node: Node,
        name_node: Node,
        kind: SymbolKind,
        source_code: &str,
        scope: &Option<String>,
    ) -> Option<Symbol> {
        let name = name_node.utf8_text(source_code.as_bytes()).ok()?.to_string();
        
        let start_pos = node.start_position();
        let end_pos = node.end_position();
        
        let node_text = node.utf8_text(source_code.as_bytes()).ok()?;
        let signature = node_text.lines().next().map(|s| s.to_string());
        
        // Find if this node is exported
        let mut is_exported = false;
        
        // 1. Check parent for "export_statement" (TypeScript/JavaScript)
        let mut parent = node.parent();
        while let Some(p) = parent {
            if p.kind() == "export_statement" || p.kind() == "visibility_modifier" {
                is_exported = true;
                break;
            }
            if p.kind() == "program" || p.kind() == "source_file" || p.kind() == "module" {
                break;
            }
            parent = p.parent();
        }

        // 2. Check children for "visibility_modifier" (Rust)
        if !is_exported {
            let mut walk = node.walk();
            for child in node.children(&mut walk) {
                if child.kind() == "visibility_modifier" {
                    is_exported = true;
                    break;
                }
            }
        }

        Some(Symbol {
            name,
            kind,
            line: (start_pos.row + 1) as u32,
            column: (start_pos.column + 1) as u32,
            end_line: (end_pos.row + 1) as u32,
            end_column: (end_pos.column + 1) as u32,
            signature,
            is_exported,
            scope: scope.clone(),
        })
    }
}

impl Default for CodeParser {
    fn default() -> Self {
        Self::new().expect("Failed to create parser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parser_creation() -> Result<()> {
        let _parser = CodeParser::new()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_parse_rust_function() -> Result<()> {
        let mut parser = CodeParser::new()?;
        let code = "fn main() { println!(\"hello\"); }";

        let symbols = parser.parse_file("rust", code).await?;
        assert!(!symbols.is_empty(), "Should extract main function");
        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].kind.as_str(), "function");

        Ok(())
    }

    #[tokio::test]
    async fn test_parse_typescript() -> Result<()> {
        let mut parser = CodeParser::new()?;
        let code = "export function greet(name: string) { return `Hello, ${name}`; }";

        let symbols = parser.parse_file("typescript", code).await?;
        assert!(!symbols.is_empty(), "Should extract greet function");
        assert_eq!(symbols[0].name, "greet");
        assert!(symbols[0].is_exported, "Should be marked as exported");

        Ok(())
    }

    #[tokio::test]
    async fn test_parse_html() -> Result<()> {
        let mut parser = CodeParser::new()?;
        let code = "<div><h1>Hello</h1></div>";

        let symbols = parser.parse_file("html", code).await?;
        assert!(!symbols.is_empty(), "Should extract html elements");
        assert_eq!(symbols[0].name, "div");
        assert_eq!(symbols[1].name, "h1");

        Ok(())
    }
}