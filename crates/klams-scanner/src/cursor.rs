//! Local `SQLite` cursor for the scanner — tracks the content hash and
//! mtime of every previously-indexed file so re-runs only re-embed
//! changed files. Single-table schema, one row per absolute path.
//! Each upsert/delete is its own transaction so a mid-walk crash
//! leaves prior progress intact (FR-005).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCursor {
    pub absolute_path: String,
    pub content_hash: String,
    pub mtime_ns: i64,
    pub last_indexed_at: i64,
}

pub struct Cursor {
    conn: Connection,
}

impl std::fmt::Debug for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cursor").finish_non_exhaustive()
    }
}

impl Cursor {
    /// Open (or create) the sqlite db at `path`. Parent directory
    /// must already exist; the caller is responsible for that.
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite {}", path.display()))?;
        conn.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS file_cursor (
                absolute_path   TEXT PRIMARY KEY,
                content_hash    TEXT NOT NULL,
                mtime_ns        INTEGER NOT NULL,
                last_indexed_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS file_cursor_last_indexed_idx
              ON file_cursor (last_indexed_at);
            ",
        )?;
        Ok(Self { conn })
    }

    pub fn upsert(
        &self,
        absolute_path: &str,
        content_hash: &str,
        mtime_ns: i64,
        last_indexed_at: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO file_cursor (absolute_path, content_hash, mtime_ns, last_indexed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(absolute_path) DO UPDATE SET
                   content_hash    = excluded.content_hash,
                   mtime_ns        = excluded.mtime_ns,
                   last_indexed_at = excluded.last_indexed_at",
                params![absolute_path, content_hash, mtime_ns, last_indexed_at],
            )
            .context("upsert file_cursor")?;
        Ok(())
    }

    pub fn get(&self, absolute_path: &str) -> Result<Option<FileCursor>> {
        Ok(self
            .conn
            .query_row(
                "SELECT absolute_path, content_hash, mtime_ns, last_indexed_at
                 FROM file_cursor WHERE absolute_path = ?1",
                params![absolute_path],
                |r| {
                    Ok(FileCursor {
                        absolute_path: r.get(0)?,
                        content_hash: r.get(1)?,
                        mtime_ns: r.get(2)?,
                        last_indexed_at: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn delete(&self, absolute_path: &str) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "DELETE FROM file_cursor WHERE absolute_path = ?1",
                params![absolute_path],
            )
            .context("delete file_cursor")?;
        Ok(n > 0)
    }

    pub fn list_all(&self) -> Result<Vec<FileCursor>> {
        let mut stmt = self.conn.prepare(
            "SELECT absolute_path, content_hash, mtime_ns, last_indexed_at FROM file_cursor",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(FileCursor {
                absolute_path: r.get(0)?,
                content_hash: r.get(1)?,
                mtime_ns: r.get(2)?,
                last_indexed_at: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open(dir: &TempDir) -> Cursor {
        Cursor::open(&dir.path().join("c.sqlite")).expect("open")
    }

    #[test]
    fn upsert_then_get_roundtrips() {
        let d = TempDir::new().unwrap();
        let c = open(&d);
        c.upsert("/a/b.md", "deadbeef", 12345, 999).unwrap();
        let got = c.get("/a/b.md").unwrap().expect("row");
        assert_eq!(got.content_hash, "deadbeef");
        assert_eq!(got.mtime_ns, 12345);
        assert_eq!(got.last_indexed_at, 999);
    }

    #[test]
    fn upsert_updates_existing_row() {
        let d = TempDir::new().unwrap();
        let c = open(&d);
        c.upsert("/a/b.md", "v1", 1, 1).unwrap();
        c.upsert("/a/b.md", "v2", 2, 2).unwrap();
        let got = c.get("/a/b.md").unwrap().unwrap();
        assert_eq!(got.content_hash, "v2");
        assert_eq!(got.mtime_ns, 2);
    }

    #[test]
    fn delete_removes_row() {
        let d = TempDir::new().unwrap();
        let c = open(&d);
        c.upsert("/a/b.md", "h", 1, 1).unwrap();
        assert!(c.delete("/a/b.md").unwrap());
        assert!(c.get("/a/b.md").unwrap().is_none());
        assert!(!c.delete("/a/b.md").unwrap(), "second delete is a no-op");
    }

    #[test]
    fn partial_commit_survives_reopen() {
        let d = TempDir::new().unwrap();
        let p = d.path().join("c.sqlite");
        {
            let c = Cursor::open(&p).unwrap();
            c.upsert("/x/1.md", "a", 1, 1).unwrap();
            c.upsert("/x/2.md", "b", 2, 2).unwrap();
            // Simulate crash: drop without further work.
        }
        let c = Cursor::open(&p).unwrap();
        assert_eq!(c.list_all().unwrap().len(), 2, "prior writes survived");
    }

    #[test]
    fn list_all_returns_every_row() {
        let d = TempDir::new().unwrap();
        let c = open(&d);
        for i in 0..5 {
            c.upsert(&format!("/x/{i}"), "h", i, i).unwrap();
        }
        let rows = c.list_all().unwrap();
        assert_eq!(rows.len(), 5);
    }
}
