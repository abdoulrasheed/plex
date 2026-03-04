use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use ignore::WalkBuilder;

use crate::config::Config;
use crate::embeddings::{symbol_to_embed_text, Embedder};
use crate::parser::CodeParser;
use crate::store::Store;
use crate::types::*;

pub struct Indexer {
    store: Store,
    parser: CodeParser,
    project_root: String,
}

impl Indexer {
    pub fn new(config: &Config) -> Result<Self> {
        let store = Store::open(&config.db_path())?;
        let parser = CodeParser::new();
        let project_root = config.project_root.display().to_string();

        Ok(Indexer {
            store,
            parser,
            project_root,
        })
    }

    pub fn index_project(&mut self, generate_embeddings: bool) -> Result<()> {
        let start = std::time::Instant::now();

        eprintln!("⠋ Scanning files...");
        let files = self.discover_files()?;
        eprintln!("  Found {} supported source files", files.len());

        if files.is_empty() {
            eprintln!("  No supported source files found. Nothing to index.");
            return Ok(());
        }

        eprintln!("⠋ Parsing and indexing...");
        let pb = make_progress_bar(files.len() as u64, "Parsing");
        let mut total_symbols = 0usize;
        let mut total_relations = 0usize;

        self.store.begin_transaction()?;

        for (path, rel_path, lang) in &files {
            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => {
                    pb.inc(1);
                    continue;
                }
            };

            let hash = sha256_hex(&source);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            if let Ok(Some(existing)) = self.store.get_file_by_path(path) {
                if existing.content_hash == hash {
                    pb.inc(1);
                    continue;
                }
            }

            let file = SourceFile {
                id: 0,
                path: path.clone(),
                relative_path: rel_path.clone(),
                language: *lang,
                content_hash: hash,
                size_bytes: source.len() as u64,
                last_indexed: now,
            };

            let file_id = self.store.upsert_file(&file)?;

            let parse_result = self.parser.parse_file(&source, *lang, rel_path)?;

            let symbol_ids = self.store.replace_symbols(file_id, &parse_result.symbols)?;
            total_symbols += symbol_ids.len();

            self.store
                .replace_relations(file_id, &parse_result.relations, &symbol_ids)?;
            total_relations += parse_result.relations.len();

            pb.inc(1);
        }

        pb.finish_and_clear();

        let resolved = self.store.resolve_relations()?;
        self.store.commit()?;

        eprintln!(
            "  {} symbols, {} relations ({} resolved cross-file)",
            total_symbols, total_relations, resolved
        );

        if generate_embeddings {
            self.generate_embeddings()?;
        }

        let elapsed = start.elapsed();
        eprintln!("✓ Index complete in {:.1}s", elapsed.as_secs_f64());

        let stats = self.store.get_stats()?;
        eprintln!(
            "  {} files | {} symbols | {} relations | {} embeddings",
            stats.file_count, stats.symbol_count, stats.relation_count, stats.embedding_count
        );

        Ok(())
    }

    fn discover_files(&self) -> Result<Vec<(String, String, Language)>> {
        let root = Path::new(&self.project_root);
        let mut files = Vec::new();

        for entry in WalkBuilder::new(root)
            .follow_links(false)
            .hidden(true)           // skip hidden files/dirs
            .git_ignore(true)       // respect .gitignore
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|e| {
                if e.file_type().map_or(false, |ft| ft.is_dir()) {
                    return !Config::should_ignore(e.path());
                }
                true
            })
            .build()
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().map_or(true, |ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang = Language::from_extension(ext);

            if !lang.is_supported() {
                continue;
            }

            let abs_path = path.display().to_string();
            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();

            files.push((abs_path, rel_path, lang));
        }

        Ok(files)
    }

    fn generate_embeddings(&self) -> Result<()> {
        const BATCH_SIZE: usize = 256;

        let unembedded = self.store.get_unembedded_symbol_ids()?;
        if unembedded.is_empty() {
            eprintln!("  All symbols already embedded.");
            return Ok(());
        }

        eprintln!(
            "⠋ Generating embeddings for {} symbols...",
            unembedded.len()
        );
        let mut embedder = Embedder::load()?;

        let mut all_items: Vec<(i64, String)> = Vec::with_capacity(unembedded.len());
        let skip_kinds = ["import", "variable", "assignment", "constant"];
        let mut skipped = 0usize;
        for symbol_id in &unembedded {
            if let Ok(Some(sym)) = self.store.get_symbol(*symbol_id) {
                if skip_kinds.contains(&sym.kind.as_str()) {
                    skipped += 1;
                    continue;
                }
                let text = symbol_to_embed_text(
                    &sym.name,
                    sym.kind.as_str(),
                    sym.signature.as_deref(),
                    sym.doc_comment.as_deref(),
                    sym.body_snippet.as_deref(),
                );
                all_items.push((*symbol_id, text));
            }
        }
        if skipped > 0 {
            eprintln!("  Skipping {} trivial symbols (imports/vars) — text search covers them", skipped);
        }
        all_items.sort_by_key(|(_, text)| text.len());

        let pb = make_progress_bar(all_items.len() as u64, "Embedding");

        self.store.begin_transaction()?;

        for chunk in all_items.chunks(BATCH_SIZE) {
            let batch_ids: Vec<i64> = chunk.iter().map(|(id, _)| *id).collect();
            let batch_texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();

            if batch_texts.is_empty() {
                pb.inc(chunk.len() as u64);
                continue;
            }

            match embedder.embed_batch(&batch_texts) {
                Ok(vectors) => {
                    for (id, vector) in batch_ids.iter().zip(vectors.iter()) {
                        self.store.store_embedding(*id, vector)?;
                    }
                }
                Err(e) => {
                    tracing::warn!("Batch embed failed, falling back: {}", e);
                    for (id, text) in batch_ids.iter().zip(batch_texts.iter()) {
                        match embedder.embed(text) {
                            Ok(vector) => {
                                self.store.store_embedding(*id, &vector)?;
                            }
                            Err(e2) => {
                                tracing::warn!("Failed to embed symbol {}: {}", id, e2);
                            }
                        }
                    }
                }
            }

            pb.inc(chunk.len() as u64);
        }

        self.store.commit()?;
        pb.finish_and_clear();
        eprintln!("  ✓ {} embeddings generated", unembedded.len());

        Ok(())
    }
}

fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

fn make_progress_bar(total: u64, msg: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "  {} {{bar:40.green/dim}} {{pos}}/{{len}} ({{eta}})",
                msg
            ))
            .unwrap()
            .progress_chars("█▓░"),
    );
    pb
}

