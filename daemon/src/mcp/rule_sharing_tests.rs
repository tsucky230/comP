//! Integration tests for rule sharing (beta) through the MCP handlers.
//! Owned by the designer role; implementers must not edit.

use super::*;
use std::path::Path;

/// A git repository with instruction files; `tracked` are `git add`ed, `untracked` are only written.
fn repo(tracked: &[(&str, &str)], untracked: &[(&str, &str)], recording_on: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").args(args).current_dir(d.path()).output().expect("git");
        assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    for (path, text) in tracked.iter().chain(untracked.iter()) {
        let p = d.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    for (path, _) in tracked {
        git(&["add", path]);
    }
    set_switch(d.path(), recording_on);
    d
}

fn plain_dir(recording_on: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("AGENTS.md"), "# Testing\nUse pytest.\n").unwrap();
    set_switch(d.path(), recording_on);
    d
}

fn set_switch(ws: &Path, on: bool) {
    std::fs::create_dir_all(ws.join(".comp")).unwrap();
    std::fs::write(ws.join(".comp/config.json"), format!(r#"{{"exclude": [], "ruleSharing": {{"enabled": {}}}}}"#, on)).unwrap();
}

async fn server(ws: &Path, agent: &str) -> MCPServer {
    let state = Arc::new(crate::AppState::new(ws.to_str().unwrap(), agent).await.expect("AppState"));
    MCPServer::new(state)
}

async fn call(server: &MCPServer, name: &str, args: Value) -> Value {
    let r = server.handle_tools_call(json!({ "name": name, "arguments": args })).await.expect("tool call");
    let text = r["content"][0]["text"].as_str().expect("text content").to_string();
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

fn last_call(ws: &Path, agent: &str) -> Value {
    let p = ws.join(".comp").join("session-memory").join(format!("{}.json", agent));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).expect("session memory")).unwrap();
    let sessions = v["sessions"].as_array().unwrap();
    sessions.last().unwrap()["calls"].as_array().unwrap().last().unwrap().clone()
}

const CLAUDE_MD: &str = "# Testing\nRun the whole test suite with pytest before every commit and push to main.\n";
const AGENTS_MD: &str = "# Testing\nRun the whole test suite with unittest before every commit and push to main.\n";
const GEMINI_MD: &str = "# Testing\nAlways run pytest with coverage enabled for every test change.\n";

// ---- run_pipeline ----

#[tokio::test]
async fn run_pipeline_is_unchanged_when_rule_sharing_is_off() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[], false);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "run_pipeline", json!({ "task": "run pytest test suite" })).await;
    assert!(r.get("related_rules").is_none(), "{}", r);
    assert!(last_call(ws.path(), "claude-code").get("rules").is_none());
    assert!(!ws.path().join(".comp/rules").exists());
}

#[tokio::test]
async fn run_pipeline_returns_relevant_rules_of_other_agents_from_tracked_files_only() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[("GEMINI.md", GEMINI_MD)], true);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "run_pipeline", json!({ "task": "run pytest test suite before commit" })).await;
    let rr = &r["related_rules"];
    let items = rr["items"].as_array().unwrap_or_else(|| panic!("items in {}", rr));
    let files: Vec<&str> = items.iter().map(|i| i["file"].as_str().unwrap()).collect();
    // CLAUDE.md is the caller's own file; GEMINI.md is not tracked by git.
    assert_eq!(files, vec!["AGENTS.md"]);
    assert_eq!(items[0]["heading"], "Testing");
    assert!(items[0]["text"].as_str().unwrap().contains("unittest"));
    assert_eq!(items[0]["hash"].as_str().unwrap().len(), 64);
    assert!(rr["tokens"].as_u64().unwrap() > 0);
    assert!(rr["tokens"].as_u64().unwrap() <= RELATED_RULES_MAX_TOKENS as u64);
    let note = rr["note"].as_str().unwrap().to_lowercase();
    assert!(note.contains("reference") && note.contains("other agents"), "{}", note);
}

#[tokio::test]
async fn run_pipeline_records_and_snapshots_the_rules_it_returned() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[], true);
    let s = server(ws.path(), "gemini-cli").await;
    let r = call(&s, "run_pipeline", json!({ "task": "pytest unittest test suite" })).await;
    let items = r["related_rules"]["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 2);
    let rec = last_call(ws.path(), "gemini-cli");
    let rules = rec["rules"].as_array().expect("rules recorded");
    assert_eq!(rules.len(), items.len());
    for (rule, item) in rules.iter().zip(items.iter()) {
        assert_eq!(rule["file"], item["file"]);
        assert_eq!(rule["heading"], item["heading"]);
        assert_eq!(rule["hash"], item["hash"]);
        assert!(rule.get("text").is_none(), "records keep references, the text lives in the snapshot");
        let snap = ws.path().join(".comp/rules").join(format!("{}.md", item["hash"].as_str().unwrap()));
        let body = std::fs::read_to_string(&snap).expect("snapshot");
        assert!(body.contains(item["text"].as_str().unwrap()));
    }
}

#[tokio::test]
async fn run_pipeline_reports_unavailable_outside_git_but_still_answers() {
    let ws = plain_dir(true);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "run_pipeline", json!({ "task": "pytest" })).await;
    assert!(r["pivot_files"].is_array());
    assert!(r["related_rules"]["unavailable"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "{}", r);
    assert!(r["related_rules"].get("items").is_none());
}

#[tokio::test]
async fn run_pipeline_omits_items_when_nothing_is_relevant() {
    let ws = repo(&[("AGENTS.md", AGENTS_MD)], &[], true);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "run_pipeline", json!({ "task": "refactor kubernetes helm chart" })).await;
    assert_eq!(r["related_rules"]["items"], json!([]));
    assert!(last_call(ws.path(), "claude-code").get("rules").is_none());
}

// ---- check_rule_conflicts ----

#[tokio::test]
async fn check_rule_conflicts_is_disabled_when_off() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[], false);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "check_rule_conflicts", json!({})).await;
    assert!(r["disabled"].as_str().unwrap().contains("comp.ruleSharing.enabled"), "{}", r);
    assert!(r.get("pairs").is_none());
}

#[tokio::test]
async fn check_rule_conflicts_returns_candidate_pairs_with_a_judge_only_note() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[("GEMINI.md", GEMINI_MD)], true);
    let s = server(ws.path(), "codex").await;
    let r = call(&s, "check_rule_conflicts", json!({})).await;
    let pairs = r["pairs"].as_array().unwrap_or_else(|| panic!("pairs in {}", r));
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0]["a"]["file"], "AGENTS.md");
    assert_eq!(pairs[0]["b"]["file"], "CLAUDE.md");
    assert!(pairs[0]["similarity"].as_f64().unwrap() >= rules::CONFLICT_SIMILARITY);
    assert!(pairs[0]["a"]["text"].as_str().unwrap().contains("unittest"));
    let mut scanned: Vec<&str> = r["files_scanned"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    scanned.sort();
    assert_eq!(scanned, vec!["AGENTS.md", "CLAUDE.md"]);
    let note = r["note"].as_str().unwrap().to_lowercase();
    assert!(note.contains("judge") && note.contains("do not edit"), "{}", note);
}

#[tokio::test]
async fn check_rule_conflicts_reports_unavailable_outside_git() {
    let ws = plain_dir(true);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "check_rule_conflicts", json!({})).await;
    assert!(r["unavailable"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "{}", r);
}

#[tokio::test]
async fn check_rule_conflicts_validates_max_pairs() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[], true);
    let s = server(ws.path(), "claude-code").await;
    for bad in [json!(0), json!(101), json!(-1), json!("5"), json!(2.5)] {
        let err = s.handle_tools_call(json!({ "name": "check_rule_conflicts", "arguments": { "max_pairs": bad } }))
            .await.unwrap_err();
        assert!(err.to_string().contains("max_pairs"), "{} -> {}", bad, err);
    }
    for ok in [1, 20, 100] {
        let r = call(&s, "check_rule_conflicts", json!({ "max_pairs": ok })).await;
        assert!(r["pairs"].as_array().unwrap().len() <= ok as usize);
    }
}

#[tokio::test]
async fn tools_list_includes_check_rule_conflicts() {
    let ws = plain_dir(false);
    let s = server(ws.path(), "claude-code").await;
    let list = s.handle_tools_list().await.unwrap();
    let tool = list["tools"].as_array().unwrap().iter()
        .find(|t| t["name"] == "check_rule_conflicts").expect("listed even when off");
    assert_eq!(tool["inputSchema"]["properties"]["max_pairs"]["type"], "integer");
}

// ---- session_recall ----

#[tokio::test]
async fn session_recall_shows_the_rules_a_call_used() {
    let ws = repo(&[("CLAUDE.md", CLAUDE_MD), ("AGENTS.md", AGENTS_MD)], &[], true);
    let s = server(ws.path(), "claude-code").await;
    let r = call(&s, "run_pipeline", json!({ "task": "pytest test suite" })).await;
    let hash = r["related_rules"]["items"][0]["hash"].as_str().unwrap().to_string();
    let recall = call(&s, "session_recall", json!({})).await;
    let text = recall.as_str().unwrap();
    assert!(text.contains("**Rules**"), "{}", text);
    assert!(text.contains("AGENTS.md#Testing"), "{}", text);
    assert!(text.contains(&hash[..8]), "{}", text);
}

#[test]
fn session_call_without_rules_serializes_as_before() {
    let call = SessionCall {
        query: "q".into(), outcome: None, symbols: vec![], files: vec![], tokens: 0, stale: false,
        timestamp: 1, agent: "claude-code".into(), rules: vec![], ..Default::default()
    };
    let v: Value = serde_json::to_value(&call).unwrap();
    assert!(v.get("rules").is_none());
    let old = r#"{"query":"q","timestamp":1}"#;
    let parsed: SessionCall = serde_json::from_str(old).unwrap();
    assert!(parsed.rules.is_empty());
}
