use anyhow::Result;

use crate::config::Config;
use crate::embeddings::{cosine_similarity, Embedder};
use crate::store::Store;
use crate::types::*;

/// Combined full-text + semantic search engine.
pub struct Searcher {
    store: Store,
    embedder: Option<Embedder>,
    /// Cached embeddings for brute-force vector search.
    /// Loaded lazily on first semantic search.
    embedding_cache: Option<Vec<(i64, Vec<f32>)>>,
}

impl Searcher {
    /// Create a new searcher. Tries to load the embedding model
    /// but falls back to text-only search if unavailable.
    pub fn new(config: &Config) -> Result<Self> {
        let store = Store::open(&config.db_path())?;
        let embedder = Embedder::load().ok();

        Ok(Searcher {
            store,
            embedder,
            embedding_cache: None,
        })
    }

    /// Create a searcher using an existing Store (for MCP server reuse).
    pub fn from_store(store: Store) -> Result<Self> {
        let embedder = Embedder::load().ok();
        Ok(Searcher {
            store,
            embedder,
            embedding_cache: None,
        })
    }

    /// Hybrid search: combines FTS5 text search with semantic vector search.
    /// Results are ranked by a blended score.
    pub fn search(&mut self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let mut results_map: std::collections::HashMap<i64, (Symbol, String, f32)> =
            std::collections::HashMap::new();

        // --- Full-text search ---
        let fts_results = self.store.fts_search(query, limit * 2)?;
        for (symbol_id, rank) in &fts_results {
            if let Ok(Some(sym)) = self.store.get_symbol(*symbol_id) {
                let file_path = self
                    .store
                    .get_file_path(sym.file_id)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                // FTS5 rank is negative (lower = better), normalize to 0-1 score
                let score = 1.0 / (1.0 + rank.abs() as f32);
                results_map.insert(*symbol_id, (sym, file_path, score));
            }
        }

        // --- Semantic search (if embedder available) ---
        if let Some(ref mut embedder) = self.embedder {
            // Lazy-load the embedding cache
            if self.embedding_cache.is_none() {
                self.embedding_cache = Some(self.store.load_all_embeddings()?);
            }

            if let Some(ref cache) = self.embedding_cache {
                if !cache.is_empty() {
                    if let Ok(query_vec) = embedder.embed(query) {
                        // Compute cosine similarity against all embeddings
                        let mut scored: Vec<(i64, f32)> = cache
                            .iter()
                            .map(|(id, vec)| (*id, cosine_similarity(&query_vec, vec)))
                            .collect();

                        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

                        for (symbol_id, sem_score) in scored.iter().take(limit * 2) {
                            if let Some((_sym, _path, existing_score)) =
                                results_map.get_mut(symbol_id)
                            {
                                // Blend: boost items found by both FTS and vector search
                                *existing_score = (*existing_score + sem_score) / 2.0 + 0.1;
                            } else if let Ok(Some(sym)) = self.store.get_symbol(*symbol_id) {
                                let file_path = self
                                    .store
                                    .get_file_path(sym.file_id)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default();
                                results_map.insert(*symbol_id, (sym, file_path, *sem_score));
                            }
                        }
                    }
                }
            }
        }

        // --- Sort by score descending and take top N ---
        let mut results: Vec<SearchResult> = results_map
            .into_values()
            .map(|(sym, file_path, score)| {
                let snippet = sym
                    .signature
                    .clone()
                    .or(sym.body_snippet.clone())
                    .unwrap_or_else(|| format!("{} {}", sym.kind.as_str(), sym.name));
                SearchResult {
                    symbol: sym,
                    file_path,
                    score,
                    snippet,
                }
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results.truncate(limit);

        Ok(results)
    }

    /// Text-only search (no embedding model needed).
    pub fn text_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let fts_results = self.store.fts_search(query, limit)?;
        let mut results = Vec::new();

        for (symbol_id, rank) in fts_results {
            if let Ok(Some(sym)) = self.store.get_symbol(symbol_id) {
                let file_path = self
                    .store
                    .get_file_path(sym.file_id)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let score = 1.0 / (1.0 + rank.abs() as f32);
                let snippet = sym
                    .signature
                    .clone()
                    .or(sym.body_snippet.clone())
                    .unwrap_or_else(|| format!("{} {}", sym.kind.as_str(), sym.name));
                results.push(SearchResult {
                    symbol: sym,
                    file_path,
                    score,
                    snippet,
                });
            }
        }

        Ok(results)
    }

    /// Access the store for direct queries (used by MCP tools).
    pub fn store(&self) -> &Store {
        &self.store
    }
}
