use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    JavaScript,
    TypeScript,
    Rust,
    Go,
    Java,
    C,
    Cpp,
    Unknown,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "py" | "pyi" => Language::Python,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" => Language::TypeScript,
            "rs" => Language::Rust,
            "go" => Language::Go,
            "java" => Language::Java,
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Language::Cpp,
            _ => Language::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Java => "java",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Unknown => "unknown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "python" => Language::Python,
            "javascript" => Language::JavaScript,
            "typescript" => Language::TypeScript,
            "rust" => Language::Rust,
            "go" => Language::Go,
            "java" => Language::Java,
            "c" => Language::C,
            "cpp" => Language::Cpp,
            _ => Language::Unknown,
        }
    }

    /// Returns true if this language is supported for parsing.
    pub fn is_supported(&self) -> bool {
        !matches!(self, Language::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    Variable,
    Constant,
    Module,
    Import,
    Type,
    Field,
    Constructor,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Interface => "interface",
            SymbolKind::Trait => "trait",
            SymbolKind::Enum => "enum",
            SymbolKind::Variable => "variable",
            SymbolKind::Constant => "constant",
            SymbolKind::Module => "module",
            SymbolKind::Import => "import",
            SymbolKind::Type => "type",
            SymbolKind::Field => "field",
            SymbolKind::Constructor => "constructor",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" => SymbolKind::Class,
            "struct" => SymbolKind::Struct,
            "interface" => SymbolKind::Interface,
            "trait" => SymbolKind::Trait,
            "enum" => SymbolKind::Enum,
            "variable" => SymbolKind::Variable,
            "constant" => SymbolKind::Constant,
            "module" => SymbolKind::Module,
            "import" => SymbolKind::Import,
            "type" => SymbolKind::Type,
            "field" => SymbolKind::Field,
            "constructor" => SymbolKind::Constructor,
            _ => SymbolKind::Function,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Calls,
    CalledBy,
    Inherits,
    Implements,
    Imports,
    References,
    Contains,
}

impl RelationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelationKind::Calls => "calls",
            RelationKind::CalledBy => "called_by",
            RelationKind::Inherits => "inherits",
            RelationKind::Implements => "implements",
            RelationKind::Imports => "imports",
            RelationKind::References => "references",
            RelationKind::Contains => "contains",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "calls" => RelationKind::Calls,
            "called_by" => RelationKind::CalledBy,
            "inherits" => RelationKind::Inherits,
            "implements" => RelationKind::Implements,
            "imports" => RelationKind::Imports,
            "references" => RelationKind::References,
            "contains" => RelationKind::Contains,
            _ => RelationKind::References,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    pub id: i64,
    pub path: String,
    pub relative_path: String,
    pub language: Language,
    pub content_hash: String,
    pub size_bytes: u64,
    pub last_indexed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: i64,
    pub file_id: i64,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub end_col: u32,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub body_snippet: Option<String>,
    pub parent_symbol_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub id: i64,
    pub source_symbol_id: i64,
    pub target_symbol_name: String,
    pub target_symbol_id: Option<i64>,
    pub kind: RelationKind,
    pub file_id: i64,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub symbol: Symbol,
    pub file_path: String,
    pub score: f32,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub symbol_id: i64,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub file_path: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub kind: RelationKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallGraph {
    pub root: GraphNode,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Result of parsing a single file – not yet persisted.
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub symbols: Vec<ParsedSymbol>,
    pub relations: Vec<ParsedRelation>,
}

/// A symbol extracted from parsing, before it gets a database ID.
#[derive(Debug, Clone)]
pub struct ParsedSymbol {
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub end_col: u32,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub body_snippet: Option<String>,
    pub parent_index: Option<usize>,
}

/// A relation extracted from parsing, before symbol IDs are resolved.
#[derive(Debug, Clone)]
pub struct ParsedRelation {
    pub source_symbol_index: usize,
    pub target_name: String,
    pub kind: RelationKind,
    pub line: u32,
}

/// Project-level statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub file_count: usize,
    pub symbol_count: usize,
    pub relation_count: usize,
    pub embedding_count: usize,
    pub languages: Vec<String>,
}
