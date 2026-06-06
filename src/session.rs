//! Session persistence (bundled SQLite). Saves the conversation per workspace so
//! `--resume` can pick up where you left off.

use crate::config;
use crate::engine::ChatMessage;
use crate::Result;
use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};

pub struct Store {
    conn: Connection,
}

pub struct SessionMeta {
    pub id: i64,
    pub workspace: String,
    pub updated_at: i64,
    pub turns: i64,
}

impl Store {
    pub fn open() -> Result<Store> {
        let dir = config::config_dir();
        std::fs::create_dir_all(&dir)?;
        let conn = Connection::open(dir.join("sessions.db")).context("opening sessions.db")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workspace TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                title TEXT NOT NULL,
                messages TEXT NOT NULL
            );",
        )?;
        Ok(Store { conn })
    }

    pub fn create(&self, workspace: &str, title: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (workspace, updated_at, title, messages) VALUES (?1, ?2, ?3, '[]')",
            params![workspace, now(), title],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn save(&self, id: i64, messages: &[ChatMessage]) -> Result<()> {
        let json = serde_json::to_string(messages)?;
        self.conn.execute(
            "UPDATE sessions SET messages=?1, updated_at=?2 WHERE id=?3",
            params![json, now(), id],
        )?;
        Ok(())
    }

    /// Most recent session for a workspace, if any.
    pub fn latest_for(&self, workspace: &str) -> Result<Option<(i64, Vec<ChatMessage>)>> {
        let row = self
            .conn
            .prepare("SELECT id, messages FROM sessions WHERE workspace=?1 ORDER BY updated_at DESC LIMIT 1")?
            .query_row(params![workspace], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?;
        Ok(row.map(|(id, json)| (id, serde_json::from_str(&json).unwrap_or_default())))
    }

    pub fn list(&self, limit: usize) -> Result<Vec<SessionMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace, updated_at, messages FROM sessions ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let messages: String = r.get(3)?;
            let turns = messages.matches("\"role\":\"user\"").count() as i64;
            Ok(SessionMeta {
                id: r.get(0)?,
                workspace: r.get(1)?,
                updated_at: r.get(2)?,
                turns,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Human "… ago" from a unix timestamp.
pub fn ago(ts: i64) -> String {
    let d = (now() - ts).max(0);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}
