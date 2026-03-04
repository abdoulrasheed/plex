use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::graph::GraphAnalyzer;
use crate::search::Searcher;
use crate::store::Store;
use crate::types::*;

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "search",
            "description": "Search the codebase using semantic and text search. Finds functions, classes, and other symbols by meaning, not just name. Use this to find relevant code for any topic.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query, e.g. 'authentication handler' or 'database connection pool'"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 10)",
                        "default": 10
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_symbol",
            "description": "Get detailed information about a specific symbol (function, class, etc.) including its signature, documentation, and source location.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Symbol name to look up (exact or pattern with %). e.g. 'UserService' or '%auth%'"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "get_callers",
            "description": "Find all functions/methods that call the given symbol. Answers 'who calls this function?'",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the function/method to find callers for"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "How many levels deep to trace callers (default: 2)",
                        "default": 2
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "get_callees",
            "description": "Find all functions/methods called by the given symbol. Answers 'what does this function call?'",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the function/method to find callees for"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "How many levels deep to trace calls (default: 2)",
                        "default": 2
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "get_inheritance",
            "description": "Get the inheritance/implementation hierarchy for a class, struct, trait, or interface. Shows superclasses and subclasses.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the class/trait/interface"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "find_implementations",
            "description": "Find all implementations of an interface or trait.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the interface or trait"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "get_file_symbols",
            "description": "List all symbols (functions, classes, etc.) defined in a specific file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative file path, e.g. 'src/auth/service.ts'"
                    }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "get_project_structure",
            "description": "Get an overview of the project: file count, symbol count, language distribution, directory structure, and symbol kind breakdown.",
            "inputSchema": {
                "type": "object",
                "properties": {},
            }
        }),
        json!({
            "name": "get_references",
            "description": "Find all references to a symbol — every place it is called, used, or mentioned across the codebase.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Symbol name to find references for"
                    }
                },
                "required": ["name"]
            }
        }),
    ]
}

pub fn execute_tool(
    name: &str,
    args: &Value,
    searcher: &mut Searcher,
    graph_store: &Store,
) -> Result<String> {
    match name {
        "search" => tool_search(args, searcher),
        "get_symbol" => tool_get_symbol(args, graph_store),
        "get_callers" => tool_get_callers(args, graph_store),
        "get_callees" => tool_get_callees(args, graph_store),
        "get_inheritance" => tool_get_inheritance(args, graph_store),
        "find_implementations" => tool_find_implementations(args, graph_store),
        "get_file_symbols" => tool_get_file_symbols(args, graph_store),
        "get_project_structure" => tool_get_project_structure(graph_store),
        "get_references" => tool_get_references(args, graph_store),
        _ => bail!("Unknown tool: {}", name),
    }
}

fn tool_search(args: &Value, searcher: &mut Searcher) -> Result<String> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'query' argument"))?;
    let limit = args
        .get("limit")
        .and_then(|l| l.as_u64())
        .unwrap_or(10) as usize;

    let results = searcher.search(query, limit)?;

    if results.is_empty() {
        return Ok(format!("No results found for '{}'", query));
    }

    let mut output = format!("Found {} results for '{}':\n\n", results.len(), query);
    for (i, r) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. **{}** ({})\n   File: {}:{}\n   Score: {:.3}\n",
            i + 1,
            r.symbol.qualified_name,
            r.symbol.kind.as_str(),
            r.file_path,
            r.symbol.start_line,
            r.score,
        ));
        if let Some(ref sig) = r.symbol.signature {
            output.push_str(&format!("   Signature: {}\n", sig));
        }
        if let Some(ref doc) = r.symbol.doc_comment {
            let short_doc: String = doc.chars().take(200).collect();
            output.push_str(&format!("   Doc: {}\n", short_doc));
        }
        output.push('\n');
    }

    Ok(output)
}

fn tool_get_symbol(args: &Value, store: &Store) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;

    let pattern = if name.contains('%') {
        name.to_string()
    } else {
        name.to_string()
    };

    let symbols = store.find_symbols_by_name(&pattern)?;

    if symbols.is_empty() {
        let symbols = store.find_symbols_by_name(&format!("%{}%", name))?;
        if symbols.is_empty() {
            return Ok(format!("No symbol found matching '{}'", name));
        }
        return format_symbols(&symbols, store);
    }

    format_symbols(&symbols, store)
}

fn tool_get_callers(args: &Value, store: &Store) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;
    let depth = args
        .get("depth")
        .and_then(|d| d.as_u64())
        .unwrap_or(2) as usize;

    let analyzer = GraphAnalyzer::new(store);
    let graph = analyzer.get_callers(name, depth)?;

    if graph.nodes.len() <= 1 {
        return Ok(format!("No callers found for '{}'", name));
    }

    let mut output = format!(
        "Callers of '{}' (depth={}):\n\n",
        name, depth
    );
    output.push_str(&format!("Root: {} ({})\n\n", graph.root.qualified_name, graph.root.kind.as_str()));
    output.push_str("Callers:\n");
    for node in &graph.nodes[1..] {
        output.push_str(&format!(
            "  - {} ({}) at {}:{}\n",
            node.qualified_name, node.kind.as_str(), node.file_path, node.line
        ));
    }

    output.push_str(&format!("\n{} edges in call graph\n", graph.edges.len()));

    Ok(output)
}

fn tool_get_callees(args: &Value, store: &Store) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;
    let depth = args
        .get("depth")
        .and_then(|d| d.as_u64())
        .unwrap_or(2) as usize;

    let analyzer = GraphAnalyzer::new(store);
    let graph = analyzer.get_call_graph(name, depth)?;

    if graph.nodes.len() <= 1 {
        return Ok(format!("No callees found for '{}'", name));
    }

    let mut output = format!(
        "Functions called by '{}' (depth={}):\n\n",
        name, depth
    );
    output.push_str("Callees:\n");
    for node in &graph.nodes[1..] {
        output.push_str(&format!(
            "  - {} ({}) at {}:{}\n",
            node.qualified_name, node.kind.as_str(), node.file_path, node.line
        ));
    }

    output.push_str(&format!("\n{} edges in call graph\n", graph.edges.len()));

    Ok(output)
}

fn tool_get_inheritance(args: &Value, store: &Store) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;

    let analyzer = GraphAnalyzer::new(store);
    let graph = analyzer.get_inheritance_tree(name)?;

    let mut output = format!("Inheritance hierarchy for '{}':\n\n", name);

    let root_id = graph.root.symbol_id;
    let mut parents = Vec::new();
    let mut children = Vec::new();

    for edge in &graph.edges {
        if edge.source_id == root_id {
            if let Some(node) = graph.nodes.iter().find(|n| n.symbol_id == edge.target_id) {
                parents.push(node);
            }
        } else {
            if let Some(node) = graph.nodes.iter().find(|n| n.symbol_id == edge.source_id) {
                children.push(node);
            }
        }
    }

    if !parents.is_empty() {
        output.push_str("Extends / Implements:\n");
        for p in &parents {
            output.push_str(&format!(
                "  ↑ {} ({}) at {}:{}\n",
                p.qualified_name, p.kind.as_str(), p.file_path, p.line
            ));
        }
        output.push('\n');
    }

    output.push_str(&format!(
        "→ {} ({}) at {}:{}\n\n",
        graph.root.qualified_name,
        graph.root.kind.as_str(),
        graph.root.file_path,
        graph.root.line,
    ));

    if !children.is_empty() {
        output.push_str("Subclasses / Implementors:\n");
        for c in &children {
            output.push_str(&format!(
                "  ↓ {} ({}) at {}:{}\n",
                c.qualified_name, c.kind.as_str(), c.file_path, c.line
            ));
        }
    }

    if parents.is_empty() && children.is_empty() {
        output.push_str("No inheritance relationships found.\n");
    }

    Ok(output)
}

fn tool_find_implementations(args: &Value, store: &Store) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;

    let analyzer = GraphAnalyzer::new(store);
    let impls = analyzer.find_implementations(name)?;

    if impls.is_empty() {
        return Ok(format!("No implementations found for '{}'", name));
    }

    let mut output = format!(
        "Implementations of '{}' ({} found):\n\n",
        name,
        impls.len()
    );
    for node in &impls {
        output.push_str(&format!(
            "  - {} ({}) at {}:{}\n",
            node.qualified_name, node.kind.as_str(), node.file_path, node.line
        ));
    }

    Ok(output)
}

fn tool_get_file_symbols(args: &Value, store: &Store) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

    let files = store.list_files()?;
    let matching_file = files
        .iter()
        .find(|f| f.relative_path == path || f.relative_path.ends_with(path));

    let file = match matching_file {
        Some(f) => f,
        None => return Ok(format!("File '{}' not found in index", path)),
    };

    let symbols = store.get_file_symbols(file.id)?;

    if symbols.is_empty() {
        return Ok(format!("No symbols found in '{}'", path));
    }

    let mut output = format!(
        "Symbols in '{}' ({} found):\n\n",
        file.relative_path,
        symbols.len()
    );
    for sym in &symbols {
        output.push_str(&format!(
            "  L{}-{}: {} {} ({})\n",
            sym.start_line,
            sym.end_line,
            sym.kind.as_str(),
            sym.name,
            sym.qualified_name,
        ));
        if let Some(ref sig) = sym.signature {
            output.push_str(&format!("         {}\n", sig));
        }
    }

    Ok(output)
}

fn tool_get_project_structure(store: &Store) -> Result<String> {
    let analyzer = GraphAnalyzer::new(store);
    let structure = analyzer.get_project_structure()?;

    let mut output = String::from("Project Structure:\n\n");

    output.push_str(&format!("Files:      {}\n", structure.stats.file_count));
    output.push_str(&format!("Symbols:    {}\n", structure.stats.symbol_count));
    output.push_str(&format!("Relations:  {}\n", structure.stats.relation_count));
    output.push_str(&format!("Embeddings: {}\n\n", structure.stats.embedding_count));

    output.push_str("Languages:\n");
    let mut langs: Vec<_> = structure.languages.iter().collect();
    langs.sort_by(|a, b| b.1.cmp(a.1));
    for (lang, count) in &langs {
        output.push_str(&format!("  {}: {} files\n", lang, count));
    }

    output.push_str("\nSymbol breakdown:\n");
    let mut kinds: Vec<_> = structure.symbol_kinds.iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(a.1));
    for (kind, count) in &kinds {
        output.push_str(&format!("  {}: {}\n", kind, count));
    }

    output.push_str("\nDirectories:\n");
    let mut dirs: Vec<_> = structure.directories.iter().collect();
    dirs.sort_by_key(|(dir, _)| (*dir).clone());
    for (dir, files) in &dirs {
        output.push_str(&format!("  {}/ ({} files)\n", dir, files.len()));
    }

    Ok(output)
}

fn tool_get_references(args: &Value, store: &Store) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'name' argument"))?;

    let symbols = store.find_symbols_by_name(name)?;
    if symbols.is_empty() {
        let symbols = store.find_symbols_by_name(&format!("%{}%", name))?;
        if symbols.is_empty() {
            return Ok(format!("No symbol found matching '{}'", name));
        }
    }

    let mut output = format!("References to '{}':\n\n", name);
    let mut total_refs = 0;

    for sym in &symbols {
        let incoming = store.get_incoming_relations(sym.id)?;
        if incoming.is_empty() {
            continue;
        }

        output.push_str(&format!(
            "{}  ({}):\n",
            sym.qualified_name,
            sym.kind.as_str()
        ));

        for rel in &incoming {
            if let Ok(Some(source_sym)) = store.get_symbol(rel.source_symbol_id) {
                let file_path = store
                    .get_file_path(rel.file_id)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                output.push_str(&format!(
                    "  {} by {} ({}) at {}:{}\n",
                    rel.kind.as_str(),
                    source_sym.name,
                    source_sym.kind.as_str(),
                    file_path,
                    rel.line,
                ));
                total_refs += 1;
            }
        }
        output.push('\n');
    }

    if total_refs == 0 {
        output.push_str("No references found.\n");
    } else {
        output.push_str(&format!("Total: {} references\n", total_refs));
    }

    Ok(output)
}

fn format_symbols(symbols: &[Symbol], store: &Store) -> Result<String> {
    let mut output = format!("Found {} symbol(s):\n\n", symbols.len());

    for sym in symbols.iter().take(20) {
        let file_path = store
            .get_file_path(sym.file_id)
            .ok()
            .flatten()
            .unwrap_or_default();

        output.push_str(&format!(
            "**{}** ({})\n  File: {}:{}-{}\n",
            sym.qualified_name,
            sym.kind.as_str(),
            file_path,
            sym.start_line,
            sym.end_line,
        ));

        if let Some(ref sig) = sym.signature {
            output.push_str(&format!("  Signature: {}\n", sig));
        }
        if let Some(ref doc) = sym.doc_comment {
            let short_doc: String = doc.chars().take(300).collect();
            output.push_str(&format!("  Doc: {}\n", short_doc));
        }
        if let Some(ref snippet) = sym.body_snippet {
            let short_snippet: String = snippet.chars().take(200).collect();
            output.push_str(&format!("  Body:\n    {}\n", short_snippet.replace('\n', "\n    ")));
        }
        output.push('\n');
    }

    if symbols.len() > 20 {
        output.push_str(&format!("... and {} more\n", symbols.len() - 20));
    }

    Ok(output)
}

