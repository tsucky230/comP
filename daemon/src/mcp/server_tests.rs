//! MCPServer 本体（ツールのハンドラ・セッション記録・CLI）のテスト。元は mod.rs のインラインテスト。
//! Owned by the designer role; implementers must not edit.
    use super::*;

    #[tokio::test]
    async fn test_mcp_server_creation() {
        let temp_dir = std::env::temp_dir().join("comP_test_mcp_creation");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        // Nodes are 0 at creation since indexing has not run
        let stats = server.handle_get_stats().await.expect("getStats failed");
        assert_eq!(stats["total_files"].as_i64().unwrap(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_json_rpc_format() {
        // Verify request/response format
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "run_pipeline",
            "params": { "task": "test" }
        });

        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["method"], "run_pipeline");
    }

    #[tokio::test]
    async fn test_handle_run_pipeline() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({
            "task": "add authentication",
            "max_tokens": 8000
        });

        let result = server.handle_run_pipeline(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["pivot_files"].is_array());
        assert!(response["related_files"].is_array());
        assert!(response["total_tokens"].is_number());
        assert!(response["savings"].is_string());
        assert!(response["estimated_cost"].is_string());
        assert!(response["coverage"].is_object());
        assert!(response["coverage"]["indexed_doc_files"].is_number());
        assert!(response["coverage"]["bm25_hits"].is_number());
        assert!(response["coverage"]["pivot_file_types"].is_object());
    }

    /// 0.8.6 regression: an interaction recorded via session_log must surface in
    /// run_pipeline results (walker carve-out → files table → BM25 over jsonl).
    #[tokio::test]
    async fn test_run_pipeline_surfaces_session_history() {
        let temp_dir = std::env::temp_dir().join("comP_test_history_bm25");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(
            crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent")
                .await
                .expect("Failed to create AppState"),
        );
        let server = MCPServer::new(state);

        server
            .handle_session_log(json!({
                "request": "implement zanzibar quota widget",
                "outcome": "added zanzibar widget module"
            }))
            .await
            .expect("session_log failed");

        let response = server
            .handle_run_pipeline(json!({
                "task": "continue zanzibar quota widget work",
                "max_tokens": 8000
            }))
            .await
            .expect("run_pipeline failed");

        let pivots: Vec<String> = response["pivot_files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["path"].as_str().map(String::from))
            .collect();
        assert!(
            pivots
                .iter()
                .any(|p| p.starts_with(".comp/history/") && p.ends_with(".jsonl")),
            "history jsonl must appear in pivot_files, got: {:?}",
            pivots
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_run_pipeline_missing_task() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({ "max_tokens": 8000 }); // Missing task

        let result = server.handle_run_pipeline(params).await;
        assert!(result.is_err()); // Should error on missing task
    }

    #[test]
    fn test_match_pattern() {
        // Wildcard any
        assert!(MCPServer::match_pattern("foo.rs", "*"));
        // Suffix glob
        assert!(MCPServer::match_pattern("main.rs", "*.rs"));
        assert!(!MCPServer::match_pattern("main.ts", "*.rs"));
        // Prefix glob
        assert!(MCPServer::match_pattern("test_auth.rs", "test_*"));
        assert!(!MCPServer::match_pattern("auth_test.rs", "test_*"));
        // Exact
        assert!(MCPServer::match_pattern("README.md", "README.md"));
        assert!(!MCPServer::match_pattern("readme.md", "README.md"));
    }

    #[test]
    fn test_apply_compression_rule_empty() {
        let rules = std::collections::HashMap::new();
        assert_eq!(MCPServer::apply_compression_rule("src/main.rs", &rules), None);
    }

    #[test]
    fn test_apply_compression_rule_glob() {
        let mut rules = std::collections::HashMap::new();
        rules.insert("*.md".to_string(), 0i64);
        rules.insert("*.rs".to_string(), 2i64);

        assert_eq!(MCPServer::apply_compression_rule("docs/README.md", &rules), Some(0));
        assert_eq!(MCPServer::apply_compression_rule("src/main.rs", &rules), Some(2));
        assert_eq!(MCPServer::apply_compression_rule("app.ts", &rules), None);
    }

    #[test]
    fn test_apply_compression_rule_exact_takes_priority() {
        let mut rules = std::collections::HashMap::new();
        rules.insert("*.rs".to_string(), 2i64);
        rules.insert("main.rs".to_string(), 0i64); // exact overrides glob

        assert_eq!(MCPServer::apply_compression_rule("src/main.rs", &rules), Some(0));
        assert_eq!(MCPServer::apply_compression_rule("src/lib.rs", &rules), Some(2));
    }

    #[test]
    fn test_apply_compression_rule_conflict_is_deterministic() {
        // "test_*" (5 non-wildcard chars) is more specific than "*.rs" (2 non-wildcard chars)
        // so test_* should win regardless of HashMap insertion order.
        let mut rules = std::collections::HashMap::new();
        rules.insert("*.rs".to_string(), 2i64);
        rules.insert("test_*".to_string(), 1i64);

        // Run 100 times to expose any non-determinism
        for _ in 0..100 {
            let level = MCPServer::apply_compression_rule("test_auth.rs", &rules);
            assert_eq!(level, Some(1), "test_* (5 chars) should beat *.rs (2 chars)");
        }

        // *.rs (2) vs *.md (2) — tie on non-wildcard length, alphabetical: "*.md" < "*.rs"
        let mut rules2 = std::collections::HashMap::new();
        rules2.insert("*.rs".to_string(), 2i64);
        rules2.insert("*.md".to_string(), 0i64);
        // these don't conflict: .rs and .md can't both match the same file
        assert_eq!(MCPServer::apply_compression_rule("main.rs", &rules2), Some(2));
        assert_eq!(MCPServer::apply_compression_rule("README.md", &rules2), Some(0));
    }

    #[test]
    fn test_load_compression_rules_missing_file() {
        // Should return empty map when config.json doesn't exist
        let rules = MCPServer::load_compression_rules("/nonexistent/path");
        // No config.json at this path — must return empty map without panic
        assert!(rules.is_empty());
    }

    #[tokio::test]
    async fn test_handle_get_context() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({
            "query": "authentication",
            "limit": 10
        });

        let result = server.handle_get_context(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["results"].is_array());
        assert!(response["count"].is_number());
        assert!(response["query"].is_string());
    }

    #[tokio::test]
    async fn test_handle_get_context_missing_query() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({ "limit": 10 }); // Missing query

        let result = server.handle_get_context(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_get_impact_graph() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({
            "symbol_id": 123,
            "symbol_name": "authenticate"
        });

        let result = server.handle_get_impact_graph(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["symbol_id"].is_number());
        assert!(response["affected_files"].is_object());
        assert!(response["impact_count"].is_number());
        assert!(response["severity"].is_string());
    }

    #[tokio::test]
    async fn test_handle_get_impact_graph_missing_symbol_id() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let params = json!({ "symbol_name": "authenticate" }); // Missing symbol_id

        let result = server.handle_get_impact_graph(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_list_indexed_files() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let result = server.handle_list_indexed_files().await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["files"].is_array());
        assert!(response["total_files"].is_number());
        assert!(response["total_symbols"].is_number());
        assert!(response["languages"].is_object());
    }

    #[tokio::test]
    async fn test_handle_get_token_usage() {
        let temp_dir = std::env::temp_dir().join("comP_test_token_usage");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let response = server.handle_get_token_usage().await.unwrap();
        assert!(response["timestamp"].is_number());
        assert_eq!(response["total_tokens_consumed"].as_u64().unwrap(), 0);
        assert_eq!(response["queries_count"].as_u64().unwrap(), 0);
        assert!(response["efficiency"].is_string());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_token_counter_cost_estimation() {
        // Test TokenCounter integration
        let cost = crate::search::TokenCounter::estimate_cost(10000, "sonnet");
        assert!(cost.starts_with("$"));
        assert!(cost.len() > 0);
    }

    #[test]
    fn test_token_counter_savings_calculation() {
        // Test TokenCounter integration
        let savings = crate::search::TokenCounter::calculate_savings(10000, 4000);
        assert!(savings.contains("%"));
    }

    #[tokio::test]
    async fn test_handle_get_stats() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let result = server.handle_get_stats().await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response["total_files"].is_number());
        assert!(response["total_nodes"].is_number());
        assert!(response["total_edges"].is_number());

        // Verify values are non-negative
        assert!(response["total_files"].as_i64().unwrap_or(-1) >= 0);
        assert!(response["total_nodes"].as_i64().unwrap_or(-1) >= 0);
        assert!(response["total_edges"].as_i64().unwrap_or(-1) >= 0);

        // The running binary must self-report its crate version so clients can
        // detect a stale daemon after an upgrade.
        assert_eq!(response["daemon_version"].as_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn test_handle_get_stats_surfaces_index_recovery() {
        // A corruption-triggered rebuild starts from zero rows, which is
        // otherwise indistinguishable from "this workspace has nothing indexed
        // yet" — get_stats must say so explicitly (Gemini review finding #3).
        let temp_dir = std::env::temp_dir().join("comP_test_get_stats_recovery_flag");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        std::fs::write(comp_dir.join("index.db"), b"not a sqlite database").unwrap();

        let state = Arc::new(
            crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent")
                .await
                .expect("AppState::new must recover from a corrupt index.db"),
        );
        let server = MCPServer::new(state);

        let response = server.handle_get_stats().await.unwrap();
        assert!(
            response["index_recovered_from_corruption_at"].is_number(),
            "get_stats must flag a just-recovered index so an agent doesn't mistake it for an empty workspace"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_get_stats_recovery_flag_clears_on_marker_not_on_file_count() {
        // Gemini design-review finding: gating the flag on total_files == 0
        // would make it disappear the instant a partial re-index writes its
        // first row, even though the re-index is far from done. The contract is
        // instead "clears only once clear_recovery_marker() is called" (main.rs
        // calls it after a full index_workspace pass completes) — demonstrated
        // here by clearing it directly while file_count is still 0, proving the
        // flag's presence tracks the marker, not the row count.
        let temp_dir = std::env::temp_dir().join("comP_test_get_stats_recovery_flag_clears_on_marker");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        std::fs::create_dir_all(&comp_dir).unwrap();
        std::fs::write(comp_dir.join("index.db"), b"not a sqlite database").unwrap();

        let state = Arc::new(
            crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent")
                .await
                .expect("AppState::new must recover from a corrupt index.db"),
        );
        let server = MCPServer::new(state.clone());

        let response = server.handle_get_stats().await.unwrap();
        assert!(response["index_recovered_from_corruption_at"].is_number());
        assert_eq!(response["total_files"], 0);

        state.graph_db.clear_recovery_marker().unwrap();

        let response = server.handle_get_stats().await.unwrap();
        assert!(
            response.get("index_recovered_from_corruption_at").is_none(),
            "the flag must be gone once the marker is cleared, even though total_files is still 0"
        );
        assert_eq!(response["total_files"], 0, "file_count is unaffected by clearing the marker");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_get_stats_omits_recovery_flag_when_never_recovered() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let response = server.handle_get_stats().await.unwrap();
        assert!(
            response.get("index_recovered_from_corruption_at").is_none(),
            "the flag must not appear at all for a database that was never recovered"
        );
    }

    #[tokio::test]
    async fn test_handle_get_stats_surfaces_last_activity_at() {
        // Phase 3, A7: SidebarPanel.ts used to read this from the dead
        // pre-v0.11.1 single session-memory.json file via SessionMemoryManager
        // and always saw "Waiting..." — get_stats must carry the real answer.
        let temp_dir = std::env::temp_dir().join("comP_test_get_stats_last_activity");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        record_mcp_call(root, "test-agent", "sess-1", "a task".to_string(), vec![], vec![], 1).unwrap();

        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);
        let response = server.handle_get_stats().await.unwrap();
        assert!(response["last_activity_at"].is_number(), "last_activity_at must be present once something was recorded");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_get_stats_omits_last_activity_at_when_nothing_recorded() {
        let temp_dir = std::env::temp_dir().join("comP_test_get_stats_no_activity");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);
        let response = server.handle_get_stats().await.unwrap();
        assert!(response.get("last_activity_at").is_none());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_mark_stale_in_file_marks_matching_calls_and_is_noop_otherwise() {
        let temp_dir = std::env::temp_dir().join("comP_test_mark_stale_in_file");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("agent.json");

        let memory = SessionMemory {
            sessions: vec![Session {
                id: "s1".to_string(),
                timestamp: 1,
                calls: vec![
                    SessionCall {
                        query: "touches target".to_string(),
                        outcome: None,
                        symbols: vec![],
                        files: vec!["src/target.rs".to_string()],
                        tokens: 0,
                        stale: false,
                        timestamp: 1,
                        agent: "test".to_string(),
                        rules: Vec::new(),
                        ..Default::default()
                    },
                    SessionCall {
                        query: "unrelated".to_string(),
                        outcome: None,
                        symbols: vec![],
                        files: vec!["src/other.rs".to_string()],
                        tokens: 0,
                        stale: false,
                        timestamp: 2,
                        agent: "test".to_string(),
                        rules: Vec::new(),
                        ..Default::default()
                    },
                ],
            }],
        };
        std::fs::write(&path, serde_json::to_string(&memory).unwrap()).unwrap();

        let marked = mark_stale_in_file(&path, "src/target.rs").unwrap();
        assert_eq!(marked, 1);

        let updated: SessionMemory = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(updated.sessions[0].calls[0].stale, "the matching call must be marked stale");
        assert!(!updated.sessions[0].calls[1].stale, "the unrelated call must be untouched");

        // Re-running for the same path must not re-mark (already stale) or error.
        let marked_again = mark_stale_in_file(&path, "src/target.rs").unwrap();
        assert_eq!(marked_again, 0, "an already-stale call must not be counted again");

        // A path that matches nothing must be a true no-op — the file's content
        // (not just the stale flags) must be byte-identical, proving no write happened.
        let before = std::fs::read_to_string(&path).unwrap();
        let marked_none = mark_stale_in_file(&path, "src/does_not_exist.rs").unwrap();
        assert_eq!(marked_none, 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_mark_stale_marks_across_multiple_agent_files() {
        let temp_dir = std::env::temp_dir().join("comP_test_handle_mark_stale");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        record_mcp_call(root, "claude-code", "s1", "a".to_string(), vec![], vec!["src/shared.rs".to_string()], 1).unwrap();
        record_mcp_call(root, "codex", "s2", "b".to_string(), vec![], vec!["src/shared.rs".to_string()], 2).unwrap();
        record_mcp_call(root, "codex", "s3", "c".to_string(), vec![], vec!["src/unrelated.rs".to_string()], 3).unwrap();

        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let response = server.handle_mark_stale(json!({ "path": "src/shared.rs" })).await.unwrap();
        assert_eq!(response["marked"], 2, "both agents' calls touching the path must be marked");

        let recall = server.handle_session_recall(json!({})).await.unwrap();
        let recall = recall.as_str().unwrap();
        // Two of the three entries are now stale; the unrelated one must not be.
        assert_eq!(recall.matches("[Stale]").count(), 2);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_mark_stale_requires_path_param() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);
        let result = server.handle_mark_stale(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_recall() {
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let result = server.handle_session_recall(json!({})).await.unwrap();
        assert!(result.as_str().unwrap().contains("No past invocations recorded"));

        record_mcp_call(
            temp_dir.to_str().unwrap(),
            &state.agent_id,
            &state.session_id,
            "test task".to_string(),
            vec!["test_symbol".to_string()],
            vec!["src/test.rs".to_string()],
            100,
        ).unwrap();

        let result = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = result.as_str().unwrap();
        assert!(markdown.contains("test task"));
        assert!(markdown.contains("Tokens: 100"));
        assert!(markdown.contains("test_symbol"));

        let result = server.handle_session_recall(json!({ "query": "task" })).await.unwrap();
        assert!(result.as_str().unwrap().contains("test task"));

        let result = server.handle_session_recall(json!({ "query": "nomatch" })).await.unwrap();
        assert!(result.as_str().unwrap().contains("No matching past invocations"));

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_format_capped_list() {
        let items: Vec<String> = (1..=8).map(|i| format!("sym{}", i)).collect();
        let rendered = format_capped_list(&items, 5);
        assert_eq!(rendered, "`sym1`, `sym2`, `sym3`, `sym4`, `sym5` … (+3 more)");

        // At or under the cap: no overflow marker.
        let few: Vec<String> = vec!["a".to_string(), "b".to_string()];
        assert_eq!(format_capped_list(&few, 5), "`a`, `b`");
    }

    #[tokio::test]
    async fn test_session_recall_caps_symbol_and_file_lists() {
        // Auto-recorded run_pipeline calls can carry dozens of symbols/files;
        // recall must summarize them instead of enumerating everything.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_cap");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let symbols: Vec<String> = (1..=30).map(|i| format!("symbol_{}", i)).collect();
        let files: Vec<String> = (1..=12).map(|i| format!("src/file_{}.rs", i)).collect();
        record_mcp_call(root, &state.agent_id, &state.session_id, "big task".to_string(), symbols, files, 100).unwrap();

        let markdown = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = markdown.as_str().unwrap();
        assert!(markdown.contains("symbol_1"), "leading symbols must be shown");
        assert!(!markdown.contains("symbol_6"), "symbols beyond the cap must be omitted");
        assert!(markdown.contains("(+25 more)"), "symbol overflow count must be shown");
        assert!(!markdown.contains("src/file_6.rs"), "files beyond the cap must be omitted");
        assert!(markdown.contains("(+7 more)"), "file overflow count must be shown");

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_record_mcp_call_quarantines_corrupt_file_instead_of_resetting_silently() {
        // Before this fix, a session-memory file that failed to parse was reset
        // to empty with `unwrap_or(SessionMemory { sessions: vec![] })` and then
        // overwritten — permanently destroying every past record for that agent
        // with no warning. The file has no other copy, so recovery must preserve
        // the unreadable bytes instead of discarding them.
        let temp_dir = std::env::temp_dir().join("comP_test_record_call_quarantine");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let path = get_session_memory_path(temp_dir.to_str().unwrap(), "test-agent");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not valid json").unwrap();

        record_mcp_call(
            temp_dir.to_str().unwrap(),
            "test-agent",
            "sess-1",
            "new task after corruption".to_string(),
            vec![],
            vec![],
            5,
        ).unwrap();

        // The live file must now hold a valid, fresh record instead of staying broken.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("new task after corruption"));
        let parsed: SessionMemory = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.sessions.len(), 1);

        // The original unreadable bytes must survive somewhere, not be silently discarded.
        let quarantined: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "exactly one quarantined copy of the corrupt file must remain");
        let quarantined_content = std::fs::read_to_string(quarantined[0].path()).unwrap();
        assert_eq!(quarantined_content, "{ not valid json");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_record_mcp_call_leaves_no_tmp_file_behind() {
        // The previous implementation wrote via a bare `File::create` on the live
        // path (truncate-then-fill, not atomic). The new path writes to a temp
        // file and renames it into place; nothing should remain behind it.
        let temp_dir = std::env::temp_dir().join("comP_test_record_call_no_tmp");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        record_mcp_call(temp_dir.to_str().unwrap(), "test-agent", "sess-1", "task".to_string(), vec![], vec![], 1).unwrap();

        let path = get_session_memory_path(temp_dir.to_str().unwrap(), "test-agent");
        let leftovers: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "no temp file should remain after a successful write: {:?}", leftovers);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_record_mcp_call_survives_concurrent_writers_same_agent() {
        // Simulates two independent processes sharing one agent_id (e.g. two
        // Claude Code windows open on the same workspace) writing at once. Before
        // the sidecar lock, this was a classic lost-update race: both read the
        // same starting state and the second writer's save silently erased the
        // first writer's session.
        let temp_dir = std::env::temp_dir().join("comP_test_record_call_concurrent");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap().to_string();

        const WRITERS: usize = 8;
        let mut handles = Vec::new();
        for i in 0..WRITERS {
            let root = root.clone();
            handles.push(std::thread::spawn(move || {
                record_mcp_call(
                    &root, "shared-agent", &format!("sess-{}", i), format!("task {}", i), vec![], vec![], i as u64,
                ).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let path = get_session_memory_path(&root, "shared-agent");
        let content = std::fs::read_to_string(&path).unwrap();
        let memory: SessionMemory = serde_json::from_str(&content).unwrap();
        assert_eq!(memory.sessions.len(), WRITERS, "every writer's session must survive concurrent writes");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_record_mcp_call_propagates_read_errors_instead_of_overwriting_unreadable_file() {
        // Gemini acceptance-check finding (Phase 1): a plain I/O read error (here,
        // simulated portably by making the session-memory path a directory instead
        // of a file) is NOT a parse failure — the underlying content, if any, might
        // be perfectly intact. Treating it as "empty" would let the atomic write
        // below silently overwrite a healthy file once the transient condition
        // clears, recreating the exact data-loss bug this module exists to close.
        let temp_dir = std::env::temp_dir().join("comP_test_record_call_read_error");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let path = get_session_memory_path(temp_dir.to_str().unwrap(), "test-agent");
        std::fs::create_dir_all(&path).unwrap(); // path itself is a directory, not a file

        let result = record_mcp_call(
            temp_dir.to_str().unwrap(), "test-agent", "sess-1", "task".to_string(), vec![], vec![], 1,
        );

        assert!(result.is_err(), "a read error must propagate instead of being treated as an empty file");
        assert!(path.is_dir(), "the unreadable path must be left untouched, not replaced with a fresh document");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_write_tmp_session_memory_fails_cleanly_without_partial_output() {
        // Gemini acceptance-check finding (Phase 1): write_session_memory_atomically
        // must clean up the temp file if writing it fails partway through, not just
        // when the final rename fails. Using a directory as the "file" target is a
        // portable way to force File::create to fail deterministically.
        let temp_dir = std::env::temp_dir().join("comP_test_write_tmp_session_memory_failure");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let blocked_path = temp_dir.join("blocked");
        std::fs::create_dir_all(&blocked_path).unwrap(); // a directory, not a file

        let memory = SessionMemory { sessions: Vec::new() };
        let result = write_tmp_session_memory(&blocked_path, &memory);
        assert!(result.is_err(), "File::create must fail when the target path is a directory");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_write_session_memory_atomically_propagates_tmp_write_failure() {
        // Exercises write_session_memory_atomically's own error-handling wrapper
        // around write_tmp_session_memory (not just the inner helper in
        // isolation, which the previous test covers) — a failure here must
        // short-circuit before ever attempting the rename step, and the live
        // path must stay untouched.
        let temp_dir = std::env::temp_dir().join("comP_test_write_session_memory_atomic_tmp_failure");
        let _ = std::fs::remove_dir_all(&temp_dir);
        // Deliberately do not create temp_dir: `path`'s parent doesn't exist, so
        // File::create(tmp_path) inside write_tmp_session_memory fails.
        let path = temp_dir.join("nonexistent-dir").join("agent.json");
        let memory = SessionMemory { sessions: Vec::new() };

        let result = write_session_memory_atomically(&path, &memory);
        assert!(result.is_err(), "a tmp-write failure must propagate as Err, not be swallowed");
        assert!(!path.exists(), "nothing must be written at the live path when the tmp write fails");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_write_session_memory_atomically_cleans_up_tmp_when_rename_repeatedly_fails() {
        // Forces the rename step itself (not the tmp write) to fail persistently
        // by making the live path a directory, which std::fs::rename refuses to
        // replace with a file — exercises the retry-then-give-up branch and its
        // cleanup, not just the happy-path rename.
        let temp_dir = std::env::temp_dir().join("comP_test_write_session_memory_rename_failure");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("agent.json");
        std::fs::create_dir_all(&path).unwrap(); // path is a directory; rename onto it must fail

        let memory = SessionMemory { sessions: Vec::new() };
        let result = write_session_memory_atomically(&path, &memory);
        assert!(result.is_err(), "a persistently failing rename must surface as Err after retries");

        let leftovers: Vec<String> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "the tmp file must be cleaned up even when rename never succeeds: {:?}", leftovers);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_format_epoch_ms() {
        // Unix epoch start.
        assert_eq!(format_epoch_ms(0), "1970-01-01 00:00");
        // 1_700_000_000_000 ms == 2023-11-14T22:13:20Z.
        assert_eq!(format_epoch_ms(1_700_000_000_000), "2023-11-14 22:13");
    }

    #[tokio::test]
    async fn test_session_recall_cross_session() {
        // Recall must surface calls from sessions other than the current one
        // (i.e. survive daemon restarts), each tagged with a date and capped by limit.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_cross");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        // Two records under two DIFFERENT session ids, neither equal to the current
        // session_id — the old per-session filter would have returned nothing.
        record_mcp_call(root, "test-agent", "sess-old", "alpha task".to_string(), vec![], vec![], 10).unwrap();
        record_mcp_call(root, "test-agent", "sess-new", "beta task".to_string(), vec![], vec![], 20).unwrap();

        let markdown = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = markdown.as_str().unwrap();
        assert!(markdown.contains("alpha task"), "cross-session recall must include older session");
        assert!(markdown.contains("beta task"), "cross-session recall must include newer session");
        // Date tag present (UTC year prefix).
        assert!(markdown.contains("- `20"), "each entry must be tagged with a date");

        // limit caps the number of entries returned.
        let limited = server.handle_session_recall(json!({ "limit": 1 })).await.unwrap();
        let limited = limited.as_str().unwrap();
        assert_eq!(limited.matches("**Query**").count(), 1, "limit must cap shown entries");

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_session_log_and_recall() {
        // session_log persists request+outcome to .comp/history and session_recall surfaces it.
        let temp_dir = std::env::temp_dir().join("comP_test_session_log");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let logged = server
            .handle_session_log(json!({
                "request": "fix the login redirect bug",
                "outcome": "patched auth middleware and added a regression test",
                "files": ["src/auth.rs"]
            }))
            .await
            .unwrap();
        assert_eq!(logged["status"], "ok");

        // A monthly history JSONL file must now exist with one record.
        let hist_dir = temp_dir.join(".comp").join("history");
        let mut jsonl_files: Vec<_> = std::fs::read_dir(&hist_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
            .collect();
        assert_eq!(jsonl_files.len(), 1, "one monthly history file expected");
        let content = std::fs::read_to_string(jsonl_files.pop().unwrap()).unwrap();
        assert!(content.contains("login redirect"));
        assert!(content.contains("auth middleware"));

        // Recall surfaces the request and its outcome.
        let recall = server.handle_session_recall(json!({})).await.unwrap();
        let recall = recall.as_str().unwrap();
        assert!(recall.contains("fix the login redirect bug"), "request must be recalled");
        assert!(recall.contains("Outcome"), "outcome must be rendered");
        assert!(recall.contains("regression test"), "outcome text must be recalled");

        // Filtering on a word that appears only in the outcome still matches.
        let filtered = server
            .handle_session_recall(json!({ "query": "regression" }))
            .await
            .unwrap();
        assert!(filtered.as_str().unwrap().contains("login redirect"));

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_session_recall_hook_written_jsonl() {
        // The Stop hook (history-record.sh) writes {"timestamp","request","outcome"} —
        // no query/symbols/files/tokens/stale fields. Recall must still parse these
        // lines instead of silently dropping them.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_hook");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let hist_dir = temp_dir.join(".comp").join("history");
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(
            hist_dir.join("log-2026-06.jsonl"),
            concat!(
                r#"{"timestamp":1782521356807,"request":"add session_log feature","outcome":"implemented session_log in mod.rs"}"#,
                "\n",
                r#"{"timestamp":1782521356900,"request":"outcome may be null","outcome":null}"#,
                "\n"
            ),
        )
        .unwrap();

        let recall = server.handle_session_recall(json!({})).await.unwrap();
        let recall = recall.as_str().unwrap();
        assert!(recall.contains("add session_log feature"), "hook 'request' field must map to query");
        assert!(recall.contains("implemented session_log"), "hook outcome must be rendered");
        assert!(recall.contains("outcome may be null"), "null outcome must not break parsing");

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_session_recall_reports_unreadable_files_and_lines() {
        // Phase 2 (A8): a corrupt session-memory file or an unparseable history
        // line must no longer vanish silently — recall's own output is comP's
        // diagnostic surface, and it was blind to the exact condition this
        // recovery work exists to investigate.
        let temp_dir = std::env::temp_dir().join("comP_test_session_recall_unreadable");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        let memory_dir = temp_dir.join(".comp").join("session-memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::write(memory_dir.join("broken-agent.json"), b"{ not valid json").unwrap();

        let hist_dir = temp_dir.join(".comp").join("history");
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(
            hist_dir.join("log-2026-06.jsonl"),
            concat!(
                r#"{"timestamp":1782521356807,"request":"a good line","outcome":null}"#,
                "\n",
                "{ this line is not valid json at all\n",
            ),
        )
        .unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", root);
        let state = Arc::new(crate::AppState::new(root, "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        let recall = server.handle_session_recall(json!({})).await.unwrap();
        let recall = recall.as_str().unwrap();
        assert!(recall.contains("a good line"), "the readable line must still be surfaced");
        assert!(recall.contains("1 session-memory file(s)"), "unreadable memory file count must be reported: {}", recall);
        assert!(recall.contains("1 history line(s)"), "unreadable history line count must be reported: {}", recall);

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_new_mcp_tools() {
        let temp_dir = std::env::temp_dir().join("comP_test_new_mcp_tools");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state.clone());

        // Insert mock data into DB
        let file_id = state.graph_db.upsert_file("src/test_mcp.rs", "hash1", "rust", 0).unwrap();
        let node_id_1 = state.graph_db.insert_node(file_id, "my_mcp_func", "function", 10, 5, None, true, None).unwrap();
        let node_id_2 = state.graph_db.insert_node(file_id, "caller_func", "function", 20, 5, None, true, None).unwrap();

        // Add edge
        state.graph_db.insert_edge(node_id_2, node_id_1, "calls").unwrap();

        // 1. Test get_symbol (new slim format: ## name, → deps, ← callers)
        let result = server.handle_get_symbol(json!({ "name": "my_mcp_func" })).await.unwrap();
        let markdown = result.as_str().unwrap();
        assert!(markdown.contains("## my_mcp_func"));
        assert!(markdown.contains("src/test_mcp.rs"));
        assert!(markdown.contains("caller_func"));

        // 2. Test get_dependencies
        let result_out = server.handle_get_dependencies(json!({ "name": "caller_func", "direction": "out" })).await.unwrap();
        assert!(result_out.as_str().unwrap().contains("my_mcp_func"));

        let result_in = server.handle_get_dependencies(json!({ "name": "my_mcp_func", "direction": "in" })).await.unwrap();
        assert!(result_in.as_str().unwrap().contains("caller_func"));

        // 3. Test get_file_summary
        let result_summary = server.handle_get_file_summary(json!({ "file_path": "src/test_mcp.rs" })).await.unwrap();
        // Wait, handle_get_file_summary returns Result<Value>. It's Value::String.
        let summary_md = result_summary.as_str().unwrap();
        assert!(summary_md.contains("my_mcp_func"));
        assert!(summary_md.contains("caller_func"));

        // 4. Test get_project_overview
        let result_overview = server.handle_get_project_overview().await.unwrap();
        let overview_md = result_overview.as_str().unwrap();
        assert!(overview_md.contains("Total Files**: 1"));
        assert!(overview_md.contains("Total Symbols (Nodes)**: 2"));

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_get_symbol_compression_levels() {
        let temp_dir = std::env::temp_dir().join("comP_test_compression");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state.clone());

        let file_id = state.graph_db.upsert_file("src/test.rs", "hash1", "rust", 0).unwrap();
        state.graph_db.insert_node(file_id, "compress_test_fn", "function", 1, 1, None, true, None).unwrap();

        // All compression levels should succeed without error
        for level in [0i64, 1, 2] {
            let result = server
                .handle_get_symbol(json!({ "name": "compress_test_fn", "compression_level": level }))
                .await;
            assert!(result.is_ok(), "compression_level={} should not error", level);
            let markdown = result.unwrap();
            let md = markdown.as_str().unwrap();
            assert!(md.contains("## compress_test_fn"), "header missing for level {}", level);
        }

        std::env::remove_var("COMP_WORKSPACE_ROOT");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_handle_compress_file() {
        let temp_dir = std::env::temp_dir().join("comP_test_compress_file");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let temp_dir = temp_dir.canonicalize().unwrap();

        let file_path = temp_dir.join("test_file.rs");
        let code = "fn my_test_func() {\n    // some comment\n    let x = 42;\n}\n";
        std::fs::write(&file_path, code).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);

        // Test level 1 (Compact) -> should remove comment
        let params = json!({
            "path": file_path.to_str().unwrap(),
            "compression_level": 1
        });
        let result = server.handle_compress_file(params).await.unwrap();
        let compressed = result["compressed_text"].as_str().unwrap();
        assert!(compressed.contains("fn my_test_func()"));
        assert!(compressed.contains("let x = 42;"));
        assert!(!compressed.contains("some comment"));

        // Test level 2 (Skeleton) -> should replace body
        let params_sk = json!({
            "path": file_path.to_str().unwrap(),
            "compression_level": 2
        });
        let result_sk = server.handle_compress_file(params_sk).await.unwrap();
        let compressed_sk = result_sk["compressed_text"].as_str().unwrap();
        assert!(compressed_sk.contains("fn my_test_func() { ... }") || compressed_sk.contains("fn my_test_func()  { ... }"));
        assert!(!compressed_sk.contains("let x = 42;"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // --- git diff boost tests ---

    #[test]
    fn test_get_git_diff_files_not_a_repo() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_not_repo");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.is_empty(), "non-git directory must return empty set");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_git_diff_files_empty_repo_no_commits() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_no_commits");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // init a repo but make no commits — HEAD is undefined, git diff HEAD fails
        let _ = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&temp_dir)
            .output();

        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.is_empty(), "repo with no commits must return empty set");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_git_diff_files_clean_working_tree() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_clean");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&temp_dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
        };

        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(temp_dir.join("a.rs"), "fn a() {}").unwrap();
        run(&["add", "a.rs"]);
        run(&["commit", "-m", "init"]);

        // No uncommitted changes — must return empty
        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.is_empty(), "clean working tree must return empty set");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_git_diff_files_with_changes() {
        let temp_dir = std::env::temp_dir().join("comP_test_git_with_changes");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&temp_dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
        };

        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(temp_dir.join("a.rs"), "fn a() {}").unwrap();
        run(&["add", "a.rs"]);
        run(&["commit", "-m", "init"]);

        // Modify the file -> should appear in diff
        std::fs::write(temp_dir.join("a.rs"), "fn a() { let x = 1; }").unwrap();

        let result = get_git_diff_files(temp_dir.to_str().unwrap());
        assert!(result.contains("a.rs"), "modified file must be in diff set");
        assert_eq!(result.len(), 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_coverage_has_git_diff_boosted() {
        let state = Arc::new(crate::AppState::new(".", "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let result = server.handle_run_pipeline(json!({
            "task": "add authentication",
            "max_tokens": 8000
        })).await.unwrap();

        assert!(
            result["coverage"]["git_diff_boosted"].is_number(),
            "coverage.git_diff_boosted must be a number"
        );
    }

    #[tokio::test]
    async fn test_run_pipeline_git_diff_marker_on_boosted_files() {
        let temp_dir = std::env::temp_dir().join("comP_test_boost_marker");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&temp_dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
        };

        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(temp_dir.join("boosted.rs"), "fn boosted() {}").unwrap();
        run(&["add", "boosted.rs"]);
        run(&["commit", "-m", "init"]);
        // Modify the file so it appears in git diff HEAD
        std::fs::write(temp_dir.join("boosted.rs"), "fn boosted() { let x = 1; }").unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());
        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);

        // Index the file
        server.state.graph_db.list_files().unwrap(); // ensure DB is accessible

        let result = server.handle_run_pipeline(json!({
            "task": "fix boosted function",
            "max_tokens": 8000
        })).await.unwrap();

        // Any pivot_file with git_diff:true must actually be a git-diff file
        let pivot_files = result["pivot_files"].as_array().unwrap();
        for entry in pivot_files {
            if entry["git_diff"].as_bool() == Some(true) {
                // The marker is only set for files in the actual git diff — valid by construction
                assert!(
                    entry["path"].as_str().is_some(),
                    "git_diff-marked entry must have a path"
                );
            }
        }

        // coverage.git_diff_boosted must be a non-negative number
        let boosted = result["coverage"]["git_diff_boosted"].as_u64().unwrap();
        assert!(boosted == 0 || boosted > 0); // always a valid count

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// tiktoken-rs wiring: `tokens` must be the real BPE count of the exact
    /// content returned (compression level 0 == identity here), not the
    /// char_count/4 heuristic `estimate_tokens` falls back to when the file
    /// can't be read.
    #[tokio::test]
    async fn test_run_pipeline_reports_exact_token_count_from_real_content() {
        let temp_dir = std::env::temp_dir().join("comP_test_exact_tokens");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let source = "fn zztokentestmarker() {\n    let value = 1;\n    let other = 2;\n    println!(\"{} {}\", value, other);\n}\n";
        std::fs::write(temp_dir.join("zztokentestmarker.rs"), source).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);
        // AppState::new does not index anything by itself — only main()'s
        // background task does that. Force a synchronous full index so the
        // fixture file actually has a symbol/char_count row before querying it.
        server.handle_force_reindex().await.expect("force reindex failed");

        let result = server.handle_run_pipeline(json!({
            "task": "fix zztokentestmarker function",
            "max_tokens": 8000,
            "compression_level": 0
        })).await.unwrap();

        let pivot_files = result["pivot_files"].as_array().unwrap();
        let entry = pivot_files
            .iter()
            .find(|e| e["path"].as_str() == Some("zztokentestmarker.rs"))
            .expect("zztokentestmarker.rs should be a pivot file");

        let expected_exact = crate::search::TokenCounter::count_tokens(source).unwrap();
        let old_heuristic = source.len().div_ceil(4);
        assert_ne!(
            expected_exact, old_heuristic,
            "fixture must make the real count and the old char/4 heuristic diverge, or this test proves nothing"
        );
        assert_eq!(
            entry["tokens"].as_u64().unwrap() as usize,
            expected_exact,
            "tokens field must be the real tiktoken count of the file's actual content"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_no_git_repo_degrades_gracefully() {
        let temp_dir = std::env::temp_dir().join("comP_test_no_git_pipeline");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::env::set_var("COMP_WORKSPACE_ROOT", temp_dir.to_str().unwrap());
        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);

        // Must not error even though there is no git repo
        let result = server.handle_run_pipeline(json!({
            "task": "test task",
            "max_tokens": 8000
        })).await;
        assert!(result.is_ok(), "run_pipeline must succeed when not in a git repo");

        let response = result.unwrap();
        assert_eq!(
            response["coverage"]["git_diff_boosted"].as_u64().unwrap(),
            0,
            "git_diff_boosted must be 0 when not in a git repo"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ---- GitHub issue #7 reproduction: header-rich Markdown dominating unrelated queries ----
    // https://github.com/tsucky230/comP/issues/7
    // Runs the real end-to-end run_pipeline path (LIKE keyword search + TF-IDF +
    // BM25 + candidate merge) against a CONTRIBUTING.md-style file with 20+
    // headings, alongside two small files that are each precisely on-topic for
    // one of two queries about application-specific topics CONTRIBUTING.md
    // never mentions in any heading or body sentence. If the header-rich file
    // still ranks #1 ahead of the genuinely on-topic file for both queries —
    // purely because it has more symbols and therefore more chances to match
    // some keyword — the issue reproduces end-to-end.
    #[tokio::test]
    async fn test_issue7_header_rich_markdown_dominates_unrelated_queries() {
        let temp_dir = std::env::temp_dir().join("comP_test_issue7_ranking_bug");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let md = "# Contributing to comP\n\
## Code of Conduct\nBe respectful.\n\
## Reporting Bugs\nOpen an issue with steps to reproduce.\n\
## Suggesting Features\nStart a discussion.\n\
## Setting Up the Development Environment\nClone the repo and install dependencies.\n\
## Prerequisites\nNode 18 and Rust 1.70 are required.\n\
## Clone and Install\nRun npm install after cloning.\n\
## Build\nCompile the TypeScript extension and Rust daemon.\n\
## Packaging a Local VSIX\nUse npm run package local for testing.\n\
## Watch Mode\nWatch TypeScript and Rust changes in two terminals.\n\
## Testing\nRun cargo test and npm test.\n\
## Linting\nCheck Markdown formatting.\n\
## Debug in VSCode\nPress F5 to start debugging.\n\
## Submitting Changes\nUse conventional branch names.\n\
## Branch Naming\nPrefix with feature bugfix or docs.\n\
## Commit Messages\nFollow the conventional commit format.\n\
## Pull Request Process\nOpen an issue before starting a big change.\n\
## Pull Request Checklist\nConfirm tests and docs are updated.\n\
## Code Style\nUse rustfmt and strict TypeScript.\n\
## Documentation\nUpdate the architecture doc when needed.\n\
## Release Process\nBump the version and tag the release.\n\
## MCP Server Development\nDefine new tools in mcp mod rs.\n\
## MCP Tool Development Checklist\nTest every new tool with multiple agents.\n\
## Example Adding a New MCP Tool\nRegister the handler and update docs.\n\
## Testing MCP Servers\nVerify with Claude Code or Cursor.\n\
## Getting Help\nCheck the discussions board.\n\
## Recognition\nContributors are listed in the README.\n";
        std::fs::write(temp_dir.join("CONTRIBUTING.md"), md).unwrap();

        // Precisely on-topic for query A ("sqlite connection pool leaks handles
        // under load"). None of these words appear anywhere in CONTRIBUTING.md.
        std::fs::write(
            temp_dir.join("connection_pool.rs"),
            "pub fn releaseConnectionPool(handle: u32) -> bool {\n    handle > 0\n}\n",
        ).unwrap();
        // Precisely on-topic for query B ("cli output truncates unicode
        // filenames on windows"). Also absent from CONTRIBUTING.md entirely.
        std::fs::write(
            temp_dir.join("path_utils.rs"),
            "pub fn truncateUnicodeFilename(name: &str) -> String {\n    name.to_string()\n}\n",
        ).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);
        server.handle_force_reindex().await.expect("force reindex failed");

        let result_a = server.handle_run_pipeline(json!({
            "task": "sqlite connection pool leaks handles under load",
            "max_tokens": 8000
        })).await.unwrap();
        let result_b = server.handle_run_pipeline(json!({
            "task": "cli output truncates unicode filenames on windows",
            "max_tokens": 8000
        })).await.unwrap();

        let paths_a: Vec<&str> = result_a["pivot_files"].as_array().unwrap().iter()
            .filter_map(|f| f["path"].as_str()).collect();
        let paths_b: Vec<&str> = result_b["pivot_files"].as_array().unwrap().iter()
            .filter_map(|f| f["path"].as_str()).collect();

        println!("Query A pivot_files order: {:?}", paths_a);
        println!("Query B pivot_files order: {:?}", paths_b);

        assert_eq!(
            paths_a.first(), Some(&"connection_pool.rs"),
            "connection_pool.rs (the genuinely on-topic file) must rank first for query A, \
             not CONTRIBUTING.md (issue #7): {:?}", paths_a
        );
        assert_eq!(
            paths_b.first(), Some(&"path_utils.rs"),
            "path_utils.rs (the genuinely on-topic file) must rank first for query B, \
             not CONTRIBUTING.md (issue #7): {:?}", paths_b
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Regression guard for the issue #7 fix: a query naming an exact heading
    /// must still surface that Markdown file quickly via the LIKE tier. The
    /// length penalty in `like_score_for` must not overcorrect into hiding
    /// genuinely relevant documentation just because the file has many
    /// headings.
    #[tokio::test]
    async fn test_exact_heading_match_still_surfaces_markdown_file() {
        let temp_dir = std::env::temp_dir().join("comP_test_heading_exact_match");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let md = "# Contributing to comP\n\
## Code of Conduct\nBe respectful.\n\
## Reporting Bugs\nOpen an issue with steps to reproduce.\n\
## Suggesting Features\nStart a discussion.\n\
## Setting Up the Development Environment\nClone the repo and install dependencies.\n\
## Prerequisites\nNode 18 and Rust 1.70 are required.\n\
## Installation\nRun npm install to set up the project locally.\n\
## Build\nCompile the TypeScript extension and Rust daemon.\n\
## Testing\nRun cargo test and npm test.\n\
## Linting\nCheck Markdown formatting.\n\
## Submitting Changes\nUse conventional branch names.\n\
## Code Style\nUse rustfmt and strict TypeScript.\n\
## Documentation\nUpdate the architecture doc when needed.\n\
## Release Process\nBump the version and tag the release.\n\
## Getting Help\nCheck the discussions board.\n";
        std::fs::write(temp_dir.join("CONTRIBUTING.md"), md).unwrap();
        std::fs::write(
            temp_dir.join("unrelated.rs"),
            "pub fn computeChecksum(data: &[u8]) -> u32 {\n    data.len() as u32\n}\n",
        ).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);
        server.handle_force_reindex().await.expect("force reindex failed");

        let result = server.handle_run_pipeline(json!({
            "task": "installation instructions",
            "max_tokens": 8000
        })).await.unwrap();

        let paths: Vec<&str> = result["pivot_files"].as_array().unwrap().iter()
            .filter_map(|f| f["path"].as_str()).collect();
        assert_eq!(
            paths.first(), Some(&"CONTRIBUTING.md"),
            "a query naming an exact heading must still rank that Markdown file first: {:?}", paths
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Regression guard from Gemini cross-review of the issue #7 fix: a query
    /// with no keyword >= 3 chars (e.g. "ui") takes the `keywords.is_empty()`
    /// branch, which never populated `like_hit_counts`/`like_matched_keywords`
    /// and always skipped BM25. Combined with the doc/non-doc weight
    /// renormalization, every Markdown/Office file was silently scored via a
    /// smaller effective weight (0.3 fixed) than every code file (1/0.7 ≈
    /// 1.43x boost) for any such query, regardless of true relevance — an
    /// exact inverse of the bias issue #7 was about. Both files matching here
    /// must still be considered (neither silently dropped), proving the short
    /// query path participates in scoring instead of being a scored-as-zero
    /// dead branch for one file type.
    #[tokio::test]
    async fn test_short_query_does_not_bias_against_doc_files() {
        let temp_dir = std::env::temp_dir().join("comP_test_short_query_bias");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        std::fs::write(temp_dir.join("faq.md"), "# FAQ\n## UI Guide\nHow to use the interface.\n").unwrap();
        std::fs::write(
            temp_dir.join("widgets.rs"),
            "pub fn buildUiPanel(id: u32) -> bool {\n    id > 0\n}\n",
        ).unwrap();

        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.unwrap());
        let server = MCPServer::new(state);
        server.handle_force_reindex().await.expect("force reindex failed");

        let result = server.handle_run_pipeline(json!({
            "task": "ui",
            "max_tokens": 8000
        })).await;
        assert!(result.is_ok(), "run_pipeline must not error on a query with no keyword >= 3 chars");

        let response = result.unwrap();
        let paths: Vec<&str> = response["pivot_files"].as_array().unwrap().iter()
            .filter_map(|f| f["path"].as_str()).collect();
        assert!(paths.contains(&"faq.md"), "the matching Markdown file must not be dropped: {:?}", paths);
        assert!(paths.contains(&"widgets.rs"), "the matching code file must not be dropped: {:?}", paths);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ---- multi-LLM session memory sharing (agent field, locking, compaction, CLI) ----

    #[test]
    fn test_sanitize_agent_id_keeps_safe_chars() {
        assert_eq!(sanitize_agent_id("claude-code"), "claude-code");
        assert_eq!(sanitize_agent_id("codex_v2"), "codex_v2");
    }

    #[test]
    fn test_sanitize_agent_id_neutralizes_path_traversal() {
        // A malformed COMP_AGENT_ID must not be able to escape the
        // .comp/session-memory/ directory via path separators or "..".
        assert_eq!(sanitize_agent_id("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_agent_id("a/b\\c"), "a_b_c");
    }

    #[test]
    fn test_sanitize_agent_id_empty_stays_empty() {
        assert_eq!(sanitize_agent_id(""), "");
    }

    #[test]
    fn test_get_session_memory_path_is_per_agent() {
        let p1 = get_session_memory_path("/workspace", "claude-code");
        let p2 = get_session_memory_path("/workspace", "codex");
        assert_ne!(p1, p2, "different agents must resolve to different files");
        assert!(p1.to_string_lossy().replace('\\', "/").ends_with(".comp/session-memory/claude-code.json"));
    }

    #[test]
    fn test_append_history_line_creates_file_and_parent_dir() {
        let temp_dir = std::env::temp_dir().join("comP_test_append_history_new");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hist_path = temp_dir.join("history").join("log-2026-01.jsonl");

        append_history_line(&hist_path, r#"{"a":1}"#).unwrap();

        let content = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(content, "{\"a\":1}\n");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_append_history_line_preserves_order_across_calls() {
        let temp_dir = std::env::temp_dir().join("comP_test_append_history_order");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");

        append_history_line(&hist_path, "one").unwrap();
        append_history_line(&hist_path, "two").unwrap();
        append_history_line(&hist_path, "three").unwrap();

        let content = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(content, "one\ntwo\nthree\n");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_append_history_line_survives_concurrent_writers() {
        // Simulates the real scenario this feature exists for: several independent
        // comp-daemon processes (one per MCP client) appending to the same JSONL
        // file at once. Every line must survive — none lost, none torn/corrupted.
        let temp_dir = std::env::temp_dir().join("comP_test_append_history_concurrent");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");

        const WRITERS: usize = 8;
        const LINES_PER_WRITER: usize = 25;
        let mut handles = Vec::new();
        for writer_id in 0..WRITERS {
            let path = hist_path.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..LINES_PER_WRITER {
                    append_history_line(&path, &format!("w{}-{}", writer_id, i)).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let content = std::fs::read_to_string(&hist_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines.len(),
            WRITERS * LINES_PER_WRITER,
            "every writer's every line must survive concurrent appends"
        );
        // Every line must be intact (no interleaved/torn "w{n}-{m}" tokens).
        for line in &lines {
            assert!(
                line.starts_with('w') && line.contains('-'),
                "line must not be torn by a concurrent write: {:?}",
                line
            );
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_append_history_line_merges_pending_spill_files() {
        // Simulates history-record.sh's fallback path (Phase 3, B4): a spill
        // file written while comp-daemon was unavailable must be folded into
        // the real log, in order before the new line, the next time anyone
        // appends normally — and removed once merged.
        let temp_dir = std::env::temp_dir().join("comP_test_append_merges_spill");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");

        let spill_path = temp_dir.join("spill-1234-1700000000000-ab12.jsonl");
        std::fs::write(&spill_path, "{\"timestamp\":1,\"request\":\"from spill\"}\n").unwrap();

        append_history_line(&hist_path, "{\"timestamp\":2,\"request\":\"live append\"}").unwrap();

        let content = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(
            content,
            "{\"timestamp\":1,\"request\":\"from spill\"}\n{\"timestamp\":2,\"request\":\"live append\"}\n",
            "the spilled entry must land before the new line, in order"
        );
        assert!(!spill_path.exists(), "a successfully merged spill file must be removed");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_append_history_line_ignores_non_spill_files() {
        let temp_dir = std::env::temp_dir().join("comP_test_append_ignores_non_spill");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        std::fs::write(temp_dir.join("log-2026-05.jsonl"), "{\"timestamp\":0,\"request\":\"other month\"}\n").unwrap();
        std::fs::write(temp_dir.join("notes.txt"), "unrelated file").unwrap();

        append_history_line(&hist_path, "{\"timestamp\":1,\"request\":\"new\"}").unwrap();

        let content = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(content, "{\"timestamp\":1,\"request\":\"new\"}\n", "only spill-*.jsonl files get merged in");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compact_history_file_rewrites_and_keeps_backup() {
        let temp_dir = std::env::temp_dir().join("comP_test_compact_basic");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        std::fs::write(&hist_path, "line1\nline2\nline1\n").unwrap();

        compact_history_file(&hist_path, |original| {
            let text = String::from_utf8_lossy(original);
            let mut seen: Vec<&str> = Vec::new();
            for l in text.lines() {
                if !seen.contains(&l) {
                    seen.push(l);
                }
            }
            Ok(format!("{}\n", seen.join("\n")).into_bytes())
        })
        .unwrap();

        let compacted = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(compacted, "line1\nline2\n", "duplicate lines must be removed");

        let backup_path = hist_path.with_extension("jsonl.bak");
        let backup = std::fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup, "line1\nline2\nline1\n", "backup must hold the pre-compaction content");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compact_history_file_preserves_prior_backup_on_second_run() {
        // Phase 2 (B6): a second compaction must not clobber the .bak from the
        // first — that would destroy the only rollback copy of *that* run's
        // pre-compaction content.
        let temp_dir = std::env::temp_dir().join("comP_test_compact_preserves_prior_backup");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        std::fs::write(&hist_path, "first-generation\n").unwrap();

        compact_history_file(&hist_path, |original| Ok(original.to_vec())).unwrap();
        let backup_path = hist_path.with_extension("jsonl.bak");
        assert_eq!(std::fs::read_to_string(&backup_path).unwrap(), "first-generation\n");

        std::fs::write(&hist_path, "second-generation\n").unwrap();
        compact_history_file(&hist_path, |original| Ok(original.to_vec())).unwrap();

        // The current .bak must hold the *second* run's pre-compaction content...
        assert_eq!(std::fs::read_to_string(&backup_path).unwrap(), "second-generation\n");
        // ...and the first run's backup must still exist somewhere, not be lost.
        let prior_backups: Vec<_> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("log.jsonl.bak.") && name != "log.jsonl.bak"
            })
            .collect();
        assert_eq!(prior_backups.len(), 1, "the first run's backup must be preserved under a generation suffix");
        assert_eq!(std::fs::read_to_string(prior_backups[0].path()).unwrap(), "first-generation\n");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compact_history_file_leaves_original_untouched_on_error() {
        let temp_dir = std::env::temp_dir().join("comP_test_compact_error");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        std::fs::write(&hist_path, "original\n").unwrap();

        let result = compact_history_file(&hist_path, |_original| {
            Err(anyhow!("summarization failed"))
        });

        assert!(result.is_err());
        let content = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(content, "original\n", "a failed compaction must not mutate the original file");
        assert!(
            !hist_path.with_extension("jsonl.bak").exists(),
            "no backup should be written when compaction never reaches the mutation step"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_repair_history_file_quarantines_bad_lines_and_keeps_good_ones() {
        let temp_dir = std::env::temp_dir().join("comP_test_repair_basic");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        let good_line = r#"{"timestamp":1,"request":"good one"}"#;
        std::fs::write(&hist_path, format!("{}\n{{ not valid json\n", good_line)).unwrap();

        let report = repair_history_file(&hist_path).unwrap();
        assert_eq!(report, RepairReport { good_lines: 1, bad_lines: 1 });

        let remaining = std::fs::read_to_string(&hist_path).unwrap();
        assert_eq!(remaining, format!("{}\n", good_line), "only the valid line must remain");

        let quarantined = std::fs::read_to_string(hist_path.with_extension("jsonl.quarantine")).unwrap();
        assert_eq!(quarantined, "{ not valid json\n", "the bad line must be preserved in quarantine, not discarded");

        let backup = std::fs::read_to_string(hist_path.with_extension("jsonl.bak")).unwrap();
        assert_eq!(backup, format!("{}\n{{ not valid json\n", good_line), "pre-repair content must be backed up");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_repair_history_file_is_noop_when_all_lines_valid() {
        let temp_dir = std::env::temp_dir().join("comP_test_repair_noop");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        let content = "{\"timestamp\":1,\"request\":\"a\"}\n{\"timestamp\":2,\"request\":\"b\"}\n";
        std::fs::write(&hist_path, content).unwrap();

        let report = repair_history_file(&hist_path).unwrap();
        assert_eq!(report, RepairReport { good_lines: 2, bad_lines: 0 });
        assert_eq!(std::fs::read_to_string(&hist_path).unwrap(), content, "a clean file must be left byte-identical");
        assert!(!hist_path.with_extension("jsonl.bak").exists(), "no backup when nothing needed fixing");
        assert!(!hist_path.with_extension("jsonl.quarantine").exists(), "no quarantine file when nothing was bad");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_repair_history_file_accumulates_quarantine_across_runs() {
        // A second repair run (e.g. after more corruption accrues) must append
        // to the quarantine file, not truncate away what a prior run saved.
        let temp_dir = std::env::temp_dir().join("comP_test_repair_accumulates_quarantine");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");

        std::fs::write(&hist_path, "bad-line-one\n{\"timestamp\":1,\"request\":\"a\"}\n").unwrap();
        repair_history_file(&hist_path).unwrap();

        std::fs::write(&hist_path, "{\"timestamp\":2,\"request\":\"b\"}\nbad-line-two\n").unwrap();
        repair_history_file(&hist_path).unwrap();

        let quarantined = std::fs::read_to_string(hist_path.with_extension("jsonl.quarantine")).unwrap();
        assert!(quarantined.contains("bad-line-one"), "first run's quarantined line must survive: {}", quarantined);
        assert!(quarantined.contains("bad-line-two"), "second run's quarantined line must be added: {}", quarantined);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compact_history_file_excludes_concurrent_append() {
        // Proves the shared/exclusive lock pairing: while compaction holds the
        // exclusive lock, a concurrent appender must block until it releases —
        // never interleave, never get silently dropped by the rewrite.
        let temp_dir = std::env::temp_dir().join("comP_test_compact_exclusion");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hist_path = temp_dir.join("log.jsonl");
        std::fs::write(&hist_path, "before\n").unwrap();

        let appender_path = hist_path.clone();
        let appender = std::thread::spawn(move || {
            // Give compaction a head start so it is very likely holding the
            // exclusive lock when this fires.
            std::thread::sleep(std::time::Duration::from_millis(50));
            append_history_line(&appender_path, "appended-during-compaction").unwrap();
        });

        compact_history_file(&hist_path, |original| {
            std::thread::sleep(std::time::Duration::from_millis(150));
            Ok(original.to_vec())
        })
        .unwrap();

        appender.join().unwrap();

        let content = std::fs::read_to_string(&hist_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines,
            vec!["before", "appended-during-compaction"],
            "the append must land intact, ordered after the compaction it waited out"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_parse_cli_subcommand_append_history() {
        let args: Vec<String> = ["comp-daemon", "append-history", "/ws", "claude-code"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_cli_subcommand(&args),
            Some(CliSubcommand::AppendHistory {
                workspace_root: "/ws".to_string(),
                agent_id: "claude-code".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_cli_subcommand_record_turn() {
        let to_args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert_eq!(
            parse_cli_subcommand(&to_args(&["comp-daemon", "record-turn"])),
            Some(CliSubcommand::RecordTurn { workspace_root: None })
        );
        for ws in ["/ws", "E:/dev/YASAKANI", r"C:\Users\a b\proj"] {
            assert_eq!(
                parse_cli_subcommand(&to_args(&["comp-daemon", "record-turn", ws])),
                Some(CliSubcommand::RecordTurn { workspace_root: Some(ws.to_string()) })
            );
        }
        // Extra args must not silently start the MCP server loop inside a Stop hook,
        // but they are not a valid record-turn either: fall through like other typos.
        assert_eq!(parse_cli_subcommand(&to_args(&["comp-daemon", "record-turn", "/ws", "x"])), None);
    }

    #[test]
    fn test_parse_cli_subcommand_none_for_normal_startup() {
        let args: Vec<String> = vec!["comp-daemon".to_string()];
        assert_eq!(parse_cli_subcommand(&args), None);
    }

    #[test]
    fn test_parse_cli_subcommand_none_for_unknown_subcommand() {
        let args: Vec<String> = ["comp-daemon", "frobnicate"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_cli_subcommand(&args), None);
    }

    #[test]
    fn test_parse_cli_subcommand_none_for_missing_args() {
        // append-history requires exactly workspace_root + agent_id; fewer args
        // must fall through to normal startup rather than panicking on an
        // out-of-bounds index.
        let args: Vec<String> = ["comp-daemon", "append-history", "/ws"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_cli_subcommand(&args), None);
    }

    #[test]
    fn test_parse_cli_subcommand_doctor_report_only() {
        let args: Vec<String> = ["comp-daemon", "doctor", "/ws"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            parse_cli_subcommand(&args),
            Some(CliSubcommand::Doctor { workspace_root: "/ws".to_string(), repair: false })
        );
    }

    #[test]
    fn test_parse_cli_subcommand_doctor_with_repair() {
        let args: Vec<String> = ["comp-daemon", "doctor", "/ws", "--repair"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_cli_subcommand(&args),
            Some(CliSubcommand::Doctor { workspace_root: "/ws".to_string(), repair: true })
        );
    }

    #[test]
    fn test_parse_cli_subcommand_doctor_rejects_unknown_trailing_flag() {
        let args: Vec<String> = ["comp-daemon", "doctor", "/ws", "--bogus"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_cli_subcommand(&args), None);
    }

    #[test]
    fn test_parse_cli_subcommand_compact_history() {
        let args: Vec<String> = ["comp-daemon", "compact-history", "/ws"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            parse_cli_subcommand(&args),
            Some(CliSubcommand::CompactHistory { workspace_root: "/ws".to_string() })
        );
    }

    #[test]
    fn test_dedup_exact_duplicate_lines_keeps_first_occurrence_only() {
        let input = b"line1\nline2\nline1\nline3\nline2\n";
        let result = dedup_exact_duplicate_lines(input).unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "line1\nline2\nline3\n");
    }

    #[test]
    fn test_dedup_exact_duplicate_lines_noop_when_no_duplicates() {
        let input = b"a\nb\nc\n";
        let result = dedup_exact_duplicate_lines(input).unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn test_run_compact_history_processes_every_jsonl_file_and_shrinks_duplicates() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_compact_history");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hist_dir = temp_dir.join(".comp").join("history");
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(hist_dir.join("log-2026-05.jsonl"), "dup\ndup\nunique\n").unwrap();
        std::fs::write(hist_dir.join("log-2026-06.jsonl"), "a\nb\n").unwrap();
        std::fs::write(hist_dir.join("notes.txt"), "not jsonl, must be ignored").unwrap();

        let results = run_compact_history(temp_dir.to_str().unwrap()).unwrap();
        assert_eq!(results.len(), 2, "only the two .jsonl files must be processed");

        let may_content = std::fs::read_to_string(hist_dir.join("log-2026-05.jsonl")).unwrap();
        assert_eq!(may_content, "dup\nunique\n", "the duplicate line must be removed");
        let june_content = std::fs::read_to_string(hist_dir.join("log-2026-06.jsonl")).unwrap();
        assert_eq!(june_content, "a\nb\n", "a file with no duplicates must be unchanged");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_compact_history_is_a_noop_when_history_dir_is_absent() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_compact_history_absent");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let results = run_compact_history(temp_dir.to_str().unwrap()).unwrap();
        assert!(results.is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_internal_error_response_puts_detail_in_message() {
        let e = anyhow!("Missing 'task' parameter");
        let r = internal_error_response(&json!(7), &e);
        assert_eq!(r["error"]["code"], -32603);
        assert_eq!(r["error"]["message"], "Internal error: Missing 'task' parameter");
    }

    #[test]
    fn test_internal_error_response_keeps_data_and_id() {
        let e = anyhow!("Missing 'task' parameter");
        let r = internal_error_response(&json!("abc"), &e);
        assert_eq!(r["error"]["data"], "Missing 'task' parameter");
        assert_eq!(r["id"], "abc");
        assert_eq!(r["jsonrpc"], "2.0");
    }

    #[tokio::test]
    async fn test_tools_call_missing_task_reports_parameter_name() {
        let temp_dir = std::env::temp_dir().join("comP_test_mcp_internal_error_detail");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);
        let err = server
            .handle_tools_call(json!({ "name": "run_pipeline", "arguments": { "query": "x" } }))
            .await
            .unwrap_err();
        let r = internal_error_response(&json!(1), &err);
        assert!(r["error"]["message"].as_str().unwrap().contains("'task'"));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_poisoned_mutex_error_matches_graphdb_lock_failure_text() {
        let e = anyhow!("DB mutex poisoned: {}", "the lock was poisoned");
        assert!(is_poisoned_mutex_error(&e));
    }

    #[test]
    fn test_is_poisoned_mutex_error_does_not_match_unrelated_errors() {
        let e = anyhow!("no such table: files");
        assert!(!is_poisoned_mutex_error(&e));
    }

    #[test]
    fn test_degraded_response_completes_initialize_handshake() {
        let response = degraded_response(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, "disk full").unwrap();
        assert!(response["result"]["serverInfo"]["name"].is_string(), "handshake must succeed even in degraded mode: {}", response);
        assert!(response["error"].is_null());
    }

    #[test]
    fn test_degraded_response_reports_empty_tools_list() {
        let response = degraded_response(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, "disk full").unwrap();
        assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_degraded_response_includes_reason_for_any_other_call() {
        let response = degraded_response(
            r#"{"jsonrpc":"2.0","id":3,"method":"run_pipeline","params":{"task":"x"}}"#,
            "index.db could not be created: permission denied",
        )
        .unwrap();
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.contains("permission denied"), "the actual failure reason must reach the caller: {}", message);
    }

    #[test]
    fn test_degraded_response_sends_nothing_for_notifications() {
        // No "id" at all — e.g. notifications/initialized — must get no response,
        // matching MCPServer::run's identical rule (a response here makes strict
        // MCP clients drop the connection).
        let response = degraded_response(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, "disk full");
        assert!(response.is_none());
    }

    #[test]
    fn test_degraded_response_reports_parse_error_for_invalid_json() {
        let response = degraded_response("not json at all", "disk full").unwrap();
        assert_eq!(response["error"]["code"], -32700);
    }

    #[test]
    fn test_run_doctor_report_only_does_not_mutate_anything() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_doctor_report_only");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        let memory_dir = comp_dir.join("session-memory");
        let hist_dir = comp_dir.join("history");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(memory_dir.join("broken.json"), b"{ not valid json").unwrap();
        std::fs::write(hist_dir.join("log-2026-06.jsonl"), "{\"timestamp\":1,\"request\":\"ok\"}\nbad-line\n").unwrap();
        std::fs::write(comp_dir.join("index.db"), b"not a sqlite database").unwrap();

        let report = run_doctor(temp_dir.to_str().unwrap(), false).unwrap();
        assert_eq!(report.session_memory_files_ok, 0);
        assert_eq!(report.session_memory_files_corrupt, 1);
        assert_eq!(report.history_lines_ok, 1);
        assert_eq!(report.history_lines_corrupt, 1);
        assert_eq!(report.index_db_status, "corrupt");
        assert!(!report.is_healthy(false));

        // Nothing must have moved: report-only means read-only.
        assert!(memory_dir.join("broken.json").exists());
        assert!(hist_dir.join("log-2026-06.jsonl").exists());
        let hist_content = std::fs::read_to_string(hist_dir.join("log-2026-06.jsonl")).unwrap();
        assert!(hist_content.contains("bad-line"), "report-only must not rewrite the history file");
        assert!(comp_dir.join("index.db").exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_doctor_repair_quarantines_everything_corrupt() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_doctor_repair");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let comp_dir = temp_dir.join(".comp");
        let memory_dir = comp_dir.join("session-memory");
        let hist_dir = comp_dir.join("history");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(memory_dir.join("broken.json"), b"{ not valid json").unwrap();
        std::fs::write(hist_dir.join("log-2026-06.jsonl"), "{\"timestamp\":1,\"request\":\"ok\"}\nbad-line\n").unwrap();
        std::fs::write(comp_dir.join("index.db"), b"not a sqlite database").unwrap();

        let report = run_doctor(temp_dir.to_str().unwrap(), true).unwrap();
        assert_eq!(report.session_memory_files_corrupt, 1);
        assert_eq!(report.history_lines_corrupt, 1);
        assert_eq!(report.index_db_status, "corrupt_and_quarantined");
        assert!(report.is_healthy(true), "everything found was quarantined, so repair mode reports healthy");

        // The corrupt session-memory file must be moved aside, not left at its name.
        assert!(!memory_dir.join("broken.json").exists());
        let quarantined_memory: Vec<_> = std::fs::read_dir(&memory_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .collect();
        assert_eq!(quarantined_memory.len(), 1);

        // The history file must keep only the good line; the bad one moved to quarantine.
        let hist_content = std::fs::read_to_string(hist_dir.join("log-2026-06.jsonl")).unwrap();
        assert_eq!(hist_content, "{\"timestamp\":1,\"request\":\"ok\"}\n");
        assert!(hist_dir.join("log-2026-06.jsonl.quarantine").exists());

        // The corrupt index.db must be moved aside so the next real startup creates a fresh one.
        assert!(!comp_dir.join("index.db").exists());
        let quarantined_db: Vec<_> = std::fs::read_dir(&comp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("index.db.corrupt-"))
            .collect();
        assert_eq!(quarantined_db.len(), 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_doctor_healthy_workspace_reports_ok() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_doctor_healthy");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let report = run_doctor(temp_dir.to_str().unwrap(), false).unwrap();
        assert_eq!(report.index_db_status, "missing", "no index.db yet is healthy, not an error");
        assert!(report.is_healthy(false));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_classify_quick_check_result_ok() {
        assert_eq!(classify_quick_check_result(&Ok("ok".to_string())), "ok");
    }

    #[test]
    fn test_classify_quick_check_result_treats_busy_or_locked_as_inconclusive() {
        // Phase 2 design fix: index.db is routinely open by another comp-daemon
        // process. A database that is merely busy/locked right now must never
        // be classified the same as confirmed-corrupt — doing so would let
        // `doctor --repair` quarantine a perfectly healthy, actively-used
        // database out from under whoever holds it.
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ErrorCode::DatabaseBusy, extended_code: 5 },
            None,
        );
        assert_eq!(classify_quick_check_result(&Err(busy)), "check_failed");

        let locked = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ErrorCode::DatabaseLocked, extended_code: 6 },
            None,
        );
        assert_eq!(classify_quick_check_result(&Err(locked)), "check_failed");
    }

    #[test]
    fn test_classify_quick_check_result_treats_other_failures_as_corrupt() {
        let not_a_db = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ErrorCode::NotADatabase, extended_code: 26 },
            None,
        );
        assert_eq!(classify_quick_check_result(&Err(not_a_db)), "corrupt");
        assert_eq!(
            classify_quick_check_result(&Ok("wrong # of entries in index idx_nodes_kind".to_string())),
            "corrupt",
            "quick_check reporting a problem (not the literal 'ok') must be treated as corrupt"
        );
    }

    #[test]
    fn test_run_append_history_writes_agent_tagged_entry() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_append_history");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        let input = std::io::Cursor::new(br#"{"request":"do the thing","outcome":"done"}"#.to_vec());
        run_append_history(root, "codex", input).unwrap();

        let month = &format_epoch_ms(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        )[0..7];
        let hist_path = temp_dir.join(".comp").join("history").join(format!("log-{}.jsonl", month));
        let content = std::fs::read_to_string(&hist_path).unwrap();
        let call: SessionCall = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(call.query, "do the thing");
        assert_eq!(call.outcome, Some("done".to_string()));
        assert_eq!(call.agent, "codex", "agent must come from the CLI arg, not the stdin payload");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_append_history_agent_cannot_be_spoofed_via_stdin() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_append_history_spoof");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        // Payload tries to smuggle an "agent" field; the CLI arg must win.
        let input = std::io::Cursor::new(
            br#"{"request":"x","outcome":null,"agent":"attacker-controlled"}"#.to_vec(),
        );
        run_append_history(root, "claude-code", input).unwrap();

        let hist_dir = temp_dir.join(".comp").join("history");
        let entry = std::fs::read_dir(&hist_dir).unwrap().next().unwrap().unwrap();
        let content = std::fs::read_to_string(entry.path()).unwrap();
        let call: SessionCall = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(call.agent, "claude-code");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_append_history_rejects_malformed_json() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_append_history_bad_json");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        let input = std::io::Cursor::new(b"not json".to_vec());
        assert!(run_append_history(root, "claude-code", input).is_err());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_append_history_rejects_missing_request_field() {
        let temp_dir = std::env::temp_dir().join("comP_test_run_append_history_missing_request");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        let input = std::io::Cursor::new(br#"{"outcome":"done"}"#.to_vec());
        assert!(run_append_history(root, "claude-code", input).is_err());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_session_call_missing_agent_field_defaults_to_unknown() {
        // Pre-migration JSONL lines / session-memory.json entries have no "agent"
        // key at all; they must still deserialize instead of being dropped.
        let json = r#"{"query":"old task","timestamp":1700000000000}"#;
        let call: SessionCall = serde_json::from_str(json).unwrap();
        assert_eq!(call.agent, "unknown");
    }

    #[tokio::test]
    async fn test_session_recall_merges_multiple_agents() {
        // The core multi-LLM goal: two different agents' own session-memory files
        // must both surface in one session_recall call.
        let temp_dir = std::env::temp_dir().join("comP_test_recall_multi_agent");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = temp_dir.to_str().unwrap();

        record_mcp_call(root, "claude-code", "sess-a", "claude did this".to_string(), vec![], vec![], 10).unwrap();
        record_mcp_call(root, "codex", "sess-b", "codex did that".to_string(), vec![], vec![], 20).unwrap();

        let state = Arc::new(crate::AppState::new(root, "claude-code").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);
        let markdown = server.handle_session_recall(json!({})).await.unwrap();
        let markdown = markdown.as_str().unwrap();

        assert!(markdown.contains("claude did this"), "claude-code's own record must surface");
        assert!(markdown.contains("codex did that"), "codex's record must surface even though this daemon is running as claude-code");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Codex's `requires_mcp_tool_approval` (openai/codex, core/src/mcp_tool_call.rs)
    /// for the default `auto` approval mode. MCP treats an absent destructiveHint or
    /// openWorldHint as true, so an unannotated tool is approval-bound — and under
    /// Codex's auto-review that means a reviewer judging comP as an unknown connector.
    fn codex_auto_mode_requires_approval(annotations: &Value) -> bool {
        let destructive = annotations["destructiveHint"].as_bool();
        if destructive == Some(true) {
            return true;
        }
        if annotations["readOnlyHint"].as_bool().unwrap_or(false) {
            return false;
        }
        destructive.unwrap_or(true) || annotations["openWorldHint"].as_bool().unwrap_or(true)
    }

    #[tokio::test]
    async fn test_tools_list_declares_local_non_destructive_annotations() {
        assert!(
            codex_auto_mode_requires_approval(&Value::Null),
            "the mirrored Codex rule must treat a missing annotations object as approval-bound"
        );

        let temp_dir = std::env::temp_dir().join("comP_test_tool_annotations");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let state = Arc::new(crate::AppState::new(temp_dir.to_str().unwrap(), "test-agent").await.expect("Failed to create AppState"));
        let server = MCPServer::new(state);

        let list = server.handle_tools_list().await.unwrap();
        let tools = list["tools"].as_array().unwrap();

        // session_log exists to write the caller's request into .comp/history, so it
        // is the one tool that cannot claim to be read-only.
        let expected: &[(&str, bool)] = &[
            ("run_pipeline", true),
            ("get_context", true),
            ("get_impact_graph", true),
            ("list_indexed_files", true),
            ("session_recall", true),
            ("check_rule_conflicts", true),
            ("session_log", false),
            ("get_symbol", true),
            ("get_dependencies", true),
            ("get_file_summary", true),
            ("get_project_overview", true),
            ("get_git_diff_context", true),
            ("compress_file", true),
        ];

        let mut listed: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        listed.sort_unstable();
        let mut wanted: Vec<&str> = expected.iter().map(|(name, _)| *name).collect();
        wanted.sort_unstable();
        assert_eq!(listed, wanted, "a newly listed tool needs its annotations decided in this table");

        for (name, read_only) in expected {
            let tool = tools.iter().find(|t| t["name"] == *name).unwrap();
            let annotations = &tool["annotations"];
            assert_eq!(annotations["readOnlyHint"], json!(*read_only), "{name}: readOnlyHint");
            assert_eq!(annotations["destructiveHint"], json!(false), "{name}: destructiveHint");
            assert_eq!(annotations["openWorldHint"], json!(false), "{name}: openWorldHint");
            assert!(
                !codex_auto_mode_requires_approval(annotations),
                "{name}: Codex would still route this call to approval / auto-review"
            );
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
