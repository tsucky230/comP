//! Calls written on the same line as a definition (`fn a() { b(); }`) must
//! become dependencies; the defined name itself must not be read as a call.
//! Owned by the designer role; implementers must not edit.

use super::*;
use std::collections::HashMap;

/// Names of FunctionCall dependencies found on line 1.
fn calls_on_line_1(language: &str, source: &str) -> Vec<String> {
    let mut names: Vec<String> = DependencyAnalyzer::extract_dependencies(language, source, "f")
        .unwrap()
        .into_iter()
        .filter(|d| matches!(d.kind, EdgeKind::FunctionCall) && d.line == 1)
        .map(|d| d.to)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn calls_on_a_definition_line_are_found_and_the_defined_name_is_not() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("rust", "fn a() { b(); }", &["b"]),
        ("rust", "pub fn a<T: Clone>(x: T) -> T { b(x) }", &["b"]),
        ("rust", "pub async fn a() { b().await; }", &["b"]),
        ("rust", "pub(crate) fn a() -> u32 { b() + c() }", &["b", "c"]),
        ("typescript", "function a() { return b(); }", &["b"]),
        ("typescript", "export async function a(x: number) { await b(x); }", &["b"]),
        ("javascript", "export function a() { b(); }", &["b"]),
        ("python", "def a(): return b()", &["b"]),
        ("python", "async def a(x): await b(x)", &["b"]),
        ("python", "class A(Base): pass", &[]),
        ("go", "func a() { b() }", &["b"]),
        ("go", "func (r *Repo) Save(x int) error { return exec(x) }", &["exec"]),
    ];
    for (language, source, want) in cases {
        let want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        assert_eq!(calls_on_line_1(language, source), want, "{} {:?}", language, source);
    }
}

#[test]
fn a_definition_line_alone_still_yields_no_call() {
    for (language, source) in [
        ("rust", "pub fn a(x: u32) -> u32 {"),
        ("typescript", "export function a(x: number): number {"),
        ("python", "def a(x):"),
        ("go", "func a(x int) int {"),
    ] {
        assert_eq!(calls_on_line_1(language, source), Vec::<String>::new(), "{} {:?}", language, source);
    }
}

#[test]
fn a_same_line_call_becomes_an_edge_from_the_defined_function() {
    let source = "fn entry_point() { shared_helper(); }\n";
    let deps = DependencyAnalyzer::extract_dependencies("rust", source, "src/main.rs").unwrap();
    // entry_point (node 10) is in this file; shared_helper (node 20) is exported from another file.
    let local = vec![(10_i64, "entry_point".to_string(), 1_i32)];
    let mut global: crate::graph::GlobalSymbolIndex = HashMap::new();
    global.insert("shared_helper".to_string(), vec![(20, 2, true)]);

    let edges = DependencyAnalyzer::resolve_global(&deps, &local, &global, 1);

    assert_eq!(edges, vec![(10, 20, "function_call".to_string())]);
}
