//! Tests for trace (YASAKANI plan B). Owned by the designer role; implementers
//! must not edit. `unit_*` cover the pure functions in trace.rs; `integ_*` cover
//! their wiring into record-turn, append-history, session_log, session_recall and
//! compact-history.

use super::*;
use crate::mcp::{MCPServer, SessionCall};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const DAY: u64 = 86_400_000;

fn call(query: &str, outcome: Option<&str>, ts: u64) -> SessionCall {
    SessionCall {
        query: query.into(),
        outcome: outcome.map(String::from),
        timestamp: ts,
        agent: "claude-code".into(),
        ..Default::default()
    }
}

fn turn(session: &str, id: &str, ts: u64) -> SessionCall {
    SessionCall { session_id: Some(session.into()), turn_id: Some(id.into()), kind: Some("turn".into()), ..call(id, None, ts) }
}

fn deleg(session: Option<&str>, parent: Option<&str>, ts: u64) -> SessionCall {
    SessionCall {
        session_id: session.map(String::from),
        parent_turn_id: parent.map(String::from),
        kind: Some("delegation".into()),
        agent: "codex".into(),
        ..call("d", None, ts)
    }
}

// ---------------- redact_secrets ----------------

#[test]
fn unit_redact_known_key_shapes() {
    let cases = [
        "sk-abcdefghijklmnopqrstuvwxyz0123",
        "sk-proj-ABCDEFGHIJKLMNOPQRSTUV_wx-yz12",
        "sk-ant-api03-abcdefghijklmnopqrstu",
        "AIzaSyA1234567890abcdefghijklmnopqrstuv",
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
        "xoxb-1234567890-abcdefghij",
        "AKIAABCDEFGHIJKLMNOP",
    ];
    for key in cases {
        let text = format!("鍵は {} です", key);
        let out = redact_secrets(&text);
        assert!(!out.contains(key), "{} not redacted: {}", key, out);
        assert_eq!(out, format!("鍵は {} です", REDACTED), "{}", key);
    }
}

#[test]
fn unit_redact_assignments_keeps_name() {
    let cases = [
        ("api_key=abcd1234efgh5678ijkl", "api_key=[REDACTED]"),
        ("API-KEY: \"Zx9yW8vU7tS6rQ5pO4\"", "API-KEY: \"[REDACTED]\""),
        ("token = 'a1b2c3d4e5f6g7h8i9j0'", "token = '[REDACTED]'"),
        ("password:Secr3tPassw0rdValue!", "password:[REDACTED]!"),
        ("access_key=AbCdEf0123456789GhIj", "access_key=[REDACTED]"),
    ];
    for (input, expected) in cases {
        assert_eq!(redact_secrets(input), expected, "{}", input);
    }
}

#[test]
fn unit_redact_leaves_code_and_short_values() {
    let cases = [
        "api_key = config.get(\"key\")",
        "token = read_token_from_env()",
        "let secret = load_secret_value_v2(path);",
        "password=short1",
        "token: abcdefghijklmnopqrstu",
        "token_count = 1234567890123456789",
        "sk-short",
        "普通の日本語の文章です。",
        "",
    ];
    for input in cases {
        assert_eq!(redact_secrets(input), input, "{}", input);
    }
}

#[test]
fn unit_redact_is_idempotent() {
    let text = "a sk-abcdefghijklmnopqrstuvwxyz0123 b token=abcd1234efgh5678ijkl c";
    let once = redact_secrets(text);
    assert_eq!(redact_secrets(&once), once);
}

// ---------------- tokenize / bm25 ----------------

#[test]
fn unit_tokenize_ascii_words() {
    assert_eq!(tokenize("Fix the BM25 run_pipeline, a b"), vec!["fix", "the", "bm25", "run_pipeline"]);
    assert_eq!(tokenize(""), Vec::<String>::new());
    assert_eq!(tokenize("a - b ! c"), Vec::<String>::new());
}

#[test]
fn unit_tokenize_cjk_unigrams_and_bigrams() {
    assert_eq!(tokenize("検索"), vec!["検", "検索", "索"]);
    assert_eq!(tokenize("鍵"), vec!["鍵"]);
    assert_eq!(tokenize("会話の記録"), vec!["会", "会話", "話", "話の", "の", "の記", "記", "記録", "録"]);
    assert_eq!(tokenize("カタカナー"), vec!["カ", "カタ", "タ", "タカ", "カ", "カナ", "ナ", "ナー", "ー"]);
}

#[test]
fn unit_tokenize_mixed_runs_split() {
    assert_eq!(tokenize("BM25検索、OK"), vec!["bm25", "検", "検索", "索", "ok"]);
    assert_eq!(tokenize("ｶﾅ"), vec!["ｶ", "ｶﾅ", "ﾅ"]);
}

#[test]
fn unit_bm25_prefers_matching_doc_and_handles_empty() {
    let docs = vec![
        "会話の記録を既定OFFにする".to_string(),
        "ルール共有の矛盾".to_string(),
        String::new(),
    ];
    let s = bm25_scores(&docs, "記録");
    assert_eq!(s.len(), 3);
    assert!(s[0] > 0.0 && s[1] == 0.0 && s[2] == 0.0, "{:?}", s);
    assert_eq!(bm25_scores(&docs, "  "), vec![0.0, 0.0, 0.0]);
    assert!(bm25_scores(&[], "x").is_empty());
}

#[test]
fn unit_bm25_rare_term_weighs_more() {
    let docs: Vec<String> = vec!["alpha common".into(), "beta common".into(), "common".into(), "common gamma".into()];
    let s = bm25_scores(&docs, "alpha common");
    assert!(s[0] > s[1] && s[0] > s[2], "{:?}", s);
}

#[test]
fn unit_recall_text_joins_fields() {
    let mut c = call("依頼", Some("結果"), 1);
    c.files = vec!["src/a.rs".into()];
    assert_eq!(recall_text(&c), "依頼\n結果\nsrc/a.rs");
    assert_eq!(recall_text(&call("q", None, 1)), "q");
}

// ---------------- rank_by_query ----------------

#[test]
fn unit_rank_blank_query_is_newest_first() {
    let calls = vec![call("a", None, 10), call("b", None, 30), call("c", None, 20), call("d", None, 30)];
    assert_eq!(rank_by_query(&calls, "  ", 100), vec![1, 3, 2, 0]);
}

#[test]
fn unit_rank_substring_hits_first_then_bm25() {
    let calls = vec![
        call("会話の記録をONにする", None, 10),
        call("記録 の 改善", None, 50),
        call("関係ない", None, 60),
        call("会話の記録", Some("既定OFF"), 40),
    ];
    let r = rank_by_query(&calls, "会話の記録", 100);
    assert_eq!(&r[..2], &[3, 0], "substring hits newest first: {:?}", r);
    assert!(r.contains(&1), "bm25-only hit must follow: {:?}", r);
    assert!(!r.contains(&2), "zero score must be dropped: {:?}", r);
}

#[test]
fn unit_rank_substring_is_case_insensitive_and_checks_outcome() {
    let calls = vec![call("x", Some("Fixed the LOGIN bug"), 5), call("y", None, 6)];
    assert_eq!(rank_by_query(&calls, "login", 10), vec![0]);
}

#[test]
fn unit_rank_recency_breaks_equal_relevance() {
    let now = 400 * DAY;
    let calls = vec![call("検索 改善 old", None, now - 300 * DAY), call("検索 改善 new", None, now - DAY)];
    assert_eq!(rank_by_query(&calls, "改善 検索", now), vec![1, 0]);
}

/// Fixed evaluation (plan B checklist 9): 20 queries over 40 records; the ranked
/// search must put the expected record in the top 5 more often than the old
/// substring filter, and for at least 15 of the 20.
#[test]
fn unit_eval_ranked_search_beats_substring() {
    let targets: [(&str, &str, &str); 20] = [
        ("会話の記録をβ版として既定OFFにする", "設定 comp.conversationRecording.enabled を追加", "記録 既定OFF"),
        ("ルール共有の矛盾候補を TF-IDF で出す", "check_rule_conflicts を実装", "ルール 矛盾"),
        ("委譲ランナーの契約とテストを確定", "delegate_run.py のテスト110件", "ランナー 契約"),
        ("さくらの空応答を max-tokens で回避", "Kimi は考えるモデル", "空応答 回避"),
        ("Codex が標準入力待ちで止まる不具合", "stdin を閉じて起動する", "codex stdin"),
        ("push 前のドキュメント整合チェック", "Gemini が差分と文書の食い違いを検査", "ドキュメント 食い違い"),
        ("画像を外部AIへ送らない方針", "ユーザーが個別に許可した場合だけ", "画像 許可"),
        ("履歴ファイルの容量が増え続ける", "compact-history は重複行だけ消す", "容量 履歴"),
        ("セッション検索を部分一致からBM25へ", "文字2-gram で日本語に対応", "検索 日本語"),
        ("API キーを記録に残さない伏せ字", "書き込み時に置き換える", "伏せ字 書き込み"),
        ("Stop フックの終了コード2を返さない", "Claude Code が停止を妨げられたと解釈する", "終了コード 停止"),
        ("テストを弱める変更を test_lock で検出", "assert の減少と skip の増加", "skip 検出"),
        ("worktree に未追跡ファイルを写す", "copy_untracked で .env は拒否", "未追跡 worktree"),
        ("Gemini の文字化けを UTF-8 指定で回避", "PowerShell 5.1 経由で化けた", "文字化け powershell"),
        ("LINEスタンプの代替フォント", "linestampgen3 のタグ", "スタンプ フォント"),
        ("plan-review の的中率を記録する", "review_telemetry.jsonl に追記", "的中率 review"),
        ("MAGATAMA の呼び出しグラフで影響分析", "get_call_graph を使う", "影響分析 グラフ"),
        ("Windows でのシンボリックリンクと junction", "install.ps1 が作る", "junction install"),
        ("月3,000リクエストの無料枠を数える", "sakura_usage.jsonl の ts で集計", "無料枠 集計"),
        ("親ターンと委譲の対応をセッションIDで結ぶ", "parent_turn_id がなくても時刻窓で解決", "親ターン 時刻窓"),
    ];
    let noise = [
        "テストを追加する", "実装を修正する", "ドキュメントを更新する", "記録の書式を直す", "契約書の整理",
        "設定画面の文言", "README の英語版", "CHANGELOG を更新", "依存の更新を確認", "リリース手順の見直し",
        "検索ボックスの見た目", "画像の圧縮", "フォントサイズの調整", "push の手順", "Gemini のモデル名",
        "Codex のパス更新", "セッションの再開", "ターン数の上限", "容量の小さいモデル", "日本語の文章を直す",
    ];
    let now = 1_000 * DAY;
    let mut calls = Vec::new();
    for (i, (q, o, _)) in targets.iter().enumerate() {
        calls.push(call(q, Some(o), now - (40 - i as u64) * 1000));
    }
    for (i, n) in noise.iter().enumerate() {
        calls.push(call(n, None, now - i as u64));
    }
    let substring_top5 = |query: &str| -> Vec<usize> {
        let q = query.to_lowercase();
        let mut hits: Vec<usize> = (0..calls.len())
            .filter(|&i| {
                calls[i].query.to_lowercase().contains(&q)
                    || calls[i].outcome.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .collect();
        hits.sort_by_key(|&i| std::cmp::Reverse(calls[i].timestamp));
        hits.truncate(5);
        hits
    };
    let mut ranked_ok = 0;
    let mut substring_ok = 0;
    for (i, (_, _, query)) in targets.iter().enumerate() {
        if rank_by_query(&calls, query, now).iter().take(5).any(|&x| x == i) {
            ranked_ok += 1;
        }
        if substring_top5(query).contains(&i) {
            substring_ok += 1;
        }
    }
    assert!(ranked_ok > substring_ok, "ranked {} vs substring {}", ranked_ok, substring_ok);
    assert!(ranked_ok >= 15, "ranked search found only {} of 20", ranked_ok);
}

// ---------------- delegations_for_turn / parent_turn_of ----------------

#[test]
fn unit_delegations_resolved_by_session_and_time_window() {
    let calls = vec![
        turn("S1", "t1", 1000),
        deleg(Some("S1"), None, 1500),
        deleg(Some("S1"), None, 2000),
        turn("S1", "t2", 2000),
        deleg(Some("S1"), None, 2500),
        turn("S1", "t3", 3000),
        deleg(Some("S2"), None, 2600),
        deleg(None, None, 2700),
    ];
    assert_eq!(delegations_for_turn(&calls, 0), Vec::<usize>::new());
    assert_eq!(delegations_for_turn(&calls, 3), vec![1, 2]);
    assert_eq!(delegations_for_turn(&calls, 5), vec![4]);
    assert_eq!(parent_turn_of(&calls, 1), Some(3));
    assert_eq!(parent_turn_of(&calls, 4), Some(5));
    assert_eq!(parent_turn_of(&calls, 6), None);
    assert_eq!(parent_turn_of(&calls, 7), None);
}

#[test]
fn unit_explicit_parent_wins_over_time_window() {
    let calls = vec![
        turn("S1", "t1", 1000),
        turn("S1", "t2", 2000),
        deleg(Some("S1"), Some("t1"), 1500),
        deleg(Some("S9"), Some("t2"), 9999),
    ];
    assert_eq!(delegations_for_turn(&calls, 0), vec![2]);
    assert_eq!(delegations_for_turn(&calls, 1), vec![3]);
    assert_eq!(parent_turn_of(&calls, 2), Some(0));
}

#[test]
fn unit_delegations_unordered_input_sorted_oldest_first() {
    let calls = vec![deleg(Some("S1"), None, 900), turn("S1", "t", 1000), deleg(Some("S1"), None, 100)];
    assert_eq!(delegations_for_turn(&calls, 1), vec![2, 0]);
    assert_eq!(delegations_for_turn(&calls, 0), Vec::<usize>::new());
}

#[test]
fn unit_turn_without_kind_has_no_delegations() {
    let mut t = turn("S1", "t", 1000);
    t.kind = None;
    let calls = vec![t, deleg(Some("S1"), None, 500)];
    assert!(delegations_for_turn(&calls, 0).is_empty());
}

// ---------------- fold_old_lines ----------------

#[test]
fn unit_fold_old_lines_only_folds_old_long_text() {
    let now = 100 * DAY;
    let long: String = "あ".repeat(200);
    let old = json!({"query": long, "outcome": "short", "timestamp": now - 31 * DAY, "agent": "x"}).to_string();
    let recent = json!({"query": long, "timestamp": now - 29 * DAY}).to_string();
    let old_request = json!({"request": long, "timestamp": now - 40 * DAY}).to_string();
    let not_json = "not json at all";
    let input = format!("{}\n\n{}\n{}\n{}", old, recent, old_request, not_json);
    let out = String::from_utf8(fold_old_lines(input.as_bytes(), now, 30).unwrap()).unwrap();
    let lines: Vec<&str> = out.split('\n').collect();
    assert_eq!(lines.len(), 5, "4 lines + trailing newline: {:?}", lines);
    assert_eq!(lines[4], "");
    let folded: Value = serde_json::from_str(lines[0]).unwrap();
    let q = folded["query"].as_str().unwrap();
    assert_eq!(q.chars().count(), FOLD_CHARS);
    assert!(q.ends_with('…'));
    assert_eq!(folded["outcome"], "short");
    assert_eq!(folded["agent"], "x");
    assert_eq!(lines[1], recent, "recent line kept byte-for-byte");
    let r: Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(r["request"].as_str().unwrap().chars().count(), FOLD_CHARS);
    assert_eq!(lines[3], not_json);
}

#[test]
fn unit_fold_is_idempotent_and_rejects_zero_days() {
    let now = 100 * DAY;
    let line = json!({"query": "x".repeat(500), "timestamp": 1}).to_string();
    let once = fold_old_lines(line.as_bytes(), now, 1).unwrap();
    let twice = fold_old_lines(&once, now, 1).unwrap();
    assert_eq!(once, twice);
    assert!(fold_old_lines(line.as_bytes(), now, 0).is_err());
    let short = json!({"query": "x".repeat(FOLD_CHARS), "timestamp": 1}).to_string();
    let kept = fold_old_lines(short.as_bytes(), now, 1).unwrap();
    assert_eq!(kept, format!("{}\n", short).into_bytes());
}

// ---------------- extract_turn_meta ----------------

fn user(uuid: &str, text: &str) -> Value {
    json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": text}})
}

fn tool_use(name: &str, input: Value) -> Value {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": name, "input": input}]}})
}

fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn unit_extract_turn_meta_files_after_last_request() {
    let t = jsonl(&[
        user("u1", "first"),
        tool_use("Edit", json!({"file_path": "/ws/old.rs"})),
        user("u2", "second"),
        tool_use("Write", json!({"file_path": "/ws/a.rs"})),
        tool_use("Read", json!({"file_path": "/ws/read_only.rs"})),
        tool_use("MultiEdit", json!({"file_path": "/ws/b.rs"})),
        tool_use("Edit", json!({"file_path": "/ws/a.rs"})),
        tool_use("NotebookEdit", json!({"notebook_path": "/ws/n.ipynb"})),
        json!({"type": "user", "uuid": "u3", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": "/ws/side.rs"}}]}}),
        json!("not an object"),
    ]);
    let meta = extract_turn_meta(&format!("{}\n{{broken", t));
    assert_eq!(meta.request_uuid.as_deref(), Some("u2"));
    assert_eq!(meta.files, vec!["/ws/a.rs", "/ws/b.rs", "/ws/n.ipynb"]);
}

#[test]
fn unit_extract_turn_meta_skips_meta_and_reminder_only_users() {
    let t = jsonl(&[
        user("u1", "real request"),
        json!({"type": "user", "uuid": "m1", "isMeta": true, "message": {"content": "meta"}}),
        json!({"type": "user", "uuid": "r1", "message": {"content": [{"type": "text", "text": "<system-reminder>x</system-reminder>"}]}}),
        json!({"type": "user", "uuid": "s1", "isSidechain": true, "message": {"content": "side"}}),
    ]);
    assert_eq!(extract_turn_meta(&t).request_uuid.as_deref(), Some("u1"));
}

#[test]
fn unit_extract_turn_meta_empty_and_cap() {
    assert_eq!(extract_turn_meta(""), TurnMeta::default());
    let mut lines = vec![json!({"type": "user", "message": {"content": "no uuid"}})];
    for i in 0..(MAX_TOUCHED_FILES + 10) {
        lines.push(tool_use("Edit", json!({"file_path": format!("/ws/f{}.rs", i)})));
    }
    let meta = extract_turn_meta(&jsonl(&lines));
    assert_eq!(meta.request_uuid, None);
    assert_eq!(meta.files.len(), MAX_TOUCHED_FILES);
    assert_eq!(meta.files[0], "/ws/f0.rs");
}

// ---------------- integration ----------------

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn workspace_with_commit(recording_on: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.email", "t@example.com"]);
    git(d.path(), &["config", "user.name", "t"]);
    std::fs::write(d.path().join("README.md"), "x\n").unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "init"]);
    std::fs::create_dir_all(d.path().join(".comp")).unwrap();
    std::fs::write(
        d.path().join(".comp/config.json"),
        format!(r#"{{"conversationRecording": {{"enabled": {}}}}}"#, recording_on),
    )
    .unwrap();
    d
}

fn history_lines(ws: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(ws.join(".comp/history")) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                for line in std::fs::read_to_string(&p).unwrap().lines() {
                    if !line.trim().is_empty() {
                        out.push(serde_json::from_str(line).unwrap());
                    }
                }
            }
        }
    }
    out
}

fn record_turn_with(ws: &Path, transcript: &str, extra: Value) -> Vec<String> {
    let tpath = ws.join("transcript.jsonl");
    std::fs::write(&tpath, transcript).unwrap();
    let mut stdin = json!({"transcript_path": tpath.to_str().unwrap(), "cwd": ws.to_str().unwrap()});
    for (k, v) in extra.as_object().unwrap() {
        stdin[k] = v.clone();
    }
    let mut written = Vec::new();
    let result = crate::mcp::record_turn::run_record_turn(
        Some(ws.to_str().unwrap()),
        None,
        &stdin.to_string(),
        1_780_000_000_000,
        &mut |_p, line| {
            written.push(line.to_string());
            Ok(())
        },
    );
    assert_eq!(crate::mcp::record_turn::exit_code(&result), 0, "{:?}", result);
    written
}

#[test]
fn integ_record_turn_writes_trace_fields() {
    let ws = workspace_with_commit(true);
    let head = git(ws.path(), &["rev-parse", "HEAD"]);
    let abs = ws.path().join("src").join("a.rs");
    let t = jsonl(&[
        user("u-77", "直して"),
        tool_use("Edit", json!({"file_path": abs.to_str().unwrap()})),
        tool_use("Write", json!({"file_path": "/elsewhere/b.rs"})),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "直しました"}]}}),
    ]);
    let written = record_turn_with(ws.path(), &t, json!({"session_id": "S-1"}));
    assert_eq!(written.len(), 1);
    let v: Value = serde_json::from_str(&written[0]).unwrap();
    assert_eq!(v["session_id"], "S-1");
    assert_eq!(v["turn_id"], "u-77");
    assert_eq!(v["kind"], "turn");
    assert_eq!(v["commit"], head.as_str());
    assert_eq!(v["files"], json!(["src/a.rs", "/elsewhere/b.rs"]), "paths under the workspace become relative with '/'");
    assert_eq!(v["agent"], "claude-code");
    assert!(v.get("parent_turn_id").is_none() && v.get("test_exit").is_none());
}

#[test]
fn integ_record_turn_fallback_turn_id_and_no_git() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".comp")).unwrap();
    std::fs::write(ws.path().join(".comp/config.json"), r#"{"conversationRecording": {"enabled": true}}"#).unwrap();
    let t = jsonl(&[json!({"type": "user", "message": {"content": "no uuid here"}})]);
    let written = record_turn_with(ws.path(), &t, json!({"session_id": "S-2"}));
    let v: Value = serde_json::from_str(&written[0]).unwrap();
    assert_eq!(v["turn_id"], "S-2-1780000000000");
    assert!(v.get("commit").is_none(), "not a git repo: no commit field, record still written");
    let written = record_turn_with(ws.path(), &t, json!({}));
    let v: Value = serde_json::from_str(&written[0]).unwrap();
    assert_eq!(v["turn_id"], "turn-1780000000000");
    assert!(v.get("session_id").is_none());
}

#[test]
fn integ_record_turn_redacts_secrets() {
    let ws = workspace_with_commit(true);
    let key = "sk-abcdefghijklmnopqrstuvwxyz0123";
    let t = jsonl(&[
        user("u1", &format!("このキー {} を使って", key)),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": format!("token={}", "abcd1234efgh5678ijkl")}]}}),
    ]);
    let written = record_turn_with(ws.path(), &t, json!({}));
    assert!(!written[0].contains(key), "{}", written[0]);
    assert!(!written[0].contains("abcd1234efgh5678ijkl"), "{}", written[0]);
    assert!(written[0].contains(REDACTED));
}

#[test]
fn integ_record_turn_off_still_writes_nothing() {
    let ws = workspace_with_commit(false);
    let written = record_turn_with(ws.path(), &jsonl(&[user("u1", "x")]), json!({"session_id": "S"}));
    assert!(written.is_empty());
}

#[test]
fn integ_append_history_accepts_trace_fields_and_redacts() {
    let ws = workspace_with_commit(true);
    let payload = json!({
        "request": "add を実装 sk-abcdefghijklmnopqrstuvwxyz0123",
        "outcome": "codex の委譲は採用",
        "files": ["calc.py"],
        "kind": "delegation",
        "session_id": "S-1",
        "parent_turn_id": "u-77",
        "test_exit": 0,
        "agent": "spoofed",
        "timestamp": 5
    });
    crate::mcp::run_append_history(ws.path().to_str().unwrap(), "codex", payload.to_string().as_bytes()).unwrap();
    let lines = history_lines(ws.path());
    assert_eq!(lines.len(), 1);
    let v = &lines[0];
    assert_eq!(v["agent"], "codex");
    assert_ne!(v["timestamp"], 5);
    assert_eq!(v["kind"], "delegation");
    assert_eq!(v["session_id"], "S-1");
    assert_eq!(v["parent_turn_id"], "u-77");
    assert_eq!(v["test_exit"], 0);
    assert_eq!(v["files"], json!(["calc.py"]));
    assert!(!v["query"].as_str().unwrap().contains("sk-abcdefghijklmnopqrstuvwxyz0123"));
}

#[test]
fn integ_append_history_rejects_unknown_kind_and_keeps_old_payload() {
    let ws = workspace_with_commit(true);
    let bad = json!({"request": "x", "kind": "other"});
    assert!(crate::mcp::run_append_history(ws.path().to_str().unwrap(), "codex", bad.to_string().as_bytes()).is_err());
    assert!(history_lines(ws.path()).is_empty());
    let old = json!({"request": "old shape", "outcome": "ok"});
    crate::mcp::run_append_history(ws.path().to_str().unwrap(), "hook", old.to_string().as_bytes()).unwrap();
    let v = &history_lines(ws.path())[0];
    assert_eq!(v["query"], "old shape");
    for k in ["kind", "session_id", "turn_id", "parent_turn_id", "commit", "test_exit"] {
        assert!(v.get(k).is_none(), "{} must be omitted", k);
    }
}

async fn server(ws: &Path) -> MCPServer {
    let state = Arc::new(crate::AppState::new(ws.to_str().unwrap(), "claude-code").await.expect("AppState"));
    MCPServer::new(state)
}

fn write_history(ws: &Path, calls: &[SessionCall]) {
    let dir = ws.join(".comp/history");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = calls.iter().map(|c| serde_json::to_string(c).unwrap() + "\n").collect();
    std::fs::write(dir.join("log-2026-10.jsonl"), body).unwrap();
}

#[tokio::test]
async fn integ_session_recall_shows_delegations_under_their_turn() {
    let ws = workspace_with_commit(true);
    let base = 1_780_000_000_000u64;
    let mut t1 = turn("S1", "u-1", base);
    t1.query = "最初の依頼".into();
    let mut d = deleg(Some("S1"), None, base + 500);
    d.query = "calc を実装".into();
    d.outcome = Some("codex の委譲は採用（試行: pass）".into());
    d.test_exit = Some(0);
    let mut t2 = turn("S1", "u-2", base + 1000);
    t2.query = "二番目の依頼".into();
    t2.commit = Some("0123456789abcdef0123456789abcdef01234567".into());
    write_history(ws.path(), &[t1, d, t2]);
    let s = server(ws.path()).await;
    let md = s.handle_session_recall(json!({})).await.unwrap();
    let md = md.as_str().unwrap().to_string();
    let second = md.find("二番目の依頼").expect("turn 2 shown");
    let deleg_line = md[second..].find("**Delegations**").expect("delegations listed under turn 2") + second;
    let next_entry = md[second..].find("\n- `").map(|x| x + second).unwrap_or(md.len());
    assert!(deleg_line < next_entry, "Delegations must belong to turn 2's entry:\n{}", md);
    assert!(md[deleg_line..next_entry].contains("codex"), "{}", md);
    assert!(md[second..next_entry].contains("**Commit**: 01234567"), "{}", md);
    let d_entry = md.find("calc を実装").expect("delegation entry shown");
    let d_end = md[d_entry..].find("\n- `").map(|x| x + d_entry).unwrap_or(md.len());
    assert!(md[d_entry..d_end].contains("**Delegated by**"), "{}", md);
    assert!(md[d_entry..d_end].contains("二番目の依頼"), "{}", md);
}

#[tokio::test]
async fn integ_session_recall_query_finds_paraphrase() {
    let ws = workspace_with_commit(true);
    let base = 1_780_000_000_000u64;
    write_history(ws.path(), &[
        call("セッション検索を部分一致からBM25へ", Some("文字2-gram で日本語に対応"), base),
        call("関係のない作業", None, base + 1),
    ]);
    let s = server(ws.path()).await;
    let md = s.handle_session_recall(json!({"query": "検索 日本語"})).await.unwrap();
    let md = md.as_str().unwrap();
    assert!(md.contains("セッション検索を部分一致からBM25へ"), "{}", md);
    assert!(!md.contains("関係のない作業"), "{}", md);
}

#[tokio::test]
async fn integ_session_log_redacts_secrets() {
    let ws = workspace_with_commit(true);
    let s = server(ws.path()).await;
    s.handle_session_log(json!({"request": "use AKIAABCDEFGHIJKLMNOP", "outcome": "password=Secr3tPassw0rdValue"}))
        .await
        .unwrap();
    let lines = history_lines(ws.path());
    let text = serde_json::to_string(&lines).unwrap();
    assert!(!text.contains("AKIAABCDEFGHIJKLMNOP") && !text.contains("Secr3tPassw0rdValue"), "{}", text);
}

#[test]
fn integ_compact_history_fold_cli_parse() {
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match crate::mcp::parse_cli_subcommand(&args(&["comp-daemon", "compact-history", "/ws", "--fold-after-days", "30"])) {
        Some(crate::mcp::CliSubcommand::CompactHistoryFold { workspace_root, days }) => {
            assert_eq!((workspace_root.as_str(), days), ("/ws", 30));
        }
        other => panic!("unexpected {:?}", other.is_some()),
    }
    for bad in [["comp-daemon", "compact-history", "/ws", "--fold-after-days", "0"],
                ["comp-daemon", "compact-history", "/ws", "--fold-after-days", "x"],
                ["comp-daemon", "compact-history", "/ws", "--fold", "30"]] {
        assert!(crate::mcp::parse_cli_subcommand(&args(&bad)).is_none(), "{:?}", bad);
    }
}

#[test]
fn integ_compact_history_fold_folds_old_and_keeps_backup() {
    let ws = workspace_with_commit(true);
    let now = 1_780_000_000_000u64;
    let long = "い".repeat(300);
    let old = call(&long, None, now - 60 * DAY);
    let recent = call(&long, None, now - DAY);
    write_history(ws.path(), &[old.clone(), old, recent]);
    let results = crate::mcp::run_compact_history_fold(ws.path().to_str().unwrap(), 30, now).unwrap();
    assert_eq!(results.len(), 1);
    let lines = history_lines(ws.path());
    assert_eq!(lines.len(), 2, "exact duplicate removed");
    assert_eq!(lines[0]["query"].as_str().unwrap().chars().count(), FOLD_CHARS);
    assert_eq!(lines[1]["query"].as_str().unwrap().chars().count(), 300);
    let has_bak = std::fs::read_dir(ws.path().join(".comp/history"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".bak"));
    assert!(has_bak, "a .bak snapshot must be kept");
}

#[test]
fn integ_old_records_parse_and_serialize_unchanged() {
    let old = r#"{"query":"q","outcome":"o","timestamp":1,"agent":"a"}"#;
    let c: SessionCall = serde_json::from_str(old).unwrap();
    assert!(c.session_id.is_none() && c.kind.is_none() && c.test_exit.is_none());
    let v: Value = serde_json::to_value(&c).unwrap();
    for k in ["session_id", "turn_id", "parent_turn_id", "kind", "commit", "test_exit", "rules"] {
        assert!(v.get(k).is_none(), "{}", k);
    }
}
