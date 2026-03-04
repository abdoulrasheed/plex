use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

use crate::config::Config;
use crate::mcp::tools;
use crate::search::Searcher;
use crate::store::Store;

pub fn run_stdio(config: &Config) -> Result<()> {
    let store = Store::open(&config.db_path())?;
    let searcher = Searcher::from_store(store)?;

    let graph_store = Store::open(&config.db_path())?;

    let mut server = McpServer {
        searcher,
        graph_store,
        project_name: config.project_name().to_string(),
        initialized: false,
    };

    tracing::info!("Plex MCP server starting on stdio");

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line.context("Failed to read from stdin")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                writeln!(stdout, "{}", err_resp)?;
                stdout.flush()?;
                continue;
            }
        };

        let response = server.handle_request(&request);
        writeln!(stdout, "{}", response)?;
        stdout.flush()?;
    }

    Ok(())
}

#[allow(dead_code)]
struct McpServer {
    searcher: Searcher,
    graph_store: Store,
    project_name: String,
    initialized: bool,
}

impl McpServer {
    fn handle_request(&mut self, request: &Value) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        match method {
            "initialize" => self.handle_initialize(&id, &params),
            "notifications/initialized" => {
                self.initialized = true;
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                })
            }
            "tools/list" => self.handle_tools_list(&id),
            "tools/call" => self.handle_tools_call(&id, &params),
            "ping" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {}", method)
                }
            }),
        }
    }

    fn handle_initialize(&self, id: &Value, _params: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "plex",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        })
    }

    fn handle_tools_list(&self, id: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": tools::tool_definitions()
            }
        })
    }

    fn handle_tools_call(&mut self, id: &Value, params: &Value) -> Value {
        let tool_name = params
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("");
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(json!({}));

        let result =
            tools::execute_tool(tool_name, &arguments, &mut self.searcher, &self.graph_store);

        match result {
            Ok(content) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": content
                    }]
                }
            }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": format!("Error: {}", e)
                    }],
                    "isError": true
                }
            }),
        }
    }
}

