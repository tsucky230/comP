//! Rebuilding the TF-IDF index replaces its contents.
//! Owned by the designer role; implementers must not edit.

use super::*;

fn sym(path: &str, name: &str) -> (String, String, String, u32) {
    (path.to_string(), name.to_string(), "function".to_string(), 1)
}

#[test]
fn a_rebuild_forgets_files_that_are_no_longer_given() {
    let mut engine = SearchEngine::new();
    engine.build_index(&[sym("src/pkg/callee.rs", "callee_fn"), sym("src/other.rs", "other_fn")]).unwrap();
    assert!(engine.search("callee", 10).unwrap().iter().any(|h| h.file_path == "src/pkg/callee.rs"));

    engine.build_index(&[sym("src/moved/callee.rs", "callee_fn"), sym("src/other.rs", "other_fn")]).unwrap();

    let files: Vec<String> = engine.search("callee", 10).unwrap().into_iter().map(|h| h.file_path).collect();
    assert_eq!(files, vec!["src/moved/callee.rs".to_string()]);
}

#[test]
fn a_rebuild_with_the_same_symbols_gives_the_same_scores() {
    let symbols = [sym("src/a.rs", "parse_config"), sym("src/b.rs", "load_config"), sym("src/c.rs", "render")];
    let mut once = SearchEngine::new();
    once.build_index(&symbols).unwrap();
    let mut twice = SearchEngine::new();
    twice.build_index(&symbols).unwrap();
    twice.build_index(&symbols).unwrap();

    let scores = |e: &SearchEngine| -> Vec<(String, String)> {
        e.search("config", 10).unwrap().into_iter().map(|h| (h.file_path, format!("{:.5}", h.score))).collect()
    };
    assert_eq!(scores(&twice), scores(&once));
}
