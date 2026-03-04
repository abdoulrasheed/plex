use clap::{Parser, Subcommand};
use serde_json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "plex")]
#[command(version)]
#[command(about = "⚡ Blazingly fast local code intelligence – index, search, visualize, MCP")]
#[command(
    long_about = "Plex indexes your codebase into a queryable knowledge base.\n\
    Use it standalone via CLI, as an MCP server for AI assistants (Cursor, Claude, etc.),\n\
    or through its interactive visualizations.\n\n\
    Everything runs locally. No API keys. No data leaves your machine."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,

        #[arg(long)]
        no_embed: bool,
    },

    Mcp {
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    Search {
        query: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    Stats {
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    Calls {
        /// symbol name
        name: String,

        #[arg(short, long, default_value = ".")]
        path: PathBuf,

        #[arg(short, long, default_value = "3")]
        depth: usize,

        #[arg(long)]
        callers: bool,

        #[arg(long)]
        json: bool,
    },

    Serve {
        #[arg(default_value = ".")]
        path: PathBuf,

        #[arg(short = 'P', long, default_value = "7777")]
        port: u16,
    },

    Symbols {
        file: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,

        #[arg(long)]
        json: bool,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("plex=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Index { path, no_embed } => {
            let config = plex::config::Config::new(path)?;
            eprintln!("⚡ Plex — indexing '{}'", config.project_name());
            let mut indexer = plex::indexer::Indexer::new(&config)?;
            indexer.index_project(!no_embed)?;
        }

        Commands::Mcp { path } => {
            let config = plex::config::Config::new(path)?;
            plex::mcp::run_stdio(&config)?;
        }

        Commands::Search {
            query,
            path,
            limit,
            json,
        } => {
            let config = plex::config::Config::new(path)?;
            let mut searcher = plex::search::Searcher::new(&config)?;
            let results = searcher.search(&query, limit)?;

            if results.is_empty() {
                if json {
                    println!("[]");
                } else {
                    eprintln!("No results found for '{}'", query);
                }
                return Ok(());
            }

            if json {
                let json_results: Vec<serde_json::Value> = results
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "name": r.symbol.name,
                            "qualifiedName": r.symbol.qualified_name,
                            "kind": r.symbol.kind.as_str(),
                            "filePath": r.file_path,
                            "line": r.symbol.start_line,
                            "score": r.score,
                            "signature": r.symbol.signature,
                            "docComment": r.symbol.doc_comment,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&json_results)?);
            } else {
                for (i, result) in results.iter().enumerate() {
                    println!(
                        "{}. {} ({})",
                        i + 1,
                        result.symbol.qualified_name,
                        result.symbol.kind.as_str(),
                    );
                    println!("   {}:{}", result.file_path, result.symbol.start_line);
                    if let Some(ref sig) = result.symbol.signature {
                        println!("   {}", sig);
                    }
                    println!("   Score: {:.4}", result.score);
                    println!();
                }
            }
        }

        Commands::Stats { path } => {
            let config = plex::config::Config::new(path)?;
            let store = plex::store::Store::open(&config.db_path())?;
            let stats = store.get_stats()?;

            println!("⚡ Plex Index — {}", config.project_name());
            println!("═══════════════════════════════");
            println!("Files:      {}", stats.file_count);
            println!("Symbols:    {}", stats.symbol_count);
            println!("Relations:  {}", stats.relation_count);
            println!("Embeddings: {}", stats.embedding_count);
            println!("Languages:  {}", stats.languages.join(", "));
        }

        Commands::Calls {
            name,
            path,
            depth,
            callers,
            json,
        } => {
            let config = plex::config::Config::new(path)?;
            let store = plex::store::Store::open(&config.db_path())?;
            let analyzer = plex::graph::GraphAnalyzer::new(&store);

            let graph = if callers {
                analyzer.get_callers(&name, depth)?
            } else {
                analyzer.get_call_graph(&name, depth)?
            };

            if json {
                let json_nodes: Vec<serde_json::Value> = graph
                    .nodes
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "name": n.name,
                            "qualifiedName": n.qualified_name,
                            "kind": n.kind.as_str(),
                            "filePath": n.file_path,
                            "line": n.line,
                        })
                    })
                    .collect();
                let id_to_name: std::collections::HashMap<i64, &str> = graph
                    .nodes
                    .iter()
                    .map(|n| (n.symbol_id, n.qualified_name.as_str()))
                    .collect();
                let json_edges: Vec<serde_json::Value> = graph
                    .edges
                    .iter()
                    .filter_map(|e| {
                        let src = id_to_name.get(&e.source_id)?;
                        let tgt = id_to_name.get(&e.target_id)?;
                        Some(serde_json::json!({
                            "source": src,
                            "target": tgt,
                            "kind": format!("{:?}", e.kind),
                        }))
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({
                        "nodes": json_nodes,
                        "edges": json_edges,
                    })
                );
            } else {
                let direction = if callers { "Callers of" } else { "Called by" };
                println!("{} '{}' (depth={}):", direction, name, depth);
                println!();
                println!(
                    "  Root: {} ({})",
                    graph.root.qualified_name,
                    graph.root.kind.as_str()
                );
                println!();

                for node in &graph.nodes[1..] {
                    println!(
                        "  → {} ({}) at {}:{}",
                        node.qualified_name,
                        node.kind.as_str(),
                        node.file_path,
                        node.line,
                    );
                }

                println!();
                println!("{} nodes, {} edges", graph.nodes.len(), graph.edges.len());
            }
        }

        Commands::Serve { path, port } => {
            let config = plex::config::Config::new(path)?;
            eprintln!(
                "⚡ Plex visualization server starting on http://localhost:{}",
                port
            );
            plex::viz::serve(&config, port)?;
        }

        Commands::Symbols { file, path, json } => {
            let config = plex::config::Config::new(path)?;
            let store = plex::store::Store::open(&config.db_path())?;
            let symbols = store.get_file_symbols_by_path(&file)?;

            if symbols.is_empty() {
                if json {
                    println!("[]");
                } else {
                    eprintln!("No symbols found for '{}'", file);
                }
                return Ok(());
            }

            if json {
                let json_symbols: Vec<serde_json::Value> = symbols
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "qualifiedName": s.qualified_name,
                            "kind": s.kind.as_str(),
                            "filePath": file,
                            "line": s.start_line,
                            "signature": s.signature,
                            "docComment": s.doc_comment,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&json_symbols)?);
            } else {
                for s in &symbols {
                    println!(
                        "  {} ({}) at line {}",
                        s.qualified_name,
                        s.kind.as_str(),
                        s.start_line,
                    );
                    if let Some(ref sig) = s.signature {
                        println!("    {}", sig);
                    }
                }
            }
        }
    }

    Ok(())
}
