use anyhow::Result;
use std::io::{Read, Write};
use std::net::TcpListener;

use crate::config::Config;
use crate::graph::GraphAnalyzer;
use crate::store::Store;

/// Serve the interactive D3 visualization dashboard.
/// Minimal HTTP server — zero external dependencies.
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
        let path = first_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/");

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
        Err(e) => ("500 Internal Server Error", "application/json", 
            serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

fn api_structure(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            let analyzer = GraphAnalyzer::new(&store);
            match analyzer.get_project_structure() {
                Ok(structure) => {
                    // directories: HashMap<String, Vec<String>> (dir → file paths)
                    // For each directory, load symbols from those files
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
                        a["path"].as_str().unwrap_or("").cmp(b["path"].as_str().unwrap_or(""))
                    });
                    let json = serde_json::json!({ "directories": dirs_json });
                    ("200 OK", "application/json", json.to_string())
                }
                Err(e) => ("500 Internal Server Error", "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        Err(e) => ("500 Internal Server Error", "application/json",
            serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

fn api_callgraph(db_path: &std::path::Path, name: &str) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            let analyzer = GraphAnalyzer::new(&store);
            match analyzer.get_call_graph(name, 4) {
                Ok(graph) => {
                    let nodes: Vec<serde_json::Value> = graph.nodes.iter().map(|n| {
                        serde_json::json!({
                            "id": n.qualified_name,
                            "name": n.name,
                            "kind": n.kind.as_str(),
                            "file": n.file_path,
                            "line": n.line,
                            "group": kind_to_group(n.kind.as_str()),
                        })
                    }).collect();

                    // Map edge source/target IDs to qualified names for D3
                    let id_to_name: std::collections::HashMap<i64, &str> = graph.nodes.iter()
                        .map(|n| (n.symbol_id, n.qualified_name.as_str())).collect();
                    let links: Vec<serde_json::Value> = graph.edges.iter().filter_map(|e| {
                        let src = id_to_name.get(&e.source_id)?;
                        let tgt = id_to_name.get(&e.target_id)?;
                        Some(serde_json::json!({
                            "source": src,
                            "target": tgt,
                            "kind": format!("{:?}", e.kind),
                        }))
                    }).collect();

                    let json = serde_json::json!({ "nodes": nodes, "links": links });
                    ("200 OK", "application/json", json.to_string())
                }
                Err(e) => ("500 Internal Server Error", "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        Err(e) => ("500 Internal Server Error", "application/json",
            serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

fn api_search(db_path: &std::path::Path, _query: &str) -> (&'static str, &'static str, String) {
    // Semantic search requires the embedder which is heavy to spin up per request.
    // For the viz, we use text search from the store directly.
    match Store::open(db_path) {
        Ok(store) => {
            match store.text_search(_query, 20) {
                Ok(results) => {
                    let json: Vec<serde_json::Value> = results.iter().map(|s| {
                        serde_json::json!({
                            "id": s.id,
                            "name": s.name,
                            "qualifiedName": s.qualified_name,
                            "kind": s.kind.as_str(),
                            "line": s.start_line,
                            "signature": s.signature,
                        })
                    }).collect();
                    ("200 OK", "application/json", serde_json::json!(json).to_string())
                }
                Err(e) => ("500 Internal Server Error", "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        Err(e) => ("500 Internal Server Error", "application/json",
            serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

fn api_file_symbols(db_path: &std::path::Path, file_path: &str) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            match store.get_file_symbols_by_path(file_path) {
                Ok(symbols) => {
                    let json: Vec<serde_json::Value> = symbols.iter().map(|s| {
                        serde_json::json!({
                            "id": s.id,
                            "name": s.name,
                            "qualifiedName": s.qualified_name,
                            "kind": s.kind.as_str(),
                            "line": s.start_line,
                            "signature": s.signature,
                        })
                    }).collect();
                    ("200 OK", "application/json", serde_json::json!(json).to_string())
                }
                Err(e) => ("500 Internal Server Error", "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        Err(e) => ("500 Internal Server Error", "application/json",
            serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

fn api_dependencies(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            match store.get_file_dependencies() {
                Ok(deps) => {
                    // Collect unique files as nodes
                    let mut file_set = std::collections::HashSet::new();
                    for (src, tgt, _) in &deps {
                        file_set.insert(src.clone());
                        file_set.insert(tgt.clone());
                    }
                    let nodes: Vec<serde_json::Value> = file_set.iter().map(|f| {
                        let dir = std::path::Path::new(f)
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| ".".to_string());
                        serde_json::json!({ "id": f, "dir": dir })
                    }).collect();

                    // Aggregate edges: count how many relations between each file pair
                    let mut edge_counts: std::collections::HashMap<(String, String), (usize, Vec<String>)> =
                        std::collections::HashMap::new();
                    for (src, tgt, kind) in &deps {
                        let entry = edge_counts.entry((src.clone(), tgt.clone())).or_insert((0, Vec::new()));
                        entry.0 += 1;
                        if !entry.1.contains(kind) {
                            entry.1.push(kind.clone());
                        }
                    }
                    let links: Vec<serde_json::Value> = edge_counts.iter().map(|((src, tgt), (count, kinds))| {
                        serde_json::json!({
                            "source": src,
                            "target": tgt,
                            "weight": count,
                            "kinds": kinds,
                        })
                    }).collect();

                    ("200 OK", "application/json",
                        serde_json::json!({ "nodes": nodes, "links": links }).to_string())
                }
                Err(e) => ("500 Internal Server Error", "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        Err(e) => ("500 Internal Server Error", "application/json",
            serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

fn api_file_counts(db_path: &std::path::Path) -> (&'static str, &'static str, String) {
    match Store::open(db_path) {
        Ok(store) => {
            match store.get_file_symbol_counts() {
                Ok(counts) => {
                    // Group into { file: { kind: count, ... }, ... }
                    let mut map: std::collections::HashMap<String, serde_json::Map<String, serde_json::Value>> =
                        std::collections::HashMap::new();
                    for (file, kind, count) in &counts {
                        map.entry(file.clone())
                            .or_default()
                            .insert(kind.clone(), serde_json::json!(count));
                    }
                    let json: Vec<serde_json::Value> = map.iter().map(|(file, kinds)| {
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
                    }).collect();
                    ("200 OK", "application/json", serde_json::json!(json).to_string())
                }
                Err(e) => ("500 Internal Server Error", "application/json",
                    serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        Err(e) => ("500 Internal Server Error", "application/json",
            serde_json::json!({"error": e.to_string()}).to_string()),
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

// The complete D3 dashboard — embedded as a single HTML string.
// Zero external files. CDN for D3. Dark theme. Three views.
fn dashboard_html() -> String {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Plex — Code Intelligence Dashboard</title>
<script src="https://d3js.org/d3.v7.min.js"></script>
<style>
:root {
  --bg: #0d1117; --surface: #161b22; --surface2: #1c2129; --border: #30363d;
  --text: #e6edf3; --text-dim: #8b949e; --accent: #58a6ff;
  --green: #3fb950; --orange: #d29922; --red: #f85149; --purple: #bc8cff;
  --cyan: #39d2c0; --pink: #f778ba;
}
* { margin:0; padding:0; box-sizing:border-box; }
body { font-family: -apple-system, 'SF Pro Text', 'Segoe UI', sans-serif; background: var(--bg); color: var(--text); overflow: hidden; height: 100vh; }

/* Header */
.header { display:flex; align-items:center; justify-content:space-between; padding:10px 20px; border-bottom:1px solid var(--border); background: var(--surface); height: 48px; }
.header h1 { font-size:16px; font-weight:600; letter-spacing:-0.3px; }
.header h1 span { color:var(--accent); }
.stats-bar { display:flex; gap:20px; font-size:12px; color:var(--text-dim); }
.stats-bar .v { color:var(--accent); font-weight:600; }

/* Tabs */
.tabs { display:flex; background: var(--surface); border-bottom:1px solid var(--border); padding:0 20px; height: 38px; }
.tab { padding:8px 16px; font-size:13px; cursor:pointer; border-bottom:2px solid transparent; color:var(--text-dim); transition:all 0.15s; user-select:none; }
.tab:hover { color:var(--text); }
.tab.active { color:var(--accent); border-bottom-color:var(--accent); }

/* Layout */
.container { display:flex; height:calc(100vh - 86px); }
.sidebar { width:260px; border-right:1px solid var(--border); display:flex; flex-direction:column; flex-shrink:0; background: var(--surface); }
.sidebar-header { padding:10px 12px 6px; font-size:11px; font-weight:600; text-transform:uppercase; letter-spacing:0.5px; color:var(--text-dim); }
.sidebar-search { padding:0 10px 8px; }
.sidebar-search input { width:100%; padding:6px 10px; background:var(--bg); border:1px solid var(--border); border-radius:5px; color:var(--text); font-size:12px; outline:none; }
.sidebar-search input:focus { border-color:var(--accent); }
.sidebar-list { flex:1; overflow-y:auto; padding:0 6px 12px; }

/* Sidebar items */
.dir-group { margin-bottom: 2px; }
.dir-label { padding:5px 8px; font-size:11px; color:var(--text-dim); font-weight:600; cursor:pointer; border-radius:4px; display:flex; align-items:center; gap:4px; user-select:none; }
.dir-label:hover { background:var(--bg); }
.dir-label .arrow { font-size:9px; transition:transform 0.15s; display:inline-block; width:12px; }
.dir-label .arrow.open { transform:rotate(90deg); }
.dir-children { display:none; padding-left:8px; }
.dir-children.open { display:block; }
.file-item { padding:4px 8px; font-size:12px; border-radius:4px; cursor:pointer; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; display:flex; align-items:center; gap:6px; color:var(--text); }
.file-item:hover { background:var(--bg); }
.file-item.active { background:var(--accent); color:#fff; }
.file-item .count { font-size:10px; color:var(--text-dim); margin-left:auto; }
.file-item.active .count { color:rgba(255,255,255,0.7); }

/* Badges */
.badge { display:inline-block; padding:1px 5px; border-radius:3px; font-size:9px; font-weight:700; text-transform:uppercase; letter-spacing:0.3px; }
.b-class { background:#1f6feb22; color:var(--accent); }
.b-function { background:#3fb95022; color:var(--green); }
.b-method { background:#d2992222; color:var(--orange); }
.b-import { background:#bc8cff22; color:var(--purple); }
.b-other { background:#8b949e22; color:var(--text-dim); }

/* Main view */
.main-view { flex:1; position:relative; overflow:hidden; background: var(--bg); }
.view { display:none; width:100%; height:100%; position:relative; }
.view.active { display:flex; flex-direction:column; }
.view-canvas { flex:1; position:relative; overflow:hidden; }

/* Controls bar at top of views */
.view-controls { display:flex; align-items:center; gap:10px; padding:10px 16px; background:var(--surface2); border-bottom:1px solid var(--border); flex-shrink:0; }
.view-controls input, .view-controls select { padding:5px 10px; background:var(--bg); border:1px solid var(--border); border-radius:5px; color:var(--text); font-size:12px; outline:none; }
.view-controls input:focus { border-color:var(--accent); }
.view-controls button { padding:5px 14px; background:var(--accent); border:none; border-radius:5px; color:#fff; font-weight:600; cursor:pointer; font-size:12px; }
.view-controls button:hover { opacity:0.9; }
.view-controls button.secondary { background:var(--surface); border:1px solid var(--border); color:var(--text); }
.view-controls label { font-size:12px; color:var(--text-dim); display:flex; align-items:center; gap:4px; }
.view-controls .sep { width:1px; height:20px; background:var(--border); }

/* Detail panel (slides from right) */
.detail-panel { position:absolute; right:0; top:0; bottom:0; width:320px; background:var(--surface); border-left:1px solid var(--border); z-index:20; overflow-y:auto; padding:16px; transform:translateX(100%); transition:transform 0.2s ease; }
.detail-panel.open { transform:translateX(0); }
.detail-title { font-size:14px; font-weight:600; margin-bottom:12px; display:flex; justify-content:space-between; align-items:center; }
.detail-close { cursor:pointer; color:var(--text-dim); font-size:18px; }
.detail-section { margin-bottom:16px; }
.detail-section h3 { font-size:11px; text-transform:uppercase; letter-spacing:0.5px; color:var(--text-dim); margin-bottom:6px; }
.detail-row { display:flex; justify-content:space-between; font-size:12px; padding:3px 0; }
.detail-row .lbl { color:var(--text-dim); }
.detail-sym { padding:4px 8px; font-size:12px; border-radius:4px; cursor:pointer; display:flex; align-items:center; gap:6px; }
.detail-sym:hover { background:var(--bg); }

/* Tooltip */
.tooltip { position:fixed; padding:8px 12px; background:var(--surface); border:1px solid var(--border); border-radius:6px; font-size:12px; pointer-events:none; z-index:200; max-width:350px; box-shadow:0 4px 12px rgba(0,0,0,0.4); }
.tooltip .tt-n { font-weight:600; color:var(--accent); }
.tooltip .tt-k { color:var(--text-dim); margin-left:6px; }
.tooltip .tt-f { color:var(--text-dim); font-size:11px; margin-top:3px; }

/* Legend */
.legend { position:absolute; bottom:12px; left:12px; background:var(--surface); border:1px solid var(--border); border-radius:6px; padding:8px 12px; font-size:11px; z-index:10; }
.legend-item { display:flex; align-items:center; gap:6px; padding:2px 0; }
.legend-dot { width:10px; height:10px; border-radius:50%; }

/* Breadcrumb */
.breadcrumb { display:flex; align-items:center; gap:4px; font-size:12px; color:var(--text-dim); }
.breadcrumb span { cursor:pointer; }
.breadcrumb span:hover { color:var(--accent); }
.breadcrumb .bc-sep { color:var(--border); }

svg { width:100%; height:100%; display:block; }
</style>
</head>
<body>

<div class="header">
  <h1>⚡ <span>Plex</span></h1>
  <div class="stats-bar" id="statsBar"></div>
</div>

<div class="tabs">
  <div class="tab active" data-view="arch">Architecture</div>
  <div class="tab" data-view="deps">Dependencies</div>
  <div class="tab" data-view="calls">Call Explorer</div>
  <div class="tab" data-view="map">Code Map</div>
</div>

<div class="container">
  <div class="sidebar">
    <div class="sidebar-header">Explorer</div>
    <div class="sidebar-search"><input type="text" id="sidebarFilter" placeholder="Filter files..."></div>
    <div class="sidebar-list" id="sidebarList"></div>
  </div>

  <div class="main-view">
    <!-- ARCHITECTURE VIEW -->
    <div class="view active" id="view-arch">
      <div class="view-controls">
        <div class="breadcrumb" id="archBreadcrumb"><span>project</span></div>
        <div class="sep"></div>
        <label>Color: <select id="archColor"><option value="kind">By kind</option><option value="dir">By module</option></select></label>
      </div>
      <div class="view-canvas"><svg id="archSvg"></svg></div>
      <div class="legend" id="archLegend"></div>
    </div>

    <!-- DEPENDENCIES VIEW -->
    <div class="view" id="view-deps">
      <div class="view-controls">
        <input type="text" id="depsFilter" placeholder="Highlight file..." style="width:200px">
        <div class="sep"></div>
        <label>Group by module <input type="checkbox" id="depsGroup" checked></label>
        <div class="sep"></div>
        <label>Min weight: <input type="range" id="depsWeight" min="1" max="10" value="1" style="width:80px"><span id="depsWeightVal">1</span></label>
      </div>
      <div class="view-canvas" style="position:relative"><svg id="depsSvg"></svg></div>
      <div class="detail-panel" id="depsDetail"></div>
    </div>

    <!-- CALL EXPLORER VIEW -->
    <div class="view" id="view-calls">
      <div class="view-controls">
        <input type="text" id="callInput" placeholder="Enter function name..." style="width:220px">
        <button onclick="traceCall()">Trace</button>
        <div class="sep"></div>
        <label>Direction:
          <select id="callDir"><option value="callees">Callees (what it calls)</option><option value="callers">Callers (who calls it)</option></select>
        </label>
        <div class="sep"></div>
        <label>Depth: <input type="range" id="callDepth" min="1" max="8" value="3" style="width:80px"><span id="callDepthVal">3</span></label>
      </div>
      <div class="view-canvas"><svg id="callSvg"></svg></div>
      <div class="detail-panel" id="callDetail"></div>
    </div>

    <!-- CODE MAP VIEW -->
    <div class="view" id="view-map">
      <div class="view-controls">
        <label>Size by: <select id="mapSize"><option value="total">Symbol count</option><option value="class">Classes</option><option value="function">Functions</option><option value="method">Methods</option></select></label>
        <div class="sep"></div>
        <label>Color by: <select id="mapColor"><option value="dominant">Dominant kind</option><option value="dir">Module</option></select></label>
      </div>
      <div class="view-canvas"><svg id="mapSvg"></svg></div>
      <div class="detail-panel" id="mapDetail"></div>
    </div>
  </div>
</div>

<div class="tooltip" id="tooltip" style="display:none;"></div>

<script>
const API = '';
const tt = document.getElementById('tooltip');
const KindColors = {class:'#58a6ff',struct:'#58a6ff',function:'#3fb950',method:'#d29922',import:'#bc8cff',variable:'#8b949e',constant:'#8b949e',trait:'#39d2c0',interface:'#39d2c0',enum:'#f778ba',field:'#8b949e',constructor:'#f85149',module:'#d29922',type:'#39d2c0'};
const DirColors = d3.scaleOrdinal(d3.schemeTableau10);

function showTip(evt, html){ tt.style.display='block'; tt.innerHTML=html; tt.style.left=(evt.clientX+14)+'px'; tt.style.top=(evt.clientY-10)+'px'; }
function moveTip(evt){ tt.style.left=(evt.clientX+14)+'px'; tt.style.top=(evt.clientY-10)+'px'; }
function hideTip(){ tt.style.display='none'; }
function badgeFor(k){ const cls = k==='class'||k==='struct'?'b-class':k==='function'?'b-function':k==='method'?'b-method':k==='import'?'b-import':'b-other'; return `<span class="badge ${cls}">${k}</span>`; }

// ── Data cache ──
let C = { stats:null, structure:null, deps:null, fileCounts:null };

async function loadAll(){
  const [stats,structure,deps,fileCounts] = await Promise.all([
    fetch(API+'/api/stats').then(r=>r.json()),
    fetch(API+'/api/structure').then(r=>r.json()),
    fetch(API+'/api/dependencies').then(r=>r.json()),
    fetch(API+'/api/file-counts').then(r=>r.json()),
  ]);
  C = {stats,structure,deps,fileCounts};
  renderStats();
  renderSidebar();
  renderArch();
}

function renderStats(){
  const s=C.stats;
  document.getElementById('statsBar').innerHTML=`<div><span class="v">${s.files}</span> files</div><div><span class="v">${s.symbols}</span> symbols</div><div><span class="v">${s.relations}</span> relations</div><div><span class="v">${s.languages.join(', ')}</span></div>`;
}

// ── Sidebar: collapsible file tree ──
function renderSidebar(){
  const dirs = C.structure.directories||[];
  // build counts per file
  const fc = {};
  (C.fileCounts||[]).forEach(f=>{ fc[f.file]=f.total; });

  let html='';
  for(const dir of dirs.sort((a,b)=>a.path.localeCompare(b.path))){
    const files = [...new Set(dir.symbols.map(s=>s.filePath))].sort();
    html+=`<div class="dir-group"><div class="dir-label" onclick="toggleDir(this)"><span class="arrow">▶</span> 📁 ${dir.path} <span class="count" style="margin-left:auto;font-size:10px;color:var(--text-dim)">${files.length}</span></div><div class="dir-children">`;
    for(const f of files){
      const name = f.split('/').pop();
      const count = fc[f]||0;
      html+=`<div class="file-item" data-file="${f}" onclick="selectFile('${f}')">📄 ${name} <span class="count">${count}</span></div>`;
    }
    html+=`</div></div>`;
  }
  document.getElementById('sidebarList').innerHTML=html;
}
function toggleDir(el){ const c=el.nextElementSibling; c.classList.toggle('open'); el.querySelector('.arrow').classList.toggle('open'); }
function selectFile(f){
  document.querySelectorAll('.file-item').forEach(i=>i.classList.remove('active'));
  document.querySelectorAll(`.file-item[data-file="${f}"]`).forEach(i=>i.classList.add('active'));
  // Highlight in current view
  if(activeView==='deps') highlightDepFile(f);
  if(activeView==='map') highlightMapFile(f);
}

document.getElementById('sidebarFilter').addEventListener('input',e=>{
  const q=e.target.value.toLowerCase();
  document.querySelectorAll('.dir-group').forEach(g=>{
    const files=g.querySelectorAll('.file-item');
    let any=false;
    files.forEach(f=>{ const m=f.textContent.toLowerCase().includes(q); f.style.display=m?'':'none'; if(m)any=true; });
    g.style.display=any?'':'none';
    if(q && any){ g.querySelector('.dir-children').classList.add('open'); g.querySelector('.arrow').classList.add('open'); }
  });
});

// ── Tabs ──
let activeView = 'arch';
document.querySelectorAll('.tab').forEach(tab=>{
  tab.addEventListener('click',()=>{
    document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
    document.querySelectorAll('.view').forEach(v=>v.classList.remove('active'));
    tab.classList.add('active');
    const v = tab.dataset.view;
    document.getElementById('view-'+v).classList.add('active');
    activeView = v;
    if(v==='arch') renderArch();
    if(v==='deps') renderDeps();
    if(v==='calls') {}// user triggers manually
    if(v==='map') renderMap();
  });
});

// ════════════════════════════════════════════════════════════════════════════
// VIEW 1: ARCHITECTURE — Zoomable Sunburst
// ════════════════════════════════════════════════════════════════════════════
function renderArch(){
  const svg = d3.select('#archSvg');
  svg.selectAll('*').remove();
  const W = svg.node().clientWidth, H = svg.node().clientHeight;
  const radius = Math.min(W,H)/2 - 20;
  const colorBy = document.getElementById('archColor').value;

  // Build hierarchy: root → dirs → files → symbols(grouped by kind)
  const root = {name:'project', children:[]};
  const dirs = C.structure.directories||[];
  for(const dir of dirs){
    const fileMap = {};
    for(const sym of dir.symbols){
      if(!fileMap[sym.filePath]) fileMap[sym.filePath]={name:sym.filePath.split('/').pop(),fullPath:sym.filePath,children:{}};
      const k=sym.kind;
      if(!fileMap[sym.filePath].children[k]) fileMap[sym.filePath].children[k]={name:k,kind:k,value:0};
      fileMap[sym.filePath].children[k].value++;
    }
    const dirNode = {name:dir.path, children:[]};
    for(const fp in fileMap){
      const f = fileMap[fp];
      dirNode.children.push({name:f.name, fullPath:f.fullPath, children:Object.values(f.children)});
    }
    if(dirNode.children.length) root.children.push(dirNode);
  }

  const hier = d3.hierarchy(root).sum(d=>d.value||0).sort((a,b)=>b.value-a.value);
  d3.partition().size([2*Math.PI, radius])(hier);

  const g = svg.append('g').attr('transform',`translate(${W/2},${H/2})`);

  const arc = d3.arc()
    .startAngle(d=>d.x0)
    .endAngle(d=>d.x1)
    .padAngle(d=>Math.min((d.x1-d.x0)/2, 0.005))
    .padRadius(radius/2)
    .innerRadius(d=>d.y0)
    .outerRadius(d=>d.y1-1);

  function fillColor(d){
    if(d.depth===0) return 'var(--surface)';
    if(colorBy==='kind'){
      if(d.data.kind) return KindColors[d.data.kind]||'#8b949e';
      if(d.depth===1) return '#21262d';
      return '#21262d';
    } else {
      const ancestor = d.depth>=1 ? d.ancestors().find(a=>a.depth===1) : d;
      return ancestor ? DirColors(ancestor.data.name) : '#21262d';
    }
  }

  const path = g.selectAll('path')
    .data(hier.descendants().filter(d=>d.depth))
    .enter().append('path')
    .attr('d', arc)
    .attr('fill', fillColor)
    .attr('fill-opacity', d=> d.children ? 0.4 : 0.8)
    .attr('stroke', 'var(--bg)')
    .attr('stroke-width', 0.5)
    .style('cursor','pointer')
    .on('mouseover',(evt,d)=>{
      const name = d.data.name;
      const kind = d.data.kind||'';
      const val = d.value;
      let html = `<span class="tt-n">${name}</span>`;
      if(kind) html+=`<span class="tt-k">${kind}</span>`;
      html+=`<div class="tt-f">${val} symbol${val!==1?'s':''}</div>`;
      if(d.data.fullPath) html+=`<div class="tt-f">${d.data.fullPath}</div>`;
      showTip(evt,html);
    })
    .on('mousemove',moveTip)
    .on('mouseout',hideTip)
    .on('click',(evt,d)=>{
      // Zoom: click to focus
      const target = d === currentFocus ? hier : d;
      zoomSunburst(target);
    });

  // Labels for directories
  g.selectAll('text')
    .data(hier.descendants().filter(d=>d.depth===1 && (d.x1-d.x0)>0.15))
    .enter().append('text')
    .attr('transform',d=>{
      const angle = (d.x0+d.x1)/2 * 180/Math.PI - 90;
      const r = (d.y0+d.y1)/2;
      return `rotate(${angle}) translate(${r},0) rotate(${angle>90?180:0})`;
    })
    .attr('text-anchor','middle')
    .attr('dy','0.35em')
    .style('font-size','11px')
    .style('fill','var(--text)')
    .style('pointer-events','none')
    .text(d=>d.data.name.split('/').pop());

  let currentFocus = hier;
  function zoomSunburst(target){
    currentFocus = target;
    const t = svg.transition().duration(600);
    path.transition(t)
      .tween('data',d=>{
        const i = d3.interpolate({x0:d.x0,x1:d.x1}, {
          x0: Math.max(0, Math.min(1, (d.x0 - target.x0) / (target.x1 - target.x0))) * 2 * Math.PI,
          x1: Math.max(0, Math.min(1, (d.x1 - target.x0) / (target.x1 - target.x0))) * 2 * Math.PI,
        });
        return t=>{ d.x0=i(t).x0; d.x1=i(t).x1; };
      })
      .attrTween('d', d=>()=>arc(d));

    // Update breadcrumb
    const bc = document.getElementById('archBreadcrumb');
    const anc = target.ancestors().reverse();
    bc.innerHTML = anc.map((a,i)=>`${i?'<span class="bc-sep">›</span>':''}<span onclick="zoomSunburstByName('${a.data.name}')">${a.data.name}</span>`).join('');
  }
  window.zoomSunburstByName = function(name){
    const node = hier.descendants().find(d=>d.data.name===name);
    if(node) zoomSunburst(node);
  };

  // Legend
  const legend = document.getElementById('archLegend');
  if(colorBy==='kind'){
    legend.innerHTML = ['class','function','method','import','variable'].map(k=>`<div class="legend-item"><div class="legend-dot" style="background:${KindColors[k]}"></div>${k}</div>`).join('');
  } else {
    const dirNames = dirs.map(d=>d.path).slice(0,8);
    legend.innerHTML = dirNames.map(d=>`<div class="legend-item"><div class="legend-dot" style="background:${DirColors(d)}"></div>${d}</div>`).join('');
  }
}
document.getElementById('archColor').addEventListener('change',renderArch);

// ════════════════════════════════════════════════════════════════════════════
// VIEW 2: DEPENDENCIES — File-level force graph
// ════════════════════════════════════════════════════════════════════════════
let depSim = null;
function renderDeps(){
  const svg = d3.select('#depsSvg');
  svg.selectAll('*').remove();
  const W = svg.node().clientWidth, H = svg.node().clientHeight;
  const grouped = document.getElementById('depsGroup').checked;
  const minWeight = +document.getElementById('depsWeight').value;

  const data = C.deps;
  if(!data||!data.nodes||!data.links) return;

  // Filter by weight
  const links = data.links.filter(l=>l.weight>=minWeight);
  const linkedIds = new Set();
  links.forEach(l=>{ linkedIds.add(l.source.id||l.source); linkedIds.add(l.target.id||l.target); });
  const nodes = data.nodes.filter(n=>linkedIds.has(n.id));

  if(!nodes.length) return;

  // Clone for simulation
  const simNodes = nodes.map(n=>({...n}));
  const simLinks = links.map(l=>({source:l.source.id||l.source, target:l.target.id||l.target, weight:l.weight, kinds:l.kinds}));

  // Colors by directory
  const dirs = [...new Set(simNodes.map(n=>n.dir))];
  const dirColor = d3.scaleOrdinal(d3.schemeTableau10).domain(dirs);

  const g = svg.append('g');
  svg.call(d3.zoom().scaleExtent([0.1,5]).on('zoom',e=>g.attr('transform',e.transform)));

  // Arrow marker
  svg.append('defs').append('marker').attr('id','dep-arrow').attr('viewBox','0 -5 10 10')
    .attr('refX',12).attr('refY',0).attr('markerWidth',6).attr('markerHeight',6).attr('orient','auto')
    .append('path').attr('d','M0,-4L10,0L0,4').attr('fill','#30363d');

  if(depSim) depSim.stop();
  depSim = d3.forceSimulation(simNodes)
    .force('link', d3.forceLink(simLinks).id(d=>d.id).distance(d=>120/Math.sqrt(d.weight||1)))
    .force('charge', d3.forceManyBody().strength(-200))
    .force('center', d3.forceCenter(W/2, H/2))
    .force('collision', d3.forceCollide(20));

  if(grouped){
    // Group force toward cluster centers
    const dirCenters = {};
    dirs.forEach((d,i)=>{ const angle=2*Math.PI*i/dirs.length; dirCenters[d]={x:W/2+150*Math.cos(angle),y:H/2+150*Math.sin(angle)}; });
    depSim.force('x', d3.forceX(d=>dirCenters[d.dir]?.x||W/2).strength(0.1));
    depSim.force('y', d3.forceY(d=>dirCenters[d.dir]?.y||H/2).strength(0.1));
  }

  const link = g.append('g').selectAll('line').data(simLinks).enter().append('line')
    .attr('stroke','#30363d').attr('stroke-width',d=>Math.min(d.weight,5)*0.8)
    .attr('stroke-opacity',0.4).attr('marker-end','url(#dep-arrow)');

  const node = g.append('g').selectAll('g').data(simNodes).enter().append('g')
    .style('cursor','pointer')
    .call(d3.drag()
      .on('start',(e,d)=>{ if(!e.active)depSim.alphaTarget(0.3).restart(); d.fx=d.x; d.fy=d.y; })
      .on('drag',(e,d)=>{ d.fx=e.x; d.fy=e.y; })
      .on('end',(e,d)=>{ if(!e.active)depSim.alphaTarget(0); d.fx=null; d.fy=null; })
    );

  node.append('circle').attr('r',6).attr('fill',d=>dirColor(d.dir)).attr('stroke',d=>dirColor(d.dir)).attr('stroke-width',1.5).attr('fill-opacity',0.7);
  node.append('text').attr('dx',10).attr('dy','0.35em').style('font-size','10px').style('fill','var(--text-dim)').text(d=>d.id.split('/').pop());

  node.on('mouseover',(evt,d)=>{
    // Highlight connections
    link.attr('stroke-opacity',l=>(l.source.id===d.id||l.target.id===d.id)?0.9:0.05);
    link.attr('stroke',l=>(l.source.id===d.id||l.target.id===d.id)?dirColor(d.dir):'#30363d');
    node.select('circle').attr('fill-opacity',n=>(n.id===d.id||simLinks.some(l=>(l.source.id===d.id&&l.target.id===n.id)||(l.target.id===d.id&&l.source.id===n.id)))?1:0.15);
    const ins = simLinks.filter(l=>l.target.id===d.id).length;
    const outs = simLinks.filter(l=>l.source.id===d.id).length;
    showTip(evt,`<span class="tt-n">${d.id}</span><div class="tt-f">Module: ${d.dir}</div><div class="tt-f">${ins} incoming · ${outs} outgoing</div>`);
  })
  .on('mousemove',moveTip)
  .on('mouseout',()=>{
    link.attr('stroke-opacity',0.4).attr('stroke','#30363d');
    node.select('circle').attr('fill-opacity',0.7);
    hideTip();
  })
  .on('click',(evt,d)=>{
    showDepDetail(d, simLinks, simNodes);
  });

  depSim.on('tick',()=>{
    link.attr('x1',d=>d.source.x).attr('y1',d=>d.source.y).attr('x2',d=>d.target.x).attr('y2',d=>d.target.y);
    node.attr('transform',d=>`translate(${d.x},${d.y})`);
  });
}

function showDepDetail(file, links, nodes){
  const panel = document.getElementById('depsDetail');
  const imports = links.filter(l=>l.source.id===file.id).map(l=>({file:l.target.id||l.target,w:l.weight,kinds:l.kinds}));
  const importedBy = links.filter(l=>l.target.id===file.id).map(l=>({file:l.source.id||l.source,w:l.weight,kinds:l.kinds}));

  let html=`<div class="detail-title">${file.id.split('/').pop()}<span class="detail-close" onclick="this.closest('.detail-panel').classList.remove('open')">✕</span></div>`;
  html+=`<div class="detail-section"><div class="detail-row"><span class="lbl">Module</span><span>${file.dir}</span></div></div>`;
  html+=`<div class="detail-section"><h3>Imports from (${imports.length})</h3>`;
  imports.sort((a,b)=>b.w-a.w).forEach(i=>{
    html+=`<div class="detail-sym" onclick="selectFile('${i.file}')">📄 ${i.file.split('/').pop()} <span class="count">${i.w}×</span></div>`;
  });
  html+=`</div>`;
  html+=`<div class="detail-section"><h3>Imported by (${importedBy.length})</h3>`;
  importedBy.sort((a,b)=>b.w-a.w).forEach(i=>{
    html+=`<div class="detail-sym" onclick="selectFile('${i.file}')">📄 ${i.file.split('/').pop()} <span class="count">${i.w}×</span></div>`;
  });
  html+=`</div>`;

  panel.innerHTML=html;
  panel.classList.add('open');
}

function highlightDepFile(f){
  // Trigger mouseover on the matching node in deps graph
  d3.select('#depsSvg').selectAll('g circle').each(function(d){
    if(d && d.id===f) d3.select(this).dispatch('mouseover');
  });
}

document.getElementById('depsGroup').addEventListener('change',renderDeps);
document.getElementById('depsWeight').addEventListener('input',e=>{ document.getElementById('depsWeightVal').textContent=e.target.value; renderDeps(); });
document.getElementById('depsFilter').addEventListener('input',e=>{
  const q=e.target.value.toLowerCase();
  d3.select('#depsSvg').selectAll('g').each(function(d){
    if(!d||!d.id) return;
    const match = d.id.toLowerCase().includes(q);
    d3.select(this).select('circle').attr('fill-opacity', q===''?0.7: match?1:0.1);
    d3.select(this).select('text').style('opacity', q===''?1: match?1:0.15);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// VIEW 3: CALL EXPLORER — Hierarchical tree + force hybrid
// ════════════════════════════════════════════════════════════════════════════
document.getElementById('callDepth').addEventListener('input',e=>{document.getElementById('callDepthVal').textContent=e.target.value;});
document.getElementById('callInput').addEventListener('keyup',e=>{if(e.key==='Enter')traceCall();});

async function traceCall(name){
  if(name) document.getElementById('callInput').value=name;
  const sym = document.getElementById('callInput').value.trim();
  if(!sym) return;
  const dir = document.getElementById('callDir').value;
  const depth = document.getElementById('callDepth').value;

  let url;
  if(dir==='callers'){
    url = API+'/api/callgraph/'+encodeURIComponent(sym)+'?callers=1&depth='+depth;
  } else {
    url = API+'/api/callgraph/'+encodeURIComponent(sym)+'?depth='+depth;
  }

  const res = await fetch(url);
  const data = await res.json();
  if(data.error){ alert(data.error); return; }
  if(!data.nodes||!data.nodes.length){ alert('No results for "'+sym+'"'); return; }
  renderCallGraph(data, sym);
}
window.traceCall = traceCall;

function renderCallGraph(data, rootName){
  const svg = d3.select('#callSvg');
  svg.selectAll('*').remove();
  const W = svg.node().clientWidth, H = svg.node().clientHeight;

  if(!data.nodes.length) return;

  // Build tree structure from flat nodes + edges
  const nodeMap = {};
  data.nodes.forEach(n=>{ nodeMap[n.id] = {...n, children:[]}; });
  data.links.forEach(l=>{
    const src = l.source.id||l.source;
    const tgt = l.target.id||l.target;
    if(nodeMap[src] && nodeMap[tgt]){
      nodeMap[src].children.push(nodeMap[tgt]);
    }
  });

  // Find root
  const rootNode = data.nodes.find(n=>n.name===rootName)||data.nodes[0];
  const treeRoot = nodeMap[rootNode.id] || {name:rootName, children:[]};

  // Also render as force graph for complex graphs
  const g = svg.append('g');
  svg.call(d3.zoom().scaleExtent([0.2,5]).on('zoom',e=>g.attr('transform',e.transform)));

  // Arrow
  svg.append('defs').append('marker').attr('id','call-arrow').attr('viewBox','0 -5 10 10')
    .attr('refX',18).attr('refY',0).attr('markerWidth',7).attr('markerHeight',7).attr('orient','auto')
    .append('path').attr('d','M0,-4L10,0L0,4').attr('fill','var(--accent)');

  const sim = d3.forceSimulation(data.nodes)
    .force('link', d3.forceLink(data.links).id(d=>d.id).distance(100))
    .force('charge', d3.forceManyBody().strength(-500))
    .force('center', d3.forceCenter(W/2, H/2))
    .force('collision', d3.forceCollide(35));

  const link = g.append('g').selectAll('line').data(data.links).enter().append('line')
    .attr('stroke','var(--accent)').attr('stroke-opacity',0.3).attr('stroke-width',1.5)
    .attr('marker-end','url(#call-arrow)');

  const node = g.append('g').selectAll('g').data(data.nodes).enter().append('g')
    .style('cursor','pointer')
    .call(d3.drag()
      .on('start',(e,d)=>{ if(!e.active)sim.alphaTarget(0.3).restart(); d.fx=d.x; d.fy=d.y; })
      .on('drag',(e,d)=>{ d.fx=e.x; d.fy=e.y; })
      .on('end',(e,d)=>{ if(!e.active)sim.alphaTarget(0); d.fx=null; d.fy=null; })
    );

  // Root node is larger and highlighted
  node.append('circle')
    .attr('r',d=>d.id===rootNode.id?16:10)
    .attr('fill',d=>KindColors[d.kind]||'#8b949e')
    .attr('fill-opacity',0.8)
    .attr('stroke',d=>d.id===rootNode.id?'#fff':KindColors[d.kind]||'#8b949e')
    .attr('stroke-width',d=>d.id===rootNode.id?3:1.5);

  node.append('text')
    .attr('dy',-18).attr('text-anchor','middle')
    .style('font-size','11px').style('fill','var(--text)').style('font-weight',d=>d.id===rootNode.id?'700':'400')
    .text(d=>d.name);

  // Interactions
  node.on('mouseover',(evt,d)=>{
    link.attr('stroke-opacity',l=>(l.source.id===d.id||l.target.id===d.id)?0.9:0.08);
    showTip(evt,`<span class="tt-n">${d.name}</span><span class="tt-k">${d.kind}</span><div class="tt-f">${d.file}:${d.line}</div>`);
  })
  .on('mousemove',moveTip)
  .on('mouseout',()=>{ link.attr('stroke-opacity',0.3); hideTip(); })
  .on('click',(evt,d)=>{
    document.getElementById('callInput').value=d.name;
    traceCall(d.name);
  })
  .on('dblclick',(evt,d)=>{
    showCallDetail(d, data);
  });

  sim.on('tick',()=>{
    link.attr('x1',d=>d.source.x).attr('y1',d=>d.source.y).attr('x2',d=>d.target.x).attr('y2',d=>d.target.y);
    node.attr('transform',d=>`translate(${d.x},${d.y})`);
  });
}

function showCallDetail(d, data){
  const panel = document.getElementById('callDetail');
  const callees = data.links.filter(l=>(l.source.id||l.source)===d.id).map(l=>data.nodes.find(n=>n.id===(l.target.id||l.target))).filter(Boolean);
  const callers = data.links.filter(l=>(l.target.id||l.target)===d.id).map(l=>data.nodes.find(n=>n.id===(l.source.id||l.source))).filter(Boolean);

  let html=`<div class="detail-title">${d.name}<span class="detail-close" onclick="this.closest('.detail-panel').classList.remove('open')">✕</span></div>`;
  html+=`<div class="detail-section"><div class="detail-row"><span class="lbl">Kind</span>${badgeFor(d.kind)}</div>`;
  html+=`<div class="detail-row"><span class="lbl">File</span><span>${d.file}</span></div>`;
  html+=`<div class="detail-row"><span class="lbl">Line</span><span>${d.line}</span></div></div>`;
  if(callees.length){
    html+=`<div class="detail-section"><h3>Calls (${callees.length})</h3>`;
    callees.forEach(c=>{html+=`<div class="detail-sym" onclick="traceCall('${c.name}')">${badgeFor(c.kind)} ${c.name}</div>`;});
    html+=`</div>`;
  }
  if(callers.length){
    html+=`<div class="detail-section"><h3>Called by (${callers.length})</h3>`;
    callers.forEach(c=>{html+=`<div class="detail-sym" onclick="traceCall('${c.name}')">${badgeFor(c.kind)} ${c.name}</div>`;});
    html+=`</div>`;
  }
  panel.innerHTML=html;
  panel.classList.add('open');
}

// ════════════════════════════════════════════════════════════════════════════
// VIEW 4: CODE MAP — Interactive treemap with detail panel
// ════════════════════════════════════════════════════════════════════════════
function renderMap(){
  const svg = d3.select('#mapSvg');
  svg.selectAll('*').remove();
  const W = svg.node().clientWidth, H = svg.node().clientHeight;
  const sizeBy = document.getElementById('mapSize').value;
  const colorBy = document.getElementById('mapColor').value;

  const fc = C.fileCounts||[];
  if(!fc.length) return;

  // Build hierarchy: dirs → files
  const dirMap = {};
  fc.forEach(f=>{
    const dir = f.dir||'.';
    if(!dirMap[dir]) dirMap[dir]=[];
    const val = sizeBy==='total' ? f.total : (f.kinds[sizeBy]||0);
    if(val>0) dirMap[dir].push({...f, sizeVal:val});
  });

  const root = {name:'project',children:[]};
  for(const dir in dirMap){
    root.children.push({name:dir, children:dirMap[dir].map(f=>({name:f.file.split('/').pop(),fullPath:f.file,dir:f.dir,value:f.sizeVal,kinds:f.kinds,total:f.total}))});
  }

  const hier = d3.hierarchy(root).sum(d=>d.value||0).sort((a,b)=>b.value-a.value);
  d3.treemap().size([W,H]).paddingTop(18).paddingRight(2).paddingBottom(2).paddingLeft(2).paddingInner(1).round(true)(hier);

  function mapFillColor(d){
    if(d.children) return 'transparent';
    if(colorBy==='dir') return DirColors(d.data.dir||d.parent?.data.name||'');
    // Dominant kind
    const kinds = d.data.kinds||{};
    let max='',maxV=0;
    for(const k in kinds){ if(kinds[k]>maxV){maxV=kinds[k];max=k;} }
    return KindColors[max]||'#8b949e';
  }

  const g = svg.append('g');

  // Directory groups
  const dirCell = g.selectAll('.dir-cell').data(hier.children||[]).enter().append('g').attr('class','dir-cell');
  dirCell.append('rect')
    .attr('x',d=>d.x0).attr('y',d=>d.y0)
    .attr('width',d=>d.x1-d.x0).attr('height',d=>d.y1-d.y0)
    .attr('fill','none').attr('stroke','var(--border)').attr('stroke-width',1).attr('rx',3);
  dirCell.append('text')
    .attr('x',d=>d.x0+4).attr('y',d=>d.y0+13)
    .style('font-size','11px').style('fill','var(--text-dim)').style('font-weight','600')
    .text(d=>(d.x1-d.x0)>80?d.data.name:'');

  // File cells
  const cell = g.selectAll('.file-cell').data(hier.leaves()).enter().append('g').attr('class','file-cell')
    .style('cursor','pointer');

  cell.append('rect')
    .attr('x',d=>d.x0).attr('y',d=>d.y0).attr('width',d=>Math.max(0,d.x1-d.x0)).attr('height',d=>Math.max(0,d.y1-d.y0))
    .attr('fill',mapFillColor).attr('fill-opacity',0.35).attr('stroke',mapFillColor).attr('stroke-opacity',0.6).attr('rx',2);

  cell.filter(d=>(d.x1-d.x0)>50&&(d.y1-d.y0)>18)
    .append('text').attr('x',d=>d.x0+4).attr('y',d=>d.y0+13)
    .style('font-size','10px').style('fill','var(--text)').style('pointer-events','none')
    .text(d=>{ const n=d.data.name; return n.length>(d.x1-d.x0)/6 ? n.slice(0,Math.floor((d.x1-d.x0)/6))+'…' : n; });

  cell.filter(d=>(d.x1-d.x0)>60&&(d.y1-d.y0)>32)
    .append('text').attr('x',d=>d.x0+4).attr('y',d=>d.y0+25)
    .style('font-size','9px').style('fill','var(--text-dim)').style('pointer-events','none')
    .text(d=>d.data.total+' symbols');

  cell.on('mouseover',(evt,d)=>{
    d3.select(evt.currentTarget).select('rect').attr('fill-opacity',0.7).attr('stroke-opacity',1);
    const kinds = d.data.kinds||{};
    const kindStr = Object.entries(kinds).map(([k,v])=>`${k}: ${v}`).join(', ');
    showTip(evt,`<span class="tt-n">${d.data.name}</span><div class="tt-f">${d.data.fullPath}</div><div class="tt-f">${d.data.total} symbols — ${kindStr}</div>`);
  })
  .on('mousemove',moveTip)
  .on('mouseout',(evt)=>{
    d3.select(evt.currentTarget).select('rect').attr('fill-opacity',0.35).attr('stroke-opacity',0.6);
    hideTip();
  })
  .on('click',(evt,d)=>{
    showMapDetail(d.data);
  });
}

async function showMapDetail(file){
  const panel = document.getElementById('mapDetail');
  const kinds = file.kinds||{};
  let html=`<div class="detail-title">${file.name}<span class="detail-close" onclick="this.closest('.detail-panel').classList.remove('open')">✕</span></div>`;
  html+=`<div class="detail-section"><div class="detail-row"><span class="lbl">Path</span><span>${file.fullPath}</span></div>`;
  html+=`<div class="detail-row"><span class="lbl">Module</span><span>${file.dir}</span></div>`;
  html+=`<div class="detail-row"><span class="lbl">Total</span><span>${file.total} symbols</span></div></div>`;

  // Kind breakdown as mini bar
  html+=`<div class="detail-section"><h3>Symbol breakdown</h3>`;
  const sorted = Object.entries(kinds).sort((a,b)=>b[1]-a[1]);
  for(const [k,v] of sorted){
    const pct = Math.round(v/file.total*100);
    html+=`<div style="margin:3px 0"><div style="display:flex;justify-content:space-between;font-size:12px"><span>${badgeFor(k)} ${k}</span><span>${v}</span></div>`;
    html+=`<div style="background:var(--bg);height:4px;border-radius:2px;margin-top:2px"><div style="background:${KindColors[k]||'#8b949e'};height:4px;border-radius:2px;width:${pct}%"></div></div></div>`;
  }
  html+=`</div>`;

  // Load actual symbols for this file
  try{
    const res = await fetch(API+'/api/file-symbols/'+encodeURIComponent(file.fullPath));
    const syms = await res.json();
    if(Array.isArray(syms) && syms.length){
      html+=`<div class="detail-section"><h3>Symbols (${syms.length})</h3>`;
      syms.filter(s=>s.kind!=='import').slice(0,30).forEach(s=>{
        html+=`<div class="detail-sym" onclick="traceCall('${s.name}')">${badgeFor(s.kind)} ${s.name} <span style="color:var(--text-dim);font-size:10px;margin-left:auto">L${s.line}</span></div>`;
      });
      html+=`</div>`;
    }
  }catch(e){}

  panel.innerHTML=html;
  panel.classList.add('open');
}

function highlightMapFile(f){
  d3.select('#mapSvg').selectAll('.file-cell rect').attr('stroke-width',function(d){
    return d&&d.data&&d.data.fullPath===f?3:0;
  });
}

document.getElementById('mapSize').addEventListener('change',renderMap);
document.getElementById('mapColor').addEventListener('change',renderMap);

// ── Init ──
loadAll();
</script>
</body>
</html>"##.to_string()
}
