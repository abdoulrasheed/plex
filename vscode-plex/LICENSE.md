# Plex — Local Code Intelligence Engine

Plex is a blazing-fast, local-first code intelligence engine written in pure Rust. It indexes your codebase, extracts symbols and relationships, generates semantic embeddings, and exposes everything through a CLI, MCP server, and (coming soon) VS Code extension.

**No API keys. No data leaves your machine. Sub-second indexing.**

## Features

- **Multi-language parsing**: Python, JavaScript, TypeScript, Rust, Go, Java
- **Semantic search**: Natural language queries powered by local embeddings (all-MiniLM-L6-v2)
- **Call graph analysis**: Trace callers, callees, and inheritance hierarchies
- **MCP server**: Plug into Cursor, Claude Desktop, or any MCP-compatible AI assistant
- **Incremental indexing**: Only re-indexes changed files (SHA256 content hashing)
- **Single binary**: ~27MB, zero runtime dependencies

## Quick Start

```bash
# Build from source
cargo build --release

# Index your project
plex index /path/to/your/project

# Search (semantic + text)
plex search "authentication middleware"

# Show stats
plex stats

# Trace call graph
plex calls my_function

# Start MCP server (for AI assistants)
plex mcp
```

## MCP Integration

### Cursor

Add to your project's `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "plex": {
      "command": "/absolute/path/to/plex",
      "args": ["mcp"],
      "cwd": "/path/to/your/project"
    }
  }
}
```

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "plex": {
      "command": "/absolute/path/to/plex",
      "args": ["mcp"],
      "cwd": "/path/to/your/project"
    }
  }
}
```

### Available MCP Tools

| Tool | Description |
|------|-------------|
| `search` | Semantic + text search across the codebase |
| `get_symbol` | Get detailed info about a function, class, etc. |
| `get_callers` | Who calls this function? |
| `get_callees` | What does this function call? |
| `get_inheritance` | Class hierarchy (superclasses + subclasses) |
| `find_implementations` | Find all implementations of a trait/interface |
| `get_file_symbols` | List all symbols in a file |
| `get_project_structure` | Project overview: files, symbols, languages |
| `get_references` | Every usage of a symbol across the codebase |

## Architecture

```
plex
├── parser      tree-sitter AST extraction (6 languages)
├── store       SQLite + FTS5 storage layer
├── embeddings  ONNX Runtime + all-MiniLM-L6-v2 (local, CPU)
├── indexer     Pipeline orchestration (scan → parse → store → embed)
├── search      Hybrid FTS5 + semantic vector search
├── graph       BFS call graph & inheritance traversal
└── mcp         JSON-RPC 2.0 stdio transport
```

## Requirements

- Rust 1.70+ (to build)
- ~80MB disk for the embedding model (downloaded on first use)
- No Python, Node.js, or external services required
