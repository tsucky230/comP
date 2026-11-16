//! get_context returns the symbol IDs get_impact_graph requires.
//! Owned by the designer role; implementers must not edit.

use super::*;
use tempfile::TempDir;

async fn server_with_a_call() -> (TempDir, MCPServer, i64) {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(crate::AppState::new(dir.path().to_str().unwrap(), "test-agent").await.expect("AppState"));
    let lib = state.graph_db.upsert_file("src/lib.rs", "h1", "rust", 0).unwrap();
    let main = state.graph_db.upsert_file("src/main.rs", "h2", "rust", 0).unwrap();
    let callee = state.graph_db.insert_node(lib, "callee_fn", "function", 3, 0, None, true, None).unwrap();
    let caller = state.graph_db.insert_node(main, "caller_fn", "function", 7, 0, None, true, None).unwrap();
    state.graph_db.insert_edge(caller, callee, "calls").unwrap();
    (dir, MCPServer::new(state), callee)
}

#[tokio::test]
async fn get_context_results_carry_the_symbol_id() {
    let (_dir, server, callee) = server_with_a_call().await;

    let out = server.handle_get_context(json!({ "query": "callee_fn" })).await.unwrap();

    let hit = &out["results"][0];
    assert_eq!(hit["symbol"], "callee_fn");
    assert_eq!(hit["id"].as_i64(), Some(callee));
}

#[tokio::test]
async fn the_id_from_get_context_drives_get_impact_graph() {
    let (_dir, server, _) = server_with_a_call().await;

    let out = server.handle_get_context(json!({ "query": "callee_fn" })).await.unwrap();
    let id = out["results"][0]["id"].as_i64().expect("get_context must return an id");
    let impact = server
        .handle_get_impact_graph(json!({ "symbol_id": id, "symbol_name": "callee_fn" }))
        .await
        .unwrap();

    assert_eq!(impact["impact_count"].as_u64(), Some(1));
    let callers = impact["affected_files"]["src/main.rs"].as_array().expect("caller's file is affected");
    assert!(callers.iter().any(|c| c == "caller_fn"), "affected: {}", impact["affected_files"]);
}

#[test]
fn tool_descriptions_name_get_context_as_the_source_of_symbol_ids() {
    let defs = MCPServer::tool_definitions();
    let tool = |name: &str| defs["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();

    let impact = tool("get_impact_graph");
    let text = format!("{} {}", impact["description"], impact["inputSchema"]["properties"]["symbol_id"]["description"]);
    assert!(text.contains("get_context"), "{}", text);
    assert!(!text.contains("run_pipeline"), "run_pipeline returns no symbol ids: {}", text);
}
