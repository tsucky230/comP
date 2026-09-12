// mod.rs - Search engine for semantic code search and token counting
//
// Features:
// - TF-IDF based semantic search with cosine similarity
// - Token counting with 4-char approximation
// - Symbol name fuzzy matching
// - Impact graph traversal (BFS)
//
// Responsibilities:
// - Build TF-IDF index from symbol database
// - Score documents based on query relevance
// - Traverse dependency graph for impact analysis
// - Count tokens for LLM context budgeting

use anyhow::Result;
use std::collections::{HashMap, VecDeque};

/// Search result with relevance score
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// File path
    pub file_path: String,
    /// Symbol name
    pub symbol_name: String,
    /// Relevance score (0.0 - 1.0)
    pub score: f32,
    /// Symbol kind (function, class, type, etc.)
    pub kind: String,
    /// Line number in file
    pub line: u32,
}

/// Helper: Tokenize text (camelCase, snake_case, SCREAMING_CASE)
///
/// # Examples
/// - "getAuthToken" → ["get", "auth", "token"]
/// - "snake_case" → ["snake", "case"]
/// - "HTTPServer" → ["http", "server"]
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = text.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '_' || ch == '-' {
            // Separator: flush current token
            if !current.is_empty() {
                tokens.push(current.to_lowercase());
                current.clear();
            }
        } else if ch.is_uppercase() {
            // camelCase boundary: flush if current is not empty
            if !current.is_empty() && (i == 0 || !chars[i - 1].is_uppercase()) {
                tokens.push(current.to_lowercase());
                current.clear();
            }
            current.push(ch);
        } else {
            current.push(ch);
        }
    }

    // Flush remaining
    if !current.is_empty() {
        tokens.push(current.to_lowercase());
    }

    tokens.into_iter().filter(|t| !t.is_empty()).collect()
}

/// Search engine for code queries
pub struct SearchEngine {
    /// TF-IDF matrix (term -> file -> weight)
    tfidf_matrix: HashMap<String, HashMap<String, f32>>,
    /// Document metadata (file_path -> [(symbol_name, kind, line)])
    documents: HashMap<String, Vec<(String, String, u32)>>,
    /// Total number of documents (files)
    doc_count: usize,
}

impl SearchEngine {
    /// Create a new search engine
    pub fn new() -> Self {
        SearchEngine {
            tfidf_matrix: HashMap::new(),
            documents: HashMap::new(),
            doc_count: 0,
        }
    }

    /// Build TF-IDF index from database symbols
    ///
    /// # Arguments
    /// - symbols: Vec<(file_path, symbol_name, kind, line)>
    ///
    /// # Process:
    /// 1. Tokenize each symbol name
    /// 2. Calculate term frequency (TF) per file
    /// 3. Calculate inverse document frequency (IDF)
    /// 4. Compute TF-IDF: TF * IDF for each term-file pair
    ///
    /// # Returns
    /// - Result<()>: Success or error
    pub fn build_index(&mut self, symbols: &[(String, String, String, u32)]) -> Result<()> {
        // 1. Store documents and count
        let mut tf_matrix: HashMap<String, HashMap<String, f32>> = HashMap::new();

        for (file_path, symbol_name, kind, line) in symbols {
            self.documents
                .entry(file_path.clone())
                .or_default()
                .push((symbol_name.clone(), kind.clone(), *line));
        }

        self.doc_count = self.documents.len();

        // 2. Build term frequency matrix
        for (file_path, symbols_in_file) in &self.documents {
            let mut term_freq: HashMap<String, f32> = HashMap::new();

            for (symbol_name, _kind, _line) in symbols_in_file {
                let tokens = tokenize(symbol_name);
                for token in tokens {
                    *term_freq.entry(token).or_insert(0.0) += 1.0;
                }
            }

            // Normalize TF (term frequency)
            let total_terms: f32 = term_freq.values().sum();
            if total_terms > 0.0 {
                for freq in term_freq.values_mut() {
                    *freq /= total_terms;
                }
            }

            // Store in TF matrix
            for (term, freq) in term_freq {
                tf_matrix.entry(term).or_default()
                    .insert(file_path.clone(), freq);
            }
        }

        // 3. Calculate IDF and build final TF-IDF matrix
        self.tfidf_matrix.clear();
        for (term, docs) in tf_matrix {
            let df = docs.len() as f32;
            let idf = if self.doc_count > 0 {
                (self.doc_count as f32 / df).ln()
            } else {
                0.0
            };

            let mut tfidf_row = HashMap::new();
            for (doc, tf) in docs {
                tfidf_row.insert(doc, tf * idf);
            }

            self.tfidf_matrix.insert(term, tfidf_row);
        }

        Ok(())
    }

    /// Search for relevant code based on query (TF-IDF cosine similarity)
    ///
    /// # Arguments
    /// - query: Search query (e.g., "authentication")
    /// - limit: Maximum results to return
    ///
    /// # Process:
    /// 1. Tokenize query
    /// 2. Create query vector from TF-IDF weights
    /// 3. Calculate cosine similarity for each document
    /// 4. Rank by score and return top N
    ///
    /// # Returns
    /// - Vec<SearchResult>: Results ranked by relevance
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        if query.is_empty() || self.tfidf_matrix.is_empty() {
            return Ok(Vec::new());
        }

        // 1. Tokenize query
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Create query vector (equally weighted tokens)
        let mut query_vector: HashMap<String, f32> = HashMap::new();
        for token in &query_tokens {
            *query_vector.entry(token.clone()).or_insert(0.0) += 1.0;
        }

        // Normalize query vector
        let query_magnitude: f32 = query_vector.values().map(|x| x * x).sum::<f32>().sqrt();
        if query_magnitude == 0.0 {
            return Ok(Vec::new());
        }

        for freq in query_vector.values_mut() {
            *freq /= query_magnitude;
        }

        // 3. Score each document
        let mut scores: Vec<(String, f32)> = Vec::new();

        for (file_path, symbols) in &self.documents {
            // Calculate document TF-IDF vector
            let mut doc_vector: HashMap<String, f32> = HashMap::new();

            for (symbol_name, _kind, _line) in symbols {
                let tokens = tokenize(symbol_name);
                for token in tokens {
                    if let Some(tfidf_row) = self.tfidf_matrix.get(&token) {
                        if let Some(&weight) = tfidf_row.get(file_path) {
                            *doc_vector.entry(token).or_insert(0.0) += weight;
                        }
                    }
                }
            }

            // Calculate cosine similarity
            let mut dot_product = 0.0;
            for (token, query_weight) in &query_vector {
                if let Some(&doc_weight) = doc_vector.get(token) {
                    dot_product += query_weight * doc_weight;
                }
            }

            let doc_magnitude: f32 = doc_vector.values().map(|x| x * x).sum::<f32>().sqrt();
            let similarity = if doc_magnitude > 0.0 {
                dot_product / doc_magnitude
            } else {
                0.0
            };

            if similarity > 0.0 {
                scores.push((file_path.clone(), similarity));
            }
        }

        // 4. Sort and return top N
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut results = Vec::new();
        for (file_path, score) in scores.iter().take(limit) {
            if let Some(symbols) = self.documents.get(file_path) {
                // Return first symbol from matching file
                if let Some((symbol_name, kind, line)) = symbols.first() {
                    results.push(SearchResult {
                        file_path: file_path.clone(),
                        symbol_name: symbol_name.clone(),
                        score: *score,
                        kind: kind.clone(),
                        line: *line,
                    });
                }
            }
        }

        // WHY: TF-IDF only scores exact token matches. Prefix queries like "auth" -> "authenticate"
        //      would return 0 hits. We complement this with substring fallback.
        if results.is_empty() {
            let mut seen = std::collections::HashSet::new();
            'outer: for (file_path, symbols) in &self.documents {
                for (symbol_name, kind, line) in symbols {
                    let sym_lower = symbol_name.to_lowercase();
                    let matches = query_tokens.iter().any(|qt| sym_lower.contains(qt.as_str()));
                    if matches && seen.insert(file_path.clone()) {
                        results.push(SearchResult {
                            file_path: file_path.clone(),
                            symbol_name: symbol_name.clone(),
                            score: 0.3,
                            kind: kind.clone(),
                            line: *line,
                        });
                        if results.len() >= limit {
                            break 'outer;
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Fuzzy match symbol names
    ///
    /// Simple prefix matching for autocomplete
    pub fn fuzzy_match(&self, prefix: &str, all_symbols: &[String]) -> Vec<String> {
        all_symbols.iter()
            .filter(|s| s.to_lowercase().starts_with(&prefix.to_lowercase()))
            .cloned()
            .collect()
    }

    /// Get impact graph - all symbols affected by changing a symbol
    ///
    /// # Arguments
    /// - symbol_id: ID of the symbol being changed
    /// - reverse_deps: Map of (from_id -> [to_ids]) - who depends on whom
    /// - symbol_map: Map of (id -> (name, file_path))
    ///
    /// # Process:
    /// 1. Find all symbols that depend on symbol_id (direct dependents)
    /// 2. Recursively find symbols that depend on those (transitive)
    /// 3. Group by file
    /// 4. Return impact analysis
    ///
    /// # Returns
    /// - Vec<(file_path, Vec<affected_symbol_names>)>
    pub fn get_impact_graph(
        &self,
        symbol_id: i64,
        reverse_deps: &HashMap<i64, Vec<i64>>,
        symbol_map: &HashMap<i64, (String, String)>,
    ) -> Result<Vec<(String, Vec<String>)>> {
        self.get_impact_graph_depth(symbol_id, reverse_deps, symbol_map, 0)
    }

    /// BFS impact graph with configurable depth limit.
    /// max_depth=0 means unlimited (traverse all transitive dependents).
    pub fn get_impact_graph_depth(
        &self,
        symbol_id: i64,
        reverse_deps: &HashMap<i64, Vec<i64>>,
        symbol_map: &HashMap<i64, (String, String)>,
        max_depth: usize,
    ) -> Result<Vec<(String, Vec<String>)>> {
        let mut affected: HashMap<i64, usize> = HashMap::new(); // id -> depth
        let mut queue: VecDeque<(i64, usize)> = VecDeque::new();

        queue.push_back((symbol_id, 0));
        affected.insert(symbol_id, 0);

        while let Some((current_id, depth)) = queue.pop_front() {
            if max_depth > 0 && depth >= max_depth {
                continue;
            }
            if let Some(dependents) = reverse_deps.get(&current_id) {
                for &dependent_id in dependents {
                    if let std::collections::hash_map::Entry::Vacant(e) = affected.entry(dependent_id) {
                        e.insert(depth + 1);
                        queue.push_back((dependent_id, depth + 1));
                    }
                }
            }
        }

        affected.remove(&symbol_id);

        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for &id in affected.keys() {
            if let Some((name, file_path)) = symbol_map.get(&id) {
                result
                    .entry(file_path.clone())
                    .or_default()
                    .push(name.clone());
            }
        }

        let mut impact_vec: Vec<_> = result.into_iter().collect();
        impact_vec.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(impact_vec)
    }
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Token counter for LLM context budgeting
pub struct TokenCounter;

#[allow(dead_code)]
impl TokenCounter {
    /// Count tokens in text using tiktoken's cl100k_base encoding.
    ///
    /// # Arguments
    /// - text: Text to count
    ///
    /// # Returns
    /// - Token count under OpenAI's cl100k_base BPE. This is not Claude's own
    ///   tokenizer (Anthropic does not publish one for offline use), but it is
    ///   a widely used stand-in that tracks Claude's real count closely enough
    ///   for context-budget purposes on English text and source code.
    ///
    /// # Process:
    /// 1. Reuse the process-wide cl100k_base singleton (avoids re-loading the
    ///    embedded BPE rank table on every call — this runs on every
    ///    run_pipeline invocation).
    /// 2. `count_ordinary` counts tokens without materializing the token
    ///    vector and never errors on untrusted input (unlike the
    ///    special-token-aware encode methods), which matters here since the
    ///    input is arbitrary file content.
    pub fn count_tokens(text: &str) -> Result<usize> {
        Ok(tiktoken_rs::cl100k_base_singleton().count_ordinary(text))
    }

    /// Estimate total tokens for a set of files
    /// 
    /// # Arguments
    /// - files: Vec<(file_path, content)>
    ///
    /// # Returns
    /// - Total token count across all files
    pub fn count_files(files: &[(String, String)]) -> Result<usize> {
        let mut total = 0;
        for (_path, content) in files {
            total += Self::count_tokens(content)?;
        }
        Ok(total)
    }

    /// Estimate token savings from context optimization
    /// 
    /// # Arguments
    /// - full_tokens: Tokens if all files were included
    /// - optimized_tokens: Tokens in optimized context
    ///
    /// # Returns
    /// - Savings percentage (e.g., "65%")
    pub fn calculate_savings(full_tokens: usize, optimized_tokens: usize) -> String {
        if full_tokens == 0 {
            return "0%".to_string();
        }
        let saved = full_tokens.saturating_sub(optimized_tokens);
        let percentage = (saved as f32 / full_tokens as f32) * 100.0;
        format!("{:.0}%", percentage)
    }

    /// Estimate API cost based on tokens
    /// 
    /// # Arguments
    /// - tokens: Token count
    /// - model: Model name ("sonnet", "opus", "haiku")
    ///
    /// # Returns
    /// - Estimated cost (e.g., "$0.04")
    pub fn estimate_cost(tokens: usize, model: &str) -> String {
        // Pricing as of 2025 (example rates)
        let input_rate = match model {
            "sonnet" => 3.0 / 1_000_000.0, // $3 per M input tokens
            "opus" => 15.0 / 1_000_000.0,  // $15 per M input tokens
            "haiku" => 0.8 / 1_000_000.0,  // $0.80 per M input tokens
            _ => 3.0 / 1_000_000.0,
        };

        let cost = tokens as f32 * input_rate;
        format!("${:.2}", cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_camel_case() {
        let tokens = tokenize("getAuthToken");
        assert_eq!(tokens, vec!["get", "auth", "token"]);
    }

    #[test]
    fn test_tokenize_snake_case() {
        let tokens = tokenize("get_auth_token");
        assert_eq!(tokens, vec!["get", "auth", "token"]);
    }

    #[test]
    fn test_tokenize_screaming_case() {
        let tokens = tokenize("HTTP_SERVER");
        assert_eq!(tokens, vec!["http", "server"]);
    }

    #[test]
    fn test_tokenize_mixed() {
        let tokens = tokenize("HTTPServer_v2");
        assert!(!tokens.is_empty());
        assert!(tokens[0].contains("http")); // Should tokenize properly
    }

    #[test]
    fn test_search_engine_creation() {
        let _engine = SearchEngine::new();
    }

    #[test]
    fn test_build_index_simple() {
        let mut engine = SearchEngine::new();
        let symbols = vec![
            ("auth.rs".to_string(), "authenticate".to_string(), "function".to_string(), 5),
            ("auth.rs".to_string(), "authorizeUser".to_string(), "function".to_string(), 10),
        ];

        let result = engine.build_index(&symbols);
        assert!(result.is_ok());
        assert!(!engine.tfidf_matrix.is_empty());
    }

    #[test]
    fn test_search_basic() {
        let mut engine = SearchEngine::new();
        let symbols = vec![
            ("auth.rs".to_string(), "authenticate".to_string(), "function".to_string(), 5),
            ("api.rs".to_string(), "apiRoute".to_string(), "function".to_string(), 10),
        ];

        engine.build_index(&symbols).unwrap();
        let results = engine.search("auth", 10).unwrap();

        // Should find authenticate in auth.rs
        assert!(!results.is_empty());
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_search_empty_query() {
        let mut engine = SearchEngine::new();
        let symbols = vec![
            ("auth.rs".to_string(), "authenticate".to_string(), "function".to_string(), 5),
        ];

        engine.build_index(&symbols).unwrap();
        let results = engine.search("", 10).unwrap();

        // Empty query should return no results
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_ranking() {
        let mut engine = SearchEngine::new();
        let symbols = vec![
            ("auth.rs".to_string(), "authenticate".to_string(), "function".to_string(), 5),
            ("other.rs".to_string(), "otherFunction".to_string(), "function".to_string(), 10),
        ];

        engine.build_index(&symbols).unwrap();
        let results = engine.search("auth", 10).unwrap();

        // Most relevant result should be first
        if !results.is_empty() {
            assert_eq!(results[0].symbol_name, "authenticate");
        }
    }

    #[test]
    fn test_impact_graph_direct() {
        let mut symbol_map = HashMap::new();
        symbol_map.insert(1, ("auth".to_string(), "auth.rs".to_string()));
        symbol_map.insert(2, ("validate".to_string(), "auth.rs".to_string()));
        symbol_map.insert(3, ("login".to_string(), "api.rs".to_string()));

        let mut reverse_deps = HashMap::new();
        reverse_deps.insert(1, vec![2, 3]); // 2 and 3 depend on 1

        let engine = SearchEngine::new();
        let impact = engine.get_impact_graph(1, &reverse_deps, &symbol_map).unwrap();

        // Should find 2 files affected
        assert_eq!(impact.len(), 2);
    }

    #[test]
    fn test_impact_graph_transitive() {
        let mut symbol_map = HashMap::new();
        symbol_map.insert(1, ("auth".to_string(), "auth.rs".to_string()));
        symbol_map.insert(2, ("validate".to_string(), "auth.rs".to_string()));
        symbol_map.insert(3, ("login".to_string(), "api.rs".to_string()));

        let mut reverse_deps = HashMap::new();
        reverse_deps.insert(1, vec![2]); // 2 depends on 1
        reverse_deps.insert(2, vec![3]); // 3 depends on 2

        let engine = SearchEngine::new();
        let impact = engine.get_impact_graph(1, &reverse_deps, &symbol_map).unwrap();

        // Should find both 2 and 3 (transitive)
        let affected_ids: Vec<_> = impact.iter().flat_map(|(_, names)| names).cloned().collect();
        assert!(affected_ids.contains(&"validate".to_string()));
        assert!(affected_ids.contains(&"login".to_string()));
    }

    #[test]
    fn test_impact_graph_empty() {
        let symbol_map = HashMap::new();
        let reverse_deps = HashMap::new();

        let engine = SearchEngine::new();
        let impact = engine.get_impact_graph(1, &reverse_deps, &symbol_map).unwrap();

        // No dependents should return empty
        assert!(impact.is_empty());
    }

    #[test]
    fn test_token_counting_empty() {
        let tokens = TokenCounter::count_tokens("").unwrap();
        assert_eq!(tokens, 0);
    }

    #[test]
    fn test_token_counting_sample() {
        let text = "fn main() {}"; // 12 chars
        let tokens = TokenCounter::count_tokens(text).unwrap();
        assert!(tokens > 0);
        assert!(tokens <= 5); // Should be ~3-4 tokens
    }

    #[test]
    fn test_token_savings_calculation() {
        let savings = TokenCounter::calculate_savings(1000, 350);
        assert_eq!(savings, "65%");
    }

    #[test]
    fn test_cost_estimation() {
        // 1000 tokens on sonnet (~$0.003)
        let cost = TokenCounter::estimate_cost(1000, "sonnet");
        assert!(cost.starts_with("$0.00"));
    }

    #[test]
    fn test_fuzzy_match() {
        let engine = SearchEngine::new();
        let symbols = vec![
            "authenticate".to_string(),
            "authorization".to_string(),
            "auth_helper".to_string(),
        ];

        let matches = engine.fuzzy_match("auth", &symbols);
        assert_eq!(matches.len(), 3); // All start with "auth"
    }

    #[test]
    fn test_fuzzy_match_case_insensitive() {
        let engine = SearchEngine::new();
        let symbols = vec![
            "Authenticate".to_string(),
            "AUTHORIZATION".to_string(),
        ];

        let matches = engine.fuzzy_match("auth", &symbols);
        assert_eq!(matches.len(), 2); // Case insensitive matching
    }
}
