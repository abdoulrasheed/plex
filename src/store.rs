use crate::types::*;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA cache_size   = -64000;

CREATE TABLE IF NOT EXISTS files (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    path          TEXT    NOT NULL UNIQUE,
    relative_path TEXT    NOT NULL,
    language      TEXT    NOT NULL,
    content_hash  TEXT    NOT NULL,
    size_bytes    INTEGER NOT NULL,
    last_indexed  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS symbols (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id          INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    name             TEXT    NOT NULL,
    qualified_name   TEXT    NOT NULL,
    kind             TEXT    NOT NULL,
    start_line       INTEGER NOT NULL,
    end_line         INTEGER NOT NULL,
    start_col        INTEGER NOT NULL,
    end_col          INTEGER NOT NULL,
    signature        TEXT,
    doc_comment      TEXT,
    body_snippet     TEXT,
    parent_symbol_id INTEGER REFERENCES symbols(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS relations (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    source_symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    target_symbol_name TEXT   NOT NULL,
    target_symbol_id  INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
    kind              TEXT    NOT NULL,
    file_id           INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    line              INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS embeddings (
    symbol_id INTEGER PRIMARY KEY REFERENCES symbols(id) ON DELETE CASCADE,
    vector    BLOB    NOT NULL
);

-- Indexes for fast lookups
CREATE INDEX IF NOT EXISTS idx_symbols_file        ON symbols(file_id);
CREATE INDEX IF NOT EXISTS idx_symbols_name        ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_kind        ON symbols(kind);
CREATE INDEX IF NOT EXISTS idx_symbols_qname       ON symbols(qualified_name);
CREATE INDEX IF NOT EXISTS idx_relations_source    ON relations(source_symbol_id);
CREATE INDEX IF NOT EXISTS idx_relations_target    ON relations(target_symbol_id);
CREATE INDEX IF NOT EXISTS idx_relations_tname     ON relations(target_symbol_name);
CREATE INDEX IF NOT EXISTS idx_relations_kind      ON relations(kind);
CREATE INDEX IF NOT EXISTS idx_files_hash          ON files(content_hash);

-- Full-text search on symbol metadata
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
    name, qualified_name, signature, doc_comment, body_snippet,
    content='symbols',
    content_rowid='id'
);
"#;

const FTS_TRIGGERS: &str = r#"
CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
    INSERT INTO symbols_fts(rowid, name, qualified_name, signature, doc_comment, body_snippet)
    VALUES (new.id, new.name, new.qualified_name, new.signature, new.doc_comment, new.body_snippet);
END;

CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, qualified_name, signature, doc_comment, body_snippet)
    VALUES('delete', old.id, old.name, old.qualified_name, old.signature, old.doc_comment, old.body_snippet);
END;

CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, qualified_name, signature, doc_comment, body_snippet)
    VALUES('delete', old.id, old.name, old.qualified_name, old.signature, old.doc_comment, old.body_snippet);
    INSERT INTO symbols_fts(rowid, name, qualified_name, signature, doc_comment, body_snippet)
    VALUES (new.id, new.name, new.qualified_name, new.signature, new.doc_comment, new.body_snippet);
END;
"#;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) the database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open database at {}", db_path.display()))?;
        let store = Store { conn };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        self.conn.execute_batch(FTS_TRIGGERS)?;
        Ok(())
    }

    /// Insert or update a source file. Returns the file id.
    pub fn upsert_file(&self, file: &SourceFile) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO files (path, relative_path, language, content_hash, size_bytes, last_indexed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(path) DO UPDATE SET
                content_hash = excluded.content_hash,
                size_bytes   = excluded.size_bytes,
                last_indexed = excluded.last_indexed",
            params![
                file.path,
                file.relative_path,
                file.language.as_str(),
                file.content_hash,
                file.size_bytes as i64,
                file.last_indexed,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Lookup a file by its absolute path.
    pub fn get_file_by_path(&self, path: &str) -> Result<Option<SourceFile>> {
        self.conn
            .query_row(
                "SELECT id, path, relative_path, language, content_hash, size_bytes, last_indexed
                 FROM files WHERE path = ?1",
                params![path],
                |row| {
                    Ok(SourceFile {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        relative_path: row.get(2)?,
                        language: Language::from_str(&row.get::<_, String>(3)?),
                        content_hash: row.get(4)?,
                        size_bytes: row.get::<_, i64>(5)? as u64,
                        last_indexed: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Delete a file and cascade to its symbols / relations.
    pub fn delete_file(&self, file_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE id = ?1", params![file_id])?;
        Ok(())
    }

    /// List all indexed file paths.
    pub fn list_files(&self) -> Result<Vec<SourceFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, relative_path, language, content_hash, size_bytes, last_indexed
             FROM files ORDER BY relative_path",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SourceFile {
                id: row.get(0)?,
                path: row.get(1)?,
                relative_path: row.get(2)?,
                language: Language::from_str(&row.get::<_, String>(3)?),
                content_hash: row.get(4)?,
                size_bytes: row.get::<_, i64>(5)? as u64,
                last_indexed: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Replace all symbols for a file. Returns the new database IDs in order.
    pub fn replace_symbols(&self, file_id: i64, symbols: &[ParsedSymbol]) -> Result<Vec<i64>> {
        // Delete old symbols (cascades to relations & embeddings)
        self.conn
            .execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id])?;

        let mut ids = Vec::with_capacity(symbols.len());
        let mut stmt = self.conn.prepare(
            "INSERT INTO symbols
                (file_id, name, qualified_name, kind, start_line, end_line,
                 start_col, end_col, signature, doc_comment, body_snippet, parent_symbol_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        )?;

        for sym in symbols {
            stmt.execute(params![
                file_id,
                sym.name,
                sym.qualified_name,
                sym.kind.as_str(),
                sym.start_line,
                sym.end_line,
                sym.start_col,
                sym.end_col,
                sym.signature,
                sym.doc_comment,
                sym.body_snippet,
                Option::<i64>::None, // parent resolved later
            ])?;
            ids.push(self.conn.last_insert_rowid());
        }
        Ok(ids)
    }

    /// Get a single symbol by id.
    pub fn get_symbol(&self, id: i64) -> Result<Option<Symbol>> {
        self.conn
            .query_row(
                "SELECT id,file_id,name,qualified_name,kind,start_line,end_line,
                        start_col,end_col,signature,doc_comment,body_snippet,parent_symbol_id
                 FROM symbols WHERE id = ?1",
                params![id],
                |row| Self::row_to_symbol(row),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Find symbols whose name matches a pattern (SQL LIKE).
    pub fn find_symbols_by_name(&self, pattern: &str) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,file_id,name,qualified_name,kind,start_line,end_line,
                    start_col,end_col,signature,doc_comment,body_snippet,parent_symbol_id
             FROM symbols WHERE name LIKE ?1 ORDER BY name LIMIT 100",
        )?;
        let rows = stmt.query_map(params![pattern], |row| Self::row_to_symbol(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all symbols in a file.
    pub fn get_file_symbols(&self, file_id: i64) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,file_id,name,qualified_name,kind,start_line,end_line,
                    start_col,end_col,signature,doc_comment,body_snippet,parent_symbol_id
             FROM symbols WHERE file_id = ?1 ORDER BY start_line",
        )?;
        let rows = stmt.query_map(params![file_id], |row| Self::row_to_symbol(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all symbols of a particular kind.
    pub fn get_symbols_by_kind(&self, kind: SymbolKind) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,file_id,name,qualified_name,kind,start_line,end_line,
                    start_col,end_col,signature,doc_comment,body_snippet,parent_symbol_id
             FROM symbols WHERE kind = ?1 ORDER BY qualified_name",
        )?;
        let rows = stmt.query_map(params![kind.as_str()], |row| Self::row_to_symbol(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn row_to_symbol(row: &rusqlite::Row) -> rusqlite::Result<Symbol> {
        Ok(Symbol {
            id: row.get(0)?,
            file_id: row.get(1)?,
            name: row.get(2)?,
            qualified_name: row.get(3)?,
            kind: SymbolKind::from_str(&row.get::<_, String>(4)?),
            start_line: row.get(5)?,
            end_line: row.get(6)?,
            start_col: row.get(7)?,
            end_col: row.get(8)?,
            signature: row.get(9)?,
            doc_comment: row.get(10)?,
            body_snippet: row.get(11)?,
            parent_symbol_id: row.get(12)?,
        })
    }

    /// Replace all relations originating from symbols in a file.
    pub fn replace_relations(
        &self,
        file_id: i64,
        relations: &[ParsedRelation],
        symbol_ids: &[i64],
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM relations WHERE file_id = ?1",
            params![file_id],
        )?;

        let mut stmt = self.conn.prepare(
            "INSERT INTO relations
                (source_symbol_id, target_symbol_name, target_symbol_id, kind, file_id, line)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )?;

        for rel in relations {
            let source_id = symbol_ids
                .get(rel.source_symbol_index)
                .copied()
                .unwrap_or(0);
            if source_id == 0 {
                continue;
            }
            stmt.execute(params![
                source_id,
                rel.target_name,
                Option::<i64>::None,
                rel.kind.as_str(),
                file_id,
                rel.line,
            ])?;
        }
        Ok(())
    }

    /// Resolve target_symbol_id for unresolved relations (match by name).
    pub fn resolve_relations(&self) -> Result<usize> {
        let updated = self.conn.execute(
            "UPDATE relations SET target_symbol_id = (
                SELECT s.id FROM symbols s WHERE s.name = relations.target_symbol_name LIMIT 1
             )
             WHERE target_symbol_id IS NULL",
            [],
        )?;
        Ok(updated)
    }

    /// Get all relations where source matches a symbol id.
    pub fn get_outgoing_relations(&self, symbol_id: i64) -> Result<Vec<Relation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_symbol_id, target_symbol_name, target_symbol_id, kind, file_id, line
             FROM relations WHERE source_symbol_id = ?1",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| Self::row_to_relation(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all relations where target matches a symbol id.
    pub fn get_incoming_relations(&self, symbol_id: i64) -> Result<Vec<Relation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_symbol_id, target_symbol_name, target_symbol_id, kind, file_id, line
             FROM relations WHERE target_symbol_id = ?1",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| Self::row_to_relation(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all relations of a specific kind.
    pub fn get_relations_by_kind(&self, kind: RelationKind) -> Result<Vec<Relation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_symbol_id, target_symbol_name, target_symbol_id, kind, file_id, line
             FROM relations WHERE kind = ?1",
        )?;
        let rows = stmt.query_map(params![kind.as_str()], |row| Self::row_to_relation(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn row_to_relation(row: &rusqlite::Row) -> rusqlite::Result<Relation> {
        Ok(Relation {
            id: row.get(0)?,
            source_symbol_id: row.get(1)?,
            target_symbol_name: row.get(2)?,
            target_symbol_id: row.get(3)?,
            kind: RelationKind::from_str(&row.get::<_, String>(4)?),
            file_id: row.get(5)?,
            line: row.get(6)?,
        })
    }

    /// Store an embedding vector for a symbol.
    pub fn store_embedding(&self, symbol_id: i64, vector: &[f32]) -> Result<()> {
        let blob = vector_to_blob(vector);
        self.conn.execute(
            "INSERT OR REPLACE INTO embeddings (symbol_id, vector) VALUES (?1, ?2)",
            params![symbol_id, blob],
        )?;
        Ok(())
    }

    /// Load all embeddings into memory for brute-force search.
    /// Returns (symbol_id, vector) pairs.
    pub fn load_all_embeddings(&self) -> Result<Vec<(i64, Vec<f32>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT symbol_id, vector FROM embeddings")?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id, blob_to_vector(&blob)))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Count of embedded symbols.
    pub fn embedding_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))?;
        Ok(count as usize)
    }

    /// Get symbol IDs that don't have embeddings yet.
    pub fn get_unembedded_symbol_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id FROM symbols s
             LEFT JOIN embeddings e ON s.id = e.symbol_id
             WHERE e.symbol_id IS NULL
             ORDER BY s.id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Full-text search using FTS5. Returns (symbol_id, rank).
    pub fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<(i64, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, rank FROM symbols_fts WHERE symbols_fts MATCH ?1
             ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_stats(&self) -> Result<IndexStats> {
        let file_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let symbol_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
        let relation_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))?;
        let embedding_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))?;

        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT language FROM files ORDER BY language")?;
        let languages: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(IndexStats {
            file_count: file_count as usize,
            symbol_count: symbol_count as usize,
            relation_count: relation_count as usize,
            embedding_count: embedding_count as usize,
            languages,
        })
    }

    pub fn begin_transaction(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN TRANSACTION")?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    /// Get the file path for a given file id.
    pub fn get_file_path(&self, file_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT relative_path FROM files WHERE id = ?1",
                params![file_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Full-text search on symbol names using FTS5.
    pub fn text_search(&self, query: &str, limit: usize) -> Result<Vec<Symbol>> {
        // Use FTS5 MATCH for fast text search
        let fts_query = format!("{}*", query.replace('"', ""));
        let mut stmt = self.conn.prepare(
            "SELECT s.id,s.file_id,s.name,s.qualified_name,s.kind,s.start_line,s.end_line,
                    s.start_col,s.end_col,s.signature,s.doc_comment,s.body_snippet,s.parent_symbol_id
             FROM symbols_fts fts
             JOIN symbols s ON s.id = fts.rowid
             WHERE symbols_fts MATCH ?1
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], |row| Self::row_to_symbol(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all symbols in a file by relative path.
    pub fn get_file_symbols_by_path(&self, path: &str) -> Result<Vec<Symbol>> {
        let file_id: Option<i64> = self.conn
            .query_row(
                "SELECT id FROM files WHERE relative_path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()?;

        match file_id {
            Some(id) => self.get_file_symbols(id),
            None => Ok(Vec::new()),
        }
    }

    /// Get file-level dependency edges: returns (source_file_path, target_file_path) pairs.
    /// A dependency means a symbol in source_file references/calls/imports a symbol in target_file.
    pub fn get_file_dependencies(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT
                sf.relative_path as source_file,
                tf.relative_path as target_file,
                r.kind
             FROM relations r
             JOIN symbols ss ON r.source_symbol_id = ss.id
             JOIN files sf ON ss.file_id = sf.id
             JOIN symbols ts ON r.target_symbol_id = ts.id
             JOIN files tf ON ts.file_id = tf.id
             WHERE r.target_symbol_id IS NOT NULL
               AND sf.id != tf.id
               AND r.kind IN ('calls', 'imports', 'references', 'inherits', 'implements')"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get per-file symbol counts grouped by kind.
    pub fn get_file_symbol_counts(&self) -> Result<Vec<(String, String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.relative_path, s.kind, COUNT(*) as cnt
             FROM symbols s
             JOIN files f ON s.file_id = f.id
             GROUP BY f.relative_path, s.kind
             ORDER BY f.relative_path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn vector_to_blob(v: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(v.len() * 4);
    for &val in v {
        buf.extend_from_slice(&val.to_le_bytes());
    }
    buf
}

fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}
