//! Tests for record_turn. Owned by the designer role; implementers must not edit.

use super::*;
use serde_json::{json, Value};
use std::path::Path;

const NOW: u64 = 1_790_000_000_000; // 2026-09-21T14:13:20Z

// ---- transcript line builders (shapes copied from a real Claude Code transcript) ----

fn user_text(text: &str) -> Value {
    json!({"type": "user", "isSidechain": false, "uuid": "u",
           "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
}
fn user_blocks(blocks: Value) -> Value {
    json!({"type": "user", "isSidechain": false, "message": {"role": "user", "content": blocks}})
}
fn user_string(text: &str) -> Value {
    json!({"type": "user", "isSidechain": false, "message": {"role": "user", "content": text}})
}
fn tool_result() -> Value {
    json!({"type": "user", "isSidechain": false, "toolUseResult": {"stdout": "x"},
           "message": {"role": "user", "content": [
               {"type": "tool_result", "tool_use_id": "t1", "content": "output text"}]}})
}
fn assistant_text(text: &str) -> Value {
    json!({"type": "assistant", "isSidechain": false,
           "message": {"role": "assistant", "content": [
               {"type": "thinking", "thinking": "hidden"},
               {"type": "text", "text": text}]}})
}
fn assistant_tool_use() -> Value {
    json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]}})
}
fn last_prompt(text: &str) -> Value {
    json!({"type": "last-prompt", "lastPrompt": text, "sessionId": "s"})
}
fn jsonl(lines: &[Value]) -> String {
    lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n") + "\n"
}

// ---- extract_turn: request selection ----

#[test]
fn request_from_real_shape_message_content() {
    // Regression: the old hook read top-level `content` and recorded nothing.
    let t = jsonl(&[user_text("fix the bug"), assistant_text("done")]);
    let e = extract_turn(&t, None).expect("entry");
    assert_eq!(e.request, "fix the bug");
    assert_eq!(e.outcome.as_deref(), Some("done"));
}

#[test]
fn top_level_content_only_is_not_a_request() {
    let t = jsonl(&[json!({"type": "user", "content": "old shape"})]);
    assert_eq!(extract_turn(&t, None), None);
}

#[test]
fn last_prompt_line_wins_over_user_lines() {
    let t = jsonl(&[user_text("with <ide_selection> noise"), last_prompt("clean prompt"),
                    assistant_text("a")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "clean prompt");
}

#[test]
fn latest_last_prompt_is_used() {
    let t = jsonl(&[last_prompt("first"), user_text("x"), last_prompt("second"), last_prompt("third")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "third");
}

#[test]
fn user_line_after_last_prompt_wins() {
    // TCR-1: a newer user line beats an older last-prompt line (Stop may run first).
    let t = jsonl(&[user_text("old"), last_prompt("old"), user_text("new prompt"), assistant_text("a")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "new prompt");
}

#[test]
fn text_between_system_reminders_is_kept() {
    let t = jsonl(&[user_text("<system-reminder>a</system-reminder>
body
<system-reminder>b</system-reminder>")]);
    assert!(extract_turn(&t, None).unwrap().request.contains("body"));
}

#[test]
fn falls_back_to_user_lines_without_last_prompt() {
    let t = jsonl(&[user_text("one"), assistant_text("a1"), user_text("two"), assistant_text("a2")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "two");
}

#[test]
fn string_content_is_accepted() {
    let t = jsonl(&[user_string("  plain string prompt  ")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "plain string prompt");
}

#[test]
fn tool_result_lines_are_not_requests() {
    let t = jsonl(&[user_text("real prompt"), assistant_tool_use(), tool_result(),
                    assistant_tool_use(), tool_result(), assistant_text("final")]);
    let e = extract_turn(&t, None).unwrap();
    assert_eq!(e.request, "real prompt");
    assert_eq!(e.outcome.as_deref(), Some("final"));
}

#[test]
fn only_tool_results_means_no_request() {
    let t = jsonl(&[assistant_tool_use(), tool_result(), tool_result()]);
    assert_eq!(extract_turn(&t, None), None);
}

#[test]
fn sidechain_and_meta_user_lines_are_skipped() {
    let mut side = user_text("subagent prompt");
    side["isSidechain"] = json!(true);
    let mut meta = user_text("meta caveat");
    meta["isMeta"] = json!(true);
    let t = jsonl(&[user_text("main prompt"), side, meta]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "main prompt");
}

#[test]
fn system_reminder_only_blocks_are_dropped_and_rest_joined() {
    let t = jsonl(&[user_blocks(json!([
        {"type": "text", "text": "<system-reminder>\nctx\n</system-reminder>"},
        {"type": "text", "text": "line one"},
        {"type": "image", "source": {}},
        {"type": "text", "text": "line two"}
    ]))]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "line one\nline two");
}

#[test]
fn user_line_with_only_system_reminder_is_not_a_request() {
    let t = jsonl(&[user_text("earlier prompt"),
                    user_text("<system-reminder>only this</system-reminder>")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "earlier prompt");
}

#[test]
fn short_system_reminder_fragments_do_not_panic() {
    // TCR-3: a block shorter than the closing tag once underflowed `len - CLOSE.len()`.
    for frag in ["<system-reminder>", "<system-reminder>x", "<system-reminder></system-reminde"] {
        let t = jsonl(&[user_text(frag)]);
        assert_eq!(extract_turn(&t, None).unwrap().request, frag, "fragment {:?}", frag);
    }
}

#[test]
fn broken_and_empty_lines_are_skipped() {
    let t = format!("{{not json\n\n{}\n[1,2]\n\"str\"\n{}\n",
                    user_text("ok prompt"), assistant_text("ok answer"));
    let e = extract_turn(&t, None).unwrap();
    assert_eq!(e.request, "ok prompt");
    assert_eq!(e.outcome.as_deref(), Some("ok answer"));
}

#[test]
fn empty_transcript_has_no_request() {
    for t in ["", "\n\n", "{}\n"] {
        assert_eq!(extract_turn(t, None), None, "input {:?}", t);
    }
}

#[test]
fn blank_last_prompt_falls_back_to_user_line() {
    let t = jsonl(&[user_text("real"), last_prompt("   ")]);
    assert_eq!(extract_turn(&t, None).unwrap().request, "real");
}

// ---- extract_turn: outcome selection ----

#[test]
fn last_assistant_message_wins() {
    let t = jsonl(&[user_text("q"), assistant_text("from transcript")]);
    let e = extract_turn(&t, Some("from hook")).unwrap();
    assert_eq!(e.outcome.as_deref(), Some("from hook"));
}

#[test]
fn blank_last_assistant_message_falls_back() {
    for blank in ["", "   ", "\n\t"] {
        let t = jsonl(&[user_text("q"), assistant_text("from transcript")]);
        let e = extract_turn(&t, Some(blank)).unwrap();
        assert_eq!(e.outcome.as_deref(), Some("from transcript"), "blank {:?}", blank);
    }
}

#[test]
fn outcome_skips_tool_use_only_assistant_lines() {
    let t = jsonl(&[user_text("q"), assistant_text("text answer"), assistant_tool_use()]);
    assert_eq!(extract_turn(&t, None).unwrap().outcome.as_deref(), Some("text answer"));
}

#[test]
fn no_assistant_text_gives_none_outcome() {
    let t = jsonl(&[user_text("q"), assistant_tool_use()]);
    assert_eq!(extract_turn(&t, None).unwrap().outcome, None);
}

// ---- truncation (chars, not bytes) ----

#[test]
fn truncation_boundaries() {
    let cases: &[(usize, usize)] = &[(0, 0), (1, 1), (599, 599), (600, 600), (601, 600), (5000, 600)];
    for &(len, want) in cases {
        if len == 0 { continue; }
        for ch in ['a', 'あ', '🦀'] {
            let req: String = std::iter::repeat(ch).take(len).collect();
            let t = jsonl(&[user_text(&req)]);
            let e = extract_turn(&t, None).unwrap();
            assert_eq!(e.request.chars().count(), want, "len {} ch {:?}", len, ch);
            assert!(e.request.chars().all(|c| c == ch));
        }
    }
    for &(len, want) in &[(399usize, 399usize), (400, 400), (401, 400), (2000, 400)] {
        for ch in ['b', '日', '🙂'] {
            let out: String = std::iter::repeat(ch).take(len).collect();
            let t = jsonl(&[user_text("q")]);
            let e = extract_turn(&t, Some(&out)).unwrap();
            assert_eq!(e.outcome.unwrap().chars().count(), want, "len {} ch {:?}", len, ch);
        }
    }
}

// ---- run_record_turn ----

struct Ws {
    dir: tempfile::TempDir,
}
impl Ws {
    /// A workspace with conversation recording switched on (TCR-4: recording is opt-in).
    fn new() -> Self {
        let ws = Ws::bare();
        ws.config(r#"{"conversationRecording": {"enabled": true, "agents": {"claude-code": true}}}"#);
        ws
    }
    /// A workspace with no .comp/config.json at all.
    fn bare() -> Self { Ws { dir: tempfile::tempdir().unwrap() } }
    fn config(&self, text: &str) {
        let dir = self.path().join(".comp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), text).unwrap();
    }
    fn path(&self) -> &Path { self.dir.path() }
    fn s(&self) -> String { self.path().to_string_lossy().into_owned() }
    fn transcript(&self, lines: &[Value]) -> String {
        let p = self.path().join("transcript.jsonl");
        std::fs::write(&p, jsonl(lines)).unwrap();
        p.to_string_lossy().into_owned()
    }
    fn hist_dir(&self) -> PathBuf { self.path().join(".comp").join("history") }
    fn spills(&self) -> Vec<PathBuf> {
        match std::fs::read_dir(self.hist_dir()) {
            Ok(rd) => rd.flatten().map(|e| e.path())
                .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("spill-")).collect(),
            Err(_) => vec![],
        }
    }
}

fn hook_json(transcript: &str, cwd: &str, extra: Value) -> String {
    let mut v = json!({"session_id": "s", "transcript_path": transcript, "cwd": cwd,
                       "hook_event_name": "Stop", "stop_hook_active": false});
    if let Value::Object(m) = extra {
        for (k, val) in m { v[k] = val; }
    }
    v.to_string()
}

/// Real append into a temp file (no locking needed for single-threaded tests).
fn real_append(calls: &mut Vec<(PathBuf, String)>) -> impl FnMut(&Path, &str) -> anyhow::Result<()> + '_ {
    move |p: &Path, line: &str| {
        std::fs::create_dir_all(p.parent().unwrap())?;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
        writeln!(f, "{}", line)?;
        calls.push((p.to_path_buf(), line.to_string()));
        Ok(())
    }
}

#[test]
fn records_one_line_end_to_end() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("do it"), assistant_text("did it")]);
    let mut calls = vec![];
    let r = run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), NOW,
                            &mut real_append(&mut calls));
    let want = ws.hist_dir().join("log-2026-09.jsonl");
    assert_eq!(r, RecordResult::Recorded { hist_path: want.clone() });
    assert_eq!(exit_code(&r), 0);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, want);
    let v: Value = serde_json::from_str(&calls[0].1).unwrap();
    assert_eq!(v["query"], "do it");
    assert_eq!(v["outcome"], "did it");
    assert_eq!(v["agent"], "claude-code");
    assert_eq!(v["timestamp"], NOW);
    // Round-trips into the shared record type that session_recall reads.
    let call: crate::mcp::SessionCall = serde_json::from_str(&calls[0].1).unwrap();
    assert_eq!(call.agent, "claude-code");
    assert!(ws.spills().is_empty());
}

#[test]
fn workspace_precedence_arg_then_env_then_cwd() {
    let arg = Ws::new();
    let env = Ws::new();
    let cwd = Ws::new();
    let tr = cwd.transcript(&[user_text("q")]);
    let cases: Vec<(Option<String>, Option<String>, &Ws)> = vec![
        (Some(arg.s()), Some(env.s()), &arg),
        (None, Some(env.s()), &env),
        (None, None, &cwd),
        (Some(arg.s()), None, &arg),
    ];
    for (a, e, expected) in cases {
        let mut calls = vec![];
        let r = run_record_turn(a.as_deref(), e.as_deref(), &hook_json(&tr, &cwd.s(), json!({})), NOW,
                                &mut real_append(&mut calls));
        assert_eq!(r, RecordResult::Recorded { hist_path: expected.hist_dir().join("log-2026-09.jsonl") });
    }
}

#[test]
fn blank_env_project_dir_is_ignored() {
    let cwd = Ws::new();
    let tr = cwd.transcript(&[user_text("q")]);
    for blank in ["", "  "] {
        let mut calls = vec![];
        let r = run_record_turn(None, Some(blank), &hook_json(&tr, &cwd.s(), json!({})), NOW,
                                &mut real_append(&mut calls));
        assert_eq!(r, RecordResult::Recorded { hist_path: cwd.hist_dir().join("log-2026-09.jsonl") });
    }
}

#[test]
fn no_workspace_anywhere_fails_with_exit_1() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("q")]);
    let stdin = json!({"transcript_path": tr}).to_string();
    let mut calls = vec![];
    let r = run_record_turn(None, None, &stdin, NOW, &mut real_append(&mut calls));
    assert!(matches!(r, RecordResult::Failed { .. }), "{:?}", r);
    assert_eq!(exit_code(&r), 1);
    assert!(calls.is_empty());
}

#[test]
fn invalid_stdin_fails_with_exit_1() {
    let ws = Ws::new();
    for bad in ["", "not json", "[1,2]", "\"s\"", "{"] {
        let mut calls = vec![];
        let r = run_record_turn(Some(&ws.s()), None, bad, NOW, &mut real_append(&mut calls));
        assert!(matches!(r, RecordResult::Failed { .. }), "input {:?} gave {:?}", bad, r);
        assert_eq!(exit_code(&r), 1);
        assert!(calls.is_empty());
    }
    assert!(ws.spills().is_empty());
}

#[test]
fn nothing_to_record_cases_exit_0_and_write_nothing() {
    let ws = Ws::new();
    let no_req = ws.transcript(&[assistant_tool_use(), tool_result()]);
    let missing = ws.path().join("nope.jsonl").to_string_lossy().into_owned();
    let stdins = vec![
        json!({"cwd": ws.s()}).to_string(),                              // no transcript_path
        json!({"cwd": ws.s(), "transcript_path": null}).to_string(),
        json!({"cwd": ws.s(), "transcript_path": ""}).to_string(),
        hook_json(&missing, &ws.s(), json!({})),                         // file missing
        hook_json(&no_req, &ws.s(), json!({})),                          // no request
    ];
    for stdin in stdins {
        let mut calls = vec![];
        let r = run_record_turn(Some(&ws.s()), None, &stdin, NOW, &mut real_append(&mut calls));
        assert!(matches!(r, RecordResult::NothingToRecord { .. }), "{} gave {:?}", stdin, r);
        assert_eq!(exit_code(&r), 0);
        assert!(calls.is_empty());
    }
    assert!(ws.spills().is_empty());
}

#[test]
fn stop_hook_active_still_records() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("q"), assistant_text("a")]);
    let mut calls = vec![];
    let r = run_record_turn(Some(&ws.s()), None,
                            &hook_json(&tr, &ws.s(), json!({"stop_hook_active": true})), NOW,
                            &mut real_append(&mut calls));
    assert!(matches!(r, RecordResult::Recorded { .. }));
    assert_eq!(calls.len(), 1);
}

#[test]
fn last_assistant_message_from_stdin_is_recorded() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("q"), assistant_text("old")]);
    let mut calls = vec![];
    run_record_turn(Some(&ws.s()), None,
                    &hook_json(&tr, &ws.s(), json!({"last_assistant_message": "new"})), NOW,
                    &mut real_append(&mut calls));
    let v: Value = serde_json::from_str(&calls[0].1).unwrap();
    assert_eq!(v["outcome"], "new");
}

#[test]
fn agent_cannot_be_spoofed() {
    let ws = Ws::new();
    let mut u = user_text("q");
    u["agent"] = json!("evil");
    u["message"]["agent"] = json!("evil");
    let tr = ws.transcript(&[u]);
    let mut calls = vec![];
    run_record_turn(Some(&ws.s()), None,
                    &hook_json(&tr, &ws.s(), json!({"agent": "evil", "agent_id": "evil"})), NOW,
                    &mut real_append(&mut calls));
    let v: Value = serde_json::from_str(&calls[0].1).unwrap();
    assert_eq!(v["agent"], "claude-code");
}

#[test]
fn append_failure_writes_spill_and_exits_1() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("keep me"), assistant_text("answer")]);
    let mut failing = |_: &Path, _: &str| -> anyhow::Result<()> { Err(anyhow::anyhow!("lock busy")) };
    let r = run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), NOW, &mut failing);
    assert_eq!(exit_code(&r), 1);
    let spill = match &r {
        RecordResult::Failed { spill: Some(p), .. } => p.clone(),
        other => panic!("expected Failed with spill, got {:?}", other),
    };
    assert_eq!(spill.parent().unwrap(), ws.hist_dir());
    let name = spill.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("spill-") && name.ends_with(".jsonl"), "{}", name);
    assert!(name.contains(&NOW.to_string()), "{}", name);
    assert_eq!(ws.spills(), vec![spill.clone()]);
    let text = std::fs::read_to_string(&spill).unwrap();
    assert_eq!(text.lines().count(), 1);
    let call: crate::mcp::SessionCall = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(call.query, "keep me");
    assert_eq!(call.agent, "claude-code");
}

#[test]
fn two_failures_make_two_distinct_spills() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("q")]);
    let mut failing = |_: &Path, _: &str| -> anyhow::Result<()> { Err(anyhow::anyhow!("x")) };
    run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), NOW, &mut failing);
    run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), NOW, &mut failing);
    assert_eq!(ws.spills().len(), 2);
}

#[test]
fn exit_code_is_never_2() {
    let results = vec![
        RecordResult::Recorded { hist_path: PathBuf::from("x") },
        RecordResult::NothingToRecord { reason: "r".into() },
        RecordResult::Failed { reason: "r".into(), spill: None },
        RecordResult::Failed { reason: "r".into(), spill: Some(PathBuf::from("s")) },
    ];
    let codes: Vec<i32> = results.iter().map(exit_code).collect();
    assert_eq!(codes, vec![0, 0, 1, 1]);
}

#[test]
fn month_file_follows_now_ms() {
    let ws = Ws::new();
    let tr = ws.transcript(&[user_text("q")]);
    // 2026-01-01T00:00:00Z, 2025-12-31T23:59:59Z, 2026-02-28T12:00:00Z
    for (ms, month) in [(1_767_225_600_000u64, "2026-01"), (1_767_225_599_000, "2025-12"),
                        (1_772_280_000_000, "2026-02")] {
        let mut calls = vec![];
        let r = run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), ms,
                                &mut real_append(&mut calls));
        assert_eq!(r, RecordResult::Recorded {
            hist_path: ws.hist_dir().join(format!("log-{}.jsonl", month)) });
    }
}

// ---- conversation recording switch (beta, opt-in) ----

fn assert_off(ws: &Ws, expect_in_reason: &str) {
    let tr = ws.transcript(&[user_text("secret prompt"), assistant_text("a")]);
    let mut calls = vec![];
    let r = run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), NOW,
                            &mut real_append(&mut calls));
    match &r {
        RecordResult::NothingToRecord { reason } => {
            assert!(reason.contains(expect_in_reason), "reason {:?} lacks {:?}", reason, expect_in_reason)
        }
        other => panic!("expected NothingToRecord, got {:?}", other),
    }
    assert_eq!(exit_code(&r), 0);
    assert!(calls.is_empty());
    assert!(ws.spills().is_empty());
    assert!(!ws.hist_dir().join("log-2026-09.jsonl").exists());
}

#[test]
fn recording_is_off_without_config_file() {
    assert_off(&Ws::bare(), "off");
}

#[test]
fn recording_is_off_unless_enabled_is_literally_true() {
    for cfg in [
        r#"{}"#,
        r#"{"exclude": []}"#,
        r#"{"conversationRecording": {}}"#,
        r#"{"conversationRecording": {"enabled": false}}"#,
        r#"{"conversationRecording": {"enabled": "true"}}"#,
        r#"{"conversationRecording": {"enabled": 1}}"#,
        r#"{"conversationRecording": {"enabled": null}}"#,
        r#"{"conversationRecording": true}"#,
    ] {
        let ws = Ws::bare();
        ws.config(cfg);
        assert_off(&ws, "off");
    }
}

#[test]
fn recording_is_off_when_claude_code_agent_is_disabled() {
    for agents in [r#"{"claude-code": false}"#] {
        let ws = Ws::bare();
        ws.config(&format!(r#"{{"conversationRecording": {{"enabled": true, "agents": {}}}}}"#, agents));
        assert_off(&ws, "off");
    }
}

#[test]
fn invalid_config_json_is_off_and_says_so() {
    for bad in ["{", "", "not json", "[1]"] {
        let ws = Ws::bare();
        ws.config(bad);
        assert_off(&ws, "config.json");
    }
}

#[test]
fn enabled_without_agents_map_records() {
    for cfg in [
        r#"{"conversationRecording": {"enabled": true}}"#,
        r#"{"conversationRecording": {"enabled": true, "agents": {}}}"#,
        r#"{"conversationRecording": {"enabled": true, "agents": {"gemini-cli": false}}}"#,
        r#"{"exclude": ["x"], "conversationRecording": {"enabled": true, "agents": {"claude-code": true}}}"#,
    ] {
        let ws = Ws::bare();
        ws.config(cfg);
        let tr = ws.transcript(&[user_text("q")]);
        let mut calls = vec![];
        let r = run_record_turn(Some(&ws.s()), None, &hook_json(&tr, &ws.s(), json!({})), NOW,
                                &mut real_append(&mut calls));
        assert!(matches!(r, RecordResult::Recorded { .. }), "{} gave {:?}", cfg, r);
    }
}

#[test]
fn switch_is_checked_before_the_transcript_is_touched() {
    // Off + a transcript that does not exist: the reason must be the switch, not the file.
    let ws = Ws::bare();
    let missing = ws.path().join("nope.jsonl").to_string_lossy().into_owned();
    let mut calls = vec![];
    let r = run_record_turn(Some(&ws.s()), None, &hook_json(&missing, &ws.s(), json!({})), NOW,
                            &mut real_append(&mut calls));
    match r {
        RecordResult::NothingToRecord { reason } => assert!(reason.contains("off"), "{}", reason),
        other => panic!("{:?}", other),
    }
}
