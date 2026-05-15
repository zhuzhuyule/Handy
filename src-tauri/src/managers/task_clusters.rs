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
            apps: serde_json::from_str(&apps_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            source_history_ids: serde_json::from_str(&source_ids_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            total_duration_ms: row.get("total_duration_ms")?,
            entry_count: row.get("entry_count")?,
            summary: row.get("summary")?,
            blockers: serde_json::from_str(&blockers_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            next_step: row.get("next_step")?,
            keywords: serde_json::from_str(&keywords_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
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

    pub fn delete(&self, cluster_id: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM task_clusters WHERE id=?", [cluster_id])?;
        Ok(())
    }

    pub fn delete_unmodified_for_date(&self, date: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "DELETE FROM task_clusters WHERE date=? AND is_user_modified=0",
            [date],
        )?;
        Ok(())
    }

    pub fn get_protected_clusters_for_date(&self, date: &str) -> Result<Vec<TaskCluster>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM task_clusters WHERE date=? AND is_user_modified=1
             ORDER BY total_duration_ms DESC",
        )?;
        let rows = stmt.query_map([date], Self::row_to_cluster)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Extract `extract_ids` from cluster `cluster_id` into a new cluster `new_title`,
    /// transferring `extracted_duration_ms` of total_duration_ms from original to new.
    pub fn split(
        &self,
        cluster_id: &str,
        extract_ids: &[i64],
        new_title: &str,
        extracted_duration_ms: i64,
    ) -> Result<String> {
        if extract_ids.is_empty() {
            anyhow::bail!("extract_ids cannot be empty");
        }
        let mut original = self
            .get_by_id(cluster_id)?
            .ok_or_else(|| anyhow::anyhow!("cluster {} not found", cluster_id))?;
        let extract_set: std::collections::HashSet<i64> = extract_ids.iter().copied().collect();
        for id in extract_ids {
            if !original.source_history_ids.contains(id) {
                anyhow::bail!("id {} not in cluster's source_history_ids", id);
            }
        }
        if extract_set.len() == original.source_history_ids.len() {
            anyhow::bail!("cannot extract all source_history_ids — would leave empty cluster");
        }

        let now = chrono::Utc::now().timestamp_millis();
        let extracted_ids: Vec<i64> = extract_ids.to_vec();
        let remaining_ids: Vec<i64> = original
            .source_history_ids
            .iter()
            .filter(|id| !extract_set.contains(id))
            .copied()
            .collect();

        let new_id = Self::new_cluster_id();
        let new_cluster = TaskCluster {
            id: new_id.clone(),
            summary_id: original.summary_id,
            date: original.date.clone(),
            title: new_title.to_string(),
            status: original.status.clone(),
            time_span: original.time_span.clone(),
            apps: original.apps.clone(),
            source_history_ids: extracted_ids,
            total_duration_ms: extracted_duration_ms,
            entry_count: extract_ids.len() as i64,
            summary: None,
            blockers: vec![],
            next_step: None,
            keywords: original.keywords.clone(),
            is_user_modified: true,
            user_modified_fields: vec!["title".into(), "source_history_ids".into()],
            created_at: now,
            updated_at: now,
        };

        original.source_history_ids = remaining_ids;
        original.entry_count = original.source_history_ids.len() as i64;
        original.total_duration_ms -= extracted_duration_ms;
        if original.total_duration_ms < 0 {
            original.total_duration_ms = 0;
        }
        original.is_user_modified = true;
        if !original
            .user_modified_fields
            .contains(&"source_history_ids".to_string())
        {
            original
                .user_modified_fields
                .push("source_history_ids".into());
        }
        original.updated_at = now;

        self.upsert(&original)?;
        self.upsert(&new_cluster)?;
        Ok(new_id)
    }

    /// Merge `source_cluster_ids` into `target_cluster_id`. Target keeps its title.
    pub fn merge(&self, target_cluster_id: &str, source_cluster_ids: &[String]) -> Result<()> {
        if source_cluster_ids.is_empty() {
            anyhow::bail!("source_cluster_ids cannot be empty");
        }
        if source_cluster_ids.iter().any(|s| s == target_cluster_id) {
            anyhow::bail!("cannot merge cluster into itself");
        }
        let mut target = self
            .get_by_id(target_cluster_id)?
            .ok_or_else(|| anyhow::anyhow!("target {} not found", target_cluster_id))?;
        let now = chrono::Utc::now().timestamp_millis();

        let mut merged_ids = target.source_history_ids.clone();
        let mut merged_apps = target.apps.clone();
        let mut merged_keywords = target.keywords.clone();
        let mut merged_blockers = target.blockers.clone();
        let mut added_duration = 0_i64;

        for sid in source_cluster_ids {
            let s = self
                .get_by_id(sid)?
                .ok_or_else(|| anyhow::anyhow!("source {} not found", sid))?;
            for id in &s.source_history_ids {
                if !merged_ids.contains(id) {
                    merged_ids.push(*id);
                }
            }
            for app in &s.apps {
                if !merged_apps.contains(app) {
                    merged_apps.push(app.clone());
                }
            }
            for kw in &s.keywords {
                if !merged_keywords.contains(kw) {
                    merged_keywords.push(kw.clone());
                }
            }
            for bl in &s.blockers {
                if !merged_blockers.contains(bl) {
                    merged_blockers.push(bl.clone());
                }
            }
            added_duration += s.total_duration_ms;
            self.delete(sid)?;
        }

        target.source_history_ids = merged_ids;
        target.entry_count = target.source_history_ids.len() as i64;
        target.total_duration_ms += added_duration;
        target.apps = merged_apps;
        target.keywords = merged_keywords;
        target.blockers = merged_blockers;
        target.is_user_modified = true;
        if !target.user_modified_fields.contains(&"merged".to_string()) {
            target.user_modified_fields.push("merged".into());
        }
        target.updated_at = now;
        self.upsert(&target)?;
        Ok(())
    }

    /// Called when a transcription_history row is deleted. Removes the id from all
    /// clusters' source_history_ids and decrements their counts/durations.
    /// Does NOT flip is_user_modified.
    pub fn remove_history_id_from_all_clusters(
        &self,
        history_id: i64,
        history_duration_ms: i64,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, source_history_ids_json, total_duration_ms, entry_count
             FROM task_clusters
             WHERE source_history_ids_json LIKE ?",
        )?;
        let pattern = format!("%{}%", history_id);
        let mut rows = stmt.query([pattern])?;

        struct UpdateRow {
            id: String,
            new_ids: Vec<i64>,
            new_duration: i64,
            new_count: i64,
        }
        let mut updates: Vec<UpdateRow> = Vec::new();

        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let json: String = row.get(1)?;
            let duration: i64 = row.get(2)?;
            let _count: i64 = row.get(3)?;
            let ids: Vec<i64> = serde_json::from_str(&json).unwrap_or_default();
            if !ids.contains(&history_id) {
                continue;
            }
            let new_ids: Vec<i64> = ids.into_iter().filter(|id| *id != history_id).collect();
            let new_count = new_ids.len() as i64;
            let new_duration = (duration - history_duration_ms).max(0);
            updates.push(UpdateRow {
                id,
                new_ids,
                new_duration,
                new_count,
            });
        }
        drop(rows);
        drop(stmt);

        let now = chrono::Utc::now().timestamp_millis();
        for u in updates {
            conn.execute(
                "UPDATE task_clusters
                 SET source_history_ids_json=?, total_duration_ms=?, entry_count=?, updated_at=?
                 WHERE id=?",
                rusqlite::params![
                    serde_json::to_string(&u.new_ids)?,
                    u.new_duration,
                    u.new_count,
                    now,
                    u.id,
                ],
            )?;
        }
        Ok(())
    }

    pub fn update_field(&self, cluster_id: &str, field: &str, value: &str) -> Result<()> {
        let allowed = ["title", "status", "next_step"];
        if !allowed.contains(&field) {
            anyhow::bail!("field '{}' is not user-editable", field);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.get_connection()?;
        let current_fields: Option<String> = match conn.query_row(
            "SELECT user_modified_fields FROM task_clusters WHERE id=?",
            [cluster_id],
            |row| row.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
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

    #[test]
    fn test_upsert_twice_preserves_created_at_and_no_duplicate() {
        let (_tmp, m) = setup_db();
        let mut c = make_cluster("2026-05-15", 1);
        let original_created_at = c.created_at;
        m.upsert(&c).unwrap();

        // Second upsert with same id but different content + later updated_at
        c.title = "Updated title".into();
        c.updated_at = c.created_at + 1000;
        // Caller is responsible for created_at — but the SQL must not overwrite it on conflict.
        // Simulate a caller that accidentally sends a different created_at: still must not overwrite.
        c.created_at = c.created_at + 999_999;
        m.upsert(&c).unwrap();

        let loaded = m.get_by_id(&c.id).unwrap().unwrap();
        assert_eq!(loaded.title, "Updated title");
        assert_eq!(
            loaded.created_at, original_created_at,
            "created_at must not be overwritten on conflict"
        );
        assert_eq!(loaded.updated_at, original_created_at + 1000);

        // No duplicate rows for the same id
        let all = m.get_by_date("2026-05-15").unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_update_field_dedups_user_modified_fields() {
        let (_tmp, m) = setup_db();
        let c = make_cluster("2026-05-15", 1);
        m.upsert(&c).unwrap();

        m.update_field(&c.id, "title", "First update").unwrap();
        m.update_field(&c.id, "title", "Second update").unwrap();
        m.update_field(&c.id, "status", "完成").unwrap();
        m.update_field(&c.id, "title", "Third update").unwrap();

        let loaded = m.get_by_id(&c.id).unwrap().unwrap();
        assert_eq!(loaded.title, "Third update");
        assert_eq!(loaded.status, "完成");
        // Both fields should appear once each, no duplicates
        let mut sorted = loaded.user_modified_fields.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["status".to_string(), "title".to_string()]);
    }

    #[test]
    fn test_split_extracts_ids_and_marks_both_modified() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.source_history_ids = vec![10, 11, 12, 13, 14, 15, 16, 17];
        a.entry_count = 8;
        a.total_duration_ms = 8000;
        m.upsert(&a).unwrap();

        let new_id = m
            .split(&a.id, &[11, 13, 15], "Extracted task", 3000)
            .unwrap();

        let original = m.get_by_id(&a.id).unwrap().unwrap();
        assert_eq!(original.source_history_ids, vec![10, 12, 14, 16, 17]);
        assert_eq!(original.entry_count, 5);
        assert_eq!(original.total_duration_ms, 5000); // 8000 - 3000
        assert!(original.is_user_modified);

        let extracted = m.get_by_id(&new_id).unwrap().unwrap();
        assert_eq!(extracted.title, "Extracted task");
        assert_eq!(extracted.source_history_ids, vec![11, 13, 15]);
        assert_eq!(extracted.entry_count, 3);
        assert_eq!(extracted.total_duration_ms, 3000);
        assert!(extracted.is_user_modified);
    }

    #[test]
    fn test_split_rejects_ids_not_in_source() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.source_history_ids = vec![10, 11, 12];
        m.upsert(&a).unwrap();
        let result = m.split(&a.id, &[99], "X", 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_split_rejects_empty_and_full() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.source_history_ids = vec![10, 11, 12];
        m.upsert(&a).unwrap();
        assert!(m.split(&a.id, &[], "X", 0).is_err());
        assert!(m.split(&a.id, &[10, 11, 12], "X", 1000).is_err());
    }

    #[test]
    fn test_merge_combines_source_ids_and_deletes_others() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.title = "Target".into();
        a.source_history_ids = vec![10, 11];
        a.entry_count = 2;
        a.total_duration_ms = 2000;
        let mut b = make_cluster("2026-05-15", 1);
        b.title = "Source".into();
        b.source_history_ids = vec![20, 21];
        b.entry_count = 2;
        b.total_duration_ms = 3000;
        m.upsert(&a).unwrap();
        m.upsert(&b).unwrap();

        m.merge(&a.id, &[b.id.clone()]).unwrap();

        let merged = m.get_by_id(&a.id).unwrap().unwrap();
        assert_eq!(merged.title, "Target"); // unchanged
        assert_eq!(merged.source_history_ids, vec![10, 11, 20, 21]);
        assert_eq!(merged.entry_count, 4);
        assert_eq!(merged.total_duration_ms, 5000);
        assert!(merged.is_user_modified);

        assert!(m.get_by_id(&b.id).unwrap().is_none());
    }

    #[test]
    fn test_delete_removes_cluster() {
        let (_tmp, m) = setup_db();
        let c = make_cluster("2026-05-15", 1);
        m.upsert(&c).unwrap();
        m.delete(&c.id).unwrap();
        assert!(m.get_by_id(&c.id).unwrap().is_none());
    }

    #[test]
    fn test_remove_history_id_from_all_clusters() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.source_history_ids = vec![10, 11, 12];
        a.entry_count = 3;
        a.total_duration_ms = 3000;
        let mut b = make_cluster("2026-05-15", 1);
        b.source_history_ids = vec![11, 22];
        b.entry_count = 2;
        b.total_duration_ms = 2000;
        m.upsert(&a).unwrap();
        m.upsert(&b).unwrap();

        // Removing history id 11 with duration 500ms
        m.remove_history_id_from_all_clusters(11, 500).unwrap();

        let a2 = m.get_by_id(&a.id).unwrap().unwrap();
        assert_eq!(a2.source_history_ids, vec![10, 12]);
        assert_eq!(a2.entry_count, 2);
        assert_eq!(a2.total_duration_ms, 2500);
        assert!(!a2.is_user_modified); // system action does not flip flag

        let b2 = m.get_by_id(&b.id).unwrap().unwrap();
        assert_eq!(b2.source_history_ids, vec![22]);
        assert_eq!(b2.entry_count, 1);
        assert_eq!(b2.total_duration_ms, 1500);
    }

    #[test]
    fn test_get_protected_clusters_returns_only_user_modified() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.title = "AI".into();
        let mut b = make_cluster("2026-05-15", 1);
        b.title = "User".into();
        b.is_user_modified = true;
        b.user_modified_fields = vec!["title".into()];
        m.upsert(&a).unwrap();
        m.upsert(&b).unwrap();
        let protected = m.get_protected_clusters_for_date("2026-05-15").unwrap();
        assert_eq!(protected.len(), 1);
        assert_eq!(protected[0].title, "User");
    }

    #[test]
    fn test_delete_unmodified_for_date_preserves_user_modified() {
        let (_tmp, m) = setup_db();
        let mut a = make_cluster("2026-05-15", 1);
        a.title = "AI".into();
        let mut b = make_cluster("2026-05-15", 1);
        b.title = "User".into();
        b.is_user_modified = true;
        m.upsert(&a).unwrap();
        m.upsert(&b).unwrap();
        m.delete_unmodified_for_date("2026-05-15").unwrap();
        let remaining = m.get_by_date("2026-05-15").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "User");
    }
}
