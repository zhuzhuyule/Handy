# Summary Page Cluster-Axis Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the Summary page so that LLM-generated Task Clusters become the page's primary axis, with full edit/feedback/regenerate loop and source linking to transcription history.

**Architecture:** Two new SQLite tables (`task_clusters`, `cluster_feedback`) promote Task Cluster to a first-class entity with UUID. A new prompt-driven LLM generator replaces the heuristic `build_recap()`. Frontend reorganizes around three views (Day/Week/Month) with a folded AUX drawer for legacy stats/recap/profile content. Six atomic user operations (rename / status / next_step / split / merge / delete / thumb±note) flow through new Tauri commands. User-modified clusters are preserved across regeneration via an `is_user_modified` flag; negative feedback notes are injected into the next prompt.

**Tech Stack:** Rust (Tauri 2.x, rusqlite_migration, uuid), TypeScript (React 18, Zustand, Radix UI, Tailwind 4)

**Prerequisites:** Read `docs/specs/2026-05-15-summary-cluster-refactor.spec.md` before starting. All 15 BDD acceptance scenarios in that spec map to verification steps in this plan.

**Worktree note:** This plan was not authored inside a dedicated worktree. Before starting execution, run `git checkout -b feat/summary-cluster-refactor` (or create a worktree via `using-git-worktrees`) so the work stays isolated from `votype`.

**No frontend test framework:** The project has no Vitest/Jest. Frontend tasks rely on `bun tauri dev` + manual verification per the BDD scenarios. Backend tasks use `cargo test` with TDD.

---

## Milestone Map

| M   | Theme                           | Tasks   |
| --- | ------------------------------- | ------- |
| M1  | Backend DB foundation           | T1–T3   |
| M2  | Backend feedback foundation     | T4      |
| M3  | Backend LLM generator           | T5–T7   |
| M4  | Backend commands & registration | T8–T9   |
| M5  | Frontend types & state          | T10–T11 |
| M6  | Frontend cluster components     | T12–T15 |
| M7  | Frontend views & shared         | T16–T17 |
| M8  | AUX panel                       | T18–T20 |
| M9  | Integration & cleanup           | T21–T23 |
| M10 | Manual verification             | T24     |

---

## Milestone 1: Backend DB Foundation

### Task 1: Schema migrations + TaskCluster v2 struct

**Files:**

- Create: `src-tauri/src/managers/task_clusters.rs`
- Modify: `src-tauri/Cargo.toml` (add `uuid = { version = "1", features = ["v4", "serde"] }` if not present)

- [ ] **Step 1: Verify uuid crate availability**

Run: `rg '^uuid' src-tauri/Cargo.toml`
Expected: a `uuid` entry. If absent, add to `[dependencies]`:

```toml
uuid = { version = "1", features = ["v4", "serde"] }
```

Run: `cd src-tauri && cargo check 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 2: Write the migration validation test**

Create `src-tauri/src/managers/task_clusters.rs` with this initial content:

```rust
use anyhow::Result;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

static MIGRATIONS: &[M] = &[
    M::up(
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
    ),
];

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
        migrations.validate().expect("Invalid task_clusters migrations");
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
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_clusters'",
            [],
            |row| row.get(0),
        ).unwrap();
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
```

Make sure `tempfile` is available — if not, add `tempfile = "3"` under `[dev-dependencies]` in `src-tauri/Cargo.toml`.

- [ ] **Step 3: Register the new module**

Modify `src-tauri/src/managers/mod.rs` — add at the end of the existing `pub mod` list (read the file first to find where other managers are declared):

```rust
pub mod task_clusters;
```

- [ ] **Step 4: Run tests to verify they fail / compile errors**

Run: `cd src-tauri && cargo test --lib managers::task_clusters 2>&1 | tail -20`
Expected: Either compilation success and tests pass (if tempfile available), OR a clear error about a missing dep. Fix any dep issue, then re-run.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/managers/task_clusters.rs src-tauri/src/managers/mod.rs
git commit -m "Add task_clusters schema, struct, and migration with tests"
```

---

### Task 2: TaskClustersManager core CRUD

**Files:**

- Modify: `src-tauri/src/managers/task_clusters.rs`

- [ ] **Step 1: Write failing tests for upsert + get_by_date + update_field**

Append to the `#[cfg(test)] mod tests` block at the bottom of `task_clusters.rs`:

```rust
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
fn test_upsert_and_get_by_date() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
    let result = m.get_by_date("2099-01-01").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_update_field_marks_user_modified() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
    let c = make_cluster("2026-05-15", 1);
    m.upsert(&c).unwrap();
    let result = m.update_field(&c.id, "summary_id", "999");
    assert!(result.is_err());
}

#[test]
fn test_get_by_date_order_by_duration_desc() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
```

Make sure `chrono` is in `Cargo.toml` (it almost certainly is — the project already uses it). If not, add: `chrono = "0.4"`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test --lib managers::task_clusters 2>&1 | tail -30`
Expected: compilation errors — `upsert`, `get_by_date`, `update_field`, `get_by_id` not defined.

- [ ] **Step 3: Implement upsert / get_by_date / get_by_id / update_field**

Add inside the `impl TaskClustersManager` block in `src-tauri/src/managers/task_clusters.rs`:

```rust
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
                c.id, c.summary_id, c.date, c.title, c.status, c.time_span,
                serde_json::to_string(&c.apps)?,
                serde_json::to_string(&c.source_history_ids)?,
                c.total_duration_ms, c.entry_count,
                c.summary,
                serde_json::to_string(&c.blockers)?,
                c.next_step,
                serde_json::to_string(&c.keywords)?,
                if c.is_user_modified { 1_i64 } else { 0_i64 },
                serde_json::to_string(&c.user_modified_fields)?,
                c.created_at, c.updated_at,
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
        let mut stmt = conn.prepare(
            "SELECT * FROM task_clusters WHERE date=? ORDER BY total_duration_ms DESC",
        )?;
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
        let rows = conn.execute(
            &sql,
            rusqlite::params![value, fields_json, now, cluster_id],
        )?;
        if rows == 0 {
            anyhow::bail!("cluster id {} not found", cluster_id);
        }
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib managers::task_clusters 2>&1 | tail -30`
Expected: all 5 new tests + 3 from Task 1 PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/managers/task_clusters.rs
git commit -m "Add TaskClustersManager CRUD with upsert/get/update_field"
```

---

### Task 3: TaskClustersManager split / merge / delete + history cascade

**Files:**

- Modify: `src-tauri/src/managers/task_clusters.rs`
- Modify: `src-tauri/src/managers/history.rs` (to call cascade on delete)

- [ ] **Step 1: Write failing tests for split, merge, delete, cascade**

Append to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_split_extracts_ids_and_marks_both_modified() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
    let mut a = make_cluster("2026-05-15", 1);
    a.source_history_ids = vec![10, 11, 12, 13, 14, 15, 16, 17];
    a.entry_count = 8;
    a.total_duration_ms = 8000;
    m.upsert(&a).unwrap();

    let new_id = m.split(&a.id, &[11, 13, 15], "Extracted task", 3000).unwrap();

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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
    let mut a = make_cluster("2026-05-15", 1);
    a.source_history_ids = vec![10, 11, 12];
    m.upsert(&a).unwrap();
    let result = m.split(&a.id, &[99], "X", 100);
    assert!(result.is_err());
}

#[test]
fn test_split_rejects_empty_and_full() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
    let mut a = make_cluster("2026-05-15", 1);
    a.source_history_ids = vec![10, 11, 12];
    m.upsert(&a).unwrap();
    assert!(m.split(&a.id, &[], "X", 0).is_err());
    assert!(m.split(&a.id, &[10, 11, 12], "X", 1000).is_err());
}

#[test]
fn test_merge_combines_source_ids_and_deletes_others() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
    let c = make_cluster("2026-05-15", 1);
    m.upsert(&c).unwrap();
    m.delete(&c.id).unwrap();
    assert!(m.get_by_id(&c.id).unwrap().is_none());
}

#[test]
fn test_remove_history_id_from_all_clusters() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test --lib managers::task_clusters 2>&1 | tail -30`
Expected: compilation errors — `split`, `merge`, `delete`, `remove_history_id_from_all_clusters`, `get_protected_clusters_for_date`, `delete_unmodified_for_date` not defined.

- [ ] **Step 3: Implement split / merge / delete / cascade / protected query**

Append inside the `impl TaskClustersManager` block:

```rust
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
        if !original.user_modified_fields.contains(&"source_history_ids".to_string()) {
            original.user_modified_fields.push("source_history_ids".into());
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

        let now = chrono::Utc::now().timestamp_millis();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let json: String = row.get(1)?;
            let duration: i64 = row.get(2)?;
            let count: i64 = row.get(3)?;
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib managers::task_clusters 2>&1 | tail -40`
Expected: all 16 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/managers/task_clusters.rs
git commit -m "Add split/merge/delete and history cascade to TaskClustersManager"
```

---

## Milestone 2: Backend Feedback Foundation

### Task 4: ClusterFeedbackManager

**Files:**

- Create: `src-tauri/src/managers/cluster_feedback.rs`
- Modify: `src-tauri/src/managers/mod.rs`

- [ ] **Step 1: Scaffold the manager file**

Create `src-tauri/src/managers/cluster_feedback.rs`:

```rust
use anyhow::Result;
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS cluster_feedback (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cluster_id TEXT NOT NULL,
            thumb TEXT NOT NULL,
            note TEXT,
            created_at INTEGER NOT NULL
        );
         CREATE INDEX IF NOT EXISTS idx_cluster_feedback_cluster ON cluster_feedback(cluster_id);
         CREATE INDEX IF NOT EXISTS idx_cluster_feedback_created ON cluster_feedback(created_at);",
    ),
];

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
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let m = Self { db_path };
        m.init_database()?;
        Ok(m)
    }

    fn init_database(&self) -> Result<()> {
        let mut conn = Connection::open(&self.db_path)?;
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid cluster_feedback migrations");
        migrations.to_latest(&mut conn)?;
        Ok(())
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

    #[test]
    fn test_migrations_validate() {
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        migrations.validate().unwrap();
    }

    #[test]
    fn test_add_validates_thumb() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let m = ClusterFeedbackManager::new(tmp.path().to_path_buf()).unwrap();
        assert!(m.add("c1", "maybe", None).is_err());
        assert!(m.add("c1", "up", None).is_ok());
        assert!(m.add("c1", "down", Some("hi")).is_ok());
    }

    #[test]
    fn test_list_for_cluster_orders_recent_first() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let m = ClusterFeedbackManager::new(tmp.path().to_path_buf()).unwrap();
        m.add("c1", "up", Some("first")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        m.add("c1", "down", Some("second")).unwrap();
        let list = m.list_for_cluster("c1").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].note.as_deref(), Some("second"));
    }

    #[test]
    fn test_recent_negative_excludes_up_and_empty_note() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let m = ClusterFeedbackManager::new(tmp.path().to_path_buf()).unwrap();
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
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let m = ClusterFeedbackManager::new(tmp.path().to_path_buf()).unwrap();
        for i in 0..10 {
            m.add(&format!("c{}", i), "down", Some(&format!("n{}", i))).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let list = m.list_recent_negative_with_notes(30, 3).unwrap();
        assert_eq!(list.len(), 3);
    }
}
```

- [ ] **Step 2: Register module**

Modify `src-tauri/src/managers/mod.rs` — add:

```rust
pub mod cluster_feedback;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib managers::cluster_feedback 2>&1 | tail -20`
Expected: all 5 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/managers/cluster_feedback.rs src-tauri/src/managers/mod.rs
git commit -m "Add ClusterFeedbackManager with thumb/note storage and 30d window query"
```

---

## Milestone 3: Backend LLM Generator

### Task 5: Clustering prompt file + frontend types prep

**Files:**

- Create: `src-tauri/resources/prompts/system_task_clustering.md`

- [ ] **Step 1: Write the prompt file**

Create `src-tauri/resources/prompts/system_task_clustering.md`:

````markdown
# Task Clustering Prompt

You are an assistant that clusters a single day's worth of voice transcription entries into coherent **task clusters** representing what the user actually worked on.

## Clustering principles

- A cluster represents a coherent task, project, conversation, or piece of work — not an app or a time window.
- A task can span multiple apps (e.g. coding in Cursor and asking in Slack about the same problem belong together).
- A task can have time gaps (e.g. morning work, interrupted, resumed in the afternoon — still one cluster).
- Short interruptions (lunch chatter, one-off Slack reactions) should not form their own clusters.
- Aim for **3-8 clusters total** for a normal day. Fewer if the day is focused, more only if truly varied.

## Status values (use exactly one)

- `进行中` — actively worked, no clear endpoint reached
- `完成` — concluded, shipped, decided
- `卡住` — blockers detected (errors, "stuck", "broken", waiting on someone)
- `已搁置` — abandoned, switched away with no return

## Output format

Return **only** a JSON array (no prose, no markdown fences) with this exact shape per cluster:

```json
[
  {
    "title": "<short noun phrase>",
    "status": "<one of the four status values>",
    "time_span": "<HH:MM-HH:MM range>",
    "apps": ["<app names>"],
    "source_history_ids": [<int>, <int>, ...],
    "total_duration_ms": <int>,
    "entry_count": <int>,
    "summary": "<2-3 sentences in user's language>",
    "blockers": ["<short blocker phrase>", ...],
    "next_step": "<actionable next step or null>",
    "keywords": ["<lowercase keyword>", ...]
  }
]
```
````

Order clusters by `total_duration_ms` descending.

## Input

DATE: {{date}}

ENTRIES ({{entry_count}}):
{{entries}}

{{#protected_clusters_block}}
PROTECTED CLUSTERS — these source_history_ids belong to user-edited clusters. Do NOT include them in your output. Do NOT regroup them.
{{protected_clusters}}
{{/protected_clusters_block}}

{{#user_feedback_block}}
USER FEEDBACK on recent clustering — apply these corrections to your reasoning:
{{user_feedback}}
{{/user_feedback_block}}

Return the JSON array now.

````

- [ ] **Step 2: Verify the file is included in resources**

Run: `rg -n 'system_lite_polish\|system_smart_routing' src-tauri/tauri.conf.json src-tauri/Cargo.toml 2>&1 | head -10`

If you find an explicit list (e.g. `resources` in `tauri.conf.json`), add `resources/prompts/system_task_clustering.md` there. Otherwise resources are usually copied via a glob and no edit is needed.

Run: `ls src-tauri/resources/prompts/`
Expected: see `system_task_clustering.md` listed.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/resources/prompts/system_task_clustering.md src-tauri/tauri.conf.json
git commit -m "Add system_task_clustering prompt for LLM-based cluster generation"
````

---

### Task 6: task_cluster_generator action — prompt rendering + JSON parsing

**Files:**

- Create: `src-tauri/src/actions/task_cluster_generator.rs`
- Modify: `src-tauri/src/actions/mod.rs`

- [ ] **Step 1: Scaffold module with parse + render helpers and tests**

Create `src-tauri/src/actions/task_cluster_generator.rs`:

````rust
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::managers::task_clusters::TaskCluster;

/// Subset of fields the LLM returns; ids are filled, server-side fields (id, summary_id, etc.) added later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmClusterOutput {
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub time_span: Option<String>,
    #[serde(default)]
    pub apps: Vec<String>,
    pub source_history_ids: Vec<i64>,
    #[serde(default)]
    pub total_duration_ms: i64,
    #[serde(default)]
    pub entry_count: i64,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClusterableEntry {
    pub id: i64,
    pub timestamp_ms: i64,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub text: String,
    pub duration_ms: i64,
}

const ALLOWED_STATUSES: &[&str] = &["进行中", "完成", "卡住", "已搁置"];

/// Render the entries block for the prompt — chooses field strategy based on count.
pub fn render_entries_block(entries: &[ClusterableEntry]) -> String {
    let mut out = String::new();
    let truncate_each = entries.len() > 150;
    for e in entries {
        if truncate_each && e.text.chars().count() < 5 {
            continue;
        }
        let text = if truncate_each {
            e.text.chars().take(200).collect::<String>()
        } else {
            e.text.clone()
        };
        let window = e
            .window_title
            .as_deref()
            .map(|w| w.chars().take(60).collect::<String>())
            .unwrap_or_default();
        let ts = format_hhmm(e.timestamp_ms);
        let app = e.app_name.as_deref().unwrap_or("?");
        out.push_str(&format!(
            "[id={} | {} | {} | {} | \"{}\"]\n",
            e.id, ts, app, window, text
        ));
    }
    out
}

fn format_hhmm(ms: i64) -> String {
    let secs = ms / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    format!("{:02}:{:02}", h, m)
}

/// Render the protected cluster summary block.
pub fn render_protected_block(protected: &[TaskCluster]) -> String {
    let mut out = String::new();
    for c in protected {
        out.push_str(&format!(
            "- \"{}\" ids={:?}\n",
            c.title, c.source_history_ids
        ));
    }
    out
}

/// Render recent negative feedback notes for prompt injection.
pub fn render_feedback_block(
    notes: &[(i64, String)], // (created_at_ms, note)
    now_ms: i64,
) -> String {
    let mut out = String::new();
    for (ts, note) in notes {
        let days_ago = ((now_ms - ts) / (24 * 3600 * 1000)).max(0);
        out.push_str(&format!("- {}d ago: \"{}\"\n", days_ago, note));
    }
    out
}

/// Strip ```json fences and surrounding prose if the model added them.
pub fn extract_json_array(raw: &str) -> &str {
    let trimmed = raw.trim();
    // Try fenced ```json ... ```
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    // Find the first `[` and the last `]`
    let bytes = trimmed.as_bytes();
    let start = bytes.iter().position(|&b| b == b'[');
    let end = bytes.iter().rposition(|&b| b == b']');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            return &trimmed[s..=e];
        }
    }
    trimmed
}

pub fn parse_llm_output(raw: &str) -> Result<Vec<LlmClusterOutput>> {
    let extracted = extract_json_array(raw);
    let parsed: Vec<LlmClusterOutput> = serde_json::from_str(extracted)
        .map_err(|e| anyhow::anyhow!("failed to parse LLM JSON: {}; raw start: {:.120}", e, extracted))?;
    Ok(parsed)
}

/// Drop ids that do not exist in `valid_ids`. Coerce status to fallback if invalid.
pub fn sanitize_outputs(
    outputs: Vec<LlmClusterOutput>,
    valid_ids: &std::collections::HashSet<i64>,
) -> Vec<LlmClusterOutput> {
    outputs
        .into_iter()
        .map(|mut o| {
            o.source_history_ids.retain(|id| valid_ids.contains(id));
            if !ALLOWED_STATUSES.contains(&o.status.as_str()) {
                o.status = "进行中".to_string();
            }
            o.entry_count = o.source_history_ids.len() as i64;
            o
        })
        .filter(|o| !o.source_history_ids.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_array_plain() {
        let raw = "[{\"title\":\"a\"}]";
        assert_eq!(extract_json_array(raw), "[{\"title\":\"a\"}]");
    }

    #[test]
    fn test_extract_json_array_strips_fence() {
        let raw = "```json\n[{\"x\":1}]\n```";
        assert_eq!(extract_json_array(raw), "[{\"x\":1}]");
    }

    #[test]
    fn test_extract_json_array_strips_prose() {
        let raw = "Sure! Here is the result:\n[{\"x\":1}]\nLet me know.";
        assert_eq!(extract_json_array(raw), "[{\"x\":1}]");
    }

    #[test]
    fn test_parse_llm_output_minimal() {
        let raw = r#"[{"title":"OAuth","status":"进行中","source_history_ids":[1,2]}]"#;
        let parsed = parse_llm_output(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "OAuth");
        assert_eq!(parsed[0].source_history_ids, vec![1, 2]);
    }

    #[test]
    fn test_parse_llm_output_invalid_returns_err() {
        let raw = r#"[{"title":"x" missing colon}]"#;
        assert!(parse_llm_output(raw).is_err());
    }

    #[test]
    fn test_sanitize_drops_unknown_ids() {
        let valid: std::collections::HashSet<i64> = [1_i64, 2, 3].into_iter().collect();
        let inp = vec![LlmClusterOutput {
            title: "x".into(), status: "进行中".into(), time_span: None,
            apps: vec![], source_history_ids: vec![1, 99, 2], total_duration_ms: 0,
            entry_count: 3, summary: None, blockers: vec![], next_step: None, keywords: vec![],
        }];
        let out = sanitize_outputs(inp, &valid);
        assert_eq!(out[0].source_history_ids, vec![1, 2]);
        assert_eq!(out[0].entry_count, 2);
    }

    #[test]
    fn test_sanitize_drops_empty_clusters() {
        let valid: std::collections::HashSet<i64> = [1, 2].into_iter().collect();
        let inp = vec![LlmClusterOutput {
            title: "x".into(), status: "进行中".into(), time_span: None,
            apps: vec![], source_history_ids: vec![99], total_duration_ms: 0,
            entry_count: 1, summary: None, blockers: vec![], next_step: None, keywords: vec![],
        }];
        let out = sanitize_outputs(inp, &valid);
        assert!(out.is_empty());
    }

    #[test]
    fn test_sanitize_coerces_bad_status() {
        let valid: std::collections::HashSet<i64> = [1].into_iter().collect();
        let inp = vec![LlmClusterOutput {
            title: "x".into(), status: "weird".into(), time_span: None,
            apps: vec![], source_history_ids: vec![1], total_duration_ms: 0,
            entry_count: 1, summary: None, blockers: vec![], next_step: None, keywords: vec![],
        }];
        let out = sanitize_outputs(inp, &valid);
        assert_eq!(out[0].status, "进行中");
    }

    #[test]
    fn test_render_entries_truncates_when_many() {
        let mut entries = Vec::new();
        for i in 0..200 {
            entries.push(ClusterableEntry {
                id: i,
                timestamp_ms: i * 60_000,
                app_name: Some("App".into()),
                window_title: Some("win".into()),
                text: "x".repeat(500),
                duration_ms: 1000,
            });
        }
        let out = render_entries_block(&entries);
        // each line should contain the truncated text (200 chars max)
        let first_line = out.lines().next().unwrap();
        assert!(first_line.contains(&"x".repeat(200)));
        assert!(!first_line.contains(&"x".repeat(201)));
    }
}
````

- [ ] **Step 2: Register module**

Modify `src-tauri/src/actions/mod.rs` — add:

```rust
pub mod task_cluster_generator;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib actions::task_cluster_generator 2>&1 | tail -20`
Expected: all 9 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/actions/task_cluster_generator.rs src-tauri/src/actions/mod.rs
git commit -m "Add task_cluster_generator action with prompt rendering and JSON parsing"
```

---

### Task 7: generate_task_clusters orchestrator

**Files:**

- Modify: `src-tauri/src/actions/task_cluster_generator.rs`
- Modify: `src-tauri/src/managers/history.rs` (only if needed — add a query helper used by the orchestrator)

- [ ] **Step 1: Add the orchestrator function**

Append to `src-tauri/src/actions/task_cluster_generator.rs`:

```rust
use crate::actions::post_process::core::execute_llm_request_with_retry;
use crate::managers::cluster_feedback::ClusterFeedbackManager;
use crate::managers::prompt::PromptManager;
use crate::managers::task_clusters::TaskClustersManager;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

pub struct GenerateClustersInput {
    pub date: String,
    pub summary_id: i64,
    pub entries: Vec<ClusterableEntry>,
    pub force: bool,
}

pub struct GenerateClustersResult {
    pub clusters: Vec<TaskCluster>,
    pub skipped_reason: Option<String>,
    pub llm_called: bool,
}

const CACHE_TTL_MS: i64 = 60 * 60 * 1000; // 1h
const FEEDBACK_WINDOW_DAYS: i64 = 30;
const FEEDBACK_LIMIT: i64 = 5;

pub async fn generate_task_clusters(
    app_handle: &AppHandle,
    task_clusters_manager: Arc<TaskClustersManager>,
    cluster_feedback_manager: Arc<ClusterFeedbackManager>,
    prompt_manager: Arc<PromptManager>,
    input: GenerateClustersInput,
) -> Result<GenerateClustersResult> {
    let GenerateClustersInput {
        date,
        summary_id,
        entries,
        force,
    } = input;

    // 1. Check cache freshness if !force
    if !force {
        let existing = task_clusters_manager.get_by_date(&date)?;
        if !existing.is_empty() {
            let now = chrono::Utc::now().timestamp_millis();
            let all_fresh = existing.iter().all(|c| now - c.created_at < CACHE_TTL_MS);
            if all_fresh {
                return Ok(GenerateClustersResult {
                    clusters: existing,
                    skipped_reason: Some("cache_hit".into()),
                    llm_called: false,
                });
            }
        }
    }

    // 2. Empty day
    if entries.is_empty() {
        return Ok(GenerateClustersResult {
            clusters: vec![],
            skipped_reason: Some("no_entries".into()),
            llm_called: false,
        });
    }

    // 3. Protected clusters
    let protected = task_clusters_manager.get_protected_clusters_for_date(&date)?;
    let protected_ids: std::collections::HashSet<i64> = protected
        .iter()
        .flat_map(|c| c.source_history_ids.iter().copied())
        .collect();

    let candidate_entries: Vec<ClusterableEntry> = entries
        .iter()
        .filter(|e| !protected_ids.contains(&e.id))
        .cloned()
        .collect();

    if candidate_entries.is_empty() {
        return Ok(GenerateClustersResult {
            clusters: protected,
            skipped_reason: Some("all_protected".into()),
            llm_called: false,
        });
    }

    // 4. Render prompt
    let template = prompt_manager
        .get_prompt(app_handle, "system_task_clustering")
        .map_err(|e| anyhow::anyhow!("failed to load clustering prompt: {}", e))?;

    let feedback_entries = cluster_feedback_manager
        .list_recent_negative_with_notes(FEEDBACK_WINDOW_DAYS, FEEDBACK_LIMIT)?;
    let now = chrono::Utc::now().timestamp_millis();
    let feedback_lines: Vec<(i64, String)> = feedback_entries
        .iter()
        .filter_map(|f| f.note.as_ref().map(|n| (f.created_at, n.clone())))
        .collect();

    let entries_block = render_entries_block(&candidate_entries);
    let protected_block = render_protected_block(&protected);
    let feedback_block = render_feedback_block(&feedback_lines, now);

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("date", date.clone());
    vars.insert("entry_count", candidate_entries.len().to_string());
    vars.insert("entries", entries_block);
    vars.insert("protected_clusters", protected_block.clone());
    vars.insert("user_feedback", feedback_block.clone());

    let rendered = PromptManager::substitute_variables(&template, &vars);
    // Strip conditional blocks if empty
    let rendered = strip_empty_conditional(&rendered, "protected_clusters_block", protected_block.is_empty());
    let rendered = strip_empty_conditional(&rendered, "user_feedback_block", feedback_block.is_empty());

    // 5. Resolve settings + provider + model from state
    let settings = app_handle
        .state::<crate::managers::settings::SettingsManager>()
        .get_settings()
        .map_err(|e| anyhow::anyhow!("settings unavailable: {}", e))?;

    let (provider, model) = resolve_clustering_provider_and_model(&settings)?;

    let system_prompts = vec![rendered];
    let llm_result = execute_llm_request_with_retry(
        app_handle,
        &settings,
        &provider,
        &model,
        None,
        &system_prompts,
        None,
        None,
        None,
    )
    .await;

    let raw_text = match llm_result {
        Ok(resp) => resp.content,
        Err(e) => {
            anyhow::bail!("LLM call failed: {}", e);
        }
    };

    // 6. Parse with one strict-mode retry on failure
    let parsed = match parse_llm_output(&raw_text) {
        Ok(p) => p,
        Err(_first_err) => {
            // strict retry — append a final instruction
            let mut strict_system = system_prompts.clone();
            strict_system.push("Return ONLY a valid JSON array. No prose, no markdown fences.".into());
            let retry_result = execute_llm_request_with_retry(
                app_handle,
                &settings,
                &provider,
                &model,
                None,
                &strict_system,
                None,
                None,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("LLM strict retry failed: {}", e))?;
            parse_llm_output(&retry_result.content)?
        }
    };

    // 7. Sanitize against valid entry ids
    let valid_ids: std::collections::HashSet<i64> = candidate_entries.iter().map(|e| e.id).collect();
    let sanitized = sanitize_outputs(parsed, &valid_ids);

    // 8. Delete unmodified for date, insert new clusters
    task_clusters_manager.delete_unmodified_for_date(&date)?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut inserted: Vec<TaskCluster> = Vec::new();
    for o in sanitized {
        let cluster = TaskCluster {
            id: TaskClustersManager::new_cluster_id(),
            summary_id,
            date: date.clone(),
            title: o.title,
            status: o.status,
            time_span: o.time_span,
            apps: o.apps,
            source_history_ids: o.source_history_ids.clone(),
            total_duration_ms: compute_total_duration_ms(&o.source_history_ids, &candidate_entries),
            entry_count: o.source_history_ids.len() as i64,
            summary: o.summary,
            blockers: o.blockers,
            next_step: o.next_step,
            keywords: o.keywords,
            is_user_modified: false,
            user_modified_fields: vec![],
            created_at: now_ms,
            updated_at: now_ms,
        };
        task_clusters_manager.upsert(&cluster)?;
        inserted.push(cluster);
    }

    // 9. Return combined (protected + new), ordered by duration
    let mut combined = protected;
    combined.extend(inserted);
    combined.sort_by(|a, b| b.total_duration_ms.cmp(&a.total_duration_ms));

    Ok(GenerateClustersResult {
        clusters: combined,
        skipped_reason: None,
        llm_called: true,
    })
}

fn strip_empty_conditional(rendered: &str, block_name: &str, is_empty: bool) -> String {
    let open = format!("{{{{#{}}}}}", block_name);
    let close = format!("{{{{/{}}}}}", block_name);
    if !is_empty {
        // Just remove the open/close tags, keep content
        rendered.replace(&open, "").replace(&close, "")
    } else {
        // Strip the entire block including content
        if let (Some(s), Some(e)) = (rendered.find(&open), rendered.find(&close)) {
            let mut out = String::with_capacity(rendered.len());
            out.push_str(&rendered[..s]);
            out.push_str(&rendered[e + close.len()..]);
            out
        } else {
            rendered.to_string()
        }
    }
}

fn compute_total_duration_ms(ids: &[i64], entries: &[ClusterableEntry]) -> i64 {
    entries
        .iter()
        .filter(|e| ids.contains(&e.id))
        .map(|e| e.duration_ms)
        .sum()
}

fn resolve_clustering_provider_and_model(
    settings: &crate::types::AppSettings,
) -> Result<(crate::types::PostProcessProvider, String)> {
    // Use the user's configured post-process provider + model for now.
    // Future: add a dedicated `clustering_model` setting.
    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| Some(&p.id) == settings.selected_post_process_provider_id.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no selected post-process provider"))?;
    let model = settings
        .selected_post_process_model
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no selected post-process model"))?;
    Ok((provider, model))
}
```

**Important:** the `crate::types::AppSettings` field names above match what the spec assumes; if the project uses different names (e.g., `selected_provider_id`), fix them by reading `src-tauri/src/types.rs` and updating the references. Run `rg 'post_process_providers' src-tauri/src/types.rs` to verify.

- [ ] **Step 2: Add tests for strip_empty_conditional and compute_total_duration_ms**

Append to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_strip_empty_conditional_keeps_when_not_empty() {
    let t = "before {{#x_block}}content{{/x_block}} after";
    let out = strip_empty_conditional(t, "x_block", false);
    assert_eq!(out, "before content after");
}

#[test]
fn test_strip_empty_conditional_removes_when_empty() {
    let t = "before {{#x_block}}content{{/x_block}} after";
    let out = strip_empty_conditional(t, "x_block", true);
    assert_eq!(out, "before  after");
}

#[test]
fn test_compute_total_duration_ms_sums_matching_ids() {
    let entries = vec![
        ClusterableEntry { id: 1, timestamp_ms: 0, app_name: None, window_title: None, text: "".into(), duration_ms: 100 },
        ClusterableEntry { id: 2, timestamp_ms: 0, app_name: None, window_title: None, text: "".into(), duration_ms: 200 },
        ClusterableEntry { id: 3, timestamp_ms: 0, app_name: None, window_title: None, text: "".into(), duration_ms: 300 },
    ];
    assert_eq!(compute_total_duration_ms(&[1, 3], &entries), 400);
    assert_eq!(compute_total_duration_ms(&[], &entries), 0);
}
```

- [ ] **Step 3: Run tests**

Run: `cd src-tauri && cargo test --lib actions::task_cluster_generator 2>&1 | tail -30`
Expected: all tests PASS. If `compile error` mentions a missing import (e.g. `PostProcessProvider`), open `src-tauri/src/types.rs` to confirm the actual paths and adjust the `use` statements.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/actions/task_cluster_generator.rs
git commit -m "Wire generate_task_clusters orchestrator with cache, protection, retry"
```

---

## Milestone 4: Backend Commands & Registration

### Task 8: commands/task_clusters.rs

**Files:**

- Create: `src-tauri/src/commands/task_clusters.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: Create the commands file**

Create `src-tauri/src/commands/task_clusters.rs`:

```rust
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::actions::task_cluster_generator::{
    generate_task_clusters as generator_run, ClusterableEntry, GenerateClustersInput,
};
use crate::managers::cluster_feedback::ClusterFeedbackManager;
use crate::managers::history::HistoryManager;
use crate::managers::prompt::PromptManager;
use crate::managers::summary::SummaryManager;
use crate::managers::task_clusters::{TaskCluster, TaskClustersManager};

#[tauri::command]
pub async fn get_task_clusters_by_date(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    date: String,
) -> Result<Vec<TaskCluster>, String> {
    manager.get_by_date(&date).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_task_clusters(
    app: AppHandle,
    task_clusters_manager: State<'_, Arc<TaskClustersManager>>,
    cluster_feedback_manager: State<'_, Arc<ClusterFeedbackManager>>,
    prompt_manager: State<'_, Arc<PromptManager>>,
    history_manager: State<'_, Arc<HistoryManager>>,
    summary_manager: State<'_, Arc<SummaryManager>>,
    date: String,
    force: bool,
) -> Result<Vec<TaskCluster>, String> {
    // Resolve summary_id for the date (creates a placeholder summary if missing)
    let summary_id = summary_manager
        .get_or_create_summary_id_for_date(&date)
        .await
        .map_err(|e| format!("failed to resolve summary: {}", e))?;

    // Pull entries for the day
    let entries_raw = history_manager
        .get_entries_for_date(&date)
        .await
        .map_err(|e| format!("failed to load history: {}", e))?;
    let entries: Vec<ClusterableEntry> = entries_raw
        .into_iter()
        .map(|e| ClusterableEntry {
            id: e.id,
            timestamp_ms: e.timestamp,
            app_name: e.app_name,
            window_title: e.window_title,
            text: e.post_processed_text.unwrap_or(e.transcription_text),
            duration_ms: e.duration_ms.unwrap_or(0),
        })
        .collect();

    let input = GenerateClustersInput { date, summary_id, entries, force };

    let result = generator_run(
        &app,
        task_clusters_manager.inner().clone(),
        cluster_feedback_manager.inner().clone(),
        prompt_manager.inner().clone(),
        input,
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(result.clusters)
}

#[tauri::command]
pub async fn update_task_cluster_field(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    cluster_id: String,
    field: String,
    value: String,
) -> Result<(), String> {
    manager
        .update_field(&cluster_id, &field, &value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn split_task_cluster(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    cluster_id: String,
    extract_ids: Vec<i64>,
    new_title: String,
    extracted_duration_ms: i64,
) -> Result<String, String> {
    manager
        .split(&cluster_id, &extract_ids, &new_title, extracted_duration_ms)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn merge_task_clusters(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    target_cluster_id: String,
    source_cluster_ids: Vec<String>,
) -> Result<(), String> {
    manager
        .merge(&target_cluster_id, &source_cluster_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_task_cluster(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    cluster_id: String,
) -> Result<(), String> {
    manager.delete(&cluster_id).map_err(|e| e.to_string())
}
```

**Required prerequisite checks before this compiles:**

- `SummaryManager::get_or_create_summary_id_for_date(&date)` exists and is async. If not, add a thin wrapper that looks up the existing summary or creates one.
- `HistoryManager::get_entries_for_date(&date)` returns entries with `id`, `timestamp`, `app_name`, `window_title`, `transcription_text`, `post_processed_text`, `duration_ms`. If missing, add a new manager method that wraps `get_history_entries_paginated` with date filters.

Run before continuing: `rg -n 'get_or_create_summary_id_for_date|get_entries_for_date' src-tauri/src/managers/`. If either is missing, write a brief helper method on the corresponding manager.

- [ ] **Step 2: Register module**

Modify `src-tauri/src/commands/mod.rs` — add:

```rust
pub mod task_clusters;
```

- [ ] **Step 3: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -20`
Expected: PASS (or surface concrete missing-method errors to be fixed against the actual `HistoryManager` / `SummaryManager` API).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/task_clusters.rs src-tauri/src/commands/mod.rs src-tauri/src/managers/summary.rs src-tauri/src/managers/history.rs
git commit -m "Add Tauri commands for task cluster CRUD and generation"
```

---

### Task 9: commands/cluster_feedback.rs + register all new commands + state setup

**Files:**

- Create: `src-tauri/src/commands/cluster_feedback.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs` (state registration + handler list + history-delete cascade)

- [ ] **Step 1: Create cluster_feedback commands**

Create `src-tauri/src/commands/cluster_feedback.rs`:

```rust
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::managers::cluster_feedback::{ClusterFeedback, ClusterFeedbackManager};

#[tauri::command]
pub async fn add_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    cluster_id: String,
    thumb: String,
    note: Option<String>,
) -> Result<i64, String> {
    manager
        .add(&cluster_id, &thumb, note.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    cluster_id: String,
) -> Result<Vec<ClusterFeedback>, String> {
    manager
        .list_for_cluster(&cluster_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_recent_negative_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    days: i64,
    limit: i64,
) -> Result<Vec<ClusterFeedback>, String> {
    manager
        .list_recent_negative_with_notes(days, limit)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    id: i64,
) -> Result<(), String> {
    manager.delete(id).map_err(|e| e.to_string())
}
```

Modify `src-tauri/src/commands/mod.rs` — add:

```rust
pub mod cluster_feedback;
```

- [ ] **Step 2: Register managers as Tauri state**

Modify `src-tauri/src/lib.rs`. Find the setup hook (`tauri::Builder::default().setup(...)` block) and after the existing manager registrations, add:

```rust
let task_clusters_manager = Arc::new(
    crate::managers::task_clusters::TaskClustersManager::new(db_path.clone())
        .expect("Failed to init TaskClustersManager"),
);
app.manage(task_clusters_manager.clone());

let cluster_feedback_manager = Arc::new(
    crate::managers::cluster_feedback::ClusterFeedbackManager::new(db_path.clone())
        .expect("Failed to init ClusterFeedbackManager"),
);
app.manage(cluster_feedback_manager.clone());
```

(`db_path` must be the same `PathBuf` used by the other managers in setup — read the surrounding code to match the variable name.)

- [ ] **Step 3: Register commands in invoke_handler!**

In `src-tauri/src/lib.rs`, find `tauri::generate_handler![...]` and add at the end of the list:

```rust
commands::task_clusters::get_task_clusters_by_date,
commands::task_clusters::generate_task_clusters,
commands::task_clusters::update_task_cluster_field,
commands::task_clusters::split_task_cluster,
commands::task_clusters::merge_task_clusters,
commands::task_clusters::delete_task_cluster,
commands::cluster_feedback::add_cluster_feedback,
commands::cluster_feedback::list_cluster_feedback,
commands::cluster_feedback::list_recent_negative_cluster_feedback,
commands::cluster_feedback::delete_cluster_feedback,
```

- [ ] **Step 4: Add history-delete cascade**

Find the history-delete pathway. Search: `rg -n 'fn delete_history_entry\|DELETE FROM transcription_history' src-tauri/src`. In whichever function performs the delete, after the row is deleted, call:

```rust
if let Some(tc_mgr) = app_handle.try_state::<std::sync::Arc<crate::managers::task_clusters::TaskClustersManager>>() {
    let _ = tc_mgr.remove_history_id_from_all_clusters(deleted_id, deleted_duration_ms);
}
```

If the deletion happens inside a manager method that doesn't already have an `AppHandle`, pass it through (or store an `Arc<TaskClustersManager>` on `HistoryManager` at construction). Choose whichever existing pattern matches the codebase.

- [ ] **Step 5: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Verify commands are registered**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`
Expected: builds successfully.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands/cluster_feedback.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "Register task_clusters and cluster_feedback commands with state and cascade"
```

---

## Milestone 5: Frontend Types & State

### Task 10: Extended types + summaryStore

**Files:**

- Modify: `src/components/settings/summary/summaryTypes.ts`
- Create: `src/components/settings/summary/stores/summaryStore.ts`

- [ ] **Step 1: Extend `summaryTypes.ts`**

Open `src/components/settings/summary/summaryTypes.ts` and update the `TaskCluster` interface to mirror the backend struct. Replace the existing `TaskCluster` interface (currently at lines 13-24) with:

```typescript
export interface TaskCluster {
  id: string;
  summary_id: number;
  date: string;
  title: string;
  status: string; // "进行中" | "完成" | "卡住" | "已搁置"
  time_span: string | null;
  apps: string[];
  source_history_ids: number[];
  total_duration_ms: number;
  entry_count: number;
  summary: string | null;
  blockers: string[];
  next_step: string | null;
  keywords: string[];
  is_user_modified: boolean;
  user_modified_fields: string[];
  created_at: number;
  updated_at: number;
}

export interface ClusterFeedback {
  id: number;
  cluster_id: string;
  thumb: "up" | "down";
  note: string | null;
  created_at: number;
}

export type AuxSection =
  | "stats"
  | "recap"
  | "profile"
  | "hotword"
  | "export"
  | "feedback";

export type ViewMode = "day" | "week" | "month";
```

Search the file for any other references to old fields (`time_span` as required, etc.) and align them. If `SummaryStats.task_clusters?: TaskCluster[]` exists, update its docstring to "DEPRECATED — clusters now stored in task_clusters table" but leave the field for migration compatibility.

- [ ] **Step 2: Create the Zustand store**

Create `src/components/settings/summary/stores/summaryStore.ts`:

```typescript
import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import type { AuxSection, ViewMode } from "../summaryTypes";

interface SummaryStore {
  viewMode: ViewMode;
  selectedDate: string; // ISO date yyyy-mm-dd
  expandedClusterIds: Set<string>;
  auxPanelOpen: boolean;
  auxActiveSection: AuxSection;

  setViewMode: (mode: ViewMode) => void;
  setSelectedDate: (date: string) => void;
  toggleClusterExpanded: (id: string) => void;
  expandCluster: (id: string) => void;
  collapseCluster: (id: string) => void;
  openAuxPanel: (section?: AuxSection) => void;
  closeAuxPanel: () => void;
  setAuxSection: (section: AuxSection) => void;
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

export const useSummaryStore = create<SummaryStore>()(
  subscribeWithSelector((set) => ({
    viewMode: "day",
    selectedDate: todayIso(),
    expandedClusterIds: new Set<string>(),
    auxPanelOpen: false,
    auxActiveSection: "stats",

    setViewMode: (mode) => set({ viewMode: mode }),
    setSelectedDate: (date) =>
      set({ selectedDate: date, expandedClusterIds: new Set() }),
    toggleClusterExpanded: (id) =>
      set((state) => {
        const next = new Set(state.expandedClusterIds);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return { expandedClusterIds: next };
      }),
    expandCluster: (id) =>
      set((state) => {
        const next = new Set(state.expandedClusterIds);
        next.add(id);
        return { expandedClusterIds: next };
      }),
    collapseCluster: (id) =>
      set((state) => {
        const next = new Set(state.expandedClusterIds);
        next.delete(id);
        return { expandedClusterIds: next };
      }),
    openAuxPanel: (section) =>
      set((state) => ({
        auxPanelOpen: true,
        auxActiveSection: section ?? state.auxActiveSection,
      })),
    closeAuxPanel: () => set({ auxPanelOpen: false }),
    setAuxSection: (section) => set({ auxActiveSection: section }),
  })),
);
```

- [ ] **Step 3: Verify TypeScript compiles**

Run: `bun run tsc --noEmit 2>&1 | tail -20`
Expected: no new errors related to summary types or store.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/summaryTypes.ts src/components/settings/summary/stores/summaryStore.ts
git commit -m "Extend TaskCluster type and add summaryStore for UI state"
```

---

### Task 11: useTaskClusters + useClusterFeedback hooks

**Files:**

- Create: `src/components/settings/summary/hooks/useTaskClusters.ts`
- Create: `src/components/settings/summary/hooks/useClusterFeedback.ts`

- [ ] **Step 1: Create useTaskClusters**

Create `src/components/settings/summary/hooks/useTaskClusters.ts`:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import type { TaskCluster } from "../summaryTypes";

interface TaskClustersState {
  clusters: TaskCluster[];
  loading: boolean;
  generating: boolean;
  error: string | null;
}

const cacheByDate = new Map<string, TaskCluster[]>();

export function useTaskClusters(date: string) {
  const [state, setState] = useState<TaskClustersState>({
    clusters: cacheByDate.get(date) ?? [],
    loading: false,
    generating: false,
    error: null,
  });
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    setState((s) => ({ ...s, loading: true, error: null }));
    try {
      const list = await invoke<TaskCluster[]>("get_task_clusters_by_date", {
        date,
      });
      cacheByDate.set(date, list);
      if (mountedRef.current) {
        setState({
          clusters: list,
          loading: false,
          generating: false,
          error: null,
        });
      }
    } catch (e) {
      const msg = String(e);
      if (mountedRef.current) {
        setState((s) => ({ ...s, loading: false, error: msg }));
      }
      toast.error(`加载聚类失败: ${msg}`);
    }
  }, [date]);

  const generate = useCallback(
    async (force: boolean) => {
      setState((s) => ({ ...s, generating: true, error: null }));
      try {
        const list = await invoke<TaskCluster[]>("generate_task_clusters", {
          date,
          force,
        });
        cacheByDate.set(date, list);
        if (mountedRef.current) {
          setState({
            clusters: list,
            loading: false,
            generating: false,
            error: null,
          });
        }
        if (force) toast.success("已重新生成聚类");
      } catch (e) {
        const msg = String(e);
        if (mountedRef.current) {
          setState((s) => ({ ...s, generating: false, error: msg }));
        }
        toast.error(`AI 调用失败，已保留上次结果: ${msg}`);
      }
    },
    [date],
  );

  const updateField = useCallback(
    async (
      clusterId: string,
      field: "title" | "status" | "next_step",
      value: string,
    ) => {
      try {
        await invoke("update_task_cluster_field", { clusterId, field, value });
        await refresh();
      } catch (e) {
        toast.error(`更新失败: ${e}`);
      }
    },
    [refresh],
  );

  const split = useCallback(
    async (
      clusterId: string,
      extractIds: number[],
      newTitle: string,
      extractedDurationMs: number,
    ) => {
      try {
        await invoke<string>("split_task_cluster", {
          clusterId,
          extractIds,
          newTitle,
          extractedDurationMs,
        });
        await refresh();
        toast.success("已拆分");
      } catch (e) {
        toast.error(`拆分失败: ${e}`);
      }
    },
    [refresh],
  );

  const merge = useCallback(
    async (targetClusterId: string, sourceClusterIds: string[]) => {
      try {
        await invoke("merge_task_clusters", {
          targetClusterId,
          sourceClusterIds,
        });
        await refresh();
        toast.success("已合并");
      } catch (e) {
        toast.error(`合并失败: ${e}`);
      }
    },
    [refresh],
  );

  const remove = useCallback(
    async (clusterId: string) => {
      try {
        await invoke("delete_task_cluster", { clusterId });
        await refresh();
      } catch (e) {
        toast.error(`删除失败: ${e}`);
      }
    },
    [refresh],
  );

  // Auto-load on date change
  useEffect(() => {
    refresh();
  }, [refresh]);

  return { ...state, refresh, generate, updateField, split, merge, remove };
}
```

- [ ] **Step 2: Create useClusterFeedback**

Create `src/components/settings/summary/hooks/useClusterFeedback.ts`:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import type { ClusterFeedback } from "../summaryTypes";

export function useClusterFeedback(clusterId: string | null) {
  const [items, setItems] = useState<ClusterFeedback[]>([]);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    if (!clusterId) {
      setItems([]);
      return;
    }
    setLoading(true);
    try {
      const list = await invoke<ClusterFeedback[]>("list_cluster_feedback", {
        clusterId,
      });
      setItems(list);
    } catch (e) {
      toast.error(`加载反馈失败: ${e}`);
    } finally {
      setLoading(false);
    }
  }, [clusterId]);

  const add = useCallback(
    async (thumb: "up" | "down", note?: string) => {
      if (!clusterId) return;
      try {
        await invoke<number>("add_cluster_feedback", {
          clusterId,
          thumb,
          note: note ?? null,
        });
        await refresh();
      } catch (e) {
        toast.error(`提交反馈失败: ${e}`);
      }
    },
    [clusterId, refresh],
  );

  const remove = useCallback(
    async (id: number) => {
      try {
        await invoke("delete_cluster_feedback", { id });
        await refresh();
      } catch (e) {
        toast.error(`删除反馈失败: ${e}`);
      }
    },
    [refresh],
  );

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { items, loading, refresh, add, remove };
}

export function useRecentNegativeFeedback() {
  const [items, setItems] = useState<ClusterFeedback[]>([]);
  const refresh = useCallback(async () => {
    try {
      const list = await invoke<ClusterFeedback[]>(
        "list_recent_negative_cluster_feedback",
        { days: 30, limit: 20 },
      );
      setItems(list);
    } catch (e) {
      console.warn("failed to load recent negative feedback", e);
    }
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);
  return { items, refresh };
}
```

- [ ] **Step 3: Verify TypeScript**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/hooks/useTaskClusters.ts src/components/settings/summary/hooks/useClusterFeedback.ts
git commit -m "Add useTaskClusters and useClusterFeedback hooks"
```

---

## Milestone 6: Frontend Cluster Components

### Task 12: ClusterCard (display + inline title/status/next_step edit)

**Files:**

- Create: `src/components/settings/summary/cluster/ClusterCard.tsx`

- [ ] **Step 1: Create the file**

Create `src/components/settings/summary/cluster/ClusterCard.tsx`:

```typescript
import { Badge, Box, Flex, IconButton, Text, TextField, Select } from "@radix-ui/themes";
import { ChevronDown, ChevronRight, Pencil, Check, X } from "lucide-react";
import { useState } from "react";
import { Card } from "@/components/ui/Card";
import type { TaskCluster } from "../summaryTypes";

interface ClusterCardProps {
  cluster: TaskCluster;
  expanded: boolean;
  onToggleExpanded: () => void;
  onUpdateField: (field: "title" | "status" | "next_step", value: string) => Promise<void>;
  onOpenSplit: () => void;
  onOpenMerge: () => void;
  onOpenDelete: () => void;
  onThumb: (thumb: "up" | "down") => Promise<void>;
  detailSlot?: React.ReactNode;
}

const STATUS_OPTIONS = ["进行中", "完成", "卡住", "已搁置"];

const STATUS_COLORS: Record<string, "blue" | "green" | "amber" | "gray"> = {
  进行中: "blue",
  完成: "green",
  卡住: "amber",
  已搁置: "gray",
};

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h${m}m`;
  return `${m}m`;
}

export function ClusterCard({
  cluster,
  expanded,
  onToggleExpanded,
  onUpdateField,
  onOpenSplit,
  onOpenMerge,
  onOpenDelete,
  onThumb,
  detailSlot,
}: ClusterCardProps) {
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState(cluster.title);
  const [editingNextStep, setEditingNextStep] = useState(false);
  const [nextStepDraft, setNextStepDraft] = useState(cluster.next_step ?? "");

  const commitTitle = async () => {
    if (titleDraft.trim() && titleDraft !== cluster.title) {
      await onUpdateField("title", titleDraft.trim());
    }
    setEditingTitle(false);
  };
  const cancelTitle = () => {
    setTitleDraft(cluster.title);
    setEditingTitle(false);
  };
  const commitNextStep = async () => {
    if (nextStepDraft !== (cluster.next_step ?? "")) {
      await onUpdateField("next_step", nextStepDraft);
    }
    setEditingNextStep(false);
  };

  return (
    <Card className="mb-3">
      <Flex direction="column" gap="2">
        <Flex align="center" gap="2">
          <IconButton size="1" variant="ghost" onClick={onToggleExpanded} aria-label="展开">
            {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
          </IconButton>
          {editingTitle ? (
            <Flex align="center" gap="1" className="flex-1">
              <TextField.Root
                value={titleDraft}
                onChange={(e) => setTitleDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitTitle();
                  if (e.key === "Escape") cancelTitle();
                }}
                autoFocus
                className="flex-1"
              />
              <IconButton size="1" variant="ghost" onClick={commitTitle}>
                <Check size={14} />
              </IconButton>
              <IconButton size="1" variant="ghost" onClick={cancelTitle}>
                <X size={14} />
              </IconButton>
            </Flex>
          ) : (
            <Flex align="center" gap="2" className="flex-1 group">
              <Text size="3" weight="bold">{cluster.title}</Text>
              <IconButton
                size="1"
                variant="ghost"
                className="opacity-0 group-hover:opacity-100"
                onClick={() => {
                  setTitleDraft(cluster.title);
                  setEditingTitle(true);
                }}
              >
                <Pencil size={12} />
              </IconButton>
            </Flex>
          )}
          <Select.Root
            value={cluster.status}
            onValueChange={(v) => onUpdateField("status", v)}
          >
            <Select.Trigger variant="ghost">
              <Badge color={STATUS_COLORS[cluster.status] ?? "gray"} size="1">
                {cluster.status}
              </Badge>
            </Select.Trigger>
            <Select.Content>
              {STATUS_OPTIONS.map((s) => (
                <Select.Item key={s} value={s}>{s}</Select.Item>
              ))}
            </Select.Content>
          </Select.Root>
          <Text size="1" color="gray">
            {formatDuration(cluster.total_duration_ms)} · {cluster.entry_count} entries
          </Text>
          {cluster.is_user_modified && (
            <Badge color="violet" size="1" variant="soft">
              已编辑
            </Badge>
          )}
        </Flex>

        {cluster.keywords.length > 0 && (
          <Flex gap="1" wrap="wrap">
            {cluster.keywords.slice(0, 8).map((k) => (
              <Badge key={k} size="1" variant="soft" color="gray">{k}</Badge>
            ))}
          </Flex>
        )}

        {cluster.summary && (
          <Text size="2" color="gray">{cluster.summary}</Text>
        )}

        {cluster.blockers.length > 0 && (
          <Box className="rounded-md bg-amber-50 dark:bg-amber-950/30 px-2 py-1">
            <Text size="1" color="amber">
              ⚠ {cluster.blockers.join(" / ")}
            </Text>
          </Box>
        )}

        <Flex align="center" gap="2">
          <Text size="1" color="gray">📋 next_step:</Text>
          {editingNextStep ? (
            <Flex align="center" gap="1" className="flex-1">
              <TextField.Root
                value={nextStepDraft}
                onChange={(e) => setNextStepDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitNextStep();
                  if (e.key === "Escape") setEditingNextStep(false);
                }}
                autoFocus
                className="flex-1"
              />
              <IconButton size="1" variant="ghost" onClick={commitNextStep}>
                <Check size={14} />
              </IconButton>
            </Flex>
          ) : (
            <Flex align="center" gap="1" className="flex-1 group">
              <Text size="2">{cluster.next_step || "—"}</Text>
              <IconButton
                size="1"
                variant="ghost"
                className="opacity-0 group-hover:opacity-100"
                onClick={() => {
                  setNextStepDraft(cluster.next_step ?? "");
                  setEditingNextStep(true);
                }}
              >
                <Pencil size={12} />
              </IconButton>
            </Flex>
          )}
        </Flex>

        {expanded && detailSlot && <Box className="mt-2">{detailSlot}</Box>}

        <Flex justify="between" align="center" className="mt-1">
          <Flex gap="2">
            <button
              type="button"
              onClick={onOpenSplit}
              className="text-xs text-gray-600 hover:text-gray-900"
            >
              🔀 拆分
            </button>
            <button
              type="button"
              onClick={onOpenMerge}
              className="text-xs text-gray-600 hover:text-gray-900"
            >
              🔗 合并
            </button>
            <button
              type="button"
              onClick={onOpenDelete}
              className="text-xs text-gray-600 hover:text-red-600"
            >
              🗑️ 删除
            </button>
          </Flex>
          <Flex gap="1">
            <IconButton size="1" variant="ghost" onClick={() => onThumb("up")}>👍</IconButton>
            <IconButton size="1" variant="ghost" onClick={() => onThumb("down")}>👎</IconButton>
          </Flex>
        </Flex>
      </Flex>
    </Card>
  );
}
```

- [ ] **Step 2: Verify TypeScript**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors (Lucide icons may need installation — run `bun add lucide-react` only if missing).

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/summary/cluster/ClusterCard.tsx
git commit -m "Add ClusterCard with inline edit for title/status/next_step"
```

---

### Task 13: ClusterDetailDrawer (timeline + source jump)

**Files:**

- Create: `src/components/settings/summary/cluster/ClusterDetailDrawer.tsx`

- [ ] **Step 1: Create the file**

Create `src/components/settings/summary/cluster/ClusterDetailDrawer.tsx`:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { Box, Flex, Text, IconButton } from "@radix-ui/themes";
import { ArrowUpRight } from "lucide-react";
import { useEffect, useState } from "react";
import type { TaskCluster } from "../summaryTypes";

interface HistoryEntryLite {
  id: number;
  timestamp: number;
  app_name: string | null;
  window_title: string | null;
  transcription_text: string;
  post_processed_text: string | null;
  duration_ms: number | null;
}

interface ClusterDetailDrawerProps {
  cluster: TaskCluster;
  onNavigateToHistory: (entryId: number) => void;
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function ClusterDetailDrawer({
  cluster,
  onNavigateToHistory,
}: ClusterDetailDrawerProps) {
  const [entries, setEntries] = useState<HistoryEntryLite[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    (async () => {
      try {
        const all = await invoke<HistoryEntryLite[]>("get_history_entries_by_ids", {
          ids: cluster.source_history_ids,
        });
        if (!cancelled) {
          all.sort((a, b) => a.timestamp - b.timestamp);
          setEntries(all);
        }
      } catch (e) {
        console.warn("failed to load cluster entries", e);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cluster.source_history_ids]);

  if (loading) {
    return <Text size="2" color="gray">加载源转录...</Text>;
  }

  if (entries.length === 0) {
    return <Text size="2" color="gray">该 cluster 已失效，建议重新生成</Text>;
  }

  return (
    <Box className="border-l-2 border-gray-200 dark:border-gray-700 pl-3">
      <Flex direction="column" gap="2">
        {entries.map((e) => (
          <Flex key={e.id} align="start" gap="2">
            <Text size="1" color="gray" className="min-w-12">
              {formatTime(e.timestamp)}
            </Text>
            <Box className="flex-1">
              <Flex align="center" gap="1">
                <Text size="1" weight="medium">{e.app_name ?? "?"}</Text>
                {e.window_title && (
                  <Text size="1" color="gray" className="truncate max-w-xs">
                    · {e.window_title}
                  </Text>
                )}
                <IconButton
                  size="1"
                  variant="ghost"
                  onClick={() => onNavigateToHistory(e.id)}
                  aria-label="跳转到 Dashboard 对应条目"
                >
                  <ArrowUpRight size={12} />
                </IconButton>
              </Flex>
              <Text size="2" className="line-clamp-3">
                {e.post_processed_text || e.transcription_text}
              </Text>
            </Box>
          </Flex>
        ))}
      </Flex>
    </Box>
  );
}
```

- [ ] **Step 2: Add backend command for batch entry lookup if missing**

Run: `rg -n 'get_history_entries_by_ids' src-tauri/src/commands`
If missing, add to `src-tauri/src/commands/history.rs`:

```rust
#[tauri::command]
pub async fn get_history_entries_by_ids(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    ids: Vec<i64>,
) -> Result<Vec<HistoryEntry>, String> {
    history_manager
        .get_entries_by_ids(&ids)
        .await
        .map_err(|e| e.to_string())
}
```

Then in `HistoryManager`, add a corresponding async method that runs `SELECT ... WHERE id IN (...)`. Register the command in `lib.rs`'s handler list.

- [ ] **Step 3: Verify**

Run: `cd src-tauri && cargo check 2>&1 | tail -10`
Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: both PASS.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/cluster/ClusterDetailDrawer.tsx src-tauri/src/commands/history.rs src-tauri/src/managers/history.rs src-tauri/src/lib.rs
git commit -m "Add ClusterDetailDrawer with batch source-entry lookup"
```

---

### Task 14: ClusterFeedbackButtons + DeleteClusterConfirm

**Files:**

- Create: `src/components/settings/summary/cluster/ClusterFeedbackButtons.tsx`
- Create: `src/components/settings/summary/cluster/DeleteClusterConfirm.tsx`

- [ ] **Step 1: Create ClusterFeedbackButtons (note popup)**

Create `src/components/settings/summary/cluster/ClusterFeedbackButtons.tsx`:

```typescript
import { Popover, Button, TextArea, Flex, Text } from "@radix-ui/themes";
import { useState } from "react";
import { useClusterFeedback } from "../hooks/useClusterFeedback";

interface ClusterFeedbackButtonsProps {
  clusterId: string;
}

export function ClusterFeedbackButtons({ clusterId }: ClusterFeedbackButtonsProps) {
  const { add } = useClusterFeedback(clusterId);
  const [openThumb, setOpenThumb] = useState<"up" | "down" | null>(null);
  const [note, setNote] = useState("");

  const submit = async () => {
    if (!openThumb) return;
    await add(openThumb, note.trim() ? note.trim() : undefined);
    setNote("");
    setOpenThumb(null);
  };

  return (
    <Popover.Root
      open={openThumb !== null}
      onOpenChange={(open) => {
        if (!open) {
          setNote("");
          setOpenThumb(null);
        }
      }}
    >
      <Flex gap="1">
        <Popover.Trigger>
          <Button
            variant="ghost"
            size="1"
            onClick={() => setOpenThumb("up")}
            aria-label="赞同"
          >
            👍
          </Button>
        </Popover.Trigger>
        <Popover.Trigger>
          <Button
            variant="ghost"
            size="1"
            onClick={() => setOpenThumb("down")}
            aria-label="反对"
          >
            👎
          </Button>
        </Popover.Trigger>
      </Flex>
      <Popover.Content>
        <Flex direction="column" gap="2" style={{ minWidth: 240 }}>
          <Text size="2">
            {openThumb === "down" ? "反馈：哪里不对？" : "可选备注"}
          </Text>
          <TextArea
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder={
              openThumb === "down"
                ? "例：Slack 消息应该单独成簇..."
                : "可选"
            }
            rows={3}
          />
          <Flex gap="2" justify="end">
            <Button variant="soft" onClick={() => setOpenThumb(null)} size="1">
              取消
            </Button>
            <Button onClick={submit} size="1">
              {note.trim() ? "提交" : "提交（无备注）"}
            </Button>
          </Flex>
        </Flex>
      </Popover.Content>
    </Popover.Root>
  );
}
```

- [ ] **Step 2: Create DeleteClusterConfirm**

Create `src/components/settings/summary/cluster/DeleteClusterConfirm.tsx`:

```typescript
import { AlertDialog, Button, Flex } from "@radix-ui/themes";

interface DeleteClusterConfirmProps {
  open: boolean;
  clusterTitle: string;
  onCancel: () => void;
  onConfirm: () => void;
}

export function DeleteClusterConfirm({
  open,
  clusterTitle,
  onCancel,
  onConfirm,
}: DeleteClusterConfirmProps) {
  return (
    <AlertDialog.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <AlertDialog.Content style={{ maxWidth: 400 }}>
        <AlertDialog.Title>删除 cluster</AlertDialog.Title>
        <AlertDialog.Description size="2">
          确认删除「{clusterTitle}」？源转录记录不会被删除。下次重新生成时 AI 可能再次提出类似聚类。
        </AlertDialog.Description>
        <Flex gap="3" mt="4" justify="end">
          <AlertDialog.Cancel>
            <Button variant="soft" color="gray">取消</Button>
          </AlertDialog.Cancel>
          <AlertDialog.Action>
            <Button color="red" onClick={onConfirm}>删除</Button>
          </AlertDialog.Action>
        </Flex>
      </AlertDialog.Content>
    </AlertDialog.Root>
  );
}
```

**Note:** Replace the existing `ClusterFeedbackButtons` placeholder in `ClusterCard.tsx` with a real import:

```typescript
import { ClusterFeedbackButtons } from "./ClusterFeedbackButtons";
```

Then in the JSX, replace the two `<IconButton>` thumb buttons inside `ClusterCard` with `<ClusterFeedbackButtons clusterId={cluster.id} />` (and remove the `onThumb` prop from the component interface — pass via `ClusterFeedbackButtons`'s internal hook instead).

Refactor `ClusterCard.tsx` accordingly: remove the `onThumb` prop, add `<ClusterFeedbackButtons clusterId={cluster.id} />` where the old buttons were.

- [ ] **Step 3: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/cluster/
git commit -m "Add ClusterFeedbackButtons with note popup and DeleteClusterConfirm"
```

---

### Task 15: SplitClusterDialog + MergeClusterDialog

**Files:**

- Create: `src/components/settings/summary/cluster/SplitClusterDialog.tsx`
- Create: `src/components/settings/summary/cluster/MergeClusterDialog.tsx`

- [ ] **Step 1: Create SplitClusterDialog**

Create `src/components/settings/summary/cluster/SplitClusterDialog.tsx`:

```typescript
import { Dialog, Button, Checkbox, Flex, Text, TextField, ScrollArea } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { TaskCluster } from "../summaryTypes";

interface HistoryEntryLite {
  id: number;
  timestamp: number;
  app_name: string | null;
  transcription_text: string;
  post_processed_text: string | null;
  duration_ms: number | null;
}

interface SplitClusterDialogProps {
  open: boolean;
  cluster: TaskCluster | null;
  onCancel: () => void;
  onConfirm: (extractIds: number[], newTitle: string, extractedDurationMs: number) => Promise<void>;
}

export function SplitClusterDialog({ open, cluster, onCancel, onConfirm }: SplitClusterDialogProps) {
  const [entries, setEntries] = useState<HistoryEntryLite[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [newTitle, setNewTitle] = useState("");
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!open || !cluster) return;
    setSelected(new Set());
    setNewTitle("");
    (async () => {
      try {
        const all = await invoke<HistoryEntryLite[]>("get_history_entries_by_ids", {
          ids: cluster.source_history_ids,
        });
        all.sort((a, b) => a.timestamp - b.timestamp);
        setEntries(all);
      } catch (e) {
        console.warn(e);
      }
    })();
  }, [open, cluster]);

  if (!cluster) return null;

  const toggle = (id: number) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const validSelection =
    selected.size > 0 &&
    selected.size < cluster.source_history_ids.length &&
    newTitle.trim().length > 0;

  const submit = async () => {
    if (!validSelection) return;
    setSubmitting(true);
    try {
      const ids = Array.from(selected);
      const duration = entries
        .filter((e) => selected.has(e.id))
        .reduce((sum, e) => sum + (e.duration_ms ?? 0), 0);
      await onConfirm(ids, newTitle.trim(), duration);
      onCancel();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <Dialog.Content style={{ maxWidth: 560 }}>
        <Dialog.Title>从「{cluster.title}」中拆出</Dialog.Title>
        <Dialog.Description size="2" mb="3">
          勾选要拆出的源转录条目，并为新 cluster 命名。
        </Dialog.Description>
        <Flex direction="column" gap="3">
          <TextField.Root
            placeholder="新 cluster 标题"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
          />
          <ScrollArea type="auto" style={{ maxHeight: 320 }}>
            <Flex direction="column" gap="2">
              {entries.map((e) => (
                <Flex key={e.id} align="start" gap="2" asChild>
                  <label>
                    <Checkbox
                      checked={selected.has(e.id)}
                      onCheckedChange={() => toggle(e.id)}
                    />
                    <Flex direction="column" className="flex-1">
                      <Text size="1" color="gray">
                        {new Date(e.timestamp).toLocaleTimeString()} · {e.app_name ?? "?"}
                      </Text>
                      <Text size="2" className="line-clamp-2">
                        {e.post_processed_text || e.transcription_text}
                      </Text>
                    </Flex>
                  </label>
                </Flex>
              ))}
            </Flex>
          </ScrollArea>
          <Text size="1" color="gray">
            已选 {selected.size} / {entries.length}
            {selected.size === 0 && " — 至少选一条"}
            {selected.size === entries.length && entries.length > 0 && " — 不能全选（剩余 cluster 会空）"}
          </Text>
        </Flex>
        <Flex gap="3" mt="4" justify="end">
          <Dialog.Close>
            <Button variant="soft" color="gray">取消</Button>
          </Dialog.Close>
          <Button disabled={!validSelection || submitting} onClick={submit}>
            拆分
          </Button>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
}
```

- [ ] **Step 2: Create MergeClusterDialog**

Create `src/components/settings/summary/cluster/MergeClusterDialog.tsx`:

```typescript
import { Dialog, Button, Checkbox, Flex, Text, ScrollArea } from "@radix-ui/themes";
import { useState } from "react";
import type { TaskCluster } from "../summaryTypes";

interface MergeClusterDialogProps {
  open: boolean;
  targetCluster: TaskCluster | null;
  otherClusters: TaskCluster[]; // candidates same date
  onCancel: () => void;
  onConfirm: (sourceClusterIds: string[]) => Promise<void>;
}

export function MergeClusterDialog({
  open,
  targetCluster,
  otherClusters,
  onCancel,
  onConfirm,
}: MergeClusterDialogProps) {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);

  if (!targetCluster) return null;

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const submit = async () => {
    if (selected.size === 0) return;
    setSubmitting(true);
    try {
      await onConfirm(Array.from(selected));
      onCancel();
    } finally {
      setSubmitting(false);
    }
  };

  const candidates = otherClusters.filter((c) => c.id !== targetCluster.id);

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <Dialog.Content style={{ maxWidth: 520 }}>
        <Dialog.Title>合并到「{targetCluster.title}」</Dialog.Title>
        <Dialog.Description size="2" mb="3">
          选择要并入的 cluster。被并入的 cluster 会被删除，其 source_ids 与关键词合并到当前 cluster。
        </Dialog.Description>
        <ScrollArea type="auto" style={{ maxHeight: 320 }}>
          <Flex direction="column" gap="2">
            {candidates.length === 0 && (
              <Text size="2" color="gray">当日没有其他 cluster 可合并。</Text>
            )}
            {candidates.map((c) => (
              <Flex key={c.id} align="center" gap="2" asChild>
                <label>
                  <Checkbox
                    checked={selected.has(c.id)}
                    onCheckedChange={() => toggle(c.id)}
                  />
                  <Flex direction="column" className="flex-1">
                    <Text size="2" weight="medium">{c.title}</Text>
                    <Text size="1" color="gray">
                      {c.entry_count} entries · {c.status}
                    </Text>
                  </Flex>
                </label>
              </Flex>
            ))}
          </Flex>
        </ScrollArea>
        <Flex gap="3" mt="4" justify="end">
          <Dialog.Close>
            <Button variant="soft" color="gray">取消</Button>
          </Dialog.Close>
          <Button disabled={selected.size === 0 || submitting} onClick={submit}>
            合并 {selected.size > 0 && `(${selected.size})`}
          </Button>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
}
```

- [ ] **Step 3: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/cluster/SplitClusterDialog.tsx src/components/settings/summary/cluster/MergeClusterDialog.tsx
git commit -m "Add SplitClusterDialog and MergeClusterDialog"
```

---

## Milestone 7: Frontend Views & Shared

### Task 16: PeriodSelector + RegenerateButton + DayView

**Files:**

- Create: `src/components/settings/summary/shared/PeriodSelector.tsx`
- Create: `src/components/settings/summary/shared/RegenerateButton.tsx`
- Create: `src/components/settings/summary/views/DayView.tsx`

- [ ] **Step 1: Create PeriodSelector**

Create `src/components/settings/summary/shared/PeriodSelector.tsx`:

```typescript
import { Button, Flex, SegmentedControl, Text } from "@radix-ui/themes";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useSummaryStore } from "../stores/summaryStore";
import type { ViewMode } from "../summaryTypes";

function shiftDate(iso: string, mode: ViewMode, delta: number): string {
  const d = new Date(iso);
  if (mode === "day") d.setDate(d.getDate() + delta);
  else if (mode === "week") d.setDate(d.getDate() + delta * 7);
  else d.setMonth(d.getMonth() + delta);
  return d.toISOString().slice(0, 10);
}

function formatRange(iso: string, mode: ViewMode): string {
  if (mode === "day") return iso;
  const d = new Date(iso);
  if (mode === "week") {
    const start = new Date(d);
    start.setDate(d.getDate() - d.getDay());
    const end = new Date(start);
    end.setDate(start.getDate() + 6);
    return `${start.toISOString().slice(0, 10)} ~ ${end.toISOString().slice(0, 10)}`;
  }
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

export function PeriodSelector() {
  const { viewMode, selectedDate, setViewMode, setSelectedDate } = useSummaryStore();

  return (
    <Flex align="center" gap="3">
      <Flex align="center" gap="1">
        <Button
          variant="ghost"
          size="1"
          onClick={() => setSelectedDate(shiftDate(selectedDate, viewMode, -1))}
          aria-label="上一段"
        >
          <ChevronLeft size={16} />
        </Button>
        <Text size="2" weight="medium" style={{ minWidth: 180, textAlign: "center" }}>
          {formatRange(selectedDate, viewMode)}
        </Text>
        <Button
          variant="ghost"
          size="1"
          onClick={() => setSelectedDate(shiftDate(selectedDate, viewMode, 1))}
          aria-label="下一段"
        >
          <ChevronRight size={16} />
        </Button>
      </Flex>
      <SegmentedControl.Root
        value={viewMode}
        onValueChange={(v) => setViewMode(v as ViewMode)}
        size="1"
      >
        <SegmentedControl.Item value="day">day</SegmentedControl.Item>
        <SegmentedControl.Item value="week">week</SegmentedControl.Item>
        <SegmentedControl.Item value="month">month</SegmentedControl.Item>
      </SegmentedControl.Root>
    </Flex>
  );
}
```

- [ ] **Step 2: Create RegenerateButton**

Create `src/components/settings/summary/shared/RegenerateButton.tsx`:

```typescript
import { Button } from "@radix-ui/themes";
import { RotateCw } from "lucide-react";

interface RegenerateButtonProps {
  onRegenerate: () => Promise<void>;
  loading: boolean;
}

export function RegenerateButton({ onRegenerate, loading }: RegenerateButtonProps) {
  return (
    <Button
      variant="soft"
      size="1"
      onClick={onRegenerate}
      disabled={loading}
    >
      <RotateCw size={14} className={loading ? "animate-spin" : ""} />
      {loading ? "重新生成中..." : "重新生成"}
    </Button>
  );
}
```

- [ ] **Step 3: Create DayView**

Create `src/components/settings/summary/views/DayView.tsx`:

```typescript
import { Box, Flex, Text } from "@radix-ui/themes";
import { useState } from "react";
import { useTaskClusters } from "../hooks/useTaskClusters";
import { useSummaryStore } from "../stores/summaryStore";
import { ClusterCard } from "../cluster/ClusterCard";
import { ClusterDetailDrawer } from "../cluster/ClusterDetailDrawer";
import { SplitClusterDialog } from "../cluster/SplitClusterDialog";
import { MergeClusterDialog } from "../cluster/MergeClusterDialog";
import { DeleteClusterConfirm } from "../cluster/DeleteClusterConfirm";
import type { TaskCluster } from "../summaryTypes";

interface DayViewProps {
  onNavigateToHistory: (entryId: number) => void;
}

export function DayView({ onNavigateToHistory }: DayViewProps) {
  const { selectedDate, expandedClusterIds, toggleClusterExpanded } = useSummaryStore();
  const {
    clusters,
    loading,
    generating,
    generate,
    updateField,
    split,
    merge,
    remove,
  } = useTaskClusters(selectedDate);

  const [splitTarget, setSplitTarget] = useState<TaskCluster | null>(null);
  const [mergeTarget, setMergeTarget] = useState<TaskCluster | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<TaskCluster | null>(null);

  // Auto-generate if cache empty on first mount of this date
  // The hook's refresh runs once; if it returns empty, trigger generate(false).
  // We only auto-generate if there are no clusters AND not already generating.
  // Note: the empty-day case is also handled here (backend will return [] with no LLM call).
  if (!loading && !generating && clusters.length === 0) {
    // fire-and-forget
    void generate(false);
  }

  if (loading) {
    return <Text size="2" color="gray">加载中...</Text>;
  }
  if (generating && clusters.length === 0) {
    return <Text size="2" color="gray">AI 生成中...</Text>;
  }
  if (clusters.length === 0) {
    return (
      <Box className="text-center py-12">
        <Text size="3" color="gray">今天没有转录</Text>
      </Box>
    );
  }

  return (
    <Box>
      <Text size="2" weight="medium" mb="2">📌 今日聚类（{clusters.length}）</Text>
      {clusters.map((c) => (
        <ClusterCard
          key={c.id}
          cluster={c}
          expanded={expandedClusterIds.has(c.id)}
          onToggleExpanded={() => toggleClusterExpanded(c.id)}
          onUpdateField={(field, value) => updateField(c.id, field, value)}
          onOpenSplit={() => setSplitTarget(c)}
          onOpenMerge={() => setMergeTarget(c)}
          onOpenDelete={() => setDeleteTarget(c)}
          detailSlot={
            <ClusterDetailDrawer
              cluster={c}
              onNavigateToHistory={onNavigateToHistory}
            />
          }
        />
      ))}

      <SplitClusterDialog
        open={splitTarget !== null}
        cluster={splitTarget}
        onCancel={() => setSplitTarget(null)}
        onConfirm={async (extractIds, newTitle, extractedDurationMs) => {
          if (!splitTarget) return;
          await split(splitTarget.id, extractIds, newTitle, extractedDurationMs);
        }}
      />
      <MergeClusterDialog
        open={mergeTarget !== null}
        targetCluster={mergeTarget}
        otherClusters={clusters}
        onCancel={() => setMergeTarget(null)}
        onConfirm={async (sourceIds) => {
          if (!mergeTarget) return;
          await merge(mergeTarget.id, sourceIds);
        }}
      />
      <DeleteClusterConfirm
        open={deleteTarget !== null}
        clusterTitle={deleteTarget?.title ?? ""}
        onCancel={() => setDeleteTarget(null)}
        onConfirm={async () => {
          if (!deleteTarget) return;
          await remove(deleteTarget.id);
          setDeleteTarget(null);
        }}
      />
    </Box>
  );
}
```

- [ ] **Step 4: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add src/components/settings/summary/shared/ src/components/settings/summary/views/DayView.tsx
git commit -m "Add PeriodSelector, RegenerateButton, and DayView"
```

---

### Task 17: WeekView + MonthView

**Files:**

- Create: `src/components/settings/summary/views/WeekView.tsx`
- Create: `src/components/settings/summary/views/MonthView.tsx`

- [ ] **Step 1: Create WeekView**

Create `src/components/settings/summary/views/WeekView.tsx`:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { Box, Flex, Grid, Text, Badge } from "@radix-ui/themes";
import { useEffect, useState } from "react";
import { useSummaryStore } from "../stores/summaryStore";
import type { TaskCluster } from "../summaryTypes";
import { Card } from "@/components/ui/Card";

function weekDates(anchor: string): string[] {
  const d = new Date(anchor);
  const start = new Date(d);
  start.setDate(d.getDate() - d.getDay());
  return Array.from({ length: 7 }, (_, i) => {
    const x = new Date(start);
    x.setDate(start.getDate() + i);
    return x.toISOString().slice(0, 10);
  });
}

export function WeekView() {
  const { selectedDate, setSelectedDate, setViewMode } = useSummaryStore();
  const [byDate, setByDate] = useState<Record<string, TaskCluster[]>>({});

  useEffect(() => {
    const dates = weekDates(selectedDate);
    let cancelled = false;
    (async () => {
      const acc: Record<string, TaskCluster[]> = {};
      for (const d of dates) {
        try {
          const list = await invoke<TaskCluster[]>("get_task_clusters_by_date", { date: d });
          acc[d] = list;
        } catch {
          acc[d] = [];
        }
      }
      if (!cancelled) setByDate(acc);
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedDate]);

  const dates = weekDates(selectedDate);
  const allKeywords = Object.values(byDate)
    .flat()
    .flatMap((c) => c.keywords);
  const kwCounts = new Map<string, number>();
  for (const k of allKeywords) kwCounts.set(k, (kwCounts.get(k) ?? 0) + 1);
  const topKw = Array.from(kwCounts.entries())
    .sort((a, b) => b[1] - a[1])
    .slice(0, 12);

  return (
    <Box>
      {topKw.length > 0 && (
        <Card className="mb-3">
          <Text size="1" color="gray" mb="1">本周热点</Text>
          <Flex gap="1" wrap="wrap">
            {topKw.map(([k, n]) => (
              <Badge key={k} size="1" color={n >= 3 ? "blue" : "gray"} variant="soft">
                {k} {n > 1 && `×${n}`}
              </Badge>
            ))}
          </Flex>
        </Card>
      )}
      <Grid columns="7" gap="2">
        {dates.map((d) => (
          <Box key={d}>
            <Flex align="center" justify="between" mb="1">
              <Text size="1" weight="medium">{d.slice(5)}</Text>
              <button
                type="button"
                onClick={() => {
                  setSelectedDate(d);
                  setViewMode("day");
                }}
                className="text-xs text-blue-600 hover:underline"
              >
                打开
              </button>
            </Flex>
            <Flex direction="column" gap="1">
              {(byDate[d] ?? []).map((c) => (
                <Card key={c.id} className="p-2">
                  <Text size="1" weight="medium" className="truncate">
                    {c.title}
                  </Text>
                  <Text size="1" color="gray">
                    {c.entry_count}e · {Math.round(c.total_duration_ms / 60000)}m
                  </Text>
                </Card>
              ))}
              {(byDate[d] ?? []).length === 0 && (
                <Text size="1" color="gray">—</Text>
              )}
            </Flex>
          </Box>
        ))}
      </Grid>
    </Box>
  );
}
```

- [ ] **Step 2: Create MonthView**

Create `src/components/settings/summary/views/MonthView.tsx`:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { Box, Flex, Grid, Text, Badge } from "@radix-ui/themes";
import { useEffect, useState } from "react";
import { useSummaryStore } from "../stores/summaryStore";
import type { TaskCluster } from "../summaryTypes";
import { Card } from "@/components/ui/Card";

function monthDates(anchor: string): string[] {
  const d = new Date(anchor);
  const year = d.getFullYear();
  const month = d.getMonth();
  const last = new Date(year, month + 1, 0).getDate();
  return Array.from({ length: last }, (_, i) => {
    const x = new Date(year, month, i + 1);
    return x.toISOString().slice(0, 10);
  });
}

export function MonthView() {
  const { selectedDate, setSelectedDate, setViewMode } = useSummaryStore();
  const [byDate, setByDate] = useState<Record<string, TaskCluster[]>>({});

  useEffect(() => {
    const dates = monthDates(selectedDate);
    let cancelled = false;
    (async () => {
      const acc: Record<string, TaskCluster[]> = {};
      for (const d of dates) {
        try {
          const list = await invoke<TaskCluster[]>("get_task_clusters_by_date", { date: d });
          acc[d] = list;
        } catch {
          acc[d] = [];
        }
      }
      if (!cancelled) setByDate(acc);
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedDate]);

  const dates = monthDates(selectedDate);

  return (
    <Box>
      <Grid columns="7" gap="1">
        {dates.map((d) => {
          const clusters = byDate[d] ?? [];
          const total = clusters.reduce((s, c) => s + c.total_duration_ms, 0);
          const minutes = Math.round(total / 60000);
          return (
            <Card
              key={d}
              className="p-1 cursor-pointer hover:bg-gray-50 dark:hover:bg-gray-900"
            >
              <button
                type="button"
                onClick={() => {
                  setSelectedDate(d);
                  setViewMode("day");
                }}
                className="w-full text-left"
              >
                <Text size="1" weight="medium">{d.slice(-2)}</Text>
                {clusters.length > 0 ? (
                  <Flex direction="column" gap="0">
                    <Text size="1" color="gray">{clusters.length} clusters</Text>
                    <Text size="1" color="gray">{minutes}m</Text>
                  </Flex>
                ) : (
                  <Text size="1" color="gray">—</Text>
                )}
              </button>
            </Card>
          );
        })}
      </Grid>
    </Box>
  );
}
```

- [ ] **Step 3: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/views/
git commit -m "Add WeekView and MonthView with per-day cluster aggregation"
```

---

## Milestone 8: AUX Panel

### Task 18: AuxPanel container + StatsSection

**Files:**

- Create: `src/components/settings/summary/aux/AuxPanel.tsx`
- Create: `src/components/settings/summary/aux/StatsSection.tsx`

- [ ] **Step 1: Create AuxPanel (drawer with chips)**

Create `src/components/settings/summary/aux/AuxPanel.tsx`:

```typescript
import { Box, Flex, IconButton, Text } from "@radix-ui/themes";
import { ChevronRight, X } from "lucide-react";
import { useSummaryStore } from "../stores/summaryStore";
import type { AuxSection } from "../summaryTypes";

const SECTIONS: { key: AuxSection; label: string }[] = [
  { key: "stats", label: "Stats" },
  { key: "recap", label: "Recap" },
  { key: "profile", label: "Profile" },
  { key: "hotword", label: "Hotword" },
  { key: "export", label: "Export" },
  { key: "feedback", label: "Feedback History" },
];

interface AuxPanelProps {
  sectionsContent: Record<AuxSection, React.ReactNode>;
}

export function AuxPanel({ sectionsContent }: AuxPanelProps) {
  const { auxPanelOpen, auxActiveSection, openAuxPanel, closeAuxPanel, setAuxSection } =
    useSummaryStore();

  return (
    <>
      <Box className="border-y border-gray-200 dark:border-gray-800 py-2 my-3">
        <Flex align="center" gap="2" wrap="wrap">
          <Text size="1" color="gray" className="opacity-70">▸ AUX</Text>
          {SECTIONS.map((s) => (
            <button
              key={s.key}
              type="button"
              onClick={() => openAuxPanel(s.key)}
              className={
                "text-xs px-2 py-0.5 rounded-full border " +
                (auxPanelOpen && auxActiveSection === s.key
                  ? "border-blue-500 text-blue-600"
                  : "border-gray-300 dark:border-gray-700 text-gray-600 hover:border-gray-500")
              }
            >
              {s.label}
            </button>
          ))}
        </Flex>
      </Box>

      {auxPanelOpen && (
        <Box
          className="fixed top-0 right-0 h-full w-96 bg-[var(--color-panel-solid)] shadow-xl border-l border-gray-200 dark:border-gray-800 z-50 overflow-y-auto"
        >
          <Flex justify="between" align="center" className="p-3 border-b border-gray-200 dark:border-gray-800">
            <Flex gap="2" align="center">
              <Text size="2" weight="bold">
                {SECTIONS.find((s) => s.key === auxActiveSection)?.label}
              </Text>
            </Flex>
            <IconButton size="1" variant="ghost" onClick={closeAuxPanel}>
              <X size={16} />
            </IconButton>
          </Flex>
          <Flex direction="column" className="p-3">
            <Flex gap="1" mb="3" wrap="wrap">
              {SECTIONS.map((s) => (
                <button
                  key={s.key}
                  type="button"
                  onClick={() => setAuxSection(s.key)}
                  className={
                    "text-xs px-2 py-0.5 rounded " +
                    (s.key === auxActiveSection
                      ? "bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-200"
                      : "text-gray-600 hover:bg-gray-100 dark:hover:bg-gray-900")
                  }
                >
                  {s.label}
                </button>
              ))}
            </Flex>
            <Box>{sectionsContent[auxActiveSection]}</Box>
          </Flex>
        </Box>
      )}
    </>
  );
}
```

- [ ] **Step 2: Create StatsSection**

Create `src/components/settings/summary/aux/StatsSection.tsx` by extracting the four metric cards from the legacy `SummaryStats.tsx`. Since the legacy file is 299 lines, copy only the `StatCard` component plus the four-card row, removing the App distribution chart and hourly chart for now (those can be added in a follow-up task if needed).

```typescript
import { Box, Flex, Grid, Text } from "@radix-ui/themes";
import type { SummaryStats as SummaryStatsType } from "../summaryTypes";

interface StatsSectionProps {
  stats: SummaryStatsType | null;
  loading: boolean;
}

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h${m}m`;
  return `${m}m`;
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <Box className="rounded-lg p-3 bg-gray-50 dark:bg-gray-900">
      <Text size="1" color="gray" className="uppercase tracking-wide">
        {label}
      </Text>
      <Text size="5" weight="bold" className="block mt-1">
        {value}
      </Text>
    </Box>
  );
}

export function StatsSection({ stats, loading }: StatsSectionProps) {
  if (loading) return <Text size="2" color="gray">加载中...</Text>;
  if (!stats) return <Text size="2" color="gray">无统计数据</Text>;

  return (
    <Grid columns="2" gap="2">
      <StatCard label="Entries" value={String(stats.entry_count ?? 0)} />
      <StatCard label="Duration" value={formatDuration(stats.total_duration_ms ?? 0)} />
      <StatCard label="Chars" value={String(stats.total_chars ?? 0)} />
      <StatCard label="LLM Calls" value={String(stats.llm_calls ?? 0)} />
    </Grid>
  );
}
```

**Note:** If `SummaryStats` interface fields differ, open `summaryTypes.ts` and adjust the field names referenced above. The above assumes `entry_count`, `total_duration_ms`, `total_chars`, `llm_calls` exist.

- [ ] **Step 3: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors (or surface concrete name mismatches to fix).

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/summary/aux/AuxPanel.tsx src/components/settings/summary/aux/StatsSection.tsx
git commit -m "Add AuxPanel drawer and minimal StatsSection"
```

---

### Task 19: Recap / Profile / Hotword / Export sections (extract from AiAnalysisSection)

**Files:**

- Create: `src/components/settings/summary/aux/RecapSection.tsx`
- Create: `src/components/settings/summary/aux/ProfileSection.tsx`
- Create: `src/components/settings/summary/aux/HotwordSection.tsx`
- Create: `src/components/settings/summary/aux/ExportSection.tsx`

- [ ] **Step 1: Read the legacy AiAnalysisSection**

Run: `wc -l src/components/settings/summary/AiAnalysisSection.tsx`
Open the file and identify the four subsystems:

- Recap rendering (headline / key progress / friction points / next focus)
- UserProfile rendering (vocabulary / expression / time pattern tabs)
- Hotword extraction UI
- Export buttons (Markdown / JSON)

Per the spec, **logic stays the same — only relocation and visual de-emphasis**. Each section becomes a small standalone component that takes the existing data as props.

- [ ] **Step 2: Create RecapSection**

Create `src/components/settings/summary/aux/RecapSection.tsx`. Lift the Recap-related JSX from `AiAnalysisSection.tsx` and put it here, accepting `summary: Summary | null` as props:

```typescript
import { Box, Flex, Text } from "@radix-ui/themes";
import type { Summary } from "../summaryTypes";

interface RecapSectionProps {
  summary: Summary | null;
}

export function RecapSection({ summary }: RecapSectionProps) {
  const recap = summary?.ai_content?.recap;
  if (!recap) {
    return <Text size="2" color="gray">尚无 Recap</Text>;
  }

  return (
    <Flex direction="column" gap="3">
      {recap.headline && (
        <Box>
          <Text size="1" color="gray">标题</Text>
          <Text size="3" weight="bold">{recap.headline}</Text>
        </Box>
      )}
      {recap.key_progress && recap.key_progress.length > 0 && (
        <Box>
          <Text size="1" color="gray" mb="1">关键进展</Text>
          <ul className="list-disc list-inside text-sm">
            {recap.key_progress.map((p: string, i: number) => <li key={i}>{p}</li>)}
          </ul>
        </Box>
      )}
      {recap.friction_points && recap.friction_points.length > 0 && (
        <Box>
          <Text size="1" color="gray" mb="1">摩擦点</Text>
          <ul className="list-disc list-inside text-sm">
            {recap.friction_points.map((p: string, i: number) => <li key={i}>{p}</li>)}
          </ul>
        </Box>
      )}
      {recap.next_focus && (
        <Box>
          <Text size="1" color="gray">下一步重点</Text>
          <Text size="2">{recap.next_focus}</Text>
        </Box>
      )}
    </Flex>
  );
}
```

**Important:** if `Summary` does not have an `ai_content.recap` shape with these fields, inspect the old `AiAnalysisSection.tsx` to discover the actual JSON parsing logic and replicate it here. The above template assumes a discoverable typed shape — adapt accordingly.

- [ ] **Step 3: Create ProfileSection**

Create `src/components/settings/summary/aux/ProfileSection.tsx`. Copy the Profile-related JSX from `AiAnalysisSection.tsx` and adapt to take `userProfile: UserProfile | null` as a prop. The three tabs (vocabulary / expression / time pattern) should be preserved using Radix `Tabs.Root`. Keep the same render logic and styling as the legacy code — do not reimagine the UX here, only relocate.

```typescript
import { Tabs, Text, Flex } from "@radix-ui/themes";
import type { UserProfile } from "../summaryTypes";

interface ProfileSectionProps {
  userProfile: UserProfile | null;
}

export function ProfileSection({ userProfile }: ProfileSectionProps) {
  if (!userProfile) return <Text size="2" color="gray">尚无 Profile 数据</Text>;
  return (
    <Tabs.Root defaultValue="vocab">
      <Tabs.List>
        <Tabs.Trigger value="vocab">词汇</Tabs.Trigger>
        <Tabs.Trigger value="expr">表达</Tabs.Trigger>
        <Tabs.Trigger value="time">时间</Tabs.Trigger>
      </Tabs.List>
      <Tabs.Content value="vocab">
        <Flex direction="column" gap="1" mt="2">
          <Text size="2">{JSON.stringify(userProfile.vocabulary_stats ?? {}, null, 2)}</Text>
        </Flex>
      </Tabs.Content>
      <Tabs.Content value="expr">
        <Flex direction="column" gap="1" mt="2">
          <Text size="2">{JSON.stringify(userProfile.expression_stats ?? {}, null, 2)}</Text>
        </Flex>
      </Tabs.Content>
      <Tabs.Content value="time">
        <Flex direction="column" gap="1" mt="2">
          <Text size="2">{JSON.stringify(userProfile.time_pattern_stats ?? {}, null, 2)}</Text>
        </Flex>
      </Tabs.Content>
    </Tabs.Root>
  );
}
```

Then go back and replace the raw-JSON renders with the actual pretty-print JSX that lived in `AiAnalysisSection.tsx`. Do not invent new visualizations — only port what existed.

- [ ] **Step 4: Create HotwordSection**

Create `src/components/settings/summary/aux/HotwordSection.tsx`. Copy the Hotword extraction UI from `AiAnalysisSection.tsx` (the part that shows extracted hotwords and the "add to ASR vocabulary" action). Keep the existing `invoke()` calls. Accept `summary: Summary | null` as a prop.

```typescript
import { Button, Flex, Text } from "@radix-ui/themes";
import type { Summary } from "../summaryTypes";

interface HotwordSectionProps {
  summary: Summary | null;
  onAddToVocabulary: (word: string) => Promise<void>;
}

export function HotwordSection({ summary, onAddToVocabulary }: HotwordSectionProps) {
  const hotwords = summary?.ai_content?.hotwords ?? [];
  if (hotwords.length === 0) {
    return <Text size="2" color="gray">尚未提取 hotword</Text>;
  }
  return (
    <Flex direction="column" gap="2">
      {hotwords.map((w: string) => (
        <Flex key={w} justify="between" align="center">
          <Text size="2">{w}</Text>
          <Button size="1" variant="soft" onClick={() => onAddToVocabulary(w)}>
            加入词表
          </Button>
        </Flex>
      ))}
    </Flex>
  );
}
```

Adapt the prop name `onAddToVocabulary` to whatever the legacy code calls (likely an inline `invoke('add_hotword', ...)`). Port the actual implementation rather than the stub above.

- [ ] **Step 5: Create ExportSection**

Create `src/components/settings/summary/aux/ExportSection.tsx`:

```typescript
import { Button, Flex } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

interface ExportSectionProps {
  summaryId: number | null;
}

export function ExportSection({ summaryId }: ExportSectionProps) {
  if (!summaryId) return null;

  const exportAs = async (format: "markdown" | "json") => {
    try {
      const content = await invoke<string>("export_summary", { summaryId, format });
      await writeText(content);
      toast.success(`${format.toUpperCase()} 已复制到剪贴板`);
    } catch (e) {
      toast.error(`导出失败: ${e}`);
    }
  };

  return (
    <Flex direction="column" gap="2">
      <Button size="2" variant="soft" onClick={() => exportAs("markdown")}>
        导出 Markdown
      </Button>
      <Button size="2" variant="soft" onClick={() => exportAs("json")}>
        导出 JSON
      </Button>
    </Flex>
  );
}
```

- [ ] **Step 6: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no new type errors. If the legacy types differ, fix the prop accessors to match.

- [ ] **Step 7: Commit**

```bash
git add src/components/settings/summary/aux/
git commit -m "Extract RecapSection, ProfileSection, HotwordSection, ExportSection from AiAnalysisSection"
```

---

### Task 20: FeedbackHistorySection

**Files:**

- Create: `src/components/settings/summary/aux/FeedbackHistorySection.tsx`

- [ ] **Step 1: Create the component**

Create `src/components/settings/summary/aux/FeedbackHistorySection.tsx`:

```typescript
import { Badge, Box, Button, Flex, Text } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { ClusterFeedback } from "../summaryTypes";

export function FeedbackHistorySection() {
  const [items, setItems] = useState<ClusterFeedback[]>([]);
  const [loading, setLoading] = useState(false);

  const refresh = async () => {
    setLoading(true);
    try {
      const list = await invoke<ClusterFeedback[]>(
        "list_recent_negative_cluster_feedback",
        { days: 30, limit: 50 }
      );
      setItems(list);
    } catch (e) {
      toast.error(`加载失败: ${e}`);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const remove = async (id: number) => {
    try {
      await invoke("delete_cluster_feedback", { id });
      await refresh();
    } catch (e) {
      toast.error(`删除失败: ${e}`);
    }
  };

  if (loading) return <Text size="2" color="gray">加载中...</Text>;
  if (items.length === 0)
    return <Text size="2" color="gray">最近 30 天暂无负反馈记录</Text>;

  return (
    <Flex direction="column" gap="2">
      <Text size="1" color="gray">
        最近 30 天的 👎+备注 反馈（这些备注会注入下次 AI 聚类的 prompt）
      </Text>
      {items.map((f) => (
        <Box
          key={f.id}
          className="rounded-md border border-gray-200 dark:border-gray-800 p-2"
        >
          <Flex justify="between" align="start" gap="2">
            <Box className="flex-1">
              <Flex gap="1" align="center" mb="1">
                <Badge color="red" size="1">👎</Badge>
                <Text size="1" color="gray">
                  {new Date(f.created_at).toLocaleString()}
                </Text>
              </Flex>
              <Text size="2">{f.note ?? "(空备注)"}</Text>
            </Box>
            <Button size="1" variant="ghost" color="gray" onClick={() => remove(f.id)}>
              删除
            </Button>
          </Flex>
        </Box>
      ))}
    </Flex>
  );
}
```

- [ ] **Step 2: Verify**

Run: `bun run tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/summary/aux/FeedbackHistorySection.tsx
git commit -m "Add FeedbackHistorySection listing recent negative feedback"
```

---

## Milestone 9: Integration & Cleanup

### Task 21: New SummaryPage orchestrator

**Files:**

- Modify: `src/components/settings/summary/SummaryPage.tsx`

- [ ] **Step 1: Replace SummaryPage with the new orchestrator**

Overwrite `src/components/settings/summary/SummaryPage.tsx` with a slimmed-down orchestrator that:

1. Reads `viewMode` and `selectedDate` from `useSummaryStore`
2. Renders `PeriodSelector` + `RegenerateButton` in a top bar
3. Routes to DayView / WeekView / MonthView based on `viewMode`
4. Renders `AuxPanel` with all 6 sections
5. Hooks legacy `useSummary` for stats/profile/summary needed by AUX sections

```typescript
import { Box, Flex } from "@radix-ui/themes";
import { useSummaryStore } from "./stores/summaryStore";
import { useSummary } from "./hooks/useSummary";
import { useTaskClusters } from "./hooks/useTaskClusters";
import { PeriodSelector } from "./shared/PeriodSelector";
import { RegenerateButton } from "./shared/RegenerateButton";
import { DayView } from "./views/DayView";
import { WeekView } from "./views/WeekView";
import { MonthView } from "./views/MonthView";
import { AuxPanel } from "./aux/AuxPanel";
import { StatsSection } from "./aux/StatsSection";
import { RecapSection } from "./aux/RecapSection";
import { ProfileSection } from "./aux/ProfileSection";
import { HotwordSection } from "./aux/HotwordSection";
import { ExportSection } from "./aux/ExportSection";
import { FeedbackHistorySection } from "./aux/FeedbackHistorySection";
import type { AuxSection } from "./summaryTypes";

interface SummaryPageProps {
  onNavigateToDashboardEntry?: (entryId: number) => void;
}

export function SummaryPage({ onNavigateToDashboardEntry }: SummaryPageProps) {
  const { viewMode, selectedDate } = useSummaryStore();
  const { stats, summary, userProfile, loading: summaryLoading } = useSummary(
    selectedDate,
    viewMode
  );
  const { generate, generating } = useTaskClusters(selectedDate);

  const auxSections: Record<AuxSection, React.ReactNode> = {
    stats: <StatsSection stats={stats} loading={summaryLoading} />,
    recap: <RecapSection summary={summary} />,
    profile: <ProfileSection userProfile={userProfile} />,
    hotword: (
      <HotwordSection
        summary={summary}
        onAddToVocabulary={async () => {
          /* original legacy implementation */
        }}
      />
    ),
    export: <ExportSection summaryId={summary?.id ?? null} />,
    feedback: <FeedbackHistorySection />,
  };

  const navigateToHistory = (entryId: number) => {
    onNavigateToDashboardEntry?.(entryId);
  };

  return (
    <Box className="p-4 max-w-5xl mx-auto">
      <Flex justify="between" align="center" mb="3">
        <PeriodSelector />
        {viewMode === "day" && (
          <RegenerateButton
            onRegenerate={() => generate(true)}
            loading={generating}
          />
        )}
      </Flex>

      <AuxPanel sectionsContent={auxSections} />

      {viewMode === "day" && <DayView onNavigateToHistory={navigateToHistory} />}
      {viewMode === "week" && <WeekView />}
      {viewMode === "month" && <MonthView />}
    </Box>
  );
}
```

**Important:** the `useSummary` hook's signature should still match — read the existing hook and pass arguments correctly. If the old `SummaryPage.tsx` accepted props (e.g. `onNavigate`), keep those for backward compat with the parent. If it didn't, ensure the parent caller (App.tsx) imports correctly.

Check the existing default export style — if the old file uses `export default function SummaryPage(...)`, match that. Run: `rg -n 'import.*SummaryPage' src/`.

- [ ] **Step 2: Run dev build and verify the page renders**

Run: `bun tauri dev` in a separate terminal. Manually:

1. Switch to the Summary tab (Ctrl+2)
2. Confirm the day view loads, AUX chips show, period selector works
3. Switch view to week and month

Document any visual breakage and fix before committing.

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/summary/SummaryPage.tsx
git commit -m "Rewrite SummaryPage as cluster-axis orchestrator"
```

---

### Task 22: Remove legacy build_recap + AiAnalysisSection, retire summary.stats.task_clusters

**Files:**

- Modify: `src-tauri/src/managers/summary.rs`
- Delete: `src/components/settings/summary/AiAnalysisSection.tsx`
- Delete: `src/components/settings/summary/SummaryStats.tsx` (functionality now in `aux/StatsSection.tsx`)
- Delete: `src/components/settings/summary/SummaryTimeline.tsx` (functionality merged into RecapSection)

- [ ] **Step 1: Find all callers of `build_recap()` and `task_clusters` in stats**

Run: `rg -n 'build_recap|task_clusters' src-tauri/src/managers/summary.rs`

Identify the function(s) that construct `SummaryStats` and currently populate `task_clusters`. Remove the population — the new system reads from `task_clusters` table instead. Stats can still include other aggregates (entry count, duration, char count, llm_calls).

- [ ] **Step 2: Delete `build_recap` and its helper functions**

In `src-tauri/src/managers/summary.rs`, delete:

- `build_recap()`
- `extract_keywords()` (only if used solely by build_recap)
- `detect_blockers()` (same condition)
- The call site in `calculate_stats()` that invokes `build_recap`

If any of those helpers are used elsewhere, keep them. Run `rg -n 'extract_keywords|detect_blockers' src-tauri/src` to check.

- [ ] **Step 3: Update the `SummaryStats` Rust struct**

In `src-tauri/src/managers/summary.rs`, remove the `task_clusters: Option<Vec<TaskClusterV1>>` field if present. Or, if the field name is just `task_clusters`, replace it with a `#[serde(skip_deserializing, default)]` marker so existing JSON in the DB still deserializes:

```rust
#[serde(default, skip)]
pub task_clusters: Vec<()>, // retained for backwards compat with deserializing old JSON
```

Better: leave the field for read compat but don't populate it on writes. Confirm at compile time that nothing else reads `stats.task_clusters` after this change.

- [ ] **Step 4: Delete the obsolete frontend files**

```bash
rm src/components/settings/summary/AiAnalysisSection.tsx
rm src/components/settings/summary/SummaryStats.tsx
rm src/components/settings/summary/SummaryTimeline.tsx
```

If anything imports these, fix the imports (re-export from `aux/` or update the caller). Run: `rg -n 'AiAnalysisSection|SummaryStats|SummaryTimeline' src/ | head -20`.

- [ ] **Step 5: Verify everything still compiles**

Run: `cd src-tauri && cargo check 2>&1 | tail -10`
Run: `bun run tsc --noEmit 2>&1 | tail -20`
Expected: both PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/managers/summary.rs
git rm src/components/settings/summary/AiAnalysisSection.tsx src/components/settings/summary/SummaryStats.tsx src/components/settings/summary/SummaryTimeline.tsx
git commit -m "Remove legacy build_recap heuristic and AiAnalysisSection"
```

---

### Task 23: Migrate legacy summary.stats.task_clusters into new table

**Files:**

- Modify: `src-tauri/src/managers/task_clusters.rs`
- Modify: `src-tauri/src/lib.rs` (call the migration on app startup)

- [ ] **Step 1: Write test for legacy migration**

Append to `task_clusters.rs` tests:

```rust
#[test]
fn test_migrate_from_legacy_stats_inserts_clusters() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let m = TaskClustersManager::new(tmp.path().to_path_buf()).unwrap();

    // Pretend a legacy summary row provides this JSON
    let legacy_json = r#"[
        {
            "title": "Legacy A",
            "status": "进行中",
            "time_span": "10:00-11:00",
            "apps": ["Cursor"],
            "entry_count": 3,
            "total_duration_ms": 3600000,
            "summary": "x",
            "blockers": [],
            "next_step": null,
            "keywords": []
        }
    ]"#;

    m.migrate_legacy_stats_clusters(42, "2026-04-01", legacy_json).unwrap();

    let rows = m.get_by_date("2026-04-01").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Legacy A");
    assert_eq!(rows[0].source_history_ids, Vec::<i64>::new());
    assert!(!rows[0].is_user_modified);

    // Re-run is idempotent (no duplicate insert because we check by summary+date+title combo)
    m.migrate_legacy_stats_clusters(42, "2026-04-01", legacy_json).unwrap();
    let rows2 = m.get_by_date("2026-04-01").unwrap();
    assert_eq!(rows2.len(), 1);
}
```

- [ ] **Step 2: Implement `migrate_legacy_stats_clusters` and `has_migrated_for_summary`**

Append to `impl TaskClustersManager`:

```rust
    pub fn has_clusters_for_summary(&self, summary_id: i64) -> Result<bool> {
        let conn = self.get_connection()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM task_clusters WHERE summary_id=?",
            [summary_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn migrate_legacy_stats_clusters(
        &self,
        summary_id: i64,
        date: &str,
        legacy_json: &str,
    ) -> Result<usize> {
        if self.has_clusters_for_summary(summary_id)? {
            // Idempotency guard
            return Ok(0);
        }
        #[derive(Deserialize)]
        struct LegacyCluster {
            #[serde(default)]
            title: String,
            #[serde(default)]
            status: String,
            #[serde(default)]
            time_span: Option<String>,
            #[serde(default)]
            apps: Vec<String>,
            #[serde(default)]
            entry_count: i64,
            #[serde(default)]
            total_duration_ms: i64,
            #[serde(default)]
            summary: Option<String>,
            #[serde(default)]
            blockers: Vec<String>,
            #[serde(default)]
            next_step: Option<String>,
            #[serde(default)]
            keywords: Vec<String>,
        }
        let legacy: Vec<LegacyCluster> = serde_json::from_str(legacy_json).unwrap_or_default();
        if legacy.is_empty() {
            return Ok(0);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let mut inserted = 0_usize;
        for l in legacy {
            let cluster = TaskCluster {
                id: Self::new_cluster_id(),
                summary_id,
                date: date.to_string(),
                title: if l.title.is_empty() { "(untitled)".into() } else { l.title },
                status: if l.status.is_empty() { "进行中".into() } else { l.status },
                time_span: l.time_span,
                apps: l.apps,
                source_history_ids: vec![],
                total_duration_ms: l.total_duration_ms,
                entry_count: l.entry_count,
                summary: l.summary,
                blockers: l.blockers,
                next_step: l.next_step,
                keywords: l.keywords,
                is_user_modified: false,
                user_modified_fields: vec![],
                created_at: now,
                updated_at: now,
            };
            self.upsert(&cluster)?;
            inserted += 1;
        }
        Ok(inserted)
    }
```

- [ ] **Step 3: Wire one-shot startup migration in lib.rs setup**

In `src-tauri/src/lib.rs`, after `task_clusters_manager` is created, add a one-shot pass:

```rust
{
    let tc = task_clusters_manager.clone();
    let sm = summary_manager.clone();
    tauri::async_runtime::spawn(async move {
        match sm.list_all_summaries_with_legacy_clusters().await {
            Ok(rows) => {
                for (summary_id, date, legacy_json) in rows {
                    if let Err(e) = tc.migrate_legacy_stats_clusters(summary_id, &date, &legacy_json) {
                        eprintln!("legacy cluster migration failed for summary {}: {}", summary_id, e);
                    }
                }
            }
            Err(e) => eprintln!("failed to list legacy summaries: {}", e),
        }
    });
}
```

In `SummaryManager`, add `list_all_summaries_with_legacy_clusters` that queries existing summaries whose `stats` JSON contains a `task_clusters` array and returns `(summary_id, date, legacy_json_array_as_string)`. Implementation hint:

```rust
pub async fn list_all_summaries_with_legacy_clusters(&self) -> Result<Vec<(i64, String, String)>> {
    let conn = self.get_connection()?;
    let mut stmt = conn.prepare(
        "SELECT id, date, stats FROM summaries WHERE stats LIKE '%task_clusters%'"
    )?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        let date: String = row.get(1)?;
        let stats: String = row.get(2)?;
        Ok((id, date, stats))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (id, date, stats) = r?;
        // Extract the task_clusters array
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stats) {
            if let Some(arr) = v.get("task_clusters") {
                if arr.is_array() && !arr.as_array().unwrap().is_empty() {
                    out.push((id, date, arr.to_string()));
                }
            }
        }
    }
    Ok(out)
}
```

If `SummaryManager` uses `tauri::async_runtime::spawn_blocking` for DB work, mirror that pattern.

- [ ] **Step 4: Run tests**

Run: `cd src-tauri && cargo test --lib managers::task_clusters 2>&1 | tail -20`
Expected: all tests PASS including the new migration test.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/managers/task_clusters.rs src-tauri/src/managers/summary.rs src-tauri/src/lib.rs
git commit -m "Migrate legacy summary.stats.task_clusters into task_clusters table"
```

---

## Milestone 10: Manual Verification

### Task 24: Walk all 15 BDD acceptance scenarios

**Files:** (no code changes — manual verification only)

- Reference: `docs/specs/2026-05-15-summary-cluster-refactor.spec.md` scenarios 1-15

- [ ] **Step 1: Start dev build**

```bash
bun tauri dev
```

Wait for the app window to open. Make sure your selected post-process model and provider are configured in Settings → Models (the cluster generator uses them).

- [ ] **Step 2: Scenario 1 — First generation (happy)**

Pre-condition: pick a recent day with at least 10 transcriptions in history. Use the SQLite CLI if needed:

```bash
sqlite3 ~/Library/Application\ Support/com.handy.app/handy.db \
  "SELECT date(timestamp/1000, 'unixepoch','localtime') AS d, COUNT(*) FROM transcription_history WHERE deleted=0 GROUP BY d ORDER BY d DESC LIMIT 7;"
```

In the app: Ctrl+2 → set day to that date. Confirm DayView shows a "AI 生成中..." flash followed by 3-8 cluster cards. Open the SQLite DB:

```bash
sqlite3 ~/Library/Application\ Support/com.handy.app/handy.db \
  "SELECT date, COUNT(*) FROM task_clusters GROUP BY date ORDER BY date DESC LIMIT 5;"
```

Expected: a row for that date with N clusters. ✅

- [ ] **Step 3: Scenario 2 — Cache hit**

Within the same session, click on a different date and back. Watch the dev console — there should be NO new LLM call entry (look for the request URL in the console / network panel). The clusters appear instantly. ✅

- [ ] **Step 4: Scenario 3 — Rename protection**

- Click a cluster's title pencil icon, rename to a new value, press Enter.
- Click ⟳重新生成.
- Expected: the renamed cluster remains with the new name; other clusters may change titles/contents. Open DB to confirm `is_user_modified=1` for that row:

```bash
sqlite3 ~/Library/Application\ Support/com.handy.app/handy.db \
  "SELECT id, title, is_user_modified, user_modified_fields FROM task_clusters WHERE date='YYYY-MM-DD';"
```

✅

- [ ] **Step 5: Scenario 4 — Feedback injection**

- Click 👎 on a cluster, type a note like "Slack 消息应该单独成簇", submit.
- Wait, then click ⟳重新生成.
- In a dev-build log line (or instrument with `eprintln!` if needed in `task_cluster_generator.rs`), confirm the prompt sent to the LLM contains the note text. Verify via console logs or a temporary `tracing::info!` call.
- Optional: Inspect the prompt source visible in the running app's logs to confirm the note appears in the "USER FEEDBACK" block.

✅

- [ ] **Step 6: Scenario 5 — Split**

Pick a cluster with at least 5 source ids. Click 🔀拆分. Select 2-3 entries, enter a new title, confirm.

Expected:

- A new cluster appears with the chosen title.
- The original cluster's `entry_count` and `total_duration_ms` are reduced.
- Both are marked `已编辑` badge (is_user_modified=1).

Confirm via DB:

```bash
sqlite3 ~/Library/Application\ Support/com.handy.app/handy.db \
  "SELECT title, entry_count, is_user_modified FROM task_clusters WHERE date='YYYY-MM-DD' ORDER BY updated_at DESC LIMIT 5;"
```

✅

- [ ] **Step 7: Scenario 6 — Merge**

Pick two clusters. On one, click 🔗合并 → select the other → confirm. The other cluster disappears. The target now has the merged entries. ✅

- [ ] **Step 8: Scenario 7 — Network error**

- Toggle Wi-Fi off (or use `Cmd-Shift-Click` on the menu bar icon).
- Click ⟳重新生成.
- Expected: error toast like "AI 调用失败，已保留上次结果". Existing clusters remain on screen.
- Restore network.

✅

- [ ] **Step 9: Scenario 8 — Empty day**

Navigate to a date with NO transcriptions. Expected: "今天没有转录" empty state, no LLM call. Confirm DB has no row for that date in `task_clusters`. ✅

- [ ] **Step 10: Scenario 9 — Source delete cascade**

- Pick a cluster with at least 3 source entries.
- Switch to Dashboard (Ctrl+1) and delete one of those entries.
- Switch back to Summary day view. The cluster's `entry_count` should now be one less and `source_history_ids` updated.

Confirm via DB inspection. ✅

- [ ] **Step 11: Scenario 10 — JSON parse failure retry**

This is hard to reproduce without test instrumentation. To validate: temporarily add an environment variable check in `task_cluster_generator.rs::parse_llm_output` that randomly corrupts input for testing. Or trust the unit test coverage from Task 6.

Mark as ✅ if the relevant unit tests in Task 6 pass.

- [ ] **Step 12: Scenario 11 — Week view no cross-day**

Switch to `week` view. Confirm 7-day grid, each day has its own cluster cards, no shared cluster cards spanning days. ✅

- [ ] **Step 13: Scenario 12 — 👍 not injected**

Click 👍 on multiple clusters with no notes. Trigger regenerate. Confirm via logs / DB that no feedback content is in the prompt. ✅

- [ ] **Step 14: Scenario 13 — Split validation**

Open SplitDialog. Try to confirm with 0 selected → button disabled. Try with all selected → button disabled with hint. Confirm by inspection. ✅

- [ ] **Step 15: Scenario 14 — AUX drawer**

On first load, AUX is folded into a chips row. Click "Stats" chip → right-side drawer slides in. Switch between Stats / Recap / Profile / Hotword / Export / Feedback History — content swaps without reload. Close → drawer disappears, chips remain. ✅

- [ ] **Step 16: Scenario 15 — Legacy migration idempotency**

If you have summaries from before this refactor with legacy `stats.task_clusters` JSON, restart the app once. Confirm those clusters appear in the new `task_clusters` table:

```bash
sqlite3 ~/Library/Application\ Support/com.handy.app/handy.db \
  "SELECT date, COUNT(*) FROM task_clusters WHERE is_user_modified=0 AND source_history_ids_json='[]' GROUP BY date ORDER BY date DESC;"
```

Restart again. Confirm counts don't change (idempotent guard worked). ✅

- [ ] **Step 17: Documentation update**

Open `docs/specs/2026-05-15-summary-cluster-refactor.spec.md` and fill in the "实施偏差" table at the bottom with any deviations encountered during execution. This is required per project SPEC standards.

- [ ] **Step 18: Final commit**

```bash
git add docs/specs/2026-05-15-summary-cluster-refactor.spec.md
git commit -m "Record implementation deviations and finalize cluster refactor"
```

---

## Self-Review Checklist

After implementation, run this checklist before opening a PR:

- [ ] All 24 tasks committed, branch builds (`bun tauri build` succeeds)
- [ ] `cd src-tauri && cargo test` passes
- [ ] `bun run tsc --noEmit` passes
- [ ] `bun format` ran (no diff)
- [ ] Spec deviation table filled in
- [ ] All 15 BDD scenarios manually verified
- [ ] No `tokio::spawn` from non-async contexts (per CLAUDE.md runtime rules)
- [ ] All prompts in `src-tauri/resources/prompts/*.md`, no hardcoded strings
- [ ] No new direct HTTP clients for LLM — only `execute_llm_request_with_retry`
- [ ] Old `AiAnalysisSection.tsx` / `SummaryStats.tsx` / `SummaryTimeline.tsx` deleted

---

## Open Concerns / Follow-ups (Not in This Plan)

These are intentionally deferred per the spec's 排除范围 section:

- Cross-day "project / theme" clustering
- Real-time incremental clustering as transcriptions arrive
- Integration with external task systems (Linear / Jira / Notion)
- Algorithm upgrades to Recap / UserProfile / Hotword (only location changed in this refactor)
- E2E automated testing (Playwright / Tauri WebDriver)
- Feedback anonymization / semantic compression before prompt injection
