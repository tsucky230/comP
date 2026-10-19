// doc_parser.rs - Parser for document formats (JSON, XML, Markdown, JSONL, Parquet)
//
// Responsibilities:
// - Parse JSON/JSONL files and extract structure
// - Parse XML files and extract tags/attributes
// - Parse Markdown and extract headings/sections
// - Parse Parquet files and extract schema fields
// - Create pseudo-symbols for document elements

use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::fs::File;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::io::Read;
// use parquet::file::reader::{FileReader, SerializedFileReader};
use lopdf::content::{Content, Operation};

use super::parser::Symbol;

/// Document parser for non-code formats
pub struct DocumentParser;

impl DocumentParser {
    /// Parse JSON and extract structure as pseudo-symbols
    pub fn parse_json(content: &str) -> Result<Vec<Symbol>> {
        let parsed: Value = serde_json::from_str(content).unwrap_or(Value::Null);
        let mut symbols = Vec::new();

        if let Some(obj) = parsed.as_object() {
            for (key, _) in obj {
                // Determine line number roughly by finding the key in the string
                let line_num = content.lines().position(|l| l.contains(&format!("\"{}\"", key)))
                    .map(|idx| (idx + 1) as u32)
                    .unwrap_or(1);
                
                symbols.push(Symbol {
                    name: key.clone(),
                    kind: super::parser::SymbolKind::Property,
                    line: line_num,
                    column: 1,
                    end_line: line_num,
                    end_column: key.len() as u32 + 1,
                    signature: None,
                    is_exported: true,
                    scope: None,
                });
            }
        }
        
        Ok(symbols)
    }

    /// Parse JSONL (JSON Lines) and extract structure
    pub fn parse_jsonl(content: &str) -> Result<Vec<Symbol>> {
        let mut symbols = Vec::new();
        
        if let Some(first_line) = content.lines().find(|l| !l.trim().is_empty()) {
            if let Ok(Value::Object(obj)) = serde_json::from_str(first_line) {
                for (key, _) in obj {
                    symbols.push(Symbol {
                        name: key.clone(),
                        kind: super::parser::SymbolKind::Property,
                        line: 1,
                        column: 1,
                        end_line: 1,
                        end_column: key.len() as u32 + 1,
                        signature: None,
                        is_exported: true,
                        scope: None,
                    });
                }
            }
        }
        
        Ok(symbols)
    }

    /// Parse XML and extract tags
    pub fn parse_xml(content: &str) -> Result<Vec<Symbol>> {
        let mut reader = Reader::from_str(content);
        reader.config_mut().trim_text(true);

        let mut symbols = Vec::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    
                    if !symbols.iter().any(|s: &Symbol| s.name == name) {
                        let pos = reader.buffer_position() as usize;
                        let line_num = content[..pos].lines().count() as u32;

                        symbols.push(Symbol {
                            name,
                            kind: super::parser::SymbolKind::Module,
                            line: line_num.max(1),
                            column: 1,
                            end_line: line_num.max(1),
                            end_column: 1,
                            signature: None,
                            is_exported: true,
                            scope: None,
                        });
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break, // Skip errors to gracefully handle partial/invalid XML
                _ => {}
            }
            buf.clear();
        }
        
        Ok(symbols)
    }

    /// Parse Markdown and extract structure
    pub fn parse_markdown(content: &str) -> Result<Vec<Symbol>> {
        let mut symbols = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            let line_num = (i + 1) as u32;
            let level = line.len() - line.trim_start_matches('#').len();
            if level > 0 && level <= 6 {
                let heading = line.trim_start_matches('#').trim();
                if !heading.is_empty() {
                    // WHY: First non-empty body line after heading gives agents a preview of section
                    // content, improving BM25 relevance and get_file_summary readability.
                    let signature = lines[i + 1..]
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .and_then(|l| {
                            let trimmed = l.trim();
                            if trimmed.starts_with('#') {
                                None // next heading immediately follows — no body preview
                            } else {
                                let s = if trimmed.chars().count() > 100 {
                                    format!("{}…", trimmed.chars().take(100).collect::<String>())
                                } else {
                                    trimmed.to_string()
                                };
                                Some(s)
                            }
                        });

                    symbols.push(Symbol {
                        name: heading.to_string(),
                        kind: super::parser::SymbolKind::Module,
                        line: line_num,
                        column: (level + 1) as u32,
                        end_line: line_num,
                        end_column: (heading.len() + level + 1) as u32,
                        signature,
                        is_exported: true,
                        scope: None,
                    });
                }
            }
        }

        Ok(symbols)
    }

    /// Parse Parquet and extract schema fields as symbols
    pub fn parse_parquet(path: &Path) -> Result<Vec<Symbol>> {
        let text = Self::extract_parquet_text(path)?;
        let sig = if text.is_empty() {
            None
        } else {
            let text_trimmed = text.trim();
            let mut preview = text_trimmed.chars().take(200).collect::<String>();
            if text_trimmed.chars().count() > 200 {
                preview.push_str("...");
            }
            Some(preview)
        };

        let mut symbols = vec![Symbol {
            name: "Parquet Schema".to_string(),
            kind: super::parser::SymbolKind::Module,
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 1,
            signature: sig,
            is_exported: true,
            scope: None,
        }];

        let mut file = File::open(path)?;
        let metadata = parquet2::read::read_metadata(&mut file).map_err(|e| anyhow::anyhow!("{:?}", e))?;

        for field in metadata.schema().fields() {
            symbols.push(Symbol {
                name: field.name().to_string(),
                kind: super::parser::SymbolKind::Property,
                line: 1,
                column: 1,
                end_line: 1,
                end_column: 1,
                signature: Some(format!("{:?}", field)),
                is_exported: true,
                scope: None,
            });
        }

        Ok(symbols)
    }

    /// Extract plain text from a Word (.docx) file
    pub fn extract_docx_text(path: &Path) -> Result<String> {
        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut doc_file = archive.by_name("word/document.xml")?;
        let mut xml_content = String::new();
        doc_file.read_to_string(&mut xml_content)?;

        let mut reader = Reader::from_str(&xml_content);
        let mut buf = Vec::new();
        let mut text = String::new();
        let mut in_text = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"w:t" => {
                    in_text = true;
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"w:t" => {
                    in_text = false;
                }
                Ok(Event::Text(ref e)) if in_text => {
                        text.push_str(&e.decode()?);
                        text.push(' ');
                    }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        Ok(text)
    }

    /// Extract plain text from a PowerPoint (.pptx) file
    pub fn extract_pptx_text(path: &Path) -> Result<String> {
        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut text = String::new();

        let mut slide_names = Vec::new();
        for i in 0..archive.len() {
            let f = archive.by_index(i)?;
            let name = f.name()?.into_owned();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                slide_names.push(name);
            }
        }
        slide_names.sort();

        for name in slide_names {
            let mut f = archive.by_name(&name)?;
            let mut xml_content = String::new();
            f.read_to_string(&mut xml_content)?;

            let mut reader = Reader::from_str(&xml_content);
            let mut buf = Vec::new();
            let mut in_text = false;

            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(ref e)) if e.name().as_ref() == b"a:t" => {
                        in_text = true;
                    }
                    Ok(Event::End(ref e)) if e.name().as_ref() == b"a:t" => {
                        in_text = false;
                    }
                    Ok(Event::Text(ref e)) if in_text => {
                            text.push_str(&e.decode()?);
                            text.push(' ');
                        }
                    Ok(Event::Eof) => break,
                    Err(_) => break,
                    _ => {}
                }
                buf.clear();
            }
        }

        Ok(text)
    }

    /// Extract plain text from an Excel (.xlsx) file (via sharedStrings.xml)
    pub fn extract_xlsx_text(path: &Path) -> Result<String> {
        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut text = String::new();

        if let Ok(mut f) = archive.by_name("xl/sharedStrings.xml") {
            let mut xml_content = String::new();
            f.read_to_string(&mut xml_content)?;

            let mut reader = Reader::from_str(&xml_content);
            let mut buf = Vec::new();
            let mut in_text = false;

            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(ref e)) if e.name().as_ref() == b"t" => {
                        in_text = true;
                    }
                    Ok(Event::End(ref e)) if e.name().as_ref() == b"t" => {
                        in_text = false;
                    }
                    Ok(Event::Text(ref e)) if in_text => {
                            text.push_str(&e.decode()?);
                            text.push(' ');
                        }
                    Ok(Event::Eof) => break,
                    Err(_) => break,
                    _ => {}
                }
                buf.clear();
            }
        }

        Ok(text)
    }

    /// Extract schema summary text from a Parquet file for BM25 indexing
    pub fn extract_parquet_text(path: &Path) -> Result<String> {
        let mut file = File::open(path)?;
        let metadata = parquet2::read::read_metadata(&mut file).map_err(|e| anyhow::anyhow!("{:?}", e))?;

        let mut text = format!(
            "Parquet File. Rows: {}, Columns: {}. Schema: ",
            metadata.num_rows,
            metadata.schema().fields().len()
        );

        let fields: Vec<String> = metadata.schema().fields().iter()
            .map(|field| format!("{}: {:?}", field.name(), field))
            .collect();
        text.push_str(&fields.join(", "));

        Ok(text)
    }

    /// Parse Word document (.docx) and return pseudoclass/module symbols
    pub fn parse_docx(path: &Path) -> Result<Vec<Symbol>> {
        let text = Self::extract_docx_text(path)?;
        let sig = if text.is_empty() {
            None
        } else {
            let text_trimmed = text.trim();
            let mut preview = text_trimmed.chars().take(200).collect::<String>();
            if text_trimmed.chars().count() > 200 {
                preview.push_str("...");
            }
            Some(preview)
        };

        Ok(vec![Symbol {
            name: "Word Document".to_string(),
            kind: super::parser::SymbolKind::Module,
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 1,
            signature: sig,
            is_exported: true,
            scope: None,
        }])
    }

    /// Parse PowerPoint document (.pptx) and return slide-based symbols
    pub fn parse_pptx(path: &Path) -> Result<Vec<Symbol>> {
        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut symbols = Vec::new();

        let mut slide_names = Vec::new();
        for i in 0..archive.len() {
            let f = archive.by_index(i)?;
            let name = f.name()?.into_owned();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                slide_names.push(name);
            }
        }
        slide_names.sort();

        for (idx, name) in slide_names.iter().enumerate() {
            let slide_num = idx + 1;
            let mut f = archive.by_name(name)?;
            let mut xml_content = String::new();
            f.read_to_string(&mut xml_content)?;

            let mut reader = Reader::from_str(&xml_content);
            let mut buf = Vec::new();
            let mut in_text = false;
            let mut slide_text = String::new();

            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(ref e)) if e.name().as_ref() == b"a:t" => {
                        in_text = true;
                    }
                    Ok(Event::End(ref e)) if e.name().as_ref() == b"a:t" => {
                        in_text = false;
                    }
                    Ok(Event::Text(ref e)) if in_text => {
                            slide_text.push_str(&e.decode()?);
                            slide_text.push(' ');
                        }
                    Ok(Event::Eof) => break,
                    Err(_) => break,
                    _ => {}
                }
                buf.clear();
            }

            let sig = if slide_text.is_empty() {
                None
            } else {
                let text_trimmed = slide_text.trim();
                let mut preview = text_trimmed.chars().take(200).collect::<String>();
                if text_trimmed.chars().count() > 200 {
                    preview.push_str("...");
                }
                Some(preview)
            };

            symbols.push(Symbol {
                name: format!("Slide {}", slide_num),
                kind: super::parser::SymbolKind::Module,
                line: slide_num as u32,
                column: 1,
                end_line: slide_num as u32,
                end_column: 1,
                signature: sig,
                is_exported: true,
                scope: None,
            });
        }

        Ok(symbols)
    }

    /// Parse Excel document (.xlsx) and return sheet-based symbols
    #[allow(deprecated)] // WHY: quick_xml の unescape_value() は deprecated だが normalized_value() は XmlVersion 引数が必要で安定 API がないため
    pub fn parse_xlsx(path: &Path) -> Result<Vec<Symbol>> {
        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut symbols = Vec::new();

        if let Ok(mut workbook_file) = archive.by_name("xl/workbook.xml") {
            let mut xml_content = String::new();
            workbook_file.read_to_string(&mut xml_content)?;

            let mut reader = Reader::from_str(&xml_content);
            let mut buf = Vec::new();
            let mut sheet_num = 1;

            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) if e.name().as_ref() == b"sheet" => {
                        let mut name = format!("Sheet{}", sheet_num);
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"name" {
                                name = attr.unescape_value()?.to_string();
                                break;
                            }
                        }

                        symbols.push(Symbol {
                            name: format!("Sheet: {}", name),
                            kind: super::parser::SymbolKind::Module,
                            line: sheet_num,
                            column: 1,
                            end_line: sheet_num,
                            end_column: 1,
                            signature: None,
                            is_exported: true,
                            scope: None,
                        });
                        sheet_num += 1;
                    }
                    Ok(Event::Eof) => break,
                    Err(_) => break,
                    _ => {}
                }
                buf.clear();
            }
        }

        Ok(symbols)
    }

    /// Extract plain text from a PDF file.
    ///
    /// WHY: PDFs store text in content streams using Tj/TJ operators. We decompress
    /// all streams first, then scan for string operands to reconstruct readable text.
    /// Does not handle complex font encodings (ToUnicode maps) — best-effort for
    /// standard text-encoded PDFs.
    pub fn extract_pdf_text(path: &Path) -> Result<String> {
        let mut doc = lopdf::Document::load(path)?;
        doc.decompress();

        let pages: Vec<_> = doc.get_pages().into_iter().collect();
        let mut text = String::new();

        for (_page_num, page_id) in pages {
            if let Ok(content_data) = doc.get_page_content(page_id) {
                if let Ok(content) = Content::decode(&content_data) {
                    Self::extract_text_from_operations(&content.operations, &mut text);
                }
            }
        }

        Ok(text)
    }

    fn extract_text_from_operations(ops: &[Operation], text: &mut String) {
        for op in ops {
            match op.operator.as_str() {
                "Tj" | "'" | "\"" => {
                    if let Some(lopdf::Object::String(bytes, _)) = op.operands.first() {
                        let s = String::from_utf8_lossy(bytes);
                        if !s.trim().is_empty() {
                            text.push_str(s.trim());
                            text.push(' ');
                        }
                    }
                }
                "TJ" => {
                    if let Some(lopdf::Object::Array(array)) = op.operands.first() {
                        for item in array {
                            if let lopdf::Object::String(bytes, _) = item {
                                let s = String::from_utf8_lossy(bytes);
                                if !s.trim().is_empty() {
                                    text.push_str(s.trim());
                                }
                            }
                        }
                        text.push(' ');
                    }
                }
                "ET" => text.push('\n'),
                _ => {}
            }
        }
    }

    /// Parse a PDF and return page-based symbols (one symbol per page).
    pub fn parse_pdf(path: &Path) -> Result<Vec<Symbol>> {
        let text = Self::extract_pdf_text(path).unwrap_or_default();
        let page_count = {
            let mut doc = lopdf::Document::load(path)?;
            doc.decompress();
            doc.get_pages().len()
        };

        let mut symbols = Vec::new();
        if page_count == 0 {
            return Ok(symbols);
        }

        // One symbol per page, with the first 200 chars of full-doc text in page 1
        let preview = if text.is_empty() {
            None
        } else {
            let t = text.trim();
            let mut s = t.chars().take(200).collect::<String>();
            if t.chars().count() > 200 {
                s.push_str("...");
            }
            Some(s)
        };

        for page in 1..=page_count {
            symbols.push(Symbol {
                name: format!("Page {}", page),
                kind: super::parser::SymbolKind::Module,
                line: page as u32,
                column: 1,
                end_line: page as u32,
                end_column: 1,
                signature: if page == 1 { preview.clone() } else { None },
                is_exported: true,
                scope: None,
            });
        }

        Ok(symbols)
    }
}

/// BM25 full-text scorer for document files.
///
/// Reads files at query time and computes Okapi BM25 scores across the corpus.
/// Suitable for < 1000-file corpora (e.g. Markdown novel chapters) where
/// per-query I/O cost is acceptable and a persistent inverted index is overkill.
pub struct Bm25Scorer;

impl Bm25Scorer {
    // Standard Okapi BM25 hyperparameters
    const K1: f64 = 1.5;
    const B: f64 = 0.75;

    /// Read and BM25-score `file_paths` against `query_terms`.
    ///
    /// Returns (path, score) pairs sorted by score descending, capped at `top_k`.
    /// Files with score 0 (no query term present) are excluded.
    pub fn search_files(
        workspace_root: &str,
        file_paths: &[String],
        query_terms: &[&str],
        top_k: usize,
    ) -> Vec<(String, f64)> {
        use std::collections::HashMap;

        if query_terms.is_empty() || file_paths.is_empty() {
            return Vec::new();
        }

        let query_lower: Vec<String> = query_terms.iter().map(|t| t.to_lowercase()).collect();

        // Read and tokenize all files; skip unreadable ones silently
        let docs: Vec<(String, Vec<String>)> = file_paths
            .iter()
            .filter_map(|path| {
                let full = Path::new(workspace_root).join(path);
                let content = if path.ends_with(".docx") {
                    DocumentParser::extract_docx_text(&full).ok()?
                } else if path.ends_with(".pptx") {
                    DocumentParser::extract_pptx_text(&full).ok()?
                } else if path.ends_with(".xlsx") {
                    DocumentParser::extract_xlsx_text(&full).ok()?
                } else if path.ends_with(".pdf") {
                    DocumentParser::extract_pdf_text(&full).ok()?
                } else if path.ends_with(".parquet") {
                    DocumentParser::extract_parquet_text(&full).ok()?
                } else {
                    std::fs::read_to_string(&full).ok()?
                };
                let tokens = Self::tokenize(&content);
                if tokens.is_empty() { return None; }
                Some((path.clone(), tokens))
            })
            .collect();

        if docs.is_empty() {
            return Vec::new();
        }

        let num_docs = docs.len();
        let avg_doc_len =
            docs.iter().map(|(_, t)| t.len()).sum::<usize>() as f64 / num_docs as f64;

        // WHY: IDF requires knowing document frequency per term across the whole corpus.
        // We compute it once here before scoring individual documents.
        let doc_freq: HashMap<String, usize> = query_lower
            .iter()
            .map(|term| {
                let df = docs.iter().filter(|(_, tokens)| tokens.contains(term)).count();
                (term.clone(), df)
            })
            .collect();

        // Score every document
        let mut scored: Vec<(String, f64)> = docs
            .iter()
            .filter_map(|(path, tokens)| {
                let doc_len = tokens.len() as f64;
                let mut tf_map: HashMap<String, u32> = HashMap::new();
                for t in tokens {
                    *tf_map.entry(t.clone()).or_insert(0) += 1;
                }

                let score: f64 = query_lower
                    .iter()
                    .map(|term| {
                        let tf = *tf_map.get(term).unwrap_or(&0) as f64;
                        if tf == 0.0 {
                            return 0.0;
                        }
                        let df = *doc_freq.get(term).unwrap_or(&0) as f64;
                        // WHY: +1 inside ln prevents negative IDF for very common terms
                        let idf = ((num_docs as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
                        let tf_norm = tf * (Self::K1 + 1.0)
                            / (tf + Self::K1 * (1.0 - Self::B + Self::B * doc_len / avg_doc_len));
                        idf * tf_norm
                    })
                    .sum();

                if score > 0.0 { Some((path.clone(), score)) } else { None }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    /// Tokenize into lowercase alphanumeric words, discarding single-char tokens.
    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 2)
            .map(|w| w.to_lowercase())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_simple() -> Result<()> {
        let json = r#"{"name": "test", "version": "1.0.0"}"#;
        let symbols = DocumentParser::parse_json(json)?;
        
        assert!(!symbols.is_empty());
        assert!(symbols.iter().any(|s| s.name == "name"));
        assert!(symbols.iter().any(|s| s.name == "version"));
        
        Ok(())
    }

    #[test]
    fn test_parse_markdown_headings() -> Result<()> {
        let markdown = r#"# Title
## Section 1
### Subsection
## Section 2
"#;
        let symbols = DocumentParser::parse_markdown(markdown)?;

        assert_eq!(symbols.len(), 4);
        assert_eq!(symbols[0].name, "Title");
        assert_eq!(symbols[1].name, "Section 1");
        // Consecutive headings with no body produce no signature
        assert!(symbols[0].signature.is_none());

        Ok(())
    }

    #[test]
    fn test_parse_markdown_signature() -> Result<()> {
        let markdown = r#"# Overview
comP is a lightweight indexer.

## Installation
Run npm install to get started.

## Empty Section
### Nested
"#;
        let symbols = DocumentParser::parse_markdown(markdown)?;

        assert_eq!(symbols[0].name, "Overview");
        assert_eq!(symbols[0].signature.as_deref(), Some("comP is a lightweight indexer."));

        assert_eq!(symbols[1].name, "Installation");
        assert_eq!(symbols[1].signature.as_deref(), Some("Run npm install to get started."));

        // "Empty Section" is followed immediately by a sub-heading
        assert_eq!(symbols[2].name, "Empty Section");
        assert!(symbols[2].signature.is_none());

        Ok(())
    }

    #[test]
    fn test_parse_xml_tags() -> Result<()> {
        let xml = r#"<?xml version="1.0"?>
<root>
  <element1>content</element1>
  <element2>content</element2>
</root>"#;
        let symbols = DocumentParser::parse_xml(xml)?;
        
        assert_eq!(symbols.len(), 3);
        assert!(symbols.iter().any(|s| s.name == "root"));
        assert!(symbols.iter().any(|s| s.name == "element1"));
        assert!(symbols.iter().any(|s| s.name == "element2"));
        
        Ok(())
    }

    #[test]
    fn test_office_document_parsers() -> Result<()> {
        use tempfile::NamedTempFile;
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        // 1. Mock DOCX
        let docx_file = NamedTempFile::new()?;
        let mut zip = zip::ZipWriter::new(docx_file.as_file());
        zip.start_file("word/document.xml", SimpleFileOptions::default())?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello from Word docx</w:t></w:r></w:p></w:body></w:document>"#)?;
        zip.finish()?;

        let text = DocumentParser::extract_docx_text(docx_file.path())?;
        assert!(text.contains("Hello from Word docx"));

        let symbols = DocumentParser::parse_docx(docx_file.path())?;
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Word Document");
        assert!(symbols[0].signature.as_ref().unwrap().contains("Hello from Word docx"));

        // 2. Mock PPTX
        let pptx_file = NamedTempFile::new()?;
        let mut zip = zip::ZipWriter::new(pptx_file.as_file());
        zip.start_file("ppt/slides/slide1.xml", SimpleFileOptions::default())?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Hello from Slide 1</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#)?;
        zip.finish()?;

        let text = DocumentParser::extract_pptx_text(pptx_file.path())?;
        assert!(text.contains("Hello from Slide 1"));

        let symbols = DocumentParser::parse_pptx(pptx_file.path())?;
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Slide 1");
        assert!(symbols[0].signature.as_ref().unwrap().contains("Hello from Slide 1"));

        // 3. Mock XLSX
        let xlsx_file = NamedTempFile::new()?;
        let mut zip = zip::ZipWriter::new(xlsx_file.as_file());
        zip.start_file("xl/workbook.xml", SimpleFileOptions::default())?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/></sheets></workbook>"#)?;
        zip.start_file("xl/sharedStrings.xml", SimpleFileOptions::default())?;
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>Hello from Excel cell</t></si></sst>"#)?;
        zip.finish()?;

        let text = DocumentParser::extract_xlsx_text(xlsx_file.path())?;
        assert!(text.contains("Hello from Excel cell"));

        let symbols = DocumentParser::parse_xlsx(xlsx_file.path())?;
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Sheet: Sales");

        Ok(())
    }

    #[test]
    fn test_parse_parquet_metadata() -> Result<()> {
        let path = Path::new("tests/data/test.parquet");
        let symbols = DocumentParser::parse_parquet(path)?;

        assert!(!symbols.is_empty(), "Symbols list should not be empty");
        assert_eq!(symbols[0].name, "Parquet Schema");
        assert_eq!(symbols[0].kind.as_str(), "module");
        
        let sig = symbols[0].signature.as_ref().unwrap();
        println!("DEBUG PARQUET SIG: {}", sig);
        assert!(sig.contains("Parquet File. Rows: 3, Columns: 3"));
        assert!(sig.contains("id:"));

        let names: Vec<String> = symbols.iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"id".to_string()));
        assert!(names.contains(&"name".to_string()));
        assert!(names.contains(&"score".to_string()));

        let text = DocumentParser::extract_parquet_text(path)?;
        assert!(text.contains("Parquet File. Rows: 3, Columns: 3"));
        assert!(text.contains("score:"));

        Ok(())
    }
}
