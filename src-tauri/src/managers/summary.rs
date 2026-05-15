use anyhow::Result;
use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppStats {
    pub count: i64,
    pub chars: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DailyOverview {
    pub headline: String,
    pub key_progress: Vec<String>,
    pub friction_points: Vec<String>,
    pub next_focus: String,
}

/// DEPRECATED: replaced by `crate::managers::task_clusters::TaskCluster` (v2 with UUID + source_history_ids). Will be removed in T22.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskCluster {
    pub title: String,
    pub status: String,
    pub time_span: String,
    pub apps: Vec<String>,
    pub entry_count: i64,
    pub total_duration_ms: i64,
    pub summary: String,
    pub blockers: Vec<String>,
    pub next_step: Option<String>,
    pub keywords: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContextPack {
    pub active_tasks: Vec<String>,
    pub recent_decisions: Vec<String>,
    pub pending_followups: Vec<String>,
    pub task_entities: Vec<String>,
    pub context_bias: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummaryStats {
    pub entry_count: i64,
    pub total_duration_ms: i64,
    pub total_chars: i64,
    pub llm_calls: i64,
    pub by_app: std::collections::HashMap<String, AppStats>,
    pub by_hour: Vec<i64>,
    pub top_skills: Vec<String>,
    #[serde(default)]
    pub daily_overview: Option<DailyOverview>,
    #[serde(default)]
    pub task_clusters: Vec<TaskCluster>,
    #[serde(default)]
    pub context_pack: Option<ContextPack>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiAnalysisEntry {
    pub timestamp: i64,
    pub model: String,
    pub summary: String,
    pub reflection: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub id: i64,
    pub period_type: String,
    pub period_start: i64,
    pub period_end: i64,
    pub stats: SummaryStats,
    pub ai_summary: Option<String>,
    pub ai_reflection: Option<String>,
    pub ai_generated_at: Option<i64>,
    pub ai_model_used: Option<String>,
    pub ai_history: Option<Vec<AiAnalysisEntry>>, // Parsed JSON array of AiAnalysisEntry
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserProfile {
    pub vocabulary_stats: Option<String>,
    pub expression_stats: Option<String>,
    pub app_usage_stats: Option<String>,
    pub time_pattern_stats: Option<String>,
    pub communication_style: Option<String>,
    pub tone_preference: Option<String>,
    pub style_prompt: Option<String>,
    pub feedback_style: String,
    pub last_analyzed_at: Option<i64>,
    pub updated_at: i64,
}

/// Default polish prompt ID - used to identify polish results vs skill results
const DEFAULT_POLISH_PROMPT_ID: &str = "system_default_polish";

pub struct SummaryManager {
    db_path: PathBuf,
}

impl SummaryManager {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn get_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    /// Check if a prompt_id represents a polish prompt (not a skill)
    /// Polish prompts: system_default_polish or null/empty (raw polish without explicit prompt)
    fn is_polish_prompt(prompt_id: &Option<String>) -> bool {
        match prompt_id {
            None => true, // No prompt = raw ASR or direct polish
            Some(id) if id.is_empty() => true,
            Some(id) => id == DEFAULT_POLISH_PROMPT_ID,
        }
    }

    fn empty_stats() -> SummaryStats {
        SummaryStats {
            entry_count: 0,
            total_duration_ms: 0,
            total_chars: 0,
            llm_calls: 0,
            by_app: std::collections::HashMap::new(),
            by_hour: vec![0; 24],
            top_skills: Vec::new(),
            daily_overview: None,
            task_clusters: Vec::new(),
            context_pack: None,
        }
    }

    fn extract_keywords(text: &str) -> Vec<String> {
        const KEYWORDS: [(&str, &str); 22] = [
            ("provider", "Provider"),
            ("api key", "API Key"),
            ("base url", "Base URL"),
            ("customer", "Customer"),
            ("custom", "Custom"),
            ("proxy", "Proxy"),
            ("summary", "Summary"),
            ("prompt", "Prompt"),
            ("review", "Review"),
            ("rewrite", "Rewrite"),
            ("model", "Model"),
            ("rate limit", "Rate Limit"),
            ("logseq", "Logseq"),
            ("mcp", "MCP"),
            ("telegram", "Telegram"),
            ("payment", "Payment"),
            ("encrypt", "Encrypt"),
            ("aigne", "Aigne"),
            ("handy", "Handy"),
            ("提供商", "提供商"),
            ("密钥", "API Key"),
            ("总结", "总结"),
        ];

        let normalized = text.to_lowercase();
        let mut keywords = Vec::new();
        for (needle, label) in KEYWORDS {
            if normalized.contains(needle) && !keywords.iter().any(|item| item == label) {
                keywords.push(label.to_string());
            }
        }
        keywords
    }

    fn find_blockers(text: &str) -> Vec<String> {
        const BLOCKERS: [(&str, &str); 18] = [
            ("问题", "出现问题"),
            ("失败", "执行失败"),
            ("异常", "出现异常"),
            ("报错", "出现报错"),
            ("卡住", "流程卡住"),
            ("不对", "结果不对"),
            ("没有", "配置未生效"),
            ("无法", "当前无法完成"),
            ("不能", "当前不能完成"),
            ("不显示", "界面未正确显示"),
            ("不生效", "配置没有生效"),
            ("read only", "只读状态异常"),
            ("error", "发生错误"),
            ("failed", "执行失败"),
            ("issue", "存在问题"),
            ("stuck", "流程卡住"),
            ("why", "存在待解释问题"),
            ("invalid", "配置无效"),
        ];

        let normalized = text.to_lowercase();
        let mut blockers = Vec::new();
        for (needle, label) in BLOCKERS {
            if normalized.contains(needle) && !blockers.iter().any(|item| item == label) {
                blockers.push(label.to_string());
            }
        }
        blockers
    }

    fn derive_title(apps: &[String], keywords: &[String]) -> String {
        if keywords.iter().any(|item| item == "Aigne") {
            return "Aigne 账号与链路排查".to_string();
        }
        if keywords.iter().any(|item| item == "Handy")
            || keywords.iter().any(|item| item == "Provider")
            || keywords.iter().any(|item| item == "API Key")
            || keywords.iter().any(|item| item == "Base URL")
        {
            return "Handy Provider 与配置调整".to_string();
        }
        if keywords.iter().any(|item| item == "Rate Limit")
            || keywords.iter().any(|item| item == "Model")
            || keywords.iter().any(|item| item == "Logseq")
            || keywords.iter().any(|item| item == "MCP")
        {
            return "模型与资料调研".to_string();
        }
        if apps.iter().any(|item| item == "Telegram") {
            return "外部沟通与信息同步".to_string();
        }
        if let Some(first_keyword) = keywords.first() {
            return format!("{first_keyword} 相关推进");
        }
        if let Some(first_app) = apps.first() {
            return format!("{first_app} 场景工作");
        }
        "今日任务推进".to_string()
    }

    fn derive_status(progress_signals: usize, blockers: usize) -> String {
        if progress_signals > 0 && blockers == 0 {
            "有推进".to_string()
        } else if progress_signals > 0 && blockers > 0 {
            "推进中".to_string()
        } else if blockers > 0 {
            "卡住".to_string()
        } else {
            "整理中".to_string()
        }
    }

    fn derive_context_bias(app_counts: &std::collections::HashMap<String, AppStats>) -> String {
        let app_names: Vec<&str> = app_counts.keys().map(String::as_str).collect();
        if app_names
            .iter()
            .any(|app| matches!(*app, "Codex" | "VSCode" | "Xcode" | "Cursor"))
        {
            return "coding".to_string();
        }
        if app_names
            .iter()
            .any(|app| matches!(*app, "Comet" | "Arc" | "Safari" | "Chrome"))
        {
            return "research".to_string();
        }
        if app_names
            .iter()
            .any(|app| matches!(*app, "Telegram" | "微信" | "WeChat"))
        {
            return "communication".to_string();
        }
        "general".to_string()
    }

    fn build_recap(
        &self,
        entries: &[RichAnalysisEntry],
        by_app: &std::collections::HashMap<String, AppStats>,
    ) -> (Option<DailyOverview>, Vec<TaskCluster>, Option<ContextPack>) {
        if entries.is_empty() {
            return (None, Vec::new(), None);
        }

        let mut segments: Vec<Vec<&RichAnalysisEntry>> = Vec::new();
        let mut current: Vec<&RichAnalysisEntry> = Vec::new();
        let mut last_timestamp: Option<i64> = None;
        let mut last_app: Option<&str> = None;

        for entry in entries {
            let should_split = if let (Some(prev_ts), Some(prev_app)) = (last_timestamp, last_app) {
                let ts_gap = entry.timestamp.saturating_sub(prev_ts);
                ts_gap > 20 * 60 || (entry.app != prev_app && ts_gap > 8 * 60)
            } else {
                false
            };

            if should_split && !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            current.push(entry);
            last_timestamp = Some(entry.timestamp);
            last_app = Some(entry.app.as_str());
        }

        if !current.is_empty() {
            segments.push(current);
        }

        let mut clusters = Vec::new();
        for segment in segments {
            let mut apps = Vec::new();
            let mut keywords = Vec::new();
            let mut blockers = Vec::new();
            let mut progress_signals = 0usize;
            let mut duration_ms = 0i64;
            let mut representative = Vec::new();
            let start = segment
                .first()
                .map(|entry| entry.time.clone())
                .unwrap_or_default();
            let end = segment
                .last()
                .map(|entry| entry.time.clone())
                .unwrap_or_default();

            for entry in &segment {
                if !apps.iter().any(|item| item == &entry.app) {
                    apps.push(entry.app.clone());
                }
                for keyword in Self::extract_keywords(&format!("{} {}", entry.window, entry.text)) {
                    if !keywords.iter().any(|item| item == &keyword) && keywords.len() < 5 {
                        keywords.push(keyword);
                    }
                }
                for blocker in Self::find_blockers(&entry.text) {
                    if !blockers.iter().any(|item| item == &blocker) && blockers.len() < 4 {
                        blockers.push(blocker);
                    }
                }
                if matches!(entry.review_action.as_str(), "accept" | "edit_accept")
                    || entry.has_polish
                    || entry.text.contains("完成")
                    || entry.text.contains("改成")
                    || entry.text.to_lowercase().contains("fixed")
                {
                    progress_signals += 1;
                }
                duration_ms += entry.duration_sec * 1000;
                if representative.len() < 2 && !entry.text.trim().is_empty() {
                    representative.push(entry.text.chars().take(36).collect::<String>());
                }
            }

            let title = Self::derive_title(&apps, &keywords);
            let status = Self::derive_status(progress_signals, blockers.len());
            let summary = if representative.is_empty() {
                format!("围绕 {title} 进行了 {} 次交互。", segment.len())
            } else {
                format!("{}。", representative.join("；"))
            };
            let next_step = if blockers.is_empty() {
                representative
                    .last()
                    .map(|item| format!("继续跟进：{}", item.chars().take(24).collect::<String>()))
            } else {
                blockers.first().map(|item| format!("优先处理：{item}"))
            };

            clusters.push(TaskCluster {
                title,
                status,
                time_span: format!("{start} - {end}"),
                apps,
                entry_count: segment.len() as i64,
                total_duration_ms: duration_ms,
                summary,
                blockers,
                next_step,
                keywords,
            });
        }

        clusters.sort_by(|a, b| {
            b.total_duration_ms
                .cmp(&a.total_duration_ms)
                .then_with(|| b.entry_count.cmp(&a.entry_count))
        });
        clusters.truncate(5);

        let key_progress = clusters
            .iter()
            .take(3)
            .map(|cluster| format!("{}：{}", cluster.title, cluster.summary))
            .collect::<Vec<_>>();
        let friction_points = clusters
            .iter()
            .filter_map(|cluster| {
                cluster
                    .blockers
                    .first()
                    .map(|blocker| format!("{}：{}", cluster.title, blocker))
            })
            .take(3)
            .collect::<Vec<_>>();
        let next_focus = clusters
            .iter()
            .find_map(|cluster| cluster.next_step.clone())
            .unwrap_or_else(|| "继续收敛今天最重要的任务并补齐关键缺口。".to_string());
        let headline = if let Some(primary) = clusters.first() {
            format!(
                "今天主要围绕 {} 等 {} 个任务推进。",
                primary.title,
                clusters.len()
            )
        } else {
            "今天有一些零散记录，但尚未形成明确主线。".to_string()
        };

        let task_entities = clusters
            .iter()
            .flat_map(|cluster| cluster.keywords.iter().cloned())
            .fold(Vec::<String>::new(), |mut acc, item| {
                if !acc.iter().any(|existing| existing == &item) && acc.len() < 8 {
                    acc.push(item);
                }
                acc
            });

        let pending_followups = clusters
            .iter()
            .filter_map(|cluster| cluster.next_step.clone())
            .take(4)
            .collect::<Vec<_>>();

        let recent_decisions = clusters
            .iter()
            .filter(|cluster| cluster.status != "卡住")
            .map(|cluster| format!("{}：{}", cluster.title, cluster.status))
            .take(3)
            .collect::<Vec<_>>();

        (
            Some(DailyOverview {
                headline,
                key_progress,
                friction_points,
                next_focus,
            }),
            clusters.clone(),
            Some(ContextPack {
                active_tasks: clusters
                    .iter()
                    .take(3)
                    .map(|item| item.title.clone())
                    .collect(),
                recent_decisions,
                pending_followups,
                task_entities,
                context_bias: Self::derive_context_bias(by_app),
            }),
        )
    }

    /// Calculate stats for a given time range from history entries
    /// Uses polish results when available (ignoring skill outputs), falls back to ASR
    pub fn calculate_stats(&self, start_ts: i64, end_ts: i64) -> Result<SummaryStats> {
        let conn = self.get_connection()?;

        // Get basic counts
        let mut stmt = conn.prepare(
            "SELECT
                COUNT(*) as entry_count,
                COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                COALESCE(SUM(CASE WHEN post_processed_text IS NOT NULL AND post_processed_text != ''
                    AND (post_process_prompt_id IS NULL OR post_process_prompt_id = '' OR post_process_prompt_id = ?1)
                    THEN 1 ELSE 0 END), 0) as llm_calls
            FROM transcription_history
            WHERE timestamp >= ?2 AND timestamp <= ?3 AND deleted = 0",
        )?;

        let (entry_count, total_duration_ms, llm_calls): (i64, i64, i64) = stmt
            .query_row(params![DEFAULT_POLISH_PROMPT_ID, start_ts, end_ts], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;

        // Calculate total_chars using effective text:
        // - Priority 1: polish result (post_processed_text with polish prompt or no prompt)
        // - Priority 2: ASR result (transcription_text)
        // - Ignore: skill outputs (post_processed_text with non-polish prompt)
        let mut chars_stmt = conn.prepare(
            "SELECT
                transcription_text,
                post_processed_text,
                post_process_prompt_id
            FROM transcription_history
            WHERE timestamp >= ?1 AND timestamp <= ?2 AND deleted = 0",
        )?;

        let mut total_chars: i64 = 0;
        let rows = chars_stmt.query_map(params![start_ts, end_ts], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        for row in rows {
            let (transcription_text, post_processed_text, prompt_id) = row?;
            // Use polish result if available and is from polish prompt
            let effective_text = if let Some(ref polish_text) = post_processed_text {
                if !polish_text.is_empty() && Self::is_polish_prompt(&prompt_id) {
                    polish_text.clone()
                } else {
                    transcription_text
                }
            } else {
                transcription_text
            };
            total_chars += effective_text.chars().count() as i64;
        }

        // Get by_app stats with effective text (polish > ASR)
        let mut by_app: std::collections::HashMap<String, AppStats> =
            std::collections::HashMap::new();
        let mut app_stmt = conn.prepare(
            "SELECT app_name, transcription_text, post_processed_text, post_process_prompt_id
            FROM transcription_history
            WHERE timestamp >= ?1 AND timestamp <= ?2 AND deleted = 0 AND app_name IS NOT NULL",
        )?;
        let app_rows = app_stmt.query_map(params![start_ts, end_ts], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in app_rows {
            let (app_name, transcription_text, post_processed_text, prompt_id) = row?;
            // Use polish result if available and is from polish prompt
            let effective_text = if let Some(ref polish_text) = post_processed_text {
                if !polish_text.is_empty() && Self::is_polish_prompt(&prompt_id) {
                    polish_text.clone()
                } else {
                    transcription_text
                }
            } else {
                transcription_text
            };
            let chars = effective_text.chars().count() as i64;
            let entry = by_app
                .entry(app_name)
                .or_insert(AppStats { count: 0, chars: 0 });
            entry.count += 1;
            entry.chars += chars;
        }

        // Get by_hour stats (24 hours)
        let mut by_hour = vec![0i64; 24];
        let mut hour_stmt = conn.prepare(
            "SELECT strftime('%H', datetime(timestamp, 'unixepoch', 'localtime')) as hour, COUNT(*) as count
            FROM transcription_history
            WHERE timestamp >= ?1 AND timestamp <= ?2 AND deleted = 0
            GROUP BY hour",
        )?;
        let hour_rows = hour_stmt.query_map(params![start_ts, end_ts], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in hour_rows {
            let (hour_str, count) = row?;
            if let Ok(hour) = hour_str.parse::<usize>() {
                if hour < 24 {
                    by_hour[hour] = count;
                }
            }
        }

        // Get top skills
        let mut top_skills = Vec::new();
        let mut skill_stmt = conn.prepare(
            "SELECT post_process_prompt_id, COUNT(*) as count
            FROM transcription_history
            WHERE timestamp >= ?1 AND timestamp <= ?2 AND deleted = 0
                AND post_process_prompt_id IS NOT NULL AND post_process_prompt_id != ''
            GROUP BY post_process_prompt_id
            ORDER BY count DESC
            LIMIT 5",
        )?;
        let skill_rows =
            skill_stmt.query_map(params![start_ts, end_ts], |row| row.get::<_, String>(0))?;
        for row in skill_rows {
            top_skills.push(row?);
        }

        let rich_entries = self.get_entries_for_analysis_rich(start_ts, end_ts, 500)?;
        let (daily_overview, task_clusters, context_pack) =
            self.build_recap(&rich_entries, &by_app);

        Ok(SummaryStats {
            entry_count,
            total_duration_ms,
            total_chars,
            llm_calls,
            by_app,
            by_hour,
            top_skills,
            daily_overview,
            task_clusters,
            context_pack,
        })
    }

    /// Get or create a summary for a given period
    /// Matches by period_type and period_start only (ignores period_end for lookup)
    /// Updates stats on each access to keep data fresh
    pub fn get_or_create_summary(
        &self,
        period_type: &str,
        start_ts: i64,
        end_ts: i64,
    ) -> Result<Summary> {
        let conn = self.get_connection()?;

        // Calculate fresh stats
        let stats = self.calculate_stats(start_ts, end_ts)?;
        let now = chrono::Utc::now().timestamp();
        let stats_json = serde_json::to_string(&stats)?;

        // Try to get existing summary by period_type and period_start only
        let existing: Option<Summary> = conn
            .query_row(
                "SELECT id, period_type, period_start, period_end, stats, ai_summary, ai_reflection,
                    ai_generated_at, ai_model_used, created_at, updated_at, ai_history
             FROM summaries WHERE period_type = ?1 AND period_start = ?2
             ORDER BY id DESC LIMIT 1",
                params![period_type, start_ts],
                |row| {
                    let old_stats_json: String = row.get(4)?;
                    let old_stats: SummaryStats = serde_json::from_str(&old_stats_json)
                        .unwrap_or_else(|_| Self::empty_stats());
                    Ok(Summary {
                        id: row.get(0)?,
                        period_type: row.get(1)?,
                        period_start: row.get(2)?,
                        period_end: row.get(3)?,
                        stats: old_stats,
                        ai_summary: row.get(5)?,
                        ai_reflection: row.get(6)?,
                        ai_generated_at: row.get(7)?,
                        ai_model_used: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                        ai_history: row
                            .get::<_, Option<String>>(11)?
                            .and_then(|json| serde_json::from_str(&json).ok()),
                    })
                },
            )
            .ok();

        if let Some(mut summary) = existing {
            // Update existing summary with fresh stats
            conn.execute(
                "UPDATE summaries SET stats = ?1, period_end = ?2, updated_at = ?3 WHERE id = ?4",
                params![stats_json, end_ts, now, summary.id],
            )?;
            summary.stats = stats;
            summary.period_end = end_ts;
            summary.updated_at = now;
            return Ok(summary);
        }

        // Create new summary
        conn.execute(
            "INSERT INTO summaries (period_type, period_start, period_end, stats, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![period_type, start_ts, end_ts, stats_json, now, now],
        )?;

        let id = conn.last_insert_rowid();
        Ok(Summary {
            id,
            period_type: period_type.to_string(),
            period_start: start_ts,
            period_end: end_ts,
            stats,
            ai_summary: None,
            ai_reflection: None,
            ai_generated_at: None,
            ai_model_used: None,
            ai_history: None,
            created_at: now,
            updated_at: now,
        })
    }

    /// Resolve the daily `summary_id` for a `YYYY-MM-DD` date in local time,
    /// creating a placeholder summary row if one does not yet exist.
    ///
    /// The date string is interpreted in the *local* timezone so it matches
    /// the windows the dashboard frontend uses (it builds
    /// `new Date(day + "T00:00:00")`, which is local-midnight).
    pub async fn get_or_create_summary_id_for_date(&self, date: &str) -> Result<i64> {
        use chrono::{Local, NaiveDate, TimeZone};
        let nd = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("invalid date '{}': {}", date, e))?;
        let start_local = Local
            .from_local_datetime(&nd.and_hms_opt(0, 0, 0).expect("00:00:00 is valid"))
            .single()
            .ok_or_else(|| anyhow::anyhow!("ambiguous/invalid local start for {}", date))?;
        let end_local_dt = nd
            .succ_opt()
            .ok_or_else(|| anyhow::anyhow!("date {} has no successor", date))?
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 is valid");
        let end_local = Local
            .from_local_datetime(&end_local_dt)
            .single()
            .ok_or_else(|| anyhow::anyhow!("ambiguous/invalid local end for {}", date))?;
        let start_ts = start_local.timestamp();
        let end_ts = end_local.timestamp() - 1;
        let summary = self.get_or_create_summary("day", start_ts, end_ts)?;
        Ok(summary.id)
    }

    /// Update AI-generated content for a summary and append to history
    pub fn update_summary_ai_content(
        &self,
        summary_id: i64,
        ai_summary: Option<String>,
        ai_reflection: Option<String>,
        model_used: Option<String>,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        let now = chrono::Utc::now().timestamp();

        // 1. Fetch existing AI history
        let existing_history_json: Option<String> = conn
            .query_row(
                "SELECT ai_history FROM summaries WHERE id = ?1",
                params![summary_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let mut ai_history: Vec<AiAnalysisEntry> = existing_history_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        // 2. Create new entry if we have content
        if let (Some(summary), Some(model)) = (&ai_summary, &model_used) {
            let reflection = ai_reflection.clone().unwrap_or_default();
            let new_entry = AiAnalysisEntry {
                timestamp: now,
                model: model.clone(),
                summary: summary.clone(),
                reflection,
            };
            ai_history.push(new_entry);
        }

        let new_history_json = serde_json::to_string(&ai_history)?;

        // 3. Update database
        conn.execute(
            "UPDATE summaries SET ai_summary = ?1, ai_reflection = ?2, ai_model_used = ?3,
             ai_generated_at = ?4, ai_history = ?5, updated_at = ?6 WHERE id = ?7",
            params![
                ai_summary,
                ai_reflection,
                model_used,
                now,
                new_history_json,
                now,
                summary_id
            ],
        )?;

        info!(
            "Updated AI content and history for summary {}: model={:?}, history_len={}",
            summary_id,
            model_used,
            ai_history.len()
        );

        Ok(())
    }

    /// Delete a specific AI analysis history entry by timestamp
    pub fn delete_summary_ai_history_entry(&self, summary_id: i64, timestamp: i64) -> Result<()> {
        let conn = self.get_connection()?;
        let now = chrono::Utc::now().timestamp();

        let existing_history_json: Option<String> = conn
            .query_row(
                "SELECT ai_history FROM summaries WHERE id = ?1",
                params![summary_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let mut ai_history: Vec<AiAnalysisEntry> = existing_history_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        let original_len = ai_history.len();
        ai_history.retain(|entry| entry.timestamp != timestamp);

        if ai_history.len() == original_len && timestamp != 0 {
            return Ok(());
        }

        let new_history_json = serde_json::to_string(&ai_history)?;

        let (ai_summary, ai_model_used, ai_generated_at) = if let Some(last) = ai_history.last() {
            (
                Some(last.summary.clone()),
                Some(last.model.clone()),
                Some(last.timestamp),
            )
        } else {
            (None, None, None)
        };

        conn.execute(
            "UPDATE summaries SET ai_summary = ?1, ai_model_used = ?2, ai_generated_at = ?3,
             ai_history = ?4, updated_at = ?5 WHERE id = ?6",
            params![
                ai_summary,
                ai_model_used,
                ai_generated_at,
                new_history_json,
                now,
                summary_id
            ],
        )?;

        Ok(())
    }

    /// Clean up duplicate summaries, keeping only the most recent one for each period
    pub fn cleanup_duplicate_summaries(&self) -> Result<usize> {
        let conn = self.get_connection()?;

        // Delete duplicates, keeping the one with the highest id for each (period_type, period_start) pair
        let deleted = conn.execute(
            "DELETE FROM summaries WHERE id NOT IN (
                SELECT MAX(id) FROM summaries GROUP BY period_type, period_start
            )",
            [],
        )?;

        if deleted > 0 {
            info!("Cleaned up {} duplicate summaries", deleted);
        }

        Ok(deleted)
    }

    /// Get list of cached summaries for sidebar (deduplicated)
    pub fn get_summary_list(&self) -> Result<Vec<Summary>> {
        let conn = self.get_connection()?;

        // First, clean up any duplicates
        let _ = self.cleanup_duplicate_summaries();

        let mut stmt = conn.prepare(
            "SELECT id, period_type, period_start, period_end, stats, ai_summary, ai_reflection,
                    ai_generated_at, ai_model_used, created_at, updated_at, ai_history
             FROM summaries ORDER BY period_start DESC LIMIT 50",
        )?;

        let rows = stmt.query_map([], |row| {
            let stats_json: String = row.get(4)?;
            let stats: SummaryStats =
                serde_json::from_str(&stats_json).unwrap_or_else(|_| Self::empty_stats());
            Ok(Summary {
                id: row.get(0)?,
                period_type: row.get(1)?,
                period_start: row.get(2)?,
                period_end: row.get(3)?,
                stats,
                ai_summary: row.get(5)?,
                ai_reflection: row.get(6)?,
                ai_generated_at: row.get(7)?,
                ai_model_used: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                ai_history: row
                    .get::<_, Option<String>>(11)?
                    .and_then(|json| serde_json::from_str(&json).ok()),
            })
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }

    /// Get user profile
    pub fn get_user_profile(&self) -> Result<UserProfile> {
        let conn = self.get_connection()?;

        conn.query_row(
            "SELECT vocabulary_stats, expression_stats, app_usage_stats, time_pattern_stats,
                    communication_style, tone_preference, style_prompt, feedback_style,
                    last_analyzed_at, updated_at
             FROM user_profile WHERE id = 1",
            [],
            |row| {
                Ok(UserProfile {
                    vocabulary_stats: row.get(0)?,
                    expression_stats: row.get(1)?,
                    app_usage_stats: row.get(2)?,
                    time_pattern_stats: row.get(3)?,
                    communication_style: row.get(4)?,
                    tone_preference: row.get(5)?,
                    style_prompt: row.get(6)?,
                    feedback_style: row
                        .get::<_, Option<String>>(7)?
                        .unwrap_or_else(|| "encouraging".to_string()),
                    last_analyzed_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
        .map_err(|e| anyhow::anyhow!("Failed to get user profile: {}", e))
    }

    /// Update user profile
    #[allow(dead_code)]
    pub fn update_user_profile(&self, profile: &UserProfile) -> Result<()> {
        let conn = self.get_connection()?;
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE user_profile SET
                vocabulary_stats = ?1, expression_stats = ?2, app_usage_stats = ?3,
                time_pattern_stats = ?4, communication_style = ?5, tone_preference = ?6,
                style_prompt = ?7, feedback_style = ?8, last_analyzed_at = ?9, updated_at = ?10
             WHERE id = 1",
            params![
                profile.vocabulary_stats,
                profile.expression_stats,
                profile.app_usage_stats,
                profile.time_pattern_stats,
                profile.communication_style,
                profile.tone_preference,
                profile.style_prompt,
                profile.feedback_style,
                profile.last_analyzed_at,
                now,
            ],
        )?;

        info!("Updated user profile");

        Ok(())
    }

    /// Update only the style_prompt field
    pub fn update_style_prompt(&self, style_prompt: &str) -> Result<()> {
        let conn = self.get_connection()?;
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE user_profile SET style_prompt = ?1, updated_at = ?2 WHERE id = 1",
            params![style_prompt, now],
        )?;

        info!("Updated style prompt");

        Ok(())
    }

    /// Update feedback style preference
    pub fn update_feedback_style(&self, feedback_style: &str) -> Result<()> {
        let conn = self.get_connection()?;
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE user_profile SET feedback_style = ?1, updated_at = ?2 WHERE id = 1",
            params![feedback_style, now],
        )?;

        info!("Updated feedback style to: {}", feedback_style);

        Ok(())
    }

    /// Get entries for a time range (for LLM analysis)
    /// Returns entries with effective_text computed (polish > ASR, ignoring skills)
    #[allow(dead_code)]
    pub fn get_entries_for_analysis(
        &self,
        start_ts: i64,
        end_ts: i64,
        limit: usize,
    ) -> Result<Vec<AnalysisEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, transcription_text, post_processed_text, app_name, post_process_prompt_id
             FROM transcription_history
             WHERE timestamp >= ?1 AND timestamp <= ?2 AND deleted = 0
             ORDER BY timestamp DESC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![start_ts, end_ts, limit as i64], |row| {
            let transcription_text: String = row.get(2)?;
            let post_processed_text: Option<String> = row.get(3)?;
            let prompt_id: Option<String> = row.get(5)?;

            // Compute effective text: polish > ASR, ignore skills
            let effective_text = if let Some(ref polish_text) = post_processed_text {
                if !polish_text.is_empty() && Self::is_polish_prompt(&prompt_id) {
                    polish_text.clone()
                } else {
                    transcription_text.clone()
                }
            } else {
                transcription_text.clone()
            };

            Ok(AnalysisEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                transcription_text,
                post_processed_text,
                app_name: row.get(4)?,
                effective_text,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisEntry {
    pub id: i64,
    pub timestamp: i64,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub app_name: Option<String>,
    /// The effective text to use for analysis (polish if available and not skill, else ASR)
    pub effective_text: String,
}

/// Rich analysis entry with formatted time and additional context
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RichAnalysisEntry {
    pub id: i64,
    pub timestamp: i64,
    pub time: String,      // "09:30" 格式
    pub date: String,      // "2026-01-25" 格式
    pub app: String,       // 应用名
    pub window: String,    // 窗口标题（截断到 50 字符）
    pub duration_sec: i64, // 持续秒数
    pub text: String,      // 有效文本内容
    pub has_polish: bool,  // 是否经过 AI 润色
    pub review_action: String,
    pub char_count: i64, // 字符数
}

/// Pre-computed analysis metrics for the LLM
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreAnalysis {
    // 时间模式
    pub active_hours: Vec<(u8, i64)>, // 小时 -> 条目数 (non-zero only)
    pub peak_hour: u8,                // 最活跃小时
    pub total_active_minutes: i64,    // 总活跃分钟数

    // 应用使用
    pub app_distribution: Vec<(String, i64, i64)>, // (应用, 条目数, 总时长ms) 按使用量排序
    pub most_used_app: String,

    // 上下文切换
    pub context_switches: i64, // 应用切换次数
    pub avg_session_sec: i64,  // 平均连续使用时长

    // 内容密度
    pub entries_with_polish: i64, // 润色过的条目数（表明重要）
    pub avg_entry_chars: i64,     // 平均条目字符数
    pub total_entries: i64,       // 总条目数
}

impl SummaryManager {
    /// Get a summary by its ID
    pub fn get_summary_by_id(&self, summary_id: i64) -> Result<Summary> {
        let conn = self.get_connection()?;

        conn.query_row(
            "SELECT id, period_type, period_start, period_end, stats, ai_summary, ai_reflection,
                    ai_generated_at, ai_model_used, created_at, updated_at, ai_history
             FROM summaries WHERE id = ?1",
            params![summary_id],
            |row| {
                let stats_json: String = row.get(4)?;
                let stats: SummaryStats =
                    serde_json::from_str(&stats_json).unwrap_or_else(|_| Self::empty_stats());
                Ok(Summary {
                    id: row.get(0)?,
                    period_type: row.get(1)?,
                    period_start: row.get(2)?,
                    period_end: row.get(3)?,
                    stats,
                    ai_summary: row.get(5)?,
                    ai_reflection: row.get(6)?,
                    ai_generated_at: row.get(7)?,
                    ai_model_used: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                    ai_history: row
                        .get::<_, Option<String>>(11)?
                        .and_then(|json| serde_json::from_str(&json).ok()),
                })
            },
        )
        .map_err(|e| anyhow::anyhow!("Failed to get summary: {}", e))
    }
}

impl SummaryManager {
    /// Prepare data for AI analysis of a summary
    /// Returns the summary, entries, and template variables for prompt rendering
    #[allow(dead_code)]
    pub fn prepare_analysis_data(
        &self,
        summary_id: i64,
        feedback_style: &str,
    ) -> Result<AnalysisData> {
        // Get the summary
        let summary = self.get_summary_by_id(summary_id)?;

        // Get entries for analysis (increase limit to 1000 for comprehensive weekly/monthly analysis)
        const MAX_ENTRIES: usize = 1000;
        let entries =
            self.get_entries_for_analysis(summary.period_start, summary.period_end, MAX_ENTRIES)?;

        // Warn if data might be truncated
        if entries.len() >= MAX_ENTRIES {
            log::warn!(
                "Analysis data may be truncated: fetched {} entries (limit={}), stats.entry_count={}",
                entries.len(),
                MAX_ENTRIES,
                summary.stats.entry_count
            );
        }

        info!(
            "Preparing analysis data: summary_id={}, period={} to {}, fetched_entries={}, stats.entry_count={}",
            summary_id,
            summary.period_start,
            summary.period_end,
            entries.len(),
            summary.stats.entry_count
        );

        // Build feedback instruction
        let feedback_instruction = match feedback_style {
            "neutral" => "Be objective and factual.",
            "encouraging" => {
                "Be supportive and highlight positive patterns while gently noting areas for improvement."
            }
            "direct" => "Be direct and specific about both strengths and areas needing work.",
            _ => "Be supportive and helpful.",
        };

        // Build entries grouped by app for better analysis
        let mut entries_by_app: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entry in entries.iter() {
            let app = entry.app_name.clone().unwrap_or_else(|| "其他".to_string());
            entries_by_app
                .entry(app)
                .or_default()
                .push(entry.effective_text.clone());
        }

        // Log per-app entry counts for debugging
        for (app, texts) in entries_by_app.iter() {
            info!("  App '{}': {} entries", app, texts.len());
        }

        let entries_by_app_str: String = entries_by_app
            .iter()
            .map(|(app, texts)| {
                // Include all entries for comprehensive analysis, not just first 5
                let all_texts: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                format!(
                    "### {} ({} 条记录)\n{}",
                    app,
                    all_texts.len(),
                    all_texts.join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(AnalysisData {
            summary,
            entries,
            entry_count: summary_id.to_string(), // Will be replaced with actual value
            total_chars: String::new(),
            duration_minutes: String::new(),
            llm_calls: String::new(),
            entries_by_app: entries_by_app_str,
            feedback_style: feedback_instruction.to_string(),
        })
    }
}

/// Data prepared for AI analysis, used to fill prompt template variables
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct AnalysisData {
    pub summary: Summary,
    #[allow(dead_code)]
    pub entries: Vec<AnalysisEntry>,
    #[allow(dead_code)]
    pub entry_count: String,
    #[allow(dead_code)]
    pub total_chars: String,
    #[allow(dead_code)]
    pub duration_minutes: String,
    #[allow(dead_code)]
    pub llm_calls: String,
    pub entries_by_app: String,
    pub feedback_style: String,
}

impl AnalysisData {
    /// Fill template variables in a prompt string
    #[allow(dead_code)]
    pub fn fill_prompt(&self, template: &str) -> String {
        template
            .replace(
                "${entry_count}",
                &self.summary.stats.entry_count.to_string(),
            )
            .replace(
                "${total_chars}",
                &self.summary.stats.total_chars.to_string(),
            )
            .replace(
                "${duration_minutes}",
                &(self.summary.stats.total_duration_ms / 60000).to_string(),
            )
            .replace("${llm_calls}", &self.summary.stats.llm_calls.to_string())
            .replace("${entries_by_app}", &self.entries_by_app)
            .replace("${feedback_style}", &self.feedback_style)
    }
}

/// Enhanced analysis data with rich entries and pre-analysis for period-specific prompts
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct EnhancedAnalysisData {
    pub summary: Summary,
    #[allow(dead_code)]
    pub rich_entries: Vec<RichAnalysisEntry>,
    #[allow(dead_code)]
    pub pre_analysis: PreAnalysis,

    // Formatted template variables
    pub date: String,        // "2026-01-25" for day
    pub week_label: String,  // "2026-01-20 ~ 2026-01-26" for week
    pub month_label: String, // "2026年1月" for month
    pub voice_stream_table: String,
    pub pre_analysis_summary: String,
    pub feedback_style: String,
}

impl EnhancedAnalysisData {
    /// Fill template variables in a prompt string (for period-specific prompts)
    #[allow(dead_code)]
    pub fn fill_prompt(&self, template: &str) -> String {
        template
            // Period labels
            .replace("${date}", &self.date)
            .replace("${week_label}", &self.week_label)
            .replace("${month_label}", &self.month_label)
            // Content
            .replace("${voice_stream_table}", &self.voice_stream_table)
            .replace("${pre_analysis_summary}", &self.pre_analysis_summary)
            .replace("${feedback_style}", &self.feedback_style)
            // Legacy variables for backward compatibility
            .replace(
                "${entry_count}",
                &self.summary.stats.entry_count.to_string(),
            )
            .replace(
                "${total_chars}",
                &self.summary.stats.total_chars.to_string(),
            )
            .replace(
                "${duration_minutes}",
                &(self.summary.stats.total_duration_ms / 60000).to_string(),
            )
            .replace("${llm_calls}", &self.summary.stats.llm_calls.to_string())
    }
}

impl SummaryManager {
    /// Prepare enhanced analysis data with rich entries for period-specific prompts
    pub fn prepare_analysis_data_enhanced(
        &self,
        summary_id: i64,
        feedback_style: &str,
    ) -> Result<EnhancedAnalysisData> {
        use chrono::{Datelike, Local, TimeZone};

        // Get the summary
        let summary = self.get_summary_by_id(summary_id)?;

        // Determine max entries based on period type
        let max_entries = match summary.period_type.as_str() {
            "day" => 500,
            "week" => 1000,
            "month" => 2000,
            _ => 1000,
        };

        // Get rich entries
        let rich_entries = self.get_entries_for_analysis_rich(
            summary.period_start,
            summary.period_end,
            max_entries,
        )?;

        // Calculate pre-analysis metrics
        let pre_analysis = self.calculate_pre_analysis(&rich_entries);

        // Format voice stream table (limit rows for prompt size)
        let max_table_rows = match summary.period_type.as_str() {
            "day" => 100,
            "week" => 150,
            "month" => 100, // Sample for monthly
            _ => 100,
        };
        let voice_stream_table = self.format_voice_stream_table(&rich_entries, max_table_rows);

        // Format pre-analysis summary
        let pre_analysis_summary = self.format_pre_analysis_summary(&pre_analysis);

        // Build feedback instruction
        let feedback_instruction = match feedback_style {
            "neutral" => "客观陈述事实，不加主观评价。",
            "encouraging" => "积极鼓励，突出正面模式，温和指出改进空间。",
            "direct" => "直接具体，同时指出优势和需要改进的地方。",
            _ => "积极支持，帮助用户成长。",
        };

        // Format date labels
        let start_dt = Local
            .timestamp_opt(summary.period_start, 0)
            .single()
            .unwrap_or_else(Local::now);
        let end_dt = Local
            .timestamp_opt(summary.period_end, 0)
            .single()
            .unwrap_or_else(Local::now);

        let date = start_dt.format("%Y-%m-%d").to_string();
        let week_label = format!(
            "{} ~ {}",
            start_dt.format("%Y-%m-%d"),
            end_dt.format("%Y-%m-%d")
        );
        let month_label = format!("{}年{}月", start_dt.year(), start_dt.month());

        info!(
            "Prepared enhanced analysis data: period_type={}, entries={}, pre_analysis.total_entries={}",
            summary.period_type,
            rich_entries.len(),
            pre_analysis.total_entries
        );

        Ok(EnhancedAnalysisData {
            summary,
            rich_entries,
            pre_analysis,
            date,
            week_label,
            month_label,
            voice_stream_table,
            pre_analysis_summary,
            feedback_style: feedback_instruction.to_string(),
        })
    }
}

impl SummaryManager {
    /// Get rich analysis entries with formatted time and additional context
    pub fn get_entries_for_analysis_rich(
        &self,
        start_ts: i64,
        end_ts: i64,
        limit: usize,
    ) -> Result<Vec<RichAnalysisEntry>> {
        use chrono::{Local, TimeZone};

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, transcription_text, post_processed_text, 
                    app_name, window_title, duration_ms, char_count, post_process_prompt_id, review_action
             FROM transcription_history
             WHERE timestamp >= ?1 AND timestamp <= ?2 AND deleted = 0
             ORDER BY timestamp ASC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![start_ts, end_ts, limit as i64], |row| {
            let id: i64 = row.get(0)?;
            let timestamp: i64 = row.get(1)?;
            let transcription_text: String = row.get(2)?;
            let post_processed_text: Option<String> = row.get(3)?;
            let app_name: Option<String> = row.get(4)?;
            let window_title: Option<String> = row.get(5)?;
            let duration_ms: Option<i64> = row.get(6)?;
            let char_count: Option<i64> = row.get(7)?;
            let prompt_id: Option<String> = row.get(8)?;
            let review_action: Option<String> = row.get(9)?;

            // Format time
            let dt = Local
                .timestamp_opt(timestamp, 0)
                .single()
                .unwrap_or_else(Local::now);
            let time = dt.format("%H:%M").to_string();
            let date = dt.format("%Y-%m-%d").to_string();

            // Compute effective text and check if polished
            let has_polish = Self::is_polish_prompt(&prompt_id)
                && post_processed_text
                    .as_ref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
            let text = if has_polish {
                post_processed_text.clone().unwrap_or_default()
            } else {
                transcription_text.clone()
            };

            // Truncate window title to 50 chars
            let window = window_title
                .unwrap_or_default()
                .chars()
                .take(50)
                .collect::<String>();

            // Calculate char_count before moving text
            let text_char_count = char_count.unwrap_or(text.chars().count() as i64);

            Ok(RichAnalysisEntry {
                id,
                timestamp,
                time,
                date,
                app: app_name.unwrap_or_else(|| "其他".to_string()),
                window,
                duration_sec: duration_ms.unwrap_or(0) / 1000,
                text,
                has_polish,
                review_action: review_action.unwrap_or_default(),
                char_count: text_char_count,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        info!(
            "Loaded {} rich analysis entries from {} to {}",
            entries.len(),
            start_ts,
            end_ts
        );

        Ok(entries)
    }

    /// Calculate pre-analysis metrics from entries
    pub fn calculate_pre_analysis(&self, entries: &[RichAnalysisEntry]) -> PreAnalysis {
        use std::collections::HashMap;

        // Hour distribution
        let mut hour_counts: HashMap<u8, i64> = HashMap::new();
        let mut app_stats: HashMap<String, (i64, i64)> = HashMap::new(); // (count, duration_ms)
        let mut context_switches = 0i64;
        let mut last_app: Option<&str> = None;
        let mut total_chars = 0i64;
        let mut entries_with_polish = 0i64;
        let mut total_duration_sec = 0i64;

        for entry in entries {
            // Parse hour from time string
            if let Some(hour_str) = entry.time.split(':').next() {
                if let Ok(hour) = hour_str.parse::<u8>() {
                    *hour_counts.entry(hour).or_insert(0) += 1;
                }
            }

            // App distribution
            let app_entry = app_stats.entry(entry.app.clone()).or_insert((0, 0));
            app_entry.0 += 1;
            app_entry.1 += entry.duration_sec * 1000;

            // Context switches
            if let Some(last) = last_app {
                if last != entry.app {
                    context_switches += 1;
                }
            }
            last_app = Some(&entry.app);

            // Content stats
            total_chars += entry.char_count;
            if entry.has_polish {
                entries_with_polish += 1;
            }
            total_duration_sec += entry.duration_sec;
        }

        // Find peak hour
        let peak_hour = hour_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(hour, _)| *hour)
            .unwrap_or(0);

        // Active hours (non-zero)
        let mut active_hours: Vec<(u8, i64)> = hour_counts.into_iter().collect();
        active_hours.sort_by_key(|(hour, _)| *hour);

        // Most used app
        let most_used_app = app_stats
            .iter()
            .max_by_key(|(_, (count, _))| *count)
            .map(|(app, _)| app.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        // App distribution sorted by count
        let mut app_distribution: Vec<(String, i64, i64)> = app_stats
            .into_iter()
            .map(|(app, (count, dur))| (app, count, dur))
            .collect();
        app_distribution.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by count descending

        let total_entries = entries.len() as i64;
        let avg_entry_chars = if total_entries > 0 {
            total_chars / total_entries
        } else {
            0
        };
        let avg_session_sec = if total_entries > 0 {
            total_duration_sec / total_entries
        } else {
            0
        };

        PreAnalysis {
            active_hours,
            peak_hour,
            total_active_minutes: total_duration_sec / 60,
            app_distribution,
            most_used_app,
            context_switches,
            avg_session_sec,
            entries_with_polish,
            avg_entry_chars,
            total_entries,
        }
    }

    /// Format entries as a markdown table for LLM consumption
    pub fn format_voice_stream_table(
        &self,
        entries: &[RichAnalysisEntry],
        max_rows: usize,
    ) -> String {
        let mut table = String::from("| 时间 | 应用 | 窗口/上下文 | 时长 | 内容 |\n");
        table.push_str("|------|------|------------|------|------|\n");

        for entry in entries.iter().take(max_rows) {
            let window_short: String = entry.window.chars().take(25).collect();
            let text_short: String = entry.text.chars().take(80).collect();
            let text_escaped = text_short.replace('|', "\\|").replace('\n', " ");

            table.push_str(&format!(
                "| {} | {} | {} | {}s | {} |\n",
                entry.time, entry.app, window_short, entry.duration_sec, text_escaped
            ));
        }

        if entries.len() > max_rows {
            table.push_str(&format!(
                "\n*（显示前 {} 条，共 {} 条记录）*\n",
                max_rows,
                entries.len()
            ));
        }

        table
    }

    /// Format pre-analysis as summary text for prompt
    pub fn format_pre_analysis_summary(&self, pre: &PreAnalysis) -> String {
        let mut summary = String::new();

        // Active hours
        let active_hours_str: Vec<String> = pre
            .active_hours
            .iter()
            .filter(|(_, count)| *count > 0)
            .take(5)
            .map(|(hour, count)| format!("{}:00({} 条)", hour, count))
            .collect();
        summary.push_str(&format!("- 活跃时段: {}\n", active_hours_str.join(", ")));
        summary.push_str(&format!("- 最活跃时间: {:02}:00\n", pre.peak_hour));
        summary.push_str(&format!(
            "- 总活跃时长: {} 分钟\n",
            pre.total_active_minutes
        ));

        // App distribution
        let top_apps: Vec<String> = pre
            .app_distribution
            .iter()
            .take(3)
            .map(|(app, count, _)| format!("{}({} 条)", app, count))
            .collect();
        summary.push_str(&format!("- 主要应用: {}\n", top_apps.join(", ")));
        summary.push_str(&format!("- 上下文切换: {} 次\n", pre.context_switches));

        // Content stats
        summary.push_str(&format!("- 总记录数: {} 条\n", pre.total_entries));
        summary.push_str(&format!("- AI 润色条目: {} 条\n", pre.entries_with_polish));
        summary.push_str(&format!("- 平均条目长度: {} 字符\n", pre.avg_entry_chars));

        summary
    }
}
