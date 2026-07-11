//! Hotword Manager
//!
//! Manages hotwords (vocabulary entries) with CRUD operations, category inference,
//! and LLM injection text generation for improving transcription accuracy.

use anyhow::Result;
use chrono::Utc;
use log::{debug, error, info, warn};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use tauri::{Emitter, Manager};

use crate::settings::{Hotword, HotwordCategoryMeta, HotwordScenario};

/// Technical term suffixes for category inference
const TECHNICAL_SUFFIXES: &[&str] = &[
    "-js",
    "-ts",
    "-api",
    "Config",
    "Manager",
    "Service",
    "Handler",
    "Controller",
    "Provider",
];

fn parse_json_or_default<T>(value: Option<String>) -> T
where
    T: DeserializeOwned + Default,
{
    value
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn serialize_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

pub(crate) fn detect_scenario_from_app_name(app_name: Option<&str>) -> Option<HotwordScenario> {
    let work_apps = [
        "Code", "VSCode", "Cursor", "Terminal", "iTerm", "Slack", "Notion", "Figma", "Xcode",
        "IntelliJ",
    ];
    let casual_apps = ["WeChat", "Messages", "Telegram", "WhatsApp", "Discord"];

    let name = app_name?;
    for app in work_apps {
        if name.contains(app) {
            return Some(HotwordScenario::Work);
        }
    }
    for app in casual_apps {
        if name.contains(app) {
            return Some(HotwordScenario::Casual);
        }
    }
    None
}

fn count_hotword_occurrences(text: &str, target: &str) -> usize {
    let text = text.trim();
    let target = target.trim();
    if text.is_empty() || target.is_empty() {
        return 0;
    }

    if target
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        let pattern = format!(r"(?i)\b{}\b", regex::escape(target));
        return regex::Regex::new(&pattern)
            .map(|re| re.find_iter(text).count())
            .unwrap_or(0);
    }

    if target.is_ascii() {
        let pattern = format!(r"(?i){}", regex::escape(target));
        return regex::Regex::new(&pattern)
            .map(|re| re.find_iter(text).count())
            .unwrap_or(0);
    }

    text.match_indices(target).count()
}

/// Deterministic "does `text` contain `term`" check shared by the local
/// hotword scanner and prompt-side correction filtering.
///
/// Reuses `count_hotword_occurrences` (ASCII word boundaries, CJK substring),
/// then falls back to a space-stripped lowercase comparison for ASCII terms of
/// ≥5 condensed chars so segmentation variants like "vo type" ↔ "votype"
/// still match without weakening the word-boundary guarantee for short terms.
pub fn text_contains_term(text: &str, term: &str) -> bool {
    if count_hotword_occurrences(text, term) > 0 {
        return true;
    }
    let term = term.trim();
    if !is_ascii_word_like(term) {
        return false;
    }
    let condensed_term: String = term
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if condensed_term.chars().count() < 5 {
        return false;
    }
    let condensed_text: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    condensed_text.contains(&condensed_term)
}

fn is_ascii_word_like(term: &str) -> bool {
    term.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch.is_ascii_whitespace())
}

/// Apply the active hotword force-replacement set to `text`, resolving the
/// sqlite path from the HistoryManager tauri-state. Returns the original text
/// unchanged when state is unavailable or the replacement lookup fails.
///
/// This is the canonical chokepoint for "ASR produced text → replaced text"
/// and is called from every ASR output site (local engine, online client,
/// retranscribe) so downstream DB, post-process, and review all see the same
/// substituted content.
pub fn apply_force_replacements_via_state(app_handle: &tauri::AppHandle, text: String) -> String {
    use std::sync::Arc;
    let hm = match app_handle.try_state::<Arc<crate::managers::history::HistoryManager>>() {
        Some(hm) => hm,
        None => return text,
    };
    match HotwordManager::new(hm.db_path.clone()).apply_force_replacements(&text) {
        Ok(replaced) => replaced,
        Err(err) => {
            warn!(
                "[Hotword] Failed to apply force replacements on ASR output: {}",
                err
            );
            text
        }
    }
}

fn apply_exact_term_replacement(text: &str, original: &str, target: &str) -> String {
    let original = original.trim();
    if text.is_empty() || original.is_empty() || original == target {
        return text.to_string();
    }

    if is_ascii_word_like(original) {
        // Use ASCII-mode word boundaries so `\b` only treats `[A-Za-z0-9_]` as
        // word characters. In Rust's default Unicode mode, Chinese characters
        // count as word chars, which means `\blexi\b` fails to match when
        // "lexi" is sandwiched between Han characters (e.g. "出现lexi相关").
        // ASCII boundaries preserve the "no substring of a larger ASCII word"
        // invariant (e.g. "op" must not match inside "operator") while still
        // firing at CJK/ASCII junctions.
        let pattern = format!(r"(?i)(?-u:\b){}(?-u:\b)", regex::escape(original));
        return Regex::new(&pattern)
            .map(|re| re.replace_all(text, target).into_owned())
            .unwrap_or_else(|_| text.to_string());
    }

    text.replace(original, target)
}

/// Manages hotword vocabulary for transcription enhancement
pub struct HotwordManager {
    db_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HotwordInjection {
    pub person_names: Vec<HotwordEntry>,
    pub product_names: Vec<HotwordEntry>,
    pub domain_terms: Vec<HotwordEntry>,
    pub hotwords: Vec<HotwordEntry>,
    pub correction_pairs: Vec<CorrectionPair>,
}

/// A confirmed ASR misrecognition → correction mapping with confidence score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionPair {
    /// The misrecognized form (e.g. "scale")
    pub original: String,
    /// The correct form (e.g. "skill")
    pub target: String,
    /// Star rating: 1 = low, 2 = medium, 3 = high confidence
    pub stars: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotwordEntry {
    pub target: String,
    pub aliases: Vec<String>,
}

/// Result of a deterministic local scan of input text against the hotword
/// table. Used to override LLM judgments that are blind to the user's
/// hotword vocabulary (intent `needs_hotword`, PassThrough routing).
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalHotwordScan {
    /// Text contains a known misrecognized form (`originals` or
    /// force-replace alias) — strong evidence the ASR output needs fixing.
    pub alias_hit: bool,
    /// Text already contains a hotword target verbatim — terms should be
    /// kept intact, so injection is still worthwhile.
    pub target_hit: bool,
}

#[derive(Debug, Clone, Copy)]
struct HotwordContext<'a> {
    text: &'a str,
    weight: i64,
}

#[derive(Debug, Clone)]
struct RankedHotword {
    hotword: Hotword,
    score: i64,
}

impl HotwordManager {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn get_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    /// Infer the category of a hotword based on its target form.
    ///
    /// Heuristics:
    /// - All uppercase 2-5 chars -> "abbreviation"
    /// - Technical suffixes (-js, -ts, Config, Manager, etc.) -> "term"
    /// - Single capitalized word -> "person"
    /// - Default -> "term"
    pub fn infer_category(target: &str) -> String {
        let trimmed = target.trim();

        // Check for abbreviations (all uppercase, 2-5 characters)
        if trimmed.len() >= 2
            && trimmed.len() <= 5
            && trimmed.chars().all(|c| c.is_ascii_uppercase())
        {
            return "abbreviation".to_string();
        }

        // Check for technical terms with known suffixes
        let lower = trimmed.to_lowercase();
        for suffix in TECHNICAL_SUFFIXES {
            let suffix_lower = suffix.to_lowercase();
            if lower.ends_with(&suffix_lower) || lower.contains(&suffix_lower) {
                return "term".to_string();
            }
        }

        // Check for single capitalized word (potential person name)
        // Must be a single word, start with uppercase, rest lowercase
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.len() == 1 {
            let word = words[0];
            let chars: Vec<char> = word.chars().collect();
            if chars.len() >= 2
                && chars[0].is_uppercase()
                && chars[1..]
                    .iter()
                    .all(|c| c.is_lowercase() || !c.is_alphabetic())
            {
                return "person".to_string();
            }
        }

        "term".to_string()
    }

    /// Map a database row to a Hotword struct
    fn row_to_hotword(row: &rusqlite::Row) -> rusqlite::Result<Hotword> {
        let originals_json: String = row.get("originals")?;
        let category_str: String = row.get("category")?;
        let scenarios_json: String = row.get("scenarios")?;
        let status: String = row.get("status").unwrap_or_else(|_| "active".to_string());
        let source: String = row.get("source").unwrap_or_else(|_| "manual".to_string());
        let force_replace_originals_json = row
            .get::<_, Option<String>>("force_replace_originals")
            .ok()
            .flatten()
            .unwrap_or_else(|| "[]".to_string());
        let app_usage_stats = row
            .get::<_, Option<String>>("app_usage_stats")
            .ok()
            .flatten();
        let scenario_usage_stats = row
            .get::<_, Option<String>>("scenario_usage_stats")
            .ok()
            .flatten();

        Ok(Hotword {
            id: row.get("id")?,
            target: row.get("target")?,
            originals: serde_json::from_str(&originals_json).unwrap_or_default(),
            force_replace_originals: serde_json::from_str(&force_replace_originals_json)
                .unwrap_or_default(),
            category: category_str,
            scenarios: serde_json::from_str(&scenarios_json).unwrap_or_default(),
            user_override: row.get("user_override")?,
            use_count: row.get("use_count")?,
            recent_use_count: row.get("recent_use_count").unwrap_or(0),
            app_usage_stats: parse_json_or_default(app_usage_stats),
            scenario_usage_stats: parse_json_or_default(scenario_usage_stats),
            last_used_at: row.get("last_used_at")?,
            false_positive_count: row.get("false_positive_count")?,
            created_at: row.get("created_at")?,
            status,
            source,
        })
    }

    /// Get all hotwords from the database, including usage telemetry fields.
    pub fn get_all_with_usage_stats(&self) -> Result<Vec<Hotword>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, target, originals, category, scenarios,
                    user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats,
                    last_used_at, false_positive_count, created_at, status, source, force_replace_originals
             FROM hotwords
             ORDER BY use_count DESC, created_at DESC",
        )?;

        let rows = stmt.query_map([], Self::row_to_hotword)?;

        let mut hotwords = Vec::new();
        for row in rows {
            hotwords.push(row?);
        }

        debug!("[Hotword] Retrieved {} hotwords", hotwords.len());
        Ok(hotwords)
    }

    /// Get all hotwords from the database (both active and suggested).
    pub fn get_all(&self) -> Result<Vec<Hotword>> {
        self.get_all_with_usage_stats()
    }

    /// Get hotwords filtered by scenario
    pub fn get_by_scenario(&self, scenario: HotwordScenario) -> Result<Vec<Hotword>> {
        let all = self.get_all()?;
        let filtered: Vec<Hotword> = all
            .into_iter()
            .filter(|h| h.scenarios.contains(&scenario))
            .collect();

        debug!(
            "[Hotword] Retrieved {} hotwords for scenario {:?}",
            filtered.len(),
            scenario
        );
        Ok(filtered)
    }

    /// Add a new hotword (with case-insensitive dedup)
    pub fn add(
        &self,
        target: String,
        originals: Vec<String>,
        force_replace_originals: Vec<String>,
        category: Option<String>,
        scenarios: Option<Vec<HotwordScenario>>,
    ) -> Result<Hotword> {
        let conn = self.get_connection()?;
        let now = Utc::now().timestamp();

        // Case-insensitive duplicate check
        let existing: Option<(
            i64,
            String,
            String,
            String,
            String,
            bool,
            i64,
            i64,
            String,
            String,
            Option<i64>,
            i64,
            i64,
            String,
            String,
            String,
        )> = conn
            .query_row(
                "SELECT id, target, originals, category, scenarios, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, last_used_at, false_positive_count, created_at, status, source, force_replace_originals FROM hotwords WHERE LOWER(target) = LOWER(?1)",
                params![target],
                |row| Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                    row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
                    row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?,
                    row.get(12)?, row.get(13)?, row.get(14)?, row.get(15)?,
                )),
            )
            .ok();

        if let Some((
            id,
            existing_target,
            existing_originals_json,
            existing_category,
            existing_scenarios_json,
            existing_user_override,
            existing_use_count,
            existing_recent_use_count,
            existing_app_usage_stats_json,
            existing_scenario_usage_stats_json,
            existing_last_used_at,
            existing_fp_count,
            existing_created_at,
            existing_status,
            existing_source,
            existing_force_replace_originals_json,
        )) = existing
        {
            // Merge originals into existing hotword
            let mut existing_originals: Vec<String> =
                serde_json::from_str(&existing_originals_json).unwrap_or_default();
            let mut existing_force_replace_originals: Vec<String> =
                serde_json::from_str(&existing_force_replace_originals_json).unwrap_or_default();
            for orig in &originals {
                if !existing_originals
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(orig))
                {
                    existing_originals.push(orig.clone());
                }
            }
            for orig in &force_replace_originals {
                if !existing_force_replace_originals
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(orig))
                {
                    existing_force_replace_originals.push(orig.clone());
                }
            }
            let merged_json = serde_json::to_string(&existing_originals)?;
            let merged_force_replace_json =
                serde_json::to_string(&existing_force_replace_originals)?;
            conn.execute(
                "UPDATE hotwords SET originals = ?1, force_replace_originals = ?2 WHERE id = ?3",
                params![merged_json, merged_force_replace_json, id],
            )?;
            info!(
                "[Hotword] Merged into existing hotword \"{}\" (id={}), originals: {:?}",
                existing_target, id, existing_originals
            );
            let existing_scenarios: Vec<HotwordScenario> =
                serde_json::from_str(&existing_scenarios_json).unwrap_or_default();
            return Ok(Hotword {
                id,
                target: existing_target,
                originals: existing_originals,
                force_replace_originals: existing_force_replace_originals,
                category: existing_category,
                scenarios: existing_scenarios,
                user_override: existing_user_override,
                use_count: existing_use_count,
                recent_use_count: existing_recent_use_count,
                app_usage_stats: parse_json_or_default(Some(existing_app_usage_stats_json)),
                scenario_usage_stats: parse_json_or_default(Some(
                    existing_scenario_usage_stats_json,
                )),
                last_used_at: existing_last_used_at,
                false_positive_count: existing_fp_count,
                created_at: existing_created_at,
                status: existing_status,
                source: existing_source,
            });
        }

        // Infer category if not provided
        let final_category = category
            .clone()
            .unwrap_or_else(|| Self::infer_category(&target));
        let user_override = category.is_some();

        // Default to both scenarios if not specified
        let final_scenarios =
            scenarios.unwrap_or_else(|| vec![HotwordScenario::Work, HotwordScenario::Casual]);

        let originals_json = serde_json::to_string(&originals)?;
        let force_replace_originals_json = serde_json::to_string(&force_replace_originals)?;
        let scenarios_json = serde_json::to_string(&final_scenarios)?;

        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, user_override, use_count, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, 'active', 'manual')",
            params![
                target,
                originals_json,
                force_replace_originals_json,
                final_category,
                scenarios_json,
                user_override,
                now,
            ],
        )?;

        let id = conn.last_insert_rowid();

        info!(
            "[Hotword] Added hotword: {} (category={})",
            target, final_category
        );

        Ok(Hotword {
            id,
            target,
            originals,
            force_replace_originals,
            category: final_category,
            scenarios: final_scenarios,
            user_override,
            use_count: 0,
            recent_use_count: 0,
            app_usage_stats: HashMap::new(),
            scenario_usage_stats: HashMap::new(),
            last_used_at: None,
            false_positive_count: 0,
            created_at: now,
            status: "active".to_string(),
            source: "manual".to_string(),
        })
    }

    /// Update an existing hotword
    pub fn update(
        &self,
        id: i64,
        target: Option<String>,
        originals: Vec<String>,
        force_replace_originals: Vec<String>,
        category: String,
        scenarios: Vec<HotwordScenario>,
    ) -> Result<()> {
        let conn = self.get_connection()?;

        let originals_json = serde_json::to_string(&originals)?;
        let force_replace_originals_json = serde_json::to_string(&force_replace_originals)?;
        let scenarios_json = serde_json::to_string(&scenarios)?;

        if let Some(ref new_target) = target {
            conn.execute(
                "UPDATE hotwords
                 SET target = ?1, originals = ?2, force_replace_originals = ?3, category = ?4, scenarios = ?5, user_override = 1
                 WHERE id = ?6",
                params![
                    new_target,
                    originals_json,
                    force_replace_originals_json,
                    category,
                    scenarios_json,
                    id
                ],
            )?;
        } else {
            conn.execute(
                "UPDATE hotwords
                 SET originals = ?1, force_replace_originals = ?2, category = ?3, scenarios = ?4, user_override = 1
                 WHERE id = ?5",
                params![
                    originals_json,
                    force_replace_originals_json,
                    category,
                    scenarios_json,
                    id
                ],
            )?;
        }

        info!("[Hotword] Updated hotword id={}", id);
        Ok(())
    }

    /// Delete a hotword by ID
    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM hotwords WHERE id = ?1", params![id])?;
        info!("[Hotword] Deleted hotword id={}", id);
        Ok(())
    }

    /// Increment use count for a hotword
    #[allow(dead_code)]
    pub fn increment_use(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        let now = Utc::now().timestamp();

        conn.execute(
            "UPDATE hotwords SET use_count = use_count + 1, recent_use_count = recent_use_count + 1, last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;

        debug!("[Hotword] Incremented use count for hotword id={}", id);
        Ok(())
    }

    /// Record hotword hits from a finalized output text.
    ///
    /// This is intended for final landing text only, not raw ASR fragments.
    /// The caller is responsible for invoking it once per finalized output.
    pub fn record_final_output_usage(
        &self,
        finalized_text: &str,
        app_name: Option<&str>,
    ) -> Result<usize> {
        let finalized_text = finalized_text.trim();
        if finalized_text.is_empty() {
            return Ok(0);
        }

        let resolved_scenario =
            detect_scenario_from_app_name(app_name).unwrap_or(HotwordScenario::Work);
        let app_name = app_name.map(str::trim).filter(|name| !name.is_empty());
        let now = Utc::now().timestamp();
        let mut total_hits = 0usize;

        let hotwords: Vec<Hotword> = self
            .get_all_with_usage_stats()?
            .into_iter()
            .filter(|hotword| hotword.status == "active")
            .collect();

        if hotwords.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;

        for hotword in hotwords {
            let hit_count = count_hotword_occurrences(finalized_text, &hotword.target);
            if hit_count == 0 {
                continue;
            }

            let mut app_usage_stats = hotword.app_usage_stats.clone();
            if let Some(app_name) = app_name {
                *app_usage_stats.entry(app_name.to_string()).or_insert(0) += hit_count as i64;
            }

            let mut scenario_usage_stats = hotword.scenario_usage_stats.clone();
            *scenario_usage_stats.entry(resolved_scenario).or_insert(0) += hit_count as i64;

            conn.execute(
                "UPDATE hotwords
                 SET use_count = use_count + ?1,
                     recent_use_count = recent_use_count + ?1,
                     last_used_at = ?2,
                     app_usage_stats = ?3,
                     scenario_usage_stats = ?4
                 WHERE id = ?5",
                params![
                    hit_count as i64,
                    now,
                    serialize_json(&app_usage_stats)?,
                    serialize_json(&scenario_usage_stats)?,
                    hotword.id
                ],
            )?;

            total_hits += hit_count;
        }

        if total_hits > 0 {
            info!(
                "[Hotword] Recorded {} final-output hotword hit(s) for app={:?} scenario={:?}",
                total_hits, app_name, resolved_scenario
            );
        } else {
            debug!(
                "[Hotword] No final-output hotword hits found for app={:?} scenario={:?}",
                app_name, resolved_scenario
            );
        }

        Ok(total_hits)
    }

    pub fn apply_force_replacements(&self, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(text.to_string());
        }

        let mut pairs: Vec<(String, String)> = self
            .get_all_with_usage_stats()?
            .into_iter()
            .filter(|hotword| hotword.status == "active")
            .flat_map(|hotword| {
                hotword
                    .force_replace_originals
                    .into_iter()
                    .map(move |original| (original, hotword.target.clone()))
            })
            .filter(|(original, target)| {
                let original = original.trim();
                !original.is_empty() && original != target
            })
            .collect();

        if pairs.is_empty() {
            return Ok(text.to_string());
        }

        pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));

        let mut replaced = text.to_string();
        for (original, target) in pairs {
            replaced = apply_exact_term_replacement(&replaced, &original, &target);
        }

        Ok(replaced)
    }

    /// Increment false positive count for a hotword
    /// Adjust use_count by a delta (positive or negative). Returns the new use_count.
    pub fn adjust_use_count(&self, id: i64, delta: i64) -> Result<i64> {
        let conn = self.get_connection()?;

        conn.execute(
            "UPDATE hotwords SET use_count = MAX(0, use_count + ?1) WHERE id = ?2",
            params![delta, id],
        )?;

        let new_count: i64 = conn.query_row(
            "SELECT use_count FROM hotwords WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        info!(
            "[Hotword] Adjusted use_count for hotword id={} by {}, new count={}",
            id, delta, new_count
        );
        Ok(new_count)
    }

    pub fn increment_false_positive(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        conn.execute(
            "UPDATE hotwords SET false_positive_count = false_positive_count + 1 WHERE id = ?1",
            params![id],
        )?;

        debug!(
            "[Hotword] Incremented false positive count for hotword id={}",
            id
        );
        Ok(())
    }

    // ── Category CRUD ────────────────────────────────────────────────────

    /// Get all hotword categories from the database
    pub fn get_categories(&self) -> Result<Vec<HotwordCategoryMeta>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, label, color, icon, sort_order, is_builtin FROM hotword_categories ORDER BY sort_order ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HotwordCategoryMeta {
                id: row.get("id")?,
                label: row.get("label")?,
                color: row.get("color")?,
                icon: row.get("icon")?,
                sort_order: row.get("sort_order")?,
                is_builtin: row.get("is_builtin")?,
            })
        })?;
        let mut categories = Vec::new();
        for row in rows {
            categories.push(row?);
        }
        Ok(categories)
    }

    /// Add a new custom category
    pub fn add_category(
        &self,
        id: &str,
        label: &str,
        color: &str,
        icon: &str,
    ) -> Result<HotwordCategoryMeta> {
        let conn = self.get_connection()?;
        let now = Utc::now().timestamp();

        // Get next sort_order
        let max_order: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), 0) FROM hotword_categories",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        conn.execute(
            "INSERT INTO hotword_categories (id, label, color, icon, sort_order, is_builtin, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![id, label, color, icon, max_order + 1, now],
        )?;

        info!("[Hotword] Added category: {} ({})", label, id);
        Ok(HotwordCategoryMeta {
            id: id.to_string(),
            label: label.to_string(),
            color: color.to_string(),
            icon: icon.to_string(),
            sort_order: max_order + 1,
            is_builtin: false,
        })
    }

    /// Update an existing category (only non-builtin)
    pub fn update_category(
        &self,
        id: &str,
        label: Option<&str>,
        color: Option<&str>,
        icon: Option<&str>,
        sort_order: Option<i64>,
    ) -> Result<()> {
        let conn = self.get_connection()?;

        if let Some(label) = label {
            conn.execute(
                "UPDATE hotword_categories SET label = ?1 WHERE id = ?2",
                params![label, id],
            )?;
        }
        if let Some(color) = color {
            conn.execute(
                "UPDATE hotword_categories SET color = ?1 WHERE id = ?2",
                params![color, id],
            )?;
        }
        if let Some(icon) = icon {
            conn.execute(
                "UPDATE hotword_categories SET icon = ?1 WHERE id = ?2",
                params![icon, id],
            )?;
        }
        if let Some(sort_order) = sort_order {
            conn.execute(
                "UPDATE hotword_categories SET sort_order = ?1 WHERE id = ?2",
                params![sort_order, id],
            )?;
        }

        info!("[Hotword] Updated category: {}", id);
        Ok(())
    }

    /// Delete a custom category and reassign its hotwords to "term"
    pub fn delete_category(&self, id: &str) -> Result<()> {
        let conn = self.get_connection()?;

        // Check if builtin
        let is_builtin: bool = conn
            .query_row(
                "SELECT is_builtin FROM hotword_categories WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap_or(true);

        if is_builtin {
            return Err(anyhow::anyhow!("Cannot delete built-in category: {}", id));
        }

        // Reassign hotwords to "term"
        conn.execute(
            "UPDATE hotwords SET category = 'term' WHERE category = ?1",
            params![id],
        )?;

        // Delete the category
        conn.execute(
            "DELETE FROM hotword_categories WHERE id = ?1 AND is_builtin = 0",
            params![id],
        )?;

        info!(
            "[Hotword] Deleted category: {}, reassigned hotwords to 'term'",
            id
        );
        Ok(())
    }

    // ── LLM Injection ────────────────────────────────────────────────────

    fn normalize_hotword_bucket(category: &str) -> &'static str {
        match category {
            "person" => "person",
            "brand" => "product",
            "term" | "abbreviation" => "domain",
            _ => "hotword",
        }
    }

    fn hotword_source_score(source: &str) -> i64 {
        match source {
            "manual" => 180,
            "auto_learned" => 120,
            "ai_extracted" => 80,
            _ => 0,
        }
    }

    fn telemetry_score(
        hotword: &Hotword,
        app_name: Option<&str>,
        scenario: HotwordScenario,
    ) -> i64 {
        let app_name = app_name.map(str::trim).filter(|name| !name.is_empty());
        let recent_score = hotword.recent_use_count.min(6) * 12;
        let use_score = ((hotword.use_count as f64).sqrt() * 8.0).round() as i64;
        let app_score = app_name
            .and_then(|name| hotword.app_usage_stats.get(name))
            .copied()
            .unwrap_or(0)
            .min(4)
            * 20;
        let scenario_score = hotword
            .scenario_usage_stats
            .get(&scenario)
            .copied()
            .unwrap_or(0)
            .min(4)
            * 16;
        let false_positive_penalty = hotword.false_positive_count.min(4) * 45;

        Self::hotword_source_score(&hotword.source)
            + if hotword.user_override { 30 } else { 0 }
            + recent_score
            + use_score
            + app_score
            + scenario_score
            - false_positive_penalty
    }

    fn score_contextual_relevance(hotword: &Hotword, contexts: &[HotwordContext<'_>]) -> i64 {
        if contexts.is_empty() {
            return 0;
        }

        let mut total = 0;
        let mut matched_contexts = 0;

        for context in contexts {
            let text = context.text.trim();
            if text.is_empty() || context.weight <= 0 {
                continue;
            }

            let mut context_score = 0;
            if count_hotword_occurrences(text, &hotword.target) > 0 {
                context_score += 120 * context.weight;
            }

            for alias in &hotword.originals {
                let alias = alias.trim();
                if alias.is_empty() || alias.eq_ignore_ascii_case(&hotword.target) {
                    continue;
                }
                if count_hotword_occurrences(text, alias) > 0 {
                    context_score += 80 * context.weight;
                }
            }

            if context_score > 0 {
                matched_contexts += 1;
                total += context_score;
            }
        }

        if matched_contexts >= 2 {
            total += 60;
        }

        total
    }

    fn rank_hotwords(
        &self,
        scenario: HotwordScenario,
        contexts: &[HotwordContext<'_>],
        app_name: Option<&str>,
    ) -> Result<Vec<RankedHotword>> {
        let mut ranked: Vec<RankedHotword> = self
            .get_by_scenario(scenario)?
            .into_iter()
            .filter(|hotword| hotword.status == "active")
            .map(|hotword| {
                let score = if contexts.is_empty() && app_name.is_none() {
                    hotword.use_count
                } else {
                    Self::score_contextual_relevance(&hotword, contexts)
                        + Self::telemetry_score(&hotword, app_name, scenario)
                };
                RankedHotword { hotword, score }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| {
                    Self::hotword_source_score(&b.hotword.source)
                        .cmp(&Self::hotword_source_score(&a.hotword.source))
                })
                .then_with(|| b.hotword.recent_use_count.cmp(&a.hotword.recent_use_count))
                .then_with(|| b.hotword.use_count.cmp(&a.hotword.use_count))
                .then_with(|| {
                    a.hotword
                        .false_positive_count
                        .cmp(&b.hotword.false_positive_count)
                })
                .then_with(|| b.hotword.created_at.cmp(&a.hotword.created_at))
                .then_with(|| a.hotword.target.cmp(&b.hotword.target))
        });

        debug!(
            "[Hotword] Ranked {} hotwords for scenario {:?} (contexts={})",
            ranked.len(),
            scenario,
            contexts.len()
        );

        Ok(ranked)
    }

    fn format_hotword_entry(entry: &HotwordEntry) -> String {
        if entry.aliases.is_empty() {
            entry.target.clone()
        } else {
            format!(
                "{}（误识别形式: {}）",
                entry.target,
                entry.aliases.join(" / ")
            )
        }
    }

    pub fn summarize_injection(injection: &HotwordInjection) -> String {
        let mut sections = Vec::new();

        if !injection.person_names.is_empty() {
            sections.push(format!(
                "人名类热词：{}",
                injection
                    .person_names
                    .iter()
                    .map(Self::format_hotword_entry)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }

        if !injection.product_names.is_empty() {
            sections.push(format!(
                "产品品牌类热词：{}",
                injection
                    .product_names
                    .iter()
                    .map(Self::format_hotword_entry)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }

        if !injection.domain_terms.is_empty() {
            sections.push(format!(
                "术语缩写类热词：{}",
                injection
                    .domain_terms
                    .iter()
                    .map(Self::format_hotword_entry)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }

        if !injection.hotwords.is_empty() {
            sections.push(format!(
                "其他热词：{}",
                injection
                    .hotwords
                    .iter()
                    .map(Self::format_hotword_entry)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }

        if sections.is_empty() {
            "(none)".to_string()
        } else {
            sections.join("\n")
        }
    }

    /// Compute star rating for a correction pair based on usage evidence.
    fn correction_star_rating(use_count: i64, user_override: bool) -> u8 {
        if user_override || use_count >= 5 {
            3
        } else if use_count >= 2 {
            2
        } else {
            1
        }
    }

    /// Max aliases surfaced per hotword entry in the prompt. Keeps the
    /// injection compact while still telling the LLM the common
    /// misrecognized forms.
    const MAX_ALIASES_PER_ENTRY: usize = 3;

    fn build_injection_from_ranked(
        &self,
        ranked: &[RankedHotword],
        input_texts: &[&str],
    ) -> HotwordInjection {
        let mut injection = HotwordInjection::default();
        let mut seen_person = HashMap::new();
        let mut seen_product = HashMap::new();
        let mut seen_domain = HashMap::new();
        let mut seen_hotword = HashMap::new();

        for ranked_hotword in ranked {
            let hw = &ranked_hotword.hotword;
            let alias_count = hw.originals.len().min(Self::MAX_ALIASES_PER_ENTRY);
            let aliases = &hw.originals[..alias_count];
            match Self::normalize_hotword_bucket(&hw.category) {
                "person" => {
                    Self::merge_hotword_entry(
                        &mut injection.person_names,
                        &mut seen_person,
                        &hw.target,
                        aliases,
                    );
                }
                "product" => {
                    Self::merge_hotword_entry(
                        &mut injection.product_names,
                        &mut seen_product,
                        &hw.target,
                        aliases,
                    );
                }
                "domain" => {
                    Self::merge_hotword_entry(
                        &mut injection.domain_terms,
                        &mut seen_domain,
                        &hw.target,
                        aliases,
                    );
                }
                _ => {
                    Self::merge_hotword_entry(
                        &mut injection.hotwords,
                        &mut seen_hotword,
                        &hw.target,
                        aliases,
                    );
                }
            }
        }

        // Collect correction pairs from hotwords. Both `originals` (fuzzy
        // aliases) and `force_replace_originals` (hard-substitution aliases)
        // are exposed to the LLM as plain correction hints — the hard
        // substitution has already run on the ASR output, but we still pass
        // the pair through as a second line of defense in case the raw form
        // somehow re-enters the text downstream.
        for ranked_hotword in ranked {
            let hw = &ranked_hotword.hotword;
            if hw.status != "active" {
                continue;
            }
            if hw.originals.is_empty() && hw.force_replace_originals.is_empty() {
                continue;
            }
            let stars = Self::correction_star_rating(hw.use_count, hw.user_override);
            for orig in hw.originals.iter().chain(hw.force_replace_originals.iter()) {
                injection.correction_pairs.push(CorrectionPair {
                    original: orig.clone(),
                    target: hw.target.clone(),
                    stars,
                });
            }
        }
        // Sort by stars descending, then alphabetically for stability
        injection.correction_pairs.sort_by(|a, b| {
            b.stars
                .cmp(&a.stars)
                .then_with(|| a.original.cmp(&b.original))
        });
        // Pairs whose misrecognized form actually appears in the input must
        // survive the cap — they are the only ones the LLM can act on. The
        // partition is stable, so star ordering is preserved within each group.
        let matches_input = |original: &str| {
            input_texts
                .iter()
                .any(|t| !t.trim().is_empty() && text_contains_term(t, original))
        };
        let (mut hits, mut rest): (Vec<CorrectionPair>, Vec<CorrectionPair>) = injection
            .correction_pairs
            .drain(..)
            .partition(|p| matches_input(&p.original));
        hits.append(&mut rest);
        injection.correction_pairs = hits;
        // Cap at 15 pairs
        injection.correction_pairs.truncate(15);

        injection
    }

    /// Deterministically scan `text` against the active hotword table.
    /// No LLM involved — this is the authoritative answer to "does the
    /// input touch the user's vocabulary", used to override blind LLM
    /// judgments (intent `needs_hotword=false`, PassThrough routing).
    pub fn scan_local_match(
        &self,
        scenario: HotwordScenario,
        text: &str,
    ) -> Result<LocalHotwordScan> {
        let mut scan = LocalHotwordScan::default();
        if text.trim().is_empty() {
            return Ok(scan);
        }
        for hotword in self.get_by_scenario(scenario)? {
            if hotword.status != "active" {
                continue;
            }
            if !scan.target_hit && text_contains_term(text, &hotword.target) {
                scan.target_hit = true;
            }
            if !scan.alias_hit {
                scan.alias_hit = hotword
                    .originals
                    .iter()
                    .chain(hotword.force_replace_originals.iter())
                    .any(|alias| {
                        !alias.trim().is_empty()
                            && !alias.trim().eq_ignore_ascii_case(&hotword.target)
                            && text_contains_term(text, alias)
                    });
            }
            if scan.alias_hit && scan.target_hit {
                break;
            }
        }
        Ok(scan)
    }

    fn render_ranked_term_reference(
        &self,
        ranked: &[RankedHotword],
        input_texts: &[&str],
    ) -> String {
        let injection = self.build_injection_from_ranked(ranked, input_texts);
        let summary = Self::summarize_injection(&injection);

        if summary == "(none)" {
            "(none)".to_string()
        } else {
            format!("[热词 reference]\n{}", summary)
        }
    }

    fn merge_hotword_entry(
        entries: &mut Vec<HotwordEntry>,
        seen: &mut HashMap<String, usize>,
        target: &str,
        aliases: &[String],
    ) {
        let normalized_target = target.trim();
        if normalized_target.is_empty() {
            return;
        }

        if let Some(index) = seen.get(normalized_target).copied() {
            let entry = &mut entries[index];
            let mut alias_seen: BTreeSet<String> = entry.aliases.iter().cloned().collect();
            for alias in aliases {
                let alias = alias.trim();
                if alias.is_empty() || alias.eq_ignore_ascii_case(normalized_target) {
                    continue;
                }
                if alias_seen.insert(alias.to_string()) {
                    entry.aliases.push(alias.to_string());
                }
            }
            return;
        }

        let mut alias_seen = BTreeSet::new();
        let mut normalized_aliases = Vec::new();
        for alias in aliases {
            let alias = alias.trim();
            if alias.is_empty() || alias.eq_ignore_ascii_case(normalized_target) {
                continue;
            }
            if alias_seen.insert(alias.to_string()) {
                normalized_aliases.push(alias.to_string());
            }
        }

        let index = entries.len();
        entries.push(HotwordEntry {
            target: normalized_target.to_string(),
            aliases: normalized_aliases,
        });
        seen.insert(normalized_target.to_string(), index);
    }

    /// Build structured hotword injection data for a specific scenario.
    /// Uses ranked hotwords so the prompt stays compact and telemetry-aware.
    pub fn build_contextual_injection(
        &self,
        scenario: HotwordScenario,
        current_document: &str,
        spoken_instruction: &str,
        app_name: Option<&str>,
    ) -> Result<HotwordInjection> {
        let contexts = [
            HotwordContext {
                text: current_document,
                weight: 4,
            },
            HotwordContext {
                text: spoken_instruction,
                weight: 5,
            },
        ];
        let ranked = self.rank_hotwords(scenario, &contexts, app_name)?;
        let injection =
            self.build_injection_from_ranked(&ranked, &[current_document, spoken_instruction]);

        debug!(
            "[Hotword] Built contextual injection for scenario {:?}: person={}, product={}, domain={}, hotwords={}",
            scenario,
            injection.person_names.len(),
            injection.product_names.len(),
            injection.domain_terms.len(),
            injection.hotwords.len()
        );

        Ok(injection)
    }

    #[allow(dead_code)]
    pub fn build_injection(&self, scenario: HotwordScenario) -> Result<HotwordInjection> {
        let ranked = self.rank_hotwords(scenario, &[], None)?;
        Ok(self.build_injection_from_ranked(&ranked, &[]))
    }

    /// Build a compact ranked term reference for rewrite prompts.
    pub fn build_ranked_term_reference(
        &self,
        scenario: HotwordScenario,
        current_document: &str,
        spoken_instruction: &str,
        app_name: Option<&str>,
    ) -> Result<String> {
        let contexts = [
            HotwordContext {
                text: current_document,
                weight: 4,
            },
            HotwordContext {
                text: spoken_instruction,
                weight: 5,
            },
        ];
        let ranked = self.rank_hotwords(scenario, &contexts, app_name)?;
        let reference =
            self.render_ranked_term_reference(&ranked, &[current_document, spoken_instruction]);

        debug!(
            "[Hotword] Built ranked term reference for scenario {:?}: len={}",
            scenario,
            reference.chars().count()
        );

        Ok(reference)
    }

    // ── Auto-learning ────────────────────────────────────────────────────

    /// Record an auto-learned hotword from user edit corrections.
    /// If the target already exists, merges the original into its originals array.
    /// If not, creates a new hotword with source='auto_learned'.
    #[allow(dead_code)]
    pub fn record_auto_learned(&self, target: &str, original: &str) -> Result<()> {
        let conn = self.get_connection()?;
        let now = Utc::now().timestamp();

        // Check if hotword with same target already exists (case-insensitive)
        let existing: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, originals FROM hotwords WHERE LOWER(target) = LOWER(?1)",
                params![target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((id, originals_json)) = existing {
            // Merge original into existing originals
            let mut originals: Vec<String> =
                serde_json::from_str(&originals_json).unwrap_or_default();
            if !originals.iter().any(|o| o.eq_ignore_ascii_case(original)) {
                originals.push(original.to_string());
                let merged_json = serde_json::to_string(&originals)?;
                conn.execute(
                    "UPDATE hotwords SET originals = ?1, use_count = use_count + 1, last_used_at = ?2 WHERE id = ?3",
                    params![merged_json, now, id],
                )?;
                info!(
                    "[Hotword] Auto-learned: merged \"{}\" into existing hotword \"{}\" (id={})",
                    original, target, id
                );
            } else {
                // Original already exists, just increment use_count
                conn.execute(
                    "UPDATE hotwords SET use_count = use_count + 1, last_used_at = ?1 WHERE id = ?2",
                    params![now, id],
                )?;
                debug!(
                    "[Hotword] Auto-learned: \"{}\" already in hotword \"{}\", incremented use_count",
                    original, target
                );
            }
        } else {
            // Create new hotword
            let inferred_category = Self::infer_category(target);
            let originals_json = serde_json::to_string(&vec![original])?;

            conn.execute(
                "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, user_override, use_count, false_positive_count, created_at, status, source)
                 VALUES (?1, ?2, '[]', ?3, '[\"work\",\"casual\"]', 0, 1, 0, ?4, 'active', 'auto_learned')",
                params![target, originals_json, inferred_category, now],
            )?;

            info!(
                "[Hotword] Auto-learned: new hotword \"{}\" from correction \"{}\" (category={})",
                target, original, inferred_category
            );
        }

        Ok(())
    }

    /// Record an auto-learned hotword as a suggestion (status='suggested') from LLM analysis.
    pub fn record_auto_learned_suggested(
        &self,
        target: &str,
        original: &str,
        category: &str,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        let now = Utc::now().timestamp();

        // Check if hotword with same target already exists (case-insensitive)
        let existing: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, originals FROM hotwords WHERE LOWER(target) = LOWER(?1)",
                params![target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((id, originals_json)) = existing {
            // Merge original into existing originals and bump use_count
            let mut originals: Vec<String> =
                serde_json::from_str(&originals_json).unwrap_or_default();
            let new_variant = !originals.iter().any(|o| o.eq_ignore_ascii_case(original));
            if new_variant {
                originals.push(original.to_string());
            }
            let merged_json = serde_json::to_string(&originals)?;
            // Always increment use_count to track correction frequency
            conn.execute(
                "UPDATE hotwords SET originals = ?1, use_count = use_count + 1, recent_use_count = recent_use_count + 1 WHERE id = ?2",
                params![merged_json, id],
            )?;
            info!(
                "[Hotword] LLM suggested: {} \"{}\" for hotword \"{}\" (id={})",
                if new_variant {
                    "merged new variant"
                } else {
                    "bumped count for"
                },
                original,
                target,
                id
            );
        } else {
            let originals_json = serde_json::to_string(&vec![original])?;
            conn.execute(
                "INSERT OR IGNORE INTO hotwords (target, originals, force_replace_originals, category, scenarios, user_override, use_count, false_positive_count, created_at, status, source)
                 VALUES (?1, ?2, '[]', ?3, '[\"work\",\"casual\"]', 0, 0, 0, ?4, 'suggested', 'auto_learned')",
                params![target, originals_json, category, now],
            )?;
            info!(
                "[Hotword] LLM suggested: new hotword \"{}\" from correction \"{}\" (category={})",
                target, original, category
            );
        }

        Ok(())
    }

    /// Add AI-suggested hotwords in batch. Uses ON CONFLICT to avoid overwriting existing active hotwords.
    pub fn add_suggested(&self, items: Vec<(String, Vec<String>, String)>) -> Result<Vec<Hotword>> {
        let conn = self.get_connection()?;
        let now = Utc::now().timestamp();
        let mut added = Vec::new();

        for (target, originals, category) in items {
            let originals_json = serde_json::to_string(&originals)?;
            let scenarios_json = "[\"work\",\"casual\"]";

            let result = conn.execute(
                "INSERT OR IGNORE INTO hotwords (target, originals, force_replace_originals, category, scenarios, user_override, use_count, false_positive_count, created_at, status, source)
                 VALUES (?1, ?2, '[]', ?3, ?4, 0, 0, 0, ?5, 'suggested', 'ai_extracted')",
                params![
                    target,
                    originals_json,
                    category,
                    scenarios_json,
                    now
                ],
            )?;

            if result > 0 {
                let id = conn.last_insert_rowid();
                added.push(Hotword {
                    id,
                    target,
                    originals,
                    force_replace_originals: vec![],
                    category,
                    scenarios: vec![HotwordScenario::Work, HotwordScenario::Casual],
                    user_override: false,
                    use_count: 0,
                    recent_use_count: 0,
                    app_usage_stats: HashMap::new(),
                    scenario_usage_stats: HashMap::new(),
                    last_used_at: None,
                    false_positive_count: 0,
                    created_at: now,
                    status: "suggested".to_string(),
                    source: "ai_extracted".to_string(),
                });
            }
        }

        info!(
            "[Hotword] Added {} suggested hotwords (batch of {})",
            added.len(),
            added.len()
        );
        Ok(added)
    }

    /// Get all suggested hotwords
    pub fn get_suggestions(&self) -> Result<Vec<Hotword>> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, target, originals, category, scenarios,
                    user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats,
                    last_used_at, false_positive_count, created_at, status, source, force_replace_originals
             FROM hotwords
             WHERE status = 'suggested'
             ORDER BY created_at DESC",
        )?;

        let rows = stmt.query_map([], Self::row_to_hotword)?;

        let mut hotwords = Vec::new();
        for row in rows {
            hotwords.push(row?);
        }

        debug!("[Hotword] Retrieved {} suggestions", hotwords.len());
        Ok(hotwords)
    }

    /// Accept a suggestion: change status from 'suggested' to 'active'
    pub fn accept_suggestion(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE hotwords SET status = 'active' WHERE id = ?1 AND status = 'suggested'",
            params![id],
        )?;
        info!("[Hotword] Accepted suggestion id={}", id);
        Ok(())
    }

    /// Dismiss a suggestion: delete the suggested hotword
    pub fn dismiss_suggestion(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "DELETE FROM hotwords WHERE id = ?1 AND status = 'suggested'",
            params![id],
        )?;
        info!("[Hotword] Dismissed suggestion id={}", id);
        Ok(())
    }

    /// Accept all suggestions: batch change status to 'active'
    pub fn accept_all_suggestions(&self) -> Result<u64> {
        let conn = self.get_connection()?;
        let count = conn.execute(
            "UPDATE hotwords SET status = 'active' WHERE status = 'suggested'",
            [],
        )?;
        info!("[Hotword] Accepted all {} suggestions", count);
        Ok(count as u64)
    }

    /// Dismiss all suggestions: batch delete
    pub fn dismiss_all_suggestions(&self) -> Result<u64> {
        let conn = self.get_connection()?;
        let count = conn.execute("DELETE FROM hotwords WHERE status = 'suggested'", [])?;
        info!("[Hotword] Dismissed all {} suggestions", count);
        Ok(count as u64)
    }

    // ── LLM Correction Analysis ──────────────────────────────────────────

    /// Analyze correction diffs via LLM to determine if they are ASR errors.
    /// Runs as fire-and-forget from the edit pipeline.
    pub async fn analyze_corrections_via_llm(
        app_handle: tauri::AppHandle,
        db_path: PathBuf,
        corrections: Vec<(String, String)>, // (original, corrected)
    ) {
        if corrections.is_empty() {
            return;
        }

        info!(
            "[Hotword] LLM correction analysis: {} corrections to analyze: [{}]",
            corrections.len(),
            corrections
                .iter()
                .map(|(o, c)| format!("\"{}\" → \"{}\"", o, c))
                .collect::<Vec<_>>()
                .join(", ")
        );

        use crate::managers::prompt::{self, PromptManager};
        use crate::settings::get_settings;
        use std::sync::Arc;

        let settings = get_settings(&app_handle);

        // Resolve provider and model via CachedModel (same as main pipeline)
        let (provider, model, cached_model_id) = {
            // Try selected_prompt_model first (it's a CachedModel.id UUID)
            if let Some(cm_id) = settings
                .selected_prompt_model
                .as_ref()
                .map(|c| &c.primary_id)
            {
                if let Some(cm) = settings.get_cached_model(cm_id) {
                    let prov = settings
                        .post_process_providers
                        .iter()
                        .find(|p| p.id == cm.provider_id);
                    if let Some(p) = prov {
                        (p.clone(), cm.model_id.clone(), Some(cm.id.clone()))
                    } else {
                        info!(
                            "[Hotword] Provider {} not found for cached model {}, skipping LLM correction analysis",
                            cm.provider_id, cm_id
                        );
                        return;
                    }
                } else {
                    info!(
                        "[Hotword] Cached model {} not found, skipping LLM correction analysis",
                        cm_id
                    );
                    return;
                }
            } else {
                // Fallback to active provider + default model
                let prov = match settings.active_post_process_provider() {
                    Some(p) => p.clone(),
                    None => {
                        info!("[Hotword] No active post-process provider, skipping LLM correction analysis");
                        return;
                    }
                };
                let mdl = settings
                    .post_process_models
                    .get(&prov.id)
                    .cloned()
                    .unwrap_or_default();
                if mdl.is_empty() {
                    info!("[Hotword] No model configured, skipping LLM correction analysis");
                    return;
                }
                (prov, mdl, None)
            }
        };

        // Load prompt template
        let prompt_manager = app_handle.state::<Arc<PromptManager>>();
        let template = match prompt_manager.get_prompt(&app_handle, "system_correction_analysis") {
            Ok(t) => t,
            Err(e) => {
                warn!("[Hotword] Failed to load correction analysis prompt: {}", e);
                return;
            }
        };

        let filtered_corrections: Vec<_> = corrections
            .iter()
            .filter(|(orig, corr)| {
                let original = orig.trim();
                let corrected = corr.trim();
                !original.is_empty() && !corrected.is_empty() && original != corrected
            })
            .collect();

        if filtered_corrections.is_empty() {
            info!("[Hotword] No meaningful corrections to analyze, skipping");
            return;
        }

        // Format corrections
        let corrections_text: String = filtered_corrections
            .iter()
            .map(|(orig, corr)| format!("- \"{}\" → \"{}\"", orig, corr))
            .collect::<Vec<_>>()
            .join("\n");

        // Build reference from hotwords that have correction pairs (originals)
        let existing_hotwords_text = {
            let hotword_manager = HotwordManager::new(db_path.clone());
            match hotword_manager.get_all() {
                Ok(hotwords) => {
                    let mut with_corrections: Vec<_> = hotwords
                        .into_iter()
                        .filter(|h| !h.originals.is_empty())
                        .collect();
                    // Sort by use_count desc, multi-word first
                    with_corrections.sort_by(|a, b| {
                        let a_multi = a.target.contains(' ') || a.target.len() > 5;
                        let b_multi = b.target.contains(' ') || b.target.len() > 5;
                        b_multi.cmp(&a_multi).then(b.use_count.cmp(&a.use_count))
                    });
                    let entries: Vec<String> = with_corrections
                        .iter()
                        .take(30)
                        .map(|h| format!("- {} → {}", h.originals.join("/"), h.target))
                        .collect();
                    if entries.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "## Known correction pairs (reference)\n\n{}\n",
                            entries.join("\n")
                        )
                    }
                }
                _ => String::new(),
            }
        };

        info!(
            "[Hotword] LLM correction analysis: provider={}, model={}, corrections_text_len={}, existing_hotwords_len={}",
            provider.id,
            model,
            corrections_text.len(),
            existing_hotwords_text.len()
        );

        let mut vars = HashMap::new();
        vars.insert("corrections", corrections_text);
        vars.insert("existing_hotwords", existing_hotwords_text);
        let system_prompt = prompt::substitute_variables(&template, &vars);
        info!(
            "[Hotword] LLM correction analysis prompt:\n{}",
            system_prompt
        );

        let (response_text, is_error, error_msg, _token_count) =
            crate::actions::post_process::execute_llm_request(
                &app_handle,
                &settings,
                &provider,
                &model,
                cached_model_id.as_deref(),
                &system_prompt,
                Some("请分析上述修正对。"),
                None,
                None,
                None,
                None,
            )
            .await;

        if is_error {
            warn!(
                "[Hotword] LLM correction analysis request failed: {}",
                error_msg.unwrap_or_else(|| "unknown error".to_string())
            );
            return;
        }

        let content = match response_text {
            Some(c) if !c.trim().is_empty() => c,
            _ => {
                warn!("[Hotword] LLM returned empty response for correction analysis");
                return;
            }
        };

        info!(
            "[Hotword] LLM correction analysis raw response ({} chars):\n{}",
            content.len(),
            content
        );

        // Extract JSON from response
        let json_str = extract_json_block_standalone(&content);
        let json_str = match json_str {
            Some(s) => s,
            None => {
                warn!("[Hotword] Failed to extract JSON from LLM response");
                return;
            }
        };

        // Parse response
        #[derive(serde::Deserialize)]
        struct CorrectionItem {
            original: String,
            corrected: String,
            #[serde(rename = "type")]
            correction_type: String,
            category: Option<String>,
        }

        let items: Vec<CorrectionItem> = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "[Hotword] Failed to parse LLM correction analysis JSON: {}",
                    e
                );
                return;
            }
        };

        for item in &items {
            info!(
                "[Hotword] LLM classified: \"{}\" → \"{}\" = {} (category: {:?})",
                item.original, item.corrected, item.correction_type, item.category
            );
        }

        // Record ASR errors as suggested hotwords
        let hotword_manager = HotwordManager::new(db_path);
        let mut created_count = 0;

        for item in &items {
            if item.correction_type == "asr_error" {
                let category = item.category.as_deref().unwrap_or("term");
                info!(
                    "[Hotword] Recording LLM suggestion: \"{}\" (original: \"{}\", category: {})",
                    item.corrected, item.original, category
                );
                if let Err(e) = hotword_manager.record_auto_learned_suggested(
                    &item.corrected,
                    &item.original,
                    category,
                ) {
                    error!("[Hotword] Failed to record LLM-suggested hotword: {}", e);
                } else {
                    created_count += 1;
                }
            }
        }

        if created_count > 0 {
            info!(
                "[Hotword] LLM correction analysis: created {} suggested hotwords from {} items",
                created_count,
                items.len()
            );
            // Notify frontend
            if let Err(e) = app_handle.emit("hotword-suggestions-updated", ()) {
                error!(
                    "[Hotword] Failed to emit hotword-suggestions-updated: {}",
                    e
                );
            }
        }
    }
}

/// Standalone JSON block extractor (mirrors core::extract_json_block but accessible from here)
fn extract_json_block_standalone(content: &str) -> Option<String> {
    // Remove <think>...</think> blocks
    let mut text = content.to_string();
    while let Some(start) = text.find("<think>") {
        if let Some(end) = text[start..].find("</think>") {
            text.replace_range(start..start + end + 8, "");
        } else {
            break;
        }
    }
    let content = text.trim();

    // Try ```json block
    if let Some(start) = content.find("```json") {
        let rest = &content[start + 7..];
        if let Some(end) = rest.find("```") {
            return Some(rest[..end].trim().to_string());
        }
    }
    // Try generic ``` block
    if let Some(start) = content.find("```") {
        let rest = &content[start + 3..];
        if let Some(end) = rest.find("```") {
            return Some(rest[..end].trim().to_string());
        }
    }
    // Try raw [ or {
    let start_bracket = content.find('[');
    let start_brace = content.find('{');
    let (start_char, end_char) = match (start_bracket, start_brace) {
        (Some(b), Some(c)) if b < c => (b, content.rfind(']')?),
        (Some(_), Some(c)) => (c, content.rfind('}')?),
        (Some(b), None) => (b, content.rfind(']')?),
        (None, Some(c)) => (c, content.rfind('}')?),
        (None, None) => return None,
    };
    if end_char > start_char {
        return Some(content[start_char..=end_char].trim().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init_hotword_db(path: &std::path::Path) {
        let conn = Connection::open(path).expect("open temp db");
        conn.execute_batch(
            r#"
            CREATE TABLE hotwords (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL UNIQUE,
                originals TEXT NOT NULL DEFAULT '[]',
                force_replace_originals TEXT NOT NULL DEFAULT '[]',
                category TEXT NOT NULL DEFAULT 'term',
                scenarios TEXT NOT NULL DEFAULT '["work","casual"]',
                confidence REAL NOT NULL DEFAULT 0.5,
                user_override BOOLEAN NOT NULL DEFAULT 0,
                use_count INTEGER NOT NULL DEFAULT 0,
                recent_use_count INTEGER NOT NULL DEFAULT 0,
                app_usage_stats TEXT NOT NULL DEFAULT '{}',
                scenario_usage_stats TEXT NOT NULL DEFAULT '{}',
                last_used_at INTEGER,
                false_positive_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                source TEXT NOT NULL DEFAULT 'manual'
            );
            "#,
        )
        .expect("create hotwords table");

        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', '[]', 'term', '[\"work\",\"casual\"]', 0.5, 0, 0, 0, '{}', '{}', 0, 1, 'active', 'manual')",
            params!["Votype"],
        )
        .expect("insert hotword");
    }

    #[test]
    fn test_infer_category_abbreviation() {
        assert_eq!(HotwordManager::infer_category("API"), "abbreviation");
        assert_eq!(HotwordManager::infer_category("SDK"), "abbreviation");
        assert_eq!(HotwordManager::infer_category("CEO"), "abbreviation");
        assert_eq!(HotwordManager::infer_category("HTTP"), "abbreviation");
        assert_eq!(HotwordManager::infer_category("HTTPS"), "abbreviation");
    }

    #[test]
    fn test_infer_category_abbreviation_too_long() {
        let category = HotwordManager::infer_category("ABCDEF");
        assert_ne!(category, "abbreviation");
    }

    #[test]
    fn test_infer_category_abbreviation_too_short() {
        let category = HotwordManager::infer_category("A");
        assert_ne!(category, "abbreviation");
    }

    #[test]
    fn test_infer_category_technical_terms() {
        assert_eq!(HotwordManager::infer_category("UserConfig"), "term");
        assert_eq!(HotwordManager::infer_category("TaskManager"), "term");
        assert_eq!(HotwordManager::infer_category("AuthService"), "term");
        assert_eq!(HotwordManager::infer_category("EventHandler"), "term");
        assert_eq!(HotwordManager::infer_category("AppController"), "term");
        assert_eq!(HotwordManager::infer_category("DataProvider"), "term");
    }

    #[test]
    fn test_infer_category_person() {
        assert_eq!(HotwordManager::infer_category("John"), "person");
        assert_eq!(HotwordManager::infer_category("Alice"), "person");
    }

    #[test]
    fn test_infer_category_default() {
        assert_eq!(HotwordManager::infer_category("kubernetes"), "term");
        assert_eq!(HotwordManager::infer_category("docker"), "term");
    }

    #[test]
    fn test_infer_category_mixed_case_not_abbreviation() {
        let category = HotwordManager::infer_category("Api");
        assert_ne!(category, "abbreviation");
    }

    #[test]
    fn test_record_final_output_usage_updates_telemetry_and_is_readable() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let manager = HotwordManager::new(db_path.clone());
        let hits = manager
            .record_final_output_usage("Votype lands twice: Votype.", Some("Code"))
            .expect("record hotword usage");

        assert_eq!(hits, 2);

        let hotwords = manager
            .get_all_with_usage_stats()
            .expect("read hotwords with telemetry");
        assert_eq!(hotwords.len(), 1);

        let hotword = &hotwords[0];
        assert_eq!(hotword.target, "Votype");
        assert_eq!(hotword.use_count, 2);
        assert_eq!(hotword.recent_use_count, 2);
        assert_eq!(hotword.app_usage_stats.get("Code"), Some(&2));
        assert_eq!(
            hotword.scenario_usage_stats.get(&HotwordScenario::Work),
            Some(&2)
        );
        assert!(hotword.last_used_at.is_some());
    }

    #[test]
    fn test_ranked_retrieval_prefers_contextual_manual_over_high_use_count() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 1, 1, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["ContextualManual", serde_json::to_string(&vec!["ctx manual"]).unwrap()],
        )
        .expect("insert contextual hotword");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 0, 120, 10, '{}', '{}', 0, 3, 'active', 'ai_extracted')",
            params!["FrequentNoise", serde_json::to_string(&vec!["freq noise"]).unwrap()],
        )
        .expect("insert noisy hotword");

        let manager = HotwordManager::new(db_path);
        let reference = manager
            .build_ranked_term_reference(
                HotwordScenario::Work,
                "ContextualManual should be preserved in the document",
                "please keep contextual manual intact",
                None,
            )
            .expect("build ranked term reference");

        let first_entry = reference
            .lines()
            .find(|line| line.contains("ContextualManual"))
            .expect("first entry");

        assert!(first_entry.contains("ContextualManual"));
        assert!(reference.contains("ContextualManual"));
        assert!(reference.contains("术语缩写类热词"));
        assert!(reference.contains("FrequentNoise"));
    }

    #[test]
    fn test_ranked_injection_keeps_only_a_small_number_of_aliases_and_noise_terms() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 1, 20, 6, '{}', '{}', 0, 2, 'active', 'manual')",
            params![
                "VotypePro",
                serde_json::to_string(&vec!["vo type", "vtype", "votypeee"]).unwrap()
            ],
        )
        .expect("insert hotword with aliases");

        for idx in 0..6 {
            conn.execute(
                "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
                 VALUES (?1, '[]', 'term', '[\"work\"]', 0.5, 0, 0, 0, '{}', '{}', 0, ?2, 'active', 'ai_extracted')",
                params![format!("Noise{}", idx + 1), 10 + idx as i64],
            )
            .expect("insert noise hotword");
        }

        let manager = HotwordManager::new(db_path);
        let reference = manager
            .build_ranked_term_reference(
                HotwordScenario::Work,
                "The document mentions VotypePro directly",
                "please keep vo type and vtype in the output",
                None,
            )
            .expect("build ranked term reference");

        assert!(reference.contains("VotypePro"));
        // Aliases are now rendered (capped at MAX_ALIASES_PER_ENTRY) so the
        // LLM knows the misrecognized forms it should correct.
        assert!(reference.contains("vo type"));
        assert!(reference.contains("vtype"));
        assert!(reference.contains("误识别形式"));
        assert!(reference.contains("Noise1"));
    }

    #[test]
    fn test_contextual_injection_prefers_document_and_instruction_matches() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 1, 2, 1, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["GSON", serde_json::to_string(&vec!["Jason", "GASON"]).unwrap()],
        )
        .expect("insert contextual hotword");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', 'term', '[\"work\"]', 0.5, 0, 80, 10, '{}', '{}', 0, 3, 'active', 'ai_extracted')",
            params!["NoiseHotword"],
        )
        .expect("insert noisy hotword");

        let manager = HotwordManager::new(db_path);
        let injection = manager
            .build_contextual_injection(
                HotwordScenario::Work,
                "当前文稿里已经提到 Jason 这个术语",
                "请把 GASON 改成 GSON",
                Some("Code"),
            )
            .expect("build contextual injection");

        let rendered: Vec<String> = injection
            .domain_terms
            .iter()
            .chain(injection.hotwords.iter())
            .map(HotwordManager::format_hotword_entry)
            .collect();

        assert!(rendered.iter().any(|entry| entry.contains("GSON")));
        assert!(rendered.iter().any(|entry| entry.contains("NoiseHotword")));
        // Aliases are rendered as labelled misrecognized forms next to the target.
        assert!(rendered
            .iter()
            .any(|entry| entry.contains("误识别形式") && entry.contains("Jason")));
    }

    #[test]
    fn test_text_contains_term_word_boundary() {
        // Short ASCII terms keep strict word boundaries — no substring hits.
        assert!(!text_contains_term("format the disk", "mat"));
        assert!(text_contains_term("send it to matt please", "Matt"));
        // CJK substring match
        assert!(text_contains_term("我在用窝太普写东西", "窝太普"));
        assert!(!text_contains_term("我在用别的工具", "窝太普"));
    }

    #[test]
    fn test_text_contains_term_space_variant() {
        // Segmentation variants: "vo type" ↔ "votype" (≥5 condensed chars)
        assert!(text_contains_term("open vo type now", "votype"));
        assert!(text_contains_term("votype rocks", "vo type"));
        // Short condensed terms must NOT fall back to loose matching
        assert!(!text_contains_term("a b c d", "ab"));
    }

    #[test]
    fn test_scan_local_match_alias_and_target() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 0, 0, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["八戒", serde_json::to_string(&vec!["八哥"]).unwrap()],
        )
        .expect("insert alias hotword");

        let manager = HotwordManager::new(db_path);

        let scan = manager
            .scan_local_match(HotwordScenario::Work, "让八哥模型跑一下")
            .expect("scan alias");
        assert!(scan.alias_hit);
        assert!(!scan.target_hit);

        let scan = manager
            .scan_local_match(HotwordScenario::Work, "Votype is great")
            .expect("scan target");
        assert!(!scan.alias_hit);
        assert!(scan.target_hit);

        let scan = manager
            .scan_local_match(HotwordScenario::Work, "完全无关的内容")
            .expect("scan none");
        assert!(!scan.alias_hit);
        assert!(!scan.target_hit);
    }

    #[test]
    fn test_correction_pairs_input_hits_survive_truncation() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        // 19 high-star pairs (use_count >= 5 → ★★★) that do NOT match the input
        for idx in 0..19 {
            conn.execute(
                "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
                 VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 0, 10, 0, '{}', '{}', 0, ?3, 'active', 'manual')",
                params![
                    format!("Target{}", idx),
                    serde_json::to_string(&vec![format!("误词{}", idx)]).unwrap(),
                    10 + idx as i64
                ],
            )
            .expect("insert high-star hotword");
        }
        // 1 low-star pair (use_count 0 → ★) whose original DOES appear in input
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, 'term', '[\"work\"]', 0.5, 0, 0, 0, '{}', '{}', 0, 99, 'active', 'manual')",
            params!["真命中", serde_json::to_string(&vec!["特殊误词"]).unwrap()],
        )
        .expect("insert needle hotword");

        let manager = HotwordManager::new(db_path);
        let input = "这句话里有特殊误词需要修正";
        let injection = manager
            .build_contextual_injection(HotwordScenario::Work, input, input, None)
            .expect("build injection");

        assert!(injection.correction_pairs.len() <= 15);
        // The input-matching pair must survive the cap despite its low star rating.
        assert!(injection
            .correction_pairs
            .iter()
            .any(|p| p.original == "特殊误词" && p.target == "真命中"));
        // And it must be ordered before non-matching pairs.
        assert_eq!(injection.correction_pairs[0].original, "特殊误词");
    }

    #[test]
    fn test_build_injection_keeps_zero_count_manual_hotwords() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', 'term', '[\"work\"]', 0.5, 1, 0, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["ZeroCountTerm"],
        )
        .expect("insert zero-count hotword");

        let manager = HotwordManager::new(db_path);
        let injection = manager
            .build_injection(HotwordScenario::Work)
            .expect("build injection");

        let rendered: Vec<String> = injection
            .person_names
            .iter()
            .chain(injection.product_names.iter())
            .chain(injection.domain_terms.iter())
            .chain(injection.hotwords.iter())
            .map(HotwordManager::format_hotword_entry)
            .collect();

        assert!(rendered.iter().any(|entry| entry == "ZeroCountTerm"));
    }

    #[test]
    fn test_summarize_injection_uses_grouped_single_line_sections() {
        let summary = HotwordManager::summarize_injection(&HotwordInjection {
            person_names: vec![
                HotwordEntry {
                    target: "Matt".to_string(),
                    aliases: vec![],
                },
                HotwordEntry {
                    target: "Nate".to_string(),
                    aliases: vec![],
                },
            ],
            product_names: vec![HotwordEntry {
                target: "Votype".to_string(),
                aliases: vec![],
            }],
            domain_terms: vec![HotwordEntry {
                target: "ASR".to_string(),
                aliases: vec![],
            }],
            hotwords: vec![HotwordEntry {
                target: "悬浮窗".to_string(),
                aliases: vec![],
            }],
            correction_pairs: vec![],
        });

        assert!(summary.contains("人名类热词：Matt、Nate"));
        assert!(summary.contains("产品品牌类热词：Votype"));
        assert!(summary.contains("术语缩写类热词：ASR"));
        assert!(summary.contains("其他热词：悬浮窗"));
    }

    #[test]
    fn test_apply_force_replacements_replaces_only_active_exact_matches() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', ?2, 'term', '[\"work\"]', 0.5, 1, 0, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["差距", serde_json::to_string(&vec!["叉举"]).unwrap()],
        )
        .expect("insert force replace hotword");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, '[]', 'term', '[\"work\"]', 0.5, 1, 0, 0, '{}', '{}', 0, 3, 'active', 'manual')",
            params!["保留", serde_json::to_string(&vec!["不替换"]).unwrap()],
        )
        .expect("insert disabled hotword");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', ?2, 'term', '[\"work\"]', 0.5, 1, 0, 0, '{}', '{}', 0, 4, 'suggested', 'manual')",
            params!["忽略", serde_json::to_string(&vec!["建议词"]).unwrap()],
        )
        .expect("insert suggested hotword");

        let manager = HotwordManager::new(db_path);
        let replaced = manager
            .apply_force_replacements("这里的叉举很大，不替换和建议词都不该动。")
            .expect("apply force replacements");

        assert!(replaced.contains("差距"));
        assert!(!replaced.contains("叉举"));
        assert!(replaced.contains("不替换"));
        assert!(replaced.contains("建议词"));
    }

    #[test]
    fn test_apply_force_replacements_does_not_replace_ascii_substrings() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', ?2, 'term', '[\"work\"]', 0.5, 1, 0, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["OpenAI", serde_json::to_string(&vec!["op"]).unwrap()],
        )
        .expect("insert ascii force replace hotword");

        let manager = HotwordManager::new(db_path);
        let replaced = manager
            .apply_force_replacements("open operator op OP")
            .expect("apply force replacements");

        assert_eq!(replaced, "open operator OpenAI OpenAI");
    }

    #[test]
    fn test_apply_force_replacements_replaces_ascii_sandwiched_by_cjk() {
        // Regression: Rust regex `\b` uses Unicode word class by default, so
        // Chinese characters are considered word chars and an ASCII alias
        // sandwiched between Han letters (e.g. "出现lexi相关") was never
        // treated as having a word boundary. Force-replace must still fire.
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, '[]', ?2, 'term', '[\"work\"]', 0.5, 1, 0, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params!["legacy", serde_json::to_string(&vec!["Lexi"]).unwrap()],
        )
        .expect("insert cjk-adjacent force replace hotword");

        let manager = HotwordManager::new(db_path);
        let replaced = manager
            .apply_force_replacements("会不会再出现lexi相关的一些字段信息了？")
            .expect("apply force replacements");

        assert_eq!(
            replaced, "会不会再出现legacy相关的一些字段信息了？",
            "ASCII alias must replace at CJK/ASCII junction"
        );
    }

    #[test]
    fn test_contextual_injection_includes_force_replace_aliases_in_correction_pairs() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, ?3, 'term', '[\"work\"]', 0.5, 1, 5, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params![
                "差距",
                serde_json::to_string(&vec!["常规纠错"]).unwrap(),
                serde_json::to_string(&vec!["叉举"]).unwrap()
            ],
        )
        .expect("insert force replace hotword");

        let manager = HotwordManager::new(db_path);
        let injection = manager
            .build_contextual_injection(
                HotwordScenario::Work,
                "这里有叉举这个误识别",
                "请修正文稿",
                Some("Code"),
            )
            .expect("build contextual injection");

        assert!(injection
            .correction_pairs
            .iter()
            .any(|pair| pair.target == "差距" && pair.original == "常规纠错"));
        assert!(injection
            .correction_pairs
            .iter()
            .any(|pair| pair.target == "差距" && pair.original == "叉举"));
    }

    #[test]
    fn test_contextual_injection_includes_force_replace_when_originals_empty() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("hotwords.db");
        init_hotword_db(&db_path);

        let conn = Connection::open(&db_path).expect("open temp db");
        conn.execute(
            "INSERT INTO hotwords (target, originals, force_replace_originals, category, scenarios, confidence, user_override, use_count, recent_use_count, app_usage_stats, scenario_usage_stats, false_positive_count, created_at, status, source)
             VALUES (?1, ?2, ?3, 'term', '[\"work\"]', 0.5, 1, 5, 0, '{}', '{}', 0, 2, 'active', 'manual')",
            params![
                "Lexi",
                serde_json::to_string::<Vec<String>>(&vec![]).unwrap(),
                serde_json::to_string(&vec!["lexi"]).unwrap()
            ],
        )
        .expect("insert force-replace-only hotword");

        let manager = HotwordManager::new(db_path);
        let injection = manager
            .build_contextual_injection(
                HotwordScenario::Work,
                "lexi 版本待升级",
                "请修正文稿",
                Some("Code"),
            )
            .expect("build contextual injection");

        assert!(injection
            .correction_pairs
            .iter()
            .any(|pair| pair.target == "Lexi" && pair.original == "lexi"));
    }
}
