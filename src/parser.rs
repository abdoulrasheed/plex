use crate::types::*;
use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

pub struct CodeParser {
    parser: Parser,
}

impl CodeParser {
    pub fn new() -> Self {
        CodeParser {
            parser: Parser::new(),
        }
    }

    pub fn parse_file(
        &mut self,
        source: &str,
        language: Language,
        relative_path: &str,
    ) -> Result<ParseResult> {
        let ts_lang = match language {
            Language::Python => tree_sitter_python::language(),
            Language::JavaScript => tree_sitter_javascript::language(),
            Language::TypeScript => tree_sitter_typescript::language_typescript(),
            Language::Rust => tree_sitter_rust::language(),
            Language::Go => tree_sitter_go::language(),
            Language::Java => tree_sitter_java::language(),
            Language::C => tree_sitter_c::language(),
            Language::Cpp => tree_sitter_cpp::language(),
            Language::Unknown => {
                return Ok(ParseResult {
                    symbols: vec![],
                    relations: vec![],
                })
            }
        };

        self.parser
            .set_language(ts_lang)
            .context("Failed to set tree-sitter language")?;

        let tree = self
            .parser
            .parse(source, None)
            .context("Failed to parse source")?;

        let source_bytes = source.as_bytes();
        let root = tree.root_node();

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        self.walk_node(
            root,
            source_bytes,
            language,
            relative_path,
            None,
            &mut symbols,
            &mut relations,
        );

        Ok(ParseResult { symbols, relations })
    }

    fn walk_node(
        &self,
        node: Node,
        source: &[u8],
        lang: Language,
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        relations: &mut Vec<ParsedRelation>,
    ) {
        let kind = node.kind();

        let new_parent = match lang {
            Language::Python => self.extract_python(node, source, file_path, parent_idx, symbols, relations),
            Language::JavaScript | Language::TypeScript => {
                self.extract_js_ts(node, source, file_path, parent_idx, symbols, relations)
            }
            Language::Rust => self.extract_rust(node, source, file_path, parent_idx, symbols, relations),
            Language::Go => self.extract_go(node, source, file_path, parent_idx, symbols, relations),
            Language::Java => self.extract_java(node, source, file_path, parent_idx, symbols, relations),
            Language::C => self.extract_c(node, source, file_path, parent_idx, symbols, relations),
            Language::Cpp => self.extract_cpp(node, source, file_path, parent_idx, symbols, relations),
            Language::Unknown => None,
        };

        if matches!(kind, "call" | "call_expression" | "method_invocation") {
            if let Some(caller_idx) = parent_idx.or(new_parent) {
                if let Some(callee) = self.extract_call_target(node, source, lang) {
                    relations.push(ParsedRelation {
                        source_symbol_index: caller_idx,
                        target_name: callee,
                        kind: RelationKind::Calls,
                        line: node.start_position().row as u32 + 1,
                    });
                }
            }
        }

        let effective_parent = new_parent.or(parent_idx);
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                self.walk_node(
                    cursor.node(),
                    source,
                    lang,
                    file_path,
                    effective_parent,
                    symbols,
                    relations,
                );
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn extract_python(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "function_definition" => {
                let name = child_text(node, "name", source)?;
                let kind = if parent_idx.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                let sig = self.python_function_signature(node, source);
                let doc = self.python_docstring(node, source);
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: doc,
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "class_definition" => {
                let name = child_text(node, "name", source)?;
                let doc = self.python_docstring(node, source);
                let qualified = make_qualified(file_path, None, &name);

                if let Some(args) = node.child_by_field_name("superclasses") {
                    let mut cursor = args.walk();
                    if cursor.goto_first_child() {
                        loop {
                            let child = cursor.node();
                            if child.kind() == "identifier" || child.kind() == "attribute" {
                                if let Ok(base) = child.utf8_text(source) {
                                    relations.push(ParsedRelation {
                                        source_symbol_index: symbols.len(),
                                        target_name: base.to_string(),
                                        kind: RelationKind::Inherits,
                                        line: child.start_position().row as u32 + 1,
                                    });
                                }
                            }
                            if !cursor.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                }

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Class,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: doc,
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "import_statement" | "import_from_statement" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn python_function_signature(&self, node: Node, source: &[u8]) -> Option<String> {
        let name = child_text(node, "name", source)?;
        let params = node.child_by_field_name("parameters")?;
        let params_text = params.utf8_text(source).ok()?;
        let ret = node
            .child_by_field_name("return_type")
            .and_then(|n| n.utf8_text(source).ok());
        let sig = if let Some(ret) = ret {
            format!("def {}{}  -> {}", name, params_text, ret)
        } else {
            format!("def {}{}", name, params_text)
        };
        Some(sig)
    }

    fn python_docstring(&self, node: Node, source: &[u8]) -> Option<String> {
        let body = node.child_by_field_name("body")?;
        let first = body.child(0)?;
        if first.kind() == "expression_statement" {
            let string_node = first.child(0)?;
            if string_node.kind() == "string" || string_node.kind() == "concatenated_string" {
                let text = string_node.utf8_text(source).ok()?;
                let trimmed = text
                    .trim_start_matches("\"\"\"")
                    .trim_start_matches("'''")
                    .trim_end_matches("\"\"\"")
                    .trim_end_matches("'''")
                    .trim();
                return Some(trimmed.to_string());
            }
        }
        None
    }

    fn extract_js_ts(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "function_declaration" => {
                let name = child_text(node, "name", source)?;
                let sig = self.js_function_signature(node, source);
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Function,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "class_declaration" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                if let Some(heritage) = node.child_by_field_name("heritage") {
                    if let Ok(text) = heritage.utf8_text(source) {
                        for base in text.split(',') {
                            let base = base
                                .replace("extends", "")
                                .replace("implements", "")
                                .trim()
                                .to_string();
                            if !base.is_empty() {
                                relations.push(ParsedRelation {
                                    source_symbol_index: symbols.len(),
                                    target_name: base,
                                    kind: RelationKind::Inherits,
                                    line: node.start_position().row as u32 + 1,
                                });
                            }
                        }
                    }
                }

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Class,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "method_definition" => {
                let name = child_text(node, "name", source)?;
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Method,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "variable_declarator" => {
                let value = node.child_by_field_name("value")?;
                if value.kind() != "arrow_function" {
                    return None;
                }
                let name = child_text(node, "name", source)?;
                let snippet = body_snippet(value, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Function,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: value.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: value.end_position().column as u32,
                    signature: None,
                    doc_comment: None,
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "interface_declaration" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Interface,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "type_alias_declaration" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Type,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "import_statement" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn js_function_signature(&self, node: Node, source: &[u8]) -> Option<String> {
        let name = child_text(node, "name", source)?;
        let params = node.child_by_field_name("parameters")?;
        let params_text = params.utf8_text(source).ok()?;
        Some(format!("function {}{}", name, params_text))
    }

    fn extract_rust(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "function_item" => {
                let name = child_text(node, "name", source)?;
                let kind = if parent_idx.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                let sig = self.rust_fn_signature(node, source);
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_doc_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "struct_item" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Struct,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_doc_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "enum_item" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Enum,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_doc_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "trait_item" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Trait,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_doc_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "impl_item" => {
                let type_name = node
                    .child_by_field_name("type")
                    .and_then(|n| n.utf8_text(source).ok())
                    .unwrap_or("Unknown");

                if let Some(trait_node) = node.child_by_field_name("trait") {
                    if let Ok(trait_name) = trait_node.utf8_text(source) {
                        relations.push(ParsedRelation {
                            source_symbol_index: symbols.len(),
                            target_name: trait_name.to_string(),
                            kind: RelationKind::Implements,
                            line: node.start_position().row as u32 + 1,
                        });
                    }
                }

                let qualified = make_qualified(file_path, None, type_name);
                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name: format!("impl {}", type_name),
                    qualified_name: qualified,
                    kind: SymbolKind::Module,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: None,
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "use_declaration" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn rust_fn_signature(&self, node: Node, source: &[u8]) -> Option<String> {
        let text = node.utf8_text(source).ok()?;
        let sig = text.split('{').next()?.trim();
        Some(sig.to_string())
    }

    fn extract_go(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        _relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "function_declaration" => {
                let name = child_text(node, "name", source)?;
                let sig = node.utf8_text(source).ok().and_then(|t| {
                    Some(t.split('{').next()?.trim().to_string())
                });
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Function,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "method_declaration" => {
                let name = child_text(node, "name", source)?;
                let sig = node.utf8_text(source).ok().and_then(|t| {
                    Some(t.split('{').next()?.trim().to_string())
                });
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Method,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "type_declaration" => {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() == "type_spec" {
                            let name = child_text(child, "name", source);
                            let type_node = child.child_by_field_name("type");
                            if let (Some(name), Some(type_node)) = (name, type_node) {
                                let kind = match type_node.kind() {
                                    "struct_type" => SymbolKind::Struct,
                                    "interface_type" => SymbolKind::Interface,
                                    _ => SymbolKind::Type,
                                };
                                let qualified = make_qualified(file_path, None, &name);
                                let idx = symbols.len();
                                symbols.push(ParsedSymbol {
                                    name,
                                    qualified_name: qualified,
                                    kind,
                                    start_line: child.start_position().row as u32 + 1,
                                    end_line: child.end_position().row as u32 + 1,
                                    start_col: child.start_position().column as u32,
                                    end_col: child.end_position().column as u32,
                                    signature: None,
                                    doc_comment: preceding_comment(node, source),
                                    body_snippet: None,
                                    parent_index: parent_idx,
                                });
                                return Some(idx);
                            }
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }
                None
            }
            "import_declaration" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn extract_java(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "method_declaration" | "constructor_declaration" => {
                let name = child_text(node, "name", source)?;
                let kind = if node.kind() == "constructor_declaration" {
                    SymbolKind::Constructor
                } else {
                    SymbolKind::Method
                };
                let sig = node.utf8_text(source).ok().and_then(|t| {
                    Some(t.split('{').next()?.trim().to_string())
                });
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "class_declaration" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                if let Some(super_node) = node.child_by_field_name("superclass") {
                    if let Ok(base) = super_node.utf8_text(source) {
                        relations.push(ParsedRelation {
                            source_symbol_index: symbols.len(),
                            target_name: base.to_string(),
                            kind: RelationKind::Inherits,
                            line: node.start_position().row as u32 + 1,
                        });
                    }
                }
                if let Some(ifaces) = node.child_by_field_name("interfaces") {
                    let mut cursor = ifaces.walk();
                    if cursor.goto_first_child() {
                        loop {
                            let child = cursor.node();
                            if child.kind() == "type_identifier" {
                                if let Ok(iface) = child.utf8_text(source) {
                                    relations.push(ParsedRelation {
                                        source_symbol_index: symbols.len(),
                                        target_name: iface.to_string(),
                                        kind: RelationKind::Implements,
                                        line: child.start_position().row as u32 + 1,
                                    });
                                }
                            }
                            if !cursor.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                }

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Class,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "interface_declaration" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Interface,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "import_declaration" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn extract_c(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        _relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "function_definition" => {
                let declarator = node.child_by_field_name("declarator")?;
                let name = self.c_declarator_name(declarator, source)?;
                let sig = node.utf8_text(source).ok().and_then(|t| {
                    Some(t.split('{').next()?.trim().to_string())
                });
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Function,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "declaration" => {
                let declarator = node.child_by_field_name("declarator");
                if let Some(decl) = declarator {
                    if decl.kind() == "function_declarator" {
                        let name = self.c_declarator_name(decl, source)?;
                        let sig = node.utf8_text(source).ok().map(|t| t.trim().trim_end_matches(';').to_string());
                        let qualified = make_qualified(file_path, None, &name);
                        let idx = symbols.len();
                        symbols.push(ParsedSymbol {
                            name,
                            qualified_name: qualified,
                            kind: SymbolKind::Function,
                            start_line: node.start_position().row as u32 + 1,
                            end_line: node.end_position().row as u32 + 1,
                            start_col: node.start_position().column as u32,
                            end_col: node.end_position().column as u32,
                            signature: sig,
                            doc_comment: preceding_comment(node, source),
                            body_snippet: None,
                            parent_index: parent_idx,
                        });
                        return Some(idx);
                    }
                }
                None
            }
            "struct_specifier" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Struct,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "enum_specifier" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Enum,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "type_definition" => {
                let declarator = node.child_by_field_name("declarator");
                if let Some(decl) = declarator {
                    let name = decl.utf8_text(source).ok()?.to_string();
                    if name.is_empty() { return None; }
                    let qualified = make_qualified(file_path, None, &name);
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name,
                        qualified_name: qualified,
                        kind: SymbolKind::Type,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: node.start_position().column as u32,
                        end_col: node.end_position().column as u32,
                        signature: None,
                        doc_comment: preceding_comment(node, source),
                        body_snippet: None,
                        parent_index: parent_idx,
                    });
                    return Some(idx);
                }
                None
            }
            "preproc_def" | "preproc_function_def" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);
                let kind = if node.kind() == "preproc_function_def" {
                    SymbolKind::Function
                } else {
                    SymbolKind::Constant
                };

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "preproc_include" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn c_declarator_name(&self, node: Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "function_declarator" => {
                let inner = node.child_by_field_name("declarator")?;
                self.c_declarator_name(inner, source)
            }
            "pointer_declarator" => {
                let inner = node.child_by_field_name("declarator")?;
                self.c_declarator_name(inner, source)
            }
            "parenthesized_declarator" => {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() != "(" && child.kind() != ")" {
                            return self.c_declarator_name(child, source);
                        }
                        if !cursor.goto_next_sibling() { break; }
                    }
                }
                None
            }
            "identifier" => node.utf8_text(source).ok().map(|s| s.to_string()),
            _ => node.utf8_text(source).ok().map(|s| s.to_string()),
        }
    }

    fn extract_cpp(
        &self,
        node: Node,
        source: &[u8],
        file_path: &str,
        parent_idx: Option<usize>,
        symbols: &mut Vec<ParsedSymbol>,
        relations: &mut Vec<ParsedRelation>,
    ) -> Option<usize> {
        match node.kind() {
            "function_definition" => {
                let declarator = node.child_by_field_name("declarator")?;
                let name = self.c_declarator_name(declarator, source)?;
                let kind = if parent_idx.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                let sig = node.utf8_text(source).ok().and_then(|t| {
                    Some(t.split('{').next()?.trim().to_string())
                });
                let snippet = body_snippet(node, source, 5);
                let qualified = make_qualified(file_path, parent_idx.and_then(|i| symbols.get(i).map(|s| s.name.as_str())), &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: sig,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: snippet,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "class_specifier" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                if let Some(bases) = node.child_by_field_name("base_class_clause") {
                    let mut cursor = bases.walk();
                    if cursor.goto_first_child() {
                        loop {
                            let child = cursor.node();
                            if child.kind() == "base_class_clause" || child.kind() == "type_identifier" {
                                if let Ok(base) = child.utf8_text(source) {
                                    let base = base.trim().to_string();
                                    if !base.is_empty() && base != ":" && base != "," && base != "public" && base != "private" && base != "protected" {
                                        relations.push(ParsedRelation {
                                            source_symbol_index: symbols.len(),
                                            target_name: base,
                                            kind: RelationKind::Inherits,
                                            line: child.start_position().row as u32 + 1,
                                        });
                                    }
                                }
                            }
                            if !cursor.goto_next_sibling() { break; }
                        }
                    }
                }

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Class,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "struct_specifier" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Struct,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "enum_specifier" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Enum,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "namespace_definition" => {
                let name = child_text(node, "name", source).unwrap_or_else(|| "anonymous".to_string());
                let qualified = make_qualified(file_path, None, &name);

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind: SymbolKind::Module,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: None,
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "template_declaration" => {
                None
            }
            "type_definition" => {
                let declarator = node.child_by_field_name("declarator");
                if let Some(decl) = declarator {
                    let name = decl.utf8_text(source).ok()?.to_string();
                    if name.is_empty() { return None; }
                    let qualified = make_qualified(file_path, None, &name);
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name,
                        qualified_name: qualified,
                        kind: SymbolKind::Type,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: node.start_position().column as u32,
                        end_col: node.end_position().column as u32,
                        signature: None,
                        doc_comment: preceding_comment(node, source),
                        body_snippet: None,
                        parent_index: parent_idx,
                    });
                    return Some(idx);
                }
                None
            }
            "preproc_def" | "preproc_function_def" => {
                let name = child_text(node, "name", source)?;
                let qualified = make_qualified(file_path, None, &name);
                let kind = if node.kind() == "preproc_function_def" {
                    SymbolKind::Function
                } else {
                    SymbolKind::Constant
                };

                let idx = symbols.len();
                symbols.push(ParsedSymbol {
                    name,
                    qualified_name: qualified,
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    start_col: node.start_position().column as u32,
                    end_col: node.end_position().column as u32,
                    signature: None,
                    doc_comment: preceding_comment(node, source),
                    body_snippet: None,
                    parent_index: parent_idx,
                });
                Some(idx)
            }
            "preproc_include" => {
                if let Ok(text) = node.utf8_text(source) {
                    let idx = symbols.len();
                    symbols.push(ParsedSymbol {
                        name: text.trim().to_string(),
                        qualified_name: text.trim().to_string(),
                        kind: SymbolKind::Import,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        start_col: 0,
                        end_col: 0,
                        signature: None,
                        doc_comment: None,
                        body_snippet: None,
                        parent_index: None,
                    });
                    return Some(idx);
                }
                None
            }
            _ => None,
        }
    }

    fn extract_call_target(&self, node: Node, source: &[u8], lang: Language) -> Option<String> {
        match lang {
            Language::Python => {
                let func = node.child_by_field_name("function")?;
                match func.kind() {
                    "identifier" => func.utf8_text(source).ok().map(|s| s.to_string()),
                    "attribute" => {
                        let attr = func.child_by_field_name("attribute")?;
                        attr.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    _ => func.utf8_text(source).ok().map(|s| s.to_string()),
                }
            }
            Language::JavaScript | Language::TypeScript => {
                let func = node.child_by_field_name("function")?;
                match func.kind() {
                    "identifier" => func.utf8_text(source).ok().map(|s| s.to_string()),
                    "member_expression" => {
                        let prop = func.child_by_field_name("property")?;
                        prop.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    _ => func.utf8_text(source).ok().map(|s| s.to_string()),
                }
            }
            Language::Rust => {
                let func = node.child_by_field_name("function")?;
                match func.kind() {
                    "identifier" => func.utf8_text(source).ok().map(|s| s.to_string()),
                    "field_expression" => {
                        let field = func.child_by_field_name("field")?;
                        field.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    "scoped_identifier" => {
                        let name = func.child_by_field_name("name")?;
                        name.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    _ => func.utf8_text(source).ok().map(|s| s.to_string()),
                }
            }
            Language::Go => {
                let func = node.child_by_field_name("function")?;
                match func.kind() {
                    "identifier" => func.utf8_text(source).ok().map(|s| s.to_string()),
                    "selector_expression" => {
                        let field = func.child_by_field_name("field")?;
                        field.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    _ => func.utf8_text(source).ok().map(|s| s.to_string()),
                }
            }
            Language::Java => {
                let name = node.child_by_field_name("name")?;
                name.utf8_text(source).ok().map(|s| s.to_string())
            }
            Language::C => {
                let func = node.child_by_field_name("function")?;
                match func.kind() {
                    "identifier" => func.utf8_text(source).ok().map(|s| s.to_string()),
                    "field_expression" => {
                        let field = func.child_by_field_name("field")?;
                        field.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    _ => func.utf8_text(source).ok().map(|s| s.to_string()),
                }
            }
            Language::Cpp => {
                let func = node.child_by_field_name("function")?;
                match func.kind() {
                    "identifier" => func.utf8_text(source).ok().map(|s| s.to_string()),
                    "field_expression" => {
                        let field = func.child_by_field_name("field")?;
                        field.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    "qualified_identifier" | "scoped_identifier" => {
                        let name = func.child_by_field_name("name")?;
                        name.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    "template_function" => {
                        let name = func.child_by_field_name("name")?;
                        name.utf8_text(source).ok().map(|s| s.to_string())
                    }
                    _ => func.utf8_text(source).ok().map(|s| s.to_string()),
                }
            }
            Language::Unknown => None,
        }
    }
}

fn child_text(node: Node, field: &str, source: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    child.utf8_text(source).ok().map(|s| s.to_string())
}

fn make_qualified(file_path: &str, parent_name: Option<&str>, name: &str) -> String {
    match parent_name {
        Some(parent) => format!("{}::{}::{}", file_path, parent, name),
        None => format!("{}::{}", file_path, name),
    }
}

fn body_snippet(node: Node, source: &[u8], max_lines: usize) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= 1 {
        return None;
    }
    let snippet: String = lines
        .iter()
        .skip(1)
        .take(max_lines)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if snippet.trim().is_empty() {
        None
    } else {
        Some(snippet)
    }
}

fn preceding_comment(node: Node, source: &[u8]) -> Option<String> {
    let prev = node.prev_sibling()?;
    if prev.kind() != "comment" && !prev.kind().contains("comment") {
        return None;
    }
    if prev.end_position().row + 1 >= node.start_position().row {
        let text = prev.utf8_text(source).ok()?;
        Some(
            text.trim_start_matches("//")
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim_start_matches('#')
                .trim()
                .to_string(),
        )
    } else {
        None
    }
}

fn preceding_doc_comment(node: Node, source: &[u8]) -> Option<String> {
    let mut comments = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(prev) = current {
        if prev.kind() == "line_comment" {
            if let Ok(text) = prev.utf8_text(source) {
                if text.starts_with("///") || text.starts_with("//!") {
                    comments.push(
                        text.trim_start_matches("///")
                            .trim_start_matches("//!")
                            .trim()
                            .to_string(),
                    );
                } else {
                    break;
                }
            }
        } else {
            break;
        }
        current = prev.prev_sibling();
    }
    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        Some(comments.join("\n"))
    }
}

