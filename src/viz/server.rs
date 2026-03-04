use anyhow::Result;
use std::io::{Read, Write};
use std::net::TcpListener;

use crate::config::Config;
use crate::graph::GraphAnalyzer;
use crate::store::Store;

pub fn serve(config: &Config, port: u16) -> Result<()> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    eprintln!("  Dashboard: http://localhost:{}", port);
    eprintln!("  Press Ctrl+C to stop");

    let db_path = config.db_path();

    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);

        let first_line = request.lines().next().unwrap_or("");
        let path = first_line.split_whitespace().nth(1).unwrap_or("/");

        let (status, content_type, body) = route(path, &db_path);

        let response = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
            status, content_type, body.len()
        );

        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body.as_bytes());
    }

    Ok(())
}

fn route(path: &str, db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match path {
        "/" => ("200 OK", "text/html; charset=utf-8", dashboard_html()),
        "/api/stats" => api_stats(db_path),
        "/api/structure" => api_structure(db_path),
        "/api/dependencies" => api_dependencies(db_path),
        "/api/file-counts" => api_file_counts(db_path),
        p if p.starts_with("/api/callgraph/") => {
            let name = &p["/api/callgraph/".len()..];
            let name = urlencoding_decode(name);
            api_callgraph(db_path, &name)
        }
        p if p.starts_with("/api/search?q=") => {
            let query = &p["/api/search?q=".len()..];
            let query = urlencoding_decode(query);
            api_search(db_path, &query)
        }
        p if p.starts_with("/api/file-symbols/") => {
            let file_path = &p["/api/file-symbols/".len()..];
            let file_path = urlencoding_decode(file_path);
            api_file_symbols(db_path, &file_path)
        }
        _ => ("404 Not Found", "text/plain", "Not found".into()),
    }
}

fn urlencoding_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h = chars.next().unwrap_or(b'0');
            let l = chars.next().unwrap_or(b'0');
            let hex = format!("{}{}", h as char, l as char);
            if let Ok(val) = u8::from_str_radix(&hex, 16) {
                result.push(val as char);
            }
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn api_stats(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path).and_then(|s| s.get_stats()) {
        Ok(stats) => {
            let json = serde_json::json!({
                "files": stats.file_count,
                "symbols": stats.symbol_count,
                "relations": stats.relation_count,
                "embeddings": stats.embedding_count,
                "languages": stats.languages,
            });
            ("200 OK", "application/json", json.to_string())
        }
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn api_structure(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            let analyzer = GraphAnalyzer::new(&store);
            match analyzer.get_project_structure() {
                Ok(structure) => {
                    let mut dirs_json = Vec::new();
                    for (dir, file_paths) in &structure.directories {
                        let mut dir_symbols = Vec::new();
                        for file_path in file_paths {
                            if let Ok(syms) = store.get_file_symbols_by_path(file_path) {
                                for s in syms {
                                    dir_symbols.push(serde_json::json!({
                                        "id": s.id,
                                        "name": s.name,
                                        "qualifiedName": s.qualified_name,
                                        "kind": s.kind.as_str(),
                                        "filePath": file_path,
                                        "line": s.start_line,
                                    }));
                                }
                            }
                        }
                        dirs_json.push(serde_json::json!({
                            "path": dir,
                            "symbols": dir_symbols,
                        }));
                    }
                    dirs_json.sort_by(|a, b| {
                        a["path"]
                            .as_str()
                            .unwrap_or("")
                            .cmp(b["path"].as_str().unwrap_or(""))
                    });
                    let json = serde_json::json!({ "directories": dirs_json });
                    ("200 OK", "application/json", json.to_string())
                }
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string(),
                ),
            }
        }
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn api_callgraph(db_path: &std::path::Path, name: &str) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            let analyzer = GraphAnalyzer::new(&store);
            match analyzer.get_call_graph(name, 4) {
                Ok(graph) => {
                    let nodes: Vec<serde_json::Value> = graph
                        .nodes
                        .iter()
                        .map(|n| {
                            serde_json::json!({
                                "id": n.qualified_name,
                                "name": n.name,
                                "kind": n.kind.as_str(),
                                "file": n.file_path,
                                "line": n.line,
                                "group": kind_to_group(n.kind.as_str()),
                            })
                        })
                        .collect();

                    let id_to_name: std::collections::HashMap<i64, &str> = graph
                        .nodes
                        .iter()
                        .map(|n| (n.symbol_id, n.qualified_name.as_str()))
                        .collect();
                    let links: Vec<serde_json::Value> = graph
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

                    let json = serde_json::json!({ "nodes": nodes, "links": links });
                    ("200 OK", "application/json", json.to_string())
                }
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string(),
                ),
            }
        }
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn api_search(db_path: &std::path::Path, _query: &str) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => match store.text_search(_query, 20) {
            Ok(results) => {
                let json: Vec<serde_json::Value> = results
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id": s.id,
                            "name": s.name,
                            "qualifiedName": s.qualified_name,
                            "kind": s.kind.as_str(),
                            "line": s.start_line,
                            "signature": s.signature,
                        })
                    })
                    .collect();
                (
                    "200 OK",
                    "application/json",
                    serde_json::json!(json).to_string(),
                )
            }
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                serde_json::json!({"error": e.to_string()}).to_string(),
            ),
        },
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn api_file_symbols(
    db_path: &std::path::Path,
    file_path: &str,
) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => match store.get_file_symbols_by_path(file_path) {
            Ok(symbols) => {
                let json: Vec<serde_json::Value> = symbols
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id": s.id,
                            "name": s.name,
                            "qualifiedName": s.qualified_name,
                            "kind": s.kind.as_str(),
                            "line": s.start_line,
                            "signature": s.signature,
                        })
                    })
                    .collect();
                (
                    "200 OK",
                    "application/json",
                    serde_json::json!(json).to_string(),
                )
            }
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                serde_json::json!({"error": e.to_string()}).to_string(),
            ),
        },
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn api_dependencies(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => match store.get_file_dependencies() {
            Ok(deps) => {
                let mut file_set = std::collections::HashSet::new();
                for (src, tgt, _) in &deps {
                    file_set.insert(src.clone());
                    file_set.insert(tgt.clone());
                }
                let nodes: Vec<serde_json::Value> = file_set
                    .iter()
                    .map(|f| {
                        let dir = std::path::Path::new(f)
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| ".".to_string());
                        serde_json::json!({ "id": f, "dir": dir })
                    })
                    .collect();

                let mut edge_counts: std::collections::HashMap<
                    (String, String),
                    (usize, Vec<String>),
                > = std::collections::HashMap::new();
                for (src, tgt, kind) in &deps {
                    let entry = edge_counts
                        .entry((src.clone(), tgt.clone()))
                        .or_insert((0, Vec::new()));
                    entry.0 += 1;
                    if !entry.1.contains(kind) {
                        entry.1.push(kind.clone());
                    }
                }
                let links: Vec<serde_json::Value> = edge_counts
                    .iter()
                    .map(|((src, tgt), (count, kinds))| {
                        serde_json::json!({
                            "source": src,
                            "target": tgt,
                            "weight": count,
                            "kinds": kinds,
                        })
                    })
                    .collect();

                (
                    "200 OK",
                    "application/json",
                    serde_json::json!({ "nodes": nodes, "links": links }).to_string(),
                )
            }
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                serde_json::json!({"error": e.to_string()}).to_string(),
            ),
        },
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn api_file_counts(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => match store.get_file_symbol_counts() {
            Ok(counts) => {
                let mut map: std::collections::HashMap<
                    String,
                    serde_json::Map<String, serde_json::Value>,
                > = std::collections::HashMap::new();
                for (file, kind, count) in &counts {
                    map.entry(file.clone())
                        .or_default()
                        .insert(kind.clone(), serde_json::json!(count));
                }
                let json: Vec<serde_json::Value> = map
                    .iter()
                    .map(|(file, kinds)| {
                        let total: i64 = kinds.values().filter_map(|v| v.as_i64()).sum();
                        let dir = std::path::Path::new(file)
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| ".".to_string());
                        serde_json::json!({
                            "file": file,
                            "dir": dir,
                            "total": total,
                            "kinds": kinds,
                        })
                    })
                    .collect();
                (
                    "200 OK",
                    "application/json",
                    serde_json::json!(json).to_string(),
                )
            }
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                serde_json::json!({"error": e.to_string()}).to_string(),
            ),
        },
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

fn kind_to_group(kind: &str) -> u8 {
    match kind {
        "class" | "struct" => 1,
        "function" => 2,
        "method" => 3,
        "trait" | "interface" => 4,
        _ => 5,
    }
}

fn dashboard_html() -> String {
    include_str!("dashboard.html").to_string()
}
