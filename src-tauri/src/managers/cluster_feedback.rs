use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// SQL statements for creating the cluster_feedback table.
/// These are applied as a single migration in history.rs MIGRATIONS array.
pub const MIGRATION_SQL: &str = "
CREATE TABLE IF NOT EXISTS cluster_feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cluster_id TEXT NOT NULL,
    thumb TEXT NOT NULL,
    note TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cluster_feedback_cluster ON cluster_feedback(cluster_id);
CREATE INDEX IF NOT EXISTS idx_cluster_feedback_created ON cluster_feedback(created_at);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterFeedback {
    pub id: i64,
    pub cluster_id: String,
    pub thumb: String, // "up" | "down"
    pub note: Option<String>,
    pub created_at: i64,
}

pub struct ClusterFeedbackManager {
    db_path: PathBuf,
}

impl ClusterFeedbackManager {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn get_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    pub fn add(&self, cluster_id: &str, thumb: &str, note: Option<&str>) -> Result<i64> {
        if thumb != "up" && thumb != "down" {
            anyhow::bail!("thumb must be 'up' or 'down', got '{}'", thumb);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO cluster_feedback (cluster_id, thumb, note, created_at)
             VALUES (?,?,?,?)",
            rusqlite::params![cluster_id, thumb, note, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_for_cluster(&self, cluster_id: &str) -> Result<Vec<ClusterFeedback>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, cluster_id, thumb, note, created_at
             FROM cluster_feedback
             WHERE cluster_id=?
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([cluster_id], |row| {
            Ok(ClusterFeedback {
                id: row.get(0)?,
                cluster_id: row.get(1)?,
                thumb: row.get(2)?,
                note: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// For prompt injection: last N down-thumb feedbacks with notes within `since_days`.
    pub fn list_recent_negative_with_notes(
        &self,
        since_days: i64,
        limit: i64,
    ) -> Result<Vec<ClusterFeedback>> {
        let cutoff = chrono::Utc::now().timestamp_millis() - since_days * 24 * 3600 * 1000;
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, cluster_id, thumb, note, created_at
             FROM cluster_feedback
             WHERE thumb='down' AND note IS NOT NULL AND note != '' AND created_at >= ?
             ORDER BY created_at DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(rusqlite::params![cutoff, limit], |row| {
            Ok(ClusterFeedback {
                id: row.get(0)?,
                cluster_id: row.get(1)?,
                thumb: row.get(2)?,
                note: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM cluster_feedback WHERE id=?", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> (tempfile::NamedTempFile, ClusterFeedbackManager) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(tmp.path()).unwrap();
        conn.execute_batch(MIGRATION_SQL).unwrap();
        drop(conn);
        let m = ClusterFeedbackManager::new(tmp.path().to_path_buf());
        (tmp, m)
    }

    #[test]
    fn test_add_validates_thumb() {
        let (_tmp, m) = setup_db();
        assert!(m.add("c1", "maybe", None).is_err());
        assert!(m.add("c1", "up", None).is_ok());
        assert!(m.add("c1", "down", Some("hi")).is_ok());
    }

    #[test]
    fn test_list_for_cluster_orders_recent_first() {
        let (_tmp, m) = setup_db();
        m.add("c1", "up", Some("first")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        m.add("c1", "down", Some("second")).unwrap();
        let list = m.list_for_cluster("c1").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].note.as_deref(), Some("second"));
    }

    #[test]
    fn test_recent_negative_excludes_up_and_empty_note() {
        let (_tmp, m) = setup_db();
        m.add("c1", "up", Some("good")).unwrap();
        m.add("c2", "down", None).unwrap();
        m.add("c3", "down", Some("")).unwrap();
        m.add("c4", "down", Some("real note")).unwrap();
        let list = m.list_recent_negative_with_notes(30, 5).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].cluster_id, "c4");
    }

    #[test]
    fn test_recent_negative_respects_limit() {
        let (_tmp, m) = setup_db();
        for i in 0..10 {
            m.add(&format!("c{}", i), "down", Some(&format!("n{}", i)))
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let list = m.list_recent_negative_with_notes(30, 3).unwrap();
        assert_eq!(list.len(), 3);
    }
}
