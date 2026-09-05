// comP Rust Daemon
//
// Responsibilities:
// 1. Initialize GraphDB and SearchEngine
// 2. Listen on stdin/stdout as JSON-RPC 2.0 MCP server
// 3. Provide 5 MCP tools for AI agents
// 4. Semantic search, impact analysis, token calculation

use log::info;
use std::sync::Arc;

mod indexer;
mod graph;
mod search;
mod mcp;

use graph::GraphDB;
use search::SearchEngine;

/// Application state shared across the MCP server.
///
/// Token statistics are stored exclusively in SQLite (via GraphDB::record_tool_call)
/// so both the MCP daemon and the VSCode extension daemon read consistent numbers
/// without any cross-process synchronisation overhead.
///
/// WHY workspace_root is stored here: handlers must not re-read COMP_WORKSPACE_ROOT
/// from the environment at call time — the env var may be absent or stale after
/// the process starts. Fixing root at construction time makes the value stable.
pub struct AppState {
    pub graph_db: Arc<GraphDB>,
    pub search_engine: Arc<tokio::sync::Mutex<SearchEngine>>,
    pub session_id: String,
    pub workspace_root: String,
    /// Which MCP client this daemon process was spawned by (e.g. "claude-code", "codex",
    /// "cursor"). WHY: comP is a stdio MCP server — each client spawns its own comp-daemon
    /// subprocess (see src/mcp/AgentSetup.ts), so several independent OS processes can share
    /// one workspace's .comp/ directory at once. This id attributes session-memory / history
    /// records to the process that wrote them and namespaces the per-agent cache file
    /// (.comp/session-memory/<agent_id>.json) so concurrent processes never write the same
    /// file. Set via COMP_AGENT_ID; defaults to "unknown" for clients not yet wired to set it.
    pub agent_id: String,
}

impl AppState {
    /// Initialize application state
    ///
    /// # Input
    /// - workspace_root: Project root directory
    /// - agent_id: identifies which MCP client spawned this process (see field doc above)
    ///
    /// # Output
    /// - Result<Self>: AppState instance or initialization error
    ///
    /// # Process
    /// 1. GraphDB: Open .comp/index.db (create if not exists)
    /// 2. SearchEngine: Initialize in-memory search engine
    pub async fn new(workspace_root: &str, agent_id: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Initialize GraphDB
        let graph_db = GraphDB::new(workspace_root).await?;
        info!("GraphDB initialized: {}", workspace_root);

        // Initialize SearchEngine
        let search_engine = SearchEngine::new();
        info!("SearchEngine initialized");

        let session_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_else(|_| "0".to_string());

        Ok(AppState {
            graph_db: Arc::new(graph_db),
            search_engine: Arc::new(tokio::sync::Mutex::new(search_engine)),
            session_id,
            workspace_root: workspace_root.to_string(),
            agent_id: agent_id.to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // WHY checked before env_logger/full startup: a lightweight CLI invocation
    // (`comp-daemon append-history <workspace_root> <agent_id>`, used by
    // .claude/hooks/history-record.sh) must not pay the cost of GraphDB open +
    // background indexing on every Claude Code turn, and must exit immediately
    // rather than falling into the long-running MCP stdio server loop below.
    // See mcp::try_run_cli_subcommand for the locked-append implementation shared
    // with the session_log MCP tool.
    let cli_args: Vec<String> = std::env::args().collect();
    if let Some(exit_code) = mcp::try_run_cli_subcommand(&cli_args) {
        std::process::exit(exit_code);
    }

    env_logger::Builder::from_env(env_logger::Env::new().default_filter_or("info")).init();

    info!("comP daemon starting");

    // Determine workspace root
    let workspace_root = std::env::var("COMP_WORKSPACE_ROOT")
        .or_else(|_| std::env::var("WORKSPACE_ROOT"))  // Fallback for compatibility
        .unwrap_or_else(|_| ".".to_string());

    // Identifies which MCP client spawned this process; see AppState::agent_id doc.
    let agent_id = std::env::var("COMP_AGENT_ID").unwrap_or_else(|_| "unknown".to_string());

    // Initialize application state
    let state = Arc::new(AppState::new(&workspace_root, &agent_id).await?);
    info!("Application state initialized");

    // Start indexing in the background and immediately start the MCP server.
    // WHY: Safe to share across threads now that GraphDB uses Mutex<Connection>.
    // Claude Code requires a handshake within MCP startup timeout, so we must respond immediately
    // without waiting for indexing which can take ~48s.
    {
        let state_for_idx = Arc::clone(&state);
        let root_for_idx = workspace_root.clone();
        // Read additional paths before spawning (no async needed)
        let additional_paths = indexer::Indexer::read_additional_paths(&workspace_root);
        tokio::spawn(async move {
            info!("Starting initial workspace indexing...");
            // WHY: Load hashes from previous session database to only re-index modified files.
            let previous_hashes = state_for_idx.graph_db.get_all_file_hashes().unwrap_or_default();
            let mut main_indexer = indexer::Indexer::new(&root_for_idx);
            match main_indexer.index_workspace(Some(&previous_hashes), &state_for_idx.graph_db).await {
                Ok((total, indexed, symbols)) => {
                    info!(
                        "Initial indexing complete: indexed {}/{} files, {} symbols",
                        indexed, total, symbols
                    );
                }
                Err(e) => log::error!("Initial indexing failed: {}", e),
            }

            // Index additional paths (monorepo / multi-root support)
            for extra_root in &additional_paths {
                info!("Indexing additional path: {}", extra_root);
                let extra_hashes = state_for_idx.graph_db.get_all_file_hashes().unwrap_or_default();
                let mut extra_indexer = indexer::Indexer::new(extra_root);
                match extra_indexer.index_workspace(Some(&extra_hashes), &state_for_idx.graph_db).await {
                    Ok((t, i, s)) => info!("Additional path {}: indexed {}/{} files, {} symbols", extra_root, i, t, s),
                    Err(e) => log::warn!("Failed to index additional path {}: {}", extra_root, e),
                }
            }

            // Rebuild TF-IDF index after all indexing (main + additional) is complete
            if let Ok(all_symbols) = state_for_idx.graph_db.get_all_symbols_for_search() {
                let mut se = state_for_idx.search_engine.lock().await;
                match se.build_index(&all_symbols) {
                    Ok(()) => info!("TF-IDF index built: {} symbols", all_symbols.len()),
                    Err(e) => log::warn!("TF-IDF index build failed: {}", e),
                }
            }
        });
    }

    // Start MCP server (indexing runs concurrently in background)
    // JSON-RPC 2.0 over stdio for AI agent communication
    let mcp_server = mcp::MCPServer::new(state);
    info!("MCP server started (listening on stdin/stdout)");

    mcp_server.run().await?;

    info!("MCP server stopped");
    Ok(())
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use serde_json::json;

    /// Phase 7 Integration Test: AppState initialization
    #[tokio::test]
    async fn test_appstate_initialization() {
        // Create temporary directory
        let temp_dir = std::env::temp_dir().join("comP_test_appstate");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        // Initialize AppState
        let result = AppState::new(temp_dir.to_str().unwrap(), "test-agent").await;
        assert!(result.is_ok(), "AppState initialization failed");

        let _state = result.unwrap();
        // Verify AppState creation succeeded

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// 4-4: workspace_root stored in AppState matches the value passed to new()
    #[tokio::test]
    async fn test_appstate_workspace_root_stored() {
        let temp_dir = std::env::temp_dir().join("comP_test_appstate_root");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let root = temp_dir.to_str().unwrap();
        let state = AppState::new(root, "test-agent").await.expect("AppState::new failed");
        assert_eq!(state.workspace_root, root, "workspace_root must equal the path passed to new()");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Verify getStats returns correct initial values (0/0/0) after MCPServer creation
    #[tokio::test]
    async fn test_mcp_server_creation_with_appstate() {
        let temp_dir = std::env::temp_dir().join("comP_test_mcp_server");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let state = Arc::new(
            AppState::new(temp_dir.to_str().unwrap(), "test-agent")
                .await
                .expect("Failed to create AppState"),
        );

        let server = mcp::MCPServer::new(state);
        let stats = server.handle_get_stats().await.expect("getStats failed");
        assert_eq!(stats["total_files"], 0);
        assert_eq!(stats["total_nodes"], 0);
        assert_eq!(stats["total_edges"], 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Phase 7 Integration Test: Full pipeline (index -> search -> MCP)
    #[tokio::test]
    async fn test_full_pipeline_index_search_mcp() {
        let temp_dir = std::env::temp_dir().join("comP_test_full_pipeline");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        // Initialize AppState
        let state = Arc::new(
            AppState::new(temp_dir.to_str().unwrap(), "test-agent")
                .await
                .expect("Failed to create AppState"),
        );

        // Load test data into SearchEngine
        let symbols = vec![
            ("src/auth.rs".to_string(), "authenticate".to_string(), "function".to_string(), 10u32),
            ("src/auth.rs".to_string(), "authorize".to_string(), "function".to_string(), 25u32),
            ("src/user.rs".to_string(), "User".to_string(), "class".to_string(), 5u32),
        ];

        let mut search_engine = state.search_engine.lock().await;
        let result = search_engine.build_index(&symbols);
        assert!(result.is_ok(), "Failed to build search index");
        drop(search_engine);

        // MCP Tool: run_pipeline
        let mcp_server = mcp::MCPServer::new(state.clone());
        let run_pipeline_params = json!({
            "task": "authentication implementation",
            "max_tokens": 8000
        });

        let result = mcp_server.handle_run_pipeline(run_pipeline_params).await;
        assert!(result.is_ok(), "run_pipeline failed");
        let response = result.unwrap();
        assert!(response["pivot_files"].is_array(), "pivot_files should be array");
        assert!(response["total_tokens"].is_number(), "total_tokens should be number");

        // MCP Tool: get_context
        let get_context_params = json!({
            "query": "authentication",
            "limit": 10
        });

        let result = mcp_server.handle_get_context(get_context_params).await;
        assert!(result.is_ok(), "get_context failed");
        let response = result.unwrap();
        assert!(response["results"].is_array(), "results should be array");
        assert!(response["count"].is_number(), "count should be number");

        // MCP Tool: get_impact_graph
        let impact_params = json!({
            "symbol_id": 1,
            "symbol_name": "authenticate"
        });

        let result = mcp_server.handle_get_impact_graph(impact_params).await;
        assert!(result.is_ok(), "get_impact_graph failed");
        let response = result.unwrap();
        assert!(response["affected_files"].is_object(), "affected_files should be object");
        assert!(response["severity"].is_string(), "severity should be string");

        // MCP Tool: list_indexed_files
        let result = mcp_server.handle_list_indexed_files().await;
        assert!(result.is_ok(), "list_indexed_files failed");
        let response = result.unwrap();
        assert!(response["files"].is_array(), "files should be array");
        assert!(response["total_files"].is_number(), "total_files should be number");

        // MCP Tool: get_token_usage
        let result = mcp_server.handle_get_token_usage().await;
        assert!(result.is_ok(), "get_token_usage failed");
        let response = result.unwrap();
        assert!(response["timestamp"].is_number());
        assert!(response["queries_count"].is_number());
        assert!(response["efficiency"].is_string());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// Phase 7 E2E Test: JSON-RPC 2.0 protocol compliance
    #[tokio::test]
    async fn test_json_rpc_protocol_compliance() {
        let temp_dir = std::env::temp_dir().join("comP_test_jsonrpc");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let state = Arc::new(
            AppState::new(temp_dir.to_str().unwrap(), "test-agent")
                .await
                .expect("Failed to create AppState"),
        );

        let mcp_server = mcp::MCPServer::new(state);

        // JSON-RPC request form should be valid
        let params = json!({
            "task": "test",
            "max_tokens": 8000
        });

        let result = mcp_server.handle_run_pipeline(params).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        // Response should have jsonrpc and result fields (when wrapped)
        assert!(response.is_object(), "Response should be a JSON object");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
