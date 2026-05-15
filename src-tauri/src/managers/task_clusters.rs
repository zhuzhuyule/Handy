use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// SQL statements for creating the task_clusters table and its indexes.
/// These are applied as a single migration in history.rs MIGRATIONS array.
pub const MIGRATION_SQL: &str = "CREATE TABLE IF NOT EXISTS task_clusters (
    id TEXT PRIMARY KEY,
    summary_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    time_span TEXT,
    apps_json TEXT NOT NULL,
    source_history_ids_json TEXT NOT NULL,
    total_duration_ms INTEGER NOT NULL,
    entry_count INTEGER NOT NULL,
    summary TEXT,
    blockers_json TEXT NOT NULL,
    next_step TEXT,
    keywords_json TEXT NOT NULL,
    is_user_modified INTEGER NOT NULL DEFAULT 0,
    user_modified_fields TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_clusters_date ON task_clusters(date);
CREATE INDEX IF NOT EXISTS idx_task_clusters_summary ON task_clusters(summary_id);";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCluster {
    pub id: String,
    pub summary_id: i64,
    pub date: String,
    pub title: String,
    pub status: String,
    pub time_span: Option<String>,
    pub apps: Vec<String>,
    pub source_history_ids: Vec<i64>,
    pub total_duration_ms: i64,
    pub entry_count: i64,
    pub summary: Option<String>,
    pub blockers: Vec<String>,
    pub next_step: Option<String>,
    pub keywords: Vec<String>,
    pub is_user_modified: bool,
    pub user_modified_fields: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct TaskClustersManager {
    db_path: PathBuf,
}

impl TaskClustersManager {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn get_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    pub fn new_cluster_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn row_to_cluster(row: &rusqlite::Row) -> rusqlite::Result<TaskCluster> {
        let apps_json: String = row.get("apps_json")?;
        let source_ids_json: String = row.get("source_history_ids_json")?;
        let blockers_json: String = row.get("blockers_json")?;
        let keywords_json: String = row.get("keywords_json")?;
        let user_modified_fields_json: Option<String> = row.get("user_modified_fields")?;
        let is_user_modified_int: i64 = row.get("is_user_modified")?;
        Ok(TaskCluster {
            id: row.get("id")?,
            summary_id: row.get("summary_id")?,
            date: row.get("date")?,
            title: row.get("title")?,
            status: row.get("status")?,
            time_span: row.get("time_span")?,
            apps: serde_json::from_str(&apps_json).unwrap_or_default(),
            source_history_ids: serde_json::from_str(&source_ids_json).unwrap_or_default(),
            total_duration_ms: row.get("total_duration_ms")?,
            entry_count: row.get("entry_count")?,
            summary: row.get("summary")?,
            blockers: serde_json::from_str(&blockers_json).unwrap_or_default(),
            next_step: row.get("next_step")?,
            keywords: serde_json::from_str(&keywords_json).unwrap_or_default(),
            is_user_modified: is_user_modified_int != 0,
            user_modified_fields: user_modified_fields_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    pub fn upsert(&self, c: &TaskCluster) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO task_clusters (
                id, summary_id, date, title, status, time_span,
                apps_json, source_history_ids_json, total_duration_ms, entry_count,
                summary, blockers_json, next_step, keywords_json,
                is_user_modified, user_modified_fields, created_at, updated_at
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET
                summary_id=excluded.summary_id,
                date=excluded.date,
                title=excluded.title,
                status=excluded.status,
                time_span=excluded.time_span,
                apps_json=excluded.apps_json,
                source_history_ids_json=excluded.source_history_ids_json,
                total_duration_ms=excluded.total_duration_ms,
                entry_count=excluded.entry_count,
                summary=excluded.summary,
                blockers_json=excluded.blockers_json,
                next_step=excluded.next_step,
                keywords_json=excluded.keywords_json,
                is_user_modified=excluded.is_user_modified,
                user_modified_fields=excluded.user_modified_fields,
                updated_at=excluded.updated_at",
            rusqlite::params![
                c.id,
                c.summary_id,
                c.date,
                c.title,
                c.status,
                c.time_span,
                serde_json::to_string(&c.apps)?,
                serde_json::to_string(&c.source_history_ids)?,
                c.total_duration_ms,
                c.entry_count,
                c.summary,
                serde_json::to_string(&c.blockers)?,
                c.next_step,
                serde_json::to_string(&c.keywords)?,
                if c.is_user_modified { 1_i64 } else { 0_i64 },
                serde_json::to_string(&c.user_modified_fields)?,
                c.created_at,
                c.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_by_id(&self, id: &str) -> Result<Option<TaskCluster>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare("SELECT * FROM task_clusters WHERE id=?")?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::row_to_cluster(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_date(&self, date: &str) -> Result<Vec<TaskCluster>> {
        let conn = self.get_connection()?;
        let mut stmt = conn
            .prepare("SELECT * FROM task_clusters WHERE date=? ORDER BY total_duration_ms DESC")?;
        let rows = stmt.query_map([date], Self::row_to_cluster)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn update_field(&self, cluster_id: &str, field: &str, value: &str) -> Result<()> {
        let allowed = ["title", "status", "next_step"];
        if !allowed.contains(&field) {
            anyhow::bail!("field '{}' is not user-editable", field);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.get_connection()?;
        let current_fields: Option<String> = conn
            .query_row(
                "SELECT user_modified_fields FROM task_clusters WHERE id=?",
                [cluster_id],
                |row| row.get(0),
            )
            .ok();
        let mut fields: Vec<String> = current_fields
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        if !fields.contains(&field.to_string()) {
            fields.push(field.to_string());
        }
        let fields_json = serde_json::to_string(&fields)?;
        let sql = format!(
            "UPDATE task_clusters SET {field}=?, is_user_modified=1, user_modified_fields=?, updated_at=? WHERE id=?"
        );
        let rows = conn.execute(&sql, rusqlite::params![value, fields_json, now, cluster_id])?;
        if rows == 0 {
            anyhow::bail!("cluster id {} not found", cluster_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> (tempfile::NamedTempFile, TaskClustersManager) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(MIGRATION_SQL)
            .expect("MIGRATION_SQL should apply cleanly");
        drop(conn);
        let m = TaskClustersManager::new(tmp.path().to_path_buf());
        (tmp, m)
    }

    fn make_cluster(date: &str, summary_id: i64) -> TaskCluster {
        let now = chrono::Utc::now().timestamp_millis();
        TaskCluster {
            id: TaskClustersManager::new_cluster_id(),
            summary_id,
            date: date.to_string(),
            title: "OAuth debugging".to_string(),
            status: "进行中".to_string(),
            time_span: Some("09:15-10:27".to_string()),
            apps: vec!["Cursor".to_string(), "Slack".to_string()],
            source_history_ids: vec![10, 11, 12],
            total_duration_ms: 4_320_000,
            entry_count: 3,
            summary: Some("OAuth token refresh investigation".to_string()),
            blockers: vec!["401 error".to_string()],
            next_step: Some("Check Auth0 dashboard".to_string()),
            keywords: vec!["oauth".to_string(), "refresh-token".to_string()],
            is_user_modified: false,
            user_modified_fields: vec![],
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn test_migration_sql_creates_table_when_applied() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(MIGRATION_SQL)
            .expect("MIGRATION_SQL should apply cleanly");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_clusters'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_migration_sql_creates_indexes() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(MIGRATION_SQL)
            .expect("MIGRATION_SQL should apply cleanly");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('idx_task_clusters_date','idx_task_clusters_summary')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_new_cluster_id_unique() {
        let a = TaskClustersManager::new_cluster_id();
        let b = TaskClustersManager::new_cluster_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36); // UUID v4 length
    }

    #[test]
    fn test_upsert_and_get_by_date() {
        let (_tmp, m) = setup_db();
        let c = make_cluster("2026-05-15", 1);
        m.upsert(&c).unwrap();
        let loaded = m.get_by_date("2026-05-15").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, c.id);
        assert_eq!(loaded[0].title, "OAuth debugging");
        assert_eq!(loaded[0].source_history_ids, vec![10, 11, 12]);
    }

    #[test]
    fn test_get_by_date_returns_empty_for_unknown_date() {
        let (_tmp, m) = setup_db();
        let result = m.get_by_date("2099-01-01").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_update_field_marks_user_modified() {
        let (_tmp, m) = setup_db();
        let c = make_cluster("2026-05-15", 1);
        m.upsert(&c).unwrap();
        m.update_field(&c.id, "title", "OAuth 调试").unwrap();
        let loaded = m.get_by_id(&c.id).unwrap().unwrap();
        assert_eq!(loaded.title, "OAuth 调试");
        assert!(loaded.is_user_modified);
        assert_eq!(loaded.user_modified_fields, vec!["title".to_string()]);
    }

    #[test]
    fn test_update_field_rejects_unknown_field() {
        let (_tmp, m) = setup_db();
        let c = make_cluster("2026-05-15", 1);
        m.upsert(&c).unwrap();
        let result = m.update_field(&c.id, "summary_id", "999");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_by_date_order_by_duration_desc() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.title = "Short".into();
        a.total_duration_ms = 60_000;
        let mut b = make_cluster("2026-05-15", 1);
        b.title = "Long".into();
        b.total_duration_ms = 600_000;
        m.upsert(&a).unwrap();
        m.upsert(&b).unwrap();
        let loaded = m.get_by_date("2026-05-15").unwrap();
        assert_eq!(loaded[0].title, "Long");
        assert_eq!(loaded[1].title, "Short");
    }
}
