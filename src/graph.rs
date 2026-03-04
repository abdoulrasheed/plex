use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::store::Store;
use crate::types::*;

pub struct GraphAnalyzer<'a> {
    store: &'a Store,
}

impl<'a> GraphAnalyzer<'a> {
    pub fn new(store: &'a Store) -> Self {
        GraphAnalyzer { store }
    }

    pub fn get_call_graph(&self, symbol_name: &str, max_depth: usize) -> Result<CallGraph> {
        let symbols = self.store.find_symbols_by_name(symbol_name)?;
        let root_sym = symbols
            .first()
            .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found", symbol_name))?;

        let root_node = self.symbol_to_graph_node(root_sym)?;

        let mut nodes = vec![root_node.clone()];
        let mut edges = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(root_sym.id);

        let mut queue: VecDeque<(i64, usize)> = VecDeque::new();
        queue.push_back((root_sym.id, 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            let outgoing = self.store.get_outgoing_relations(current_id)?;
            for rel in &outgoing {
                if rel.kind != RelationKind::Calls {
                    continue;
                }
                if let Some(target_id) = rel.target_symbol_id {
                    edges.push(GraphEdge {
                        source_id: current_id,
                        target_id,
                        kind: RelationKind::Calls,
                    });

                    if visited.insert(target_id) {
                        if let Ok(Some(target_sym)) = self.store.get_symbol(target_id) {
                            nodes.push(self.symbol_to_graph_node(&target_sym)?);
                            queue.push_back((target_id, depth + 1));
                        }
                    }
                }
            }
        }

        Ok(CallGraph {
            root: nodes[0].clone(),
            nodes,
            edges,
        })
    }

    pub fn get_callers(&self, symbol_name: &str, max_depth: usize) -> Result<CallGraph> {
        let symbols = self.store.find_symbols_by_name(symbol_name)?;
        let root_sym = symbols
            .first()
            .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found", symbol_name))?;

        let root_node = self.symbol_to_graph_node(root_sym)?;

        let mut nodes = vec![root_node.clone()];
        let mut edges = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(root_sym.id);

        let mut queue: VecDeque<(i64, usize)> = VecDeque::new();
        queue.push_back((root_sym.id, 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            let incoming = self.store.get_incoming_relations(current_id)?;
            for rel in &incoming {
                if rel.kind != RelationKind::Calls {
                    continue;
                }

                let caller_id = rel.source_symbol_id;
                edges.push(GraphEdge {
                    source_id: caller_id,
                    target_id: current_id,
                    kind: RelationKind::CalledBy,
                });

                if visited.insert(caller_id) {
                    if let Ok(Some(caller_sym)) = self.store.get_symbol(caller_id) {
                        nodes.push(self.symbol_to_graph_node(&caller_sym)?);
                        queue.push_back((caller_id, depth + 1));
                    }
                }
            }
        }

        Ok(CallGraph {
            root: nodes[0].clone(),
            nodes,
            edges,
        })
    }

    pub fn get_inheritance_tree(&self, class_name: &str) -> Result<CallGraph> {
        let symbols = self.store.find_symbols_by_name(class_name)?;
        let root_sym = symbols
            .first()
            .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found", class_name))?;

        let root_node = self.symbol_to_graph_node(root_sym)?;

        let mut nodes = vec![root_node.clone()];
        let mut edges = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(root_sym.id);

        let inheritance_rels = self
            .store
            .get_relations_by_kind(RelationKind::Inherits)?;
        let implements_rels = self
            .store
            .get_relations_by_kind(RelationKind::Implements)?;

        for rel in inheritance_rels
            .iter()
            .chain(implements_rels.iter())
        {
            let targets_us = rel.target_symbol_name == class_name
                || rel.target_symbol_id == Some(root_sym.id);

            if targets_us {
                let child_id = rel.source_symbol_id;
                edges.push(GraphEdge {
                    source_id: child_id,
                    target_id: root_sym.id,
                    kind: rel.kind,
                });

                if visited.insert(child_id) {
                    if let Ok(Some(child_sym)) = self.store.get_symbol(child_id) {
                        nodes.push(self.symbol_to_graph_node(&child_sym)?);
                    }
                }
            }
        }

        let outgoing = self.store.get_outgoing_relations(root_sym.id)?;
        for rel in &outgoing {
            if rel.kind == RelationKind::Inherits || rel.kind == RelationKind::Implements {
                if let Some(parent_id) = rel.target_symbol_id {
                    edges.push(GraphEdge {
                        source_id: root_sym.id,
                        target_id: parent_id,
                        kind: rel.kind,
                    });

                    if visited.insert(parent_id) {
                        if let Ok(Some(parent_sym)) = self.store.get_symbol(parent_id) {
                            nodes.push(self.symbol_to_graph_node(&parent_sym)?);
                        }
                    }
                }
            }
        }

        Ok(CallGraph {
            root: nodes[0].clone(),
            nodes,
            edges,
        })
    }

    pub fn get_project_structure(&self) -> Result<ProjectStructure> {
        let files = self.store.list_files()?;
        let stats = self.store.get_stats()?;

        let mut dir_map: HashMap<String, Vec<String>> = HashMap::new();
        for f in &files {
            let dir = std::path::Path::new(&f.relative_path)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            dir_map
                .entry(if dir.is_empty() {
                    ".".to_string()
                } else {
                    dir
                })
                .or_default()
                .push(f.relative_path.clone());
        }

        let mut kind_counts: HashMap<String, usize> = HashMap::new();
        for kind_str in &[
            "function", "method", "class", "struct", "interface", "trait", "enum",
        ] {
            let kind = SymbolKind::from_str(kind_str);
            let syms = self.store.get_symbols_by_kind(kind)?;
            if !syms.is_empty() {
                kind_counts.insert(kind_str.to_string(), syms.len());
            }
        }

        let mut lang_counts: HashMap<String, usize> = HashMap::new();
        for f in &files {
            *lang_counts
                .entry(f.language.as_str().to_string())
                .or_default() += 1;
        }

        Ok(ProjectStructure {
            stats,
            directories: dir_map,
            symbol_kinds: kind_counts,
            languages: lang_counts,
        })
    }

    pub fn find_implementations(&self, name: &str) -> Result<Vec<GraphNode>> {
        let implements = self
            .store
            .get_relations_by_kind(RelationKind::Implements)?;

        let mut results = Vec::new();
        for rel in &implements {
            if rel.target_symbol_name == name {
                if let Ok(Some(sym)) = self.store.get_symbol(rel.source_symbol_id) {
                    results.push(self.symbol_to_graph_node(&sym)?);
                }
            }
        }
        Ok(results)
    }

    fn symbol_to_graph_node(&self, sym: &Symbol) -> Result<GraphNode> {
        let file_path = self
            .store
            .get_file_path(sym.file_id)
            .ok()
            .flatten()
            .unwrap_or_default();

        Ok(GraphNode {
            symbol_id: sym.id,
            name: sym.name.clone(),
            qualified_name: sym.qualified_name.clone(),
            kind: sym.kind,
            file_path,
            line: sym.start_line,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectStructure {
    pub stats: IndexStats,
    pub directories: HashMap<String, Vec<String>>,
    pub symbol_kinds: HashMap<String, usize>,
    pub languages: HashMap<String, usize>,
}

