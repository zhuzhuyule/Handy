use anyhow::Result;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE IF NOT EXISTS task_clusters (
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
            user_modified_fields TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
         CREATE INDEX IF NOT EXISTS idx_task_clusters_date ON task_clusters(date);
         CREATE INDEX IF NOT EXISTS idx_task_clusters_summary ON task_clusters(summary_id);",
)];

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
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let manager = Self { db_path };
        manager.init_database()?;
        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        let mut conn = Connection::open(&self.db_path)?;
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        #[cfg(debug_assertions)]
        migrations
            .validate()
            .expect("Invalid task_clusters migrations");
        migrations.to_latest(&mut conn)?;
        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    pub fn new_cluster_id() -> String {
        Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_validate() {
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        migrations.validate().expect("Migrations should validate");
    }

    #[test]
    fn test_init_database_creates_table() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let manager = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
        let conn = manager.get_connection().unwrap();
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
    fn test_new_cluster_id_unique() {
        let a = TaskClustersManager::new_cluster_id();
        let b = TaskClustersManager::new_cluster_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36); // UUID v4 length
    }
}
