use log::{debug, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_log::LogLevel;
use tauri_plugin_store::StoreExt;

use crate::fallback::ModelChain;

static SETTINGS_VERSION: AtomicU64 = AtomicU64::new(0);
static CACHED_SETTINGS: Mutex<Option<(u64, AppSettings)>> = Mutex::new(None);

pub const APPLE_INTELLIGENCE_PROVIDER_ID: &str = "apple_intelligence";
pub const APPLE_INTELLIGENCE_DEFAULT_MODEL_ID: &str = "Apple Intelligence";
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_binding: String,
    pub current_binding: String,
}

/// Skill type determines execution behavior
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "lowercase")]
pub enum SkillType {
    /// Text processing skill (polish, translate, etc.)
    #[default]
    Text,
    /// Action skill (open folder, query weather, etc.)
    Action,
    /// For compatibility with main branch
    Prompt,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillOutputMode {
    /// Polish mode: ASR result -> AI refinement -> Insert. UI shows Diff.
    Polish,
    /// Chat mode: ASR result -> AI Q&A -> Preview. UI shows Markdown.
    Chat,
    /// Silent mode: No UI output (for Action skills)
    Silent,
    // Variants for main branch compatibility
    Replace,
    Append,
    Overlay,
}

impl Default for SkillOutputMode {
    fn default() -> Self {
        SkillOutputMode::Chat
    }
}

fn default_true() -> bool {
    true
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            id: "".to_string(),
            name: "".to_string(),
            description: "".to_string(),
            instructions: "".to_string(),
            prompt: "".to_string(),
            model_id: None,
            icon: None,
            skill_type: SkillType::default(),
            source: SkillSource::default(),
            confidence_check_enabled: false,
            confidence_threshold: None,
            output_mode: SkillOutputMode::default(),
            enabled: true,
            customized: false,
            locked: false,
            param_preset: None,
            file_path: None,
        }
    }
}

/// Skill source indicates where the skill was loaded from
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, Type)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    /// Built-in skill shipped with the app
    #[default]
    Builtin,
    /// User-created skill in ~/.votype/skills/user/
    User,
    /// Imported third-party skill in ~/.votype/skills/imported/
    Imported,
    /// External skill referenced by path
    External { path: String },
}

/// Metadata for a hotword category (stored in DB)
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct HotwordCategoryMeta {
    pub id: String,
    pub label: String,
    pub color: String,
    pub icon: String,
    pub sort_order: i64,
    pub is_builtin: bool,
}

/// Usage scenario for hotwords
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Type)]
#[serde(rename_all = "lowercase")]
pub enum HotwordScenario {
    /// Work context (meetings, documents, code)
    Work,
    /// Casual conversation (chat, memos)
    Casual,
}

/// Hotword entry with classification metadata
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct Hotword {
    pub id: i64,
    /// Possible misrecognized forms (can be multiple)
    pub originals: Vec<String>,
    /// Aliases that should be force-replaced locally before any LLM step
    #[serde(default)]
    pub force_replace_originals: Vec<String>,
    /// Target correct form
    pub target: String,
    /// Semantic category (e.g. "person", "term", "brand", "abbreviation", or custom)
    pub category: String,
    /// Usage scenarios (can be multiple)
    pub scenarios: Vec<HotwordScenario>,
    /// Whether user manually overrode the category
    pub user_override: bool,
    /// Usage statistics
    pub use_count: i64,
    #[serde(default)]
    pub recent_use_count: i64,
    #[serde(default)]
    pub app_usage_stats: HashMap<String, i64>,
    #[serde(default)]
    pub scenario_usage_stats: HashMap<HotwordScenario, i64>,
    pub last_used_at: Option<i64>,
    pub false_positive_count: i64,
    pub created_at: i64,
    /// "active" (confirmed) or "suggested" (AI-suggested, pending user confirmation)
    #[serde(default = "default_hotword_status")]
    pub status: String,
    /// Source of the hotword: "manual", "auto_learned", "ai_extracted"
    #[serde(default = "default_hotword_source")]
    pub source: String,
}

fn default_hotword_status() -> String {
    "active".to_string()
}

fn default_hotword_source() -> String {
    "manual".to_string()
}

/// Backward compatibility: LLMPrompt is now an alias for Skill
pub type LLMPrompt = Skill;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Skill {
    pub id: String,
    pub name: String,
    /// Description for LLM intent recognition (Agent Skills compatible)
    #[serde(default)]
    pub description: String,
    pub instructions: String,
    pub prompt: String,
    pub model_id: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub skill_type: SkillType,
    #[serde(default)]
    pub source: SkillSource,
    #[serde(default, alias = "compliance_check_enabled")]
    pub confidence_check_enabled: bool,
    #[serde(default, alias = "compliance_threshold")]
    pub confidence_threshold: Option<u8>,
    #[serde(default)]
    pub output_mode: SkillOutputMode,
    /// Whether this skill is enabled (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether this skill has been customized by user in settings
    /// If true, user's version takes priority over file-based version
    #[serde(default)]
    pub customized: bool,
    /// Whether this skill is locked (prevents editing/deletion)
    #[serde(default)]
    pub locked: bool,
    /// 参数预设标识，用于匹配模型族的预设参数
    /// 例如: "accurate", "balanced", "creative"
    #[serde(default)]
    pub param_preset: Option<String>,
    /// File path for external skills (user/imported source only)
    /// Skipped from serialization (runtime only)
    #[serde(skip)]
    pub file_path: Option<std::path::PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct PostProcessProvider {
    pub id: String,
    pub label: String,
    pub base_url: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default = "default_true")]
    pub deletable: bool,
    #[serde(default)]
    pub allow_base_url_edit: bool,
    #[serde(default)]
    pub models_endpoint: Option<String>,
    #[serde(default)]
    pub supports_structured_output: bool,
    /// Custom HTTP headers to include in every request to this provider
    /// e.g. {"X-Custom-Auth": "token123"}
    #[serde(default)]
    pub custom_headers: Option<HashMap<String, String>>,
    /// Whether this provider should use the global proxy
    #[serde(default = "default_true")]
    pub use_proxy: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum ModelType {
    Text,
    Asr,
    Other,
}

/// Multi-model post-process configuration item
/// Uses cached_model_id and prompt_id to reference existing models and prompts
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct MultiModelPostProcessItem {
    /// Unique identifier for this item
    pub id: String,
    /// Reference to LLM provider id
    pub provider_id: String,
    /// Reference to LLM model id
    pub model_id: String,
    /// Reference to skill/prompt id
    pub prompt_id: String,
    /// Display name (optional, defaults to model name + prompt name)
    #[serde(default)]
    pub custom_label: Option<String>,
    /// Whether this item is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for MultiModelPostProcessItem {
    fn default() -> Self {
        let timestamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        Self {
            id: format!("mmpp_{}", timestamp),
            provider_id: String::new(),
            model_id: String::new(),
            prompt_id: String::new(),
            custom_label: None,
            enabled: true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct CachedModel {
    pub id: String,
    pub name: String,
    pub model_type: ModelType,
    pub provider_id: String,
    pub model_id: String,
    pub added_at: String,
    #[serde(default)]
    pub custom_label: Option<String>,
    /// 是否为 Thinking 模式（深度推理）模型
    #[serde(default)]
    pub is_thinking_model: bool,
    /// 模型族标识，用于自动匹配参数预设
    /// 例如: "qwen3", "gpt-4o", "claude", "deepseek"
    #[serde(default)]
    pub model_family: Option<String>,
    /// LLM 指令消息角色（system / developer）
    #[serde(default)]
    pub prompt_message_role: PromptMessageRole,
    /// 额外的请求参数（JSON 格式，会合并到 LLM 请求体中）
    /// 例如: {"extended_thinking": true, "thinking_budget_tokens": 10000}
    #[serde(default)]
    pub extra_params: Option<HashMap<String, serde_json::Value>>,
    /// 额外的请求头（会合并到 HTTP 请求头中）
    /// 例如: {"X-Custom-Token": "abc123"}
    #[serde(default)]
    pub extra_headers: Option<HashMap<String, String>>,
}

pub fn thinking_extra_params_with_aliases(
    model_id: &str,
    provider_id: &str,
    enable: bool,
    aliases: &[&str],
) -> Option<HashMap<String, serde_json::Value>> {
    // Check model_id and all aliases
    let candidates: Vec<String> = std::iter::once(model_id)
        .chain(aliases.iter().copied())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();

    let matches = |pattern: &str| candidates.iter().any(|c| c.contains(pattern));

    // DeepSeek: R1, reasoner
    if matches("deepseek") && (matches("r1") || matches("reasoner")) {
        let mut m = HashMap::new();
        m.insert(
            "thinking".to_string(),
            serde_json::json!({"type": if enable { "enabled" } else { "disabled" }}),
        );
        return Some(m);
    }

    // Qwen3 hybrid models
    if matches("qwen3") && !matches("instruct-2507") {
        let mut m = HashMap::new();
        if provider_id == "iflow" {
            m.insert("enable_thinking".to_string(), serde_json::json!(enable));
        } else {
            m.insert(
                "chat_template_kwargs".to_string(),
                serde_json::json!({"enable_thinking": enable}),
            );
        }
        return Some(m);
    }

    // QwQ (Qwen reasoning model)
    if matches("qwq") {
        let mut m = HashMap::new();
        m.insert(
            "chat_template_kwargs".to_string(),
            serde_json::json!({"enable_thinking": enable}),
        );
        return Some(m);
    }

    // OpenAI o-series reasoning models
    if candidates
        .iter()
        .any(|c| c.starts_with("o1") || c.starts_with("o3") || c.starts_with("o4"))
    {
        let mut m = HashMap::new();
        m.insert(
            "reasoning_effort".to_string(),
            serde_json::json!(if enable { "high" } else { "low" }),
        );
        return Some(m);
    }

    // Anthropic Claude
    if matches("claude") {
        let mut m = HashMap::new();
        if enable {
            m.insert(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled", "budget_tokens": 10000}),
            );
        } else {
            m.insert(
                "thinking".to_string(),
                serde_json::json!({"type": "disabled"}),
            );
        }
        return Some(m);
    }

    // Google Gemini 2.5 / 3 thinking models
    if matches("gemini") && (matches("2.5") || matches("2-5") || matches("3")) {
        let mut m = HashMap::new();
        m.insert(
            "reasoning_effort".to_string(),
            serde_json::json!(if enable { "high" } else { "low" }),
        );
        return Some(m);
    }

    // GLM series (all GLM models support thinking toggle)
    if matches("glm") {
        let mut m = HashMap::new();
        m.insert(
            "thinking".to_string(),
            serde_json::json!({"type": if enable { "enabled" } else { "disabled" }}),
        );
        return Some(m);
    }

    // Generic fallback: any candidate contains "thinking" or "reasoner"
    if matches("thinking") || matches("reasoner") {
        let mut m = HashMap::new();
        m.insert(
            "thinking".to_string(),
            serde_json::json!({"type": if enable { "enabled" } else { "disabled" }}),
        );
        return Some(m);
    }

    None
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "lowercase")]
pub enum PromptMessageRole {
    #[default]
    System,
    Developer,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    None,
    Top,
    Bottom,
    #[serde(rename = "follow")]
    FollowCursor,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelUnloadTimeout {
    #[default]
    Never,
    Immediately,
    Min2,
    Min5,
    Min10,
    Min15,
    Hour1,
    Sec5, // Debug mode only
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PasteMethod {
    CtrlV,
    Direct,
    None,
    ShiftInsert,
    CtrlShiftV,
    ExternalScript,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardHandling {
    #[default]
    DontModify,
    CopyToClipboard,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "snake_case")]
pub enum AutoSubmitKey {
    #[default]
    Enter,
    CtrlEnter,
    CmdEnter,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum RecordingRetentionPeriod {
    Never,
    PreserveLimit,
    Days3,
    Weeks2,
    Months3,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum AppReviewPolicy {
    /// Use global confidence threshold (default)
    Auto,
    /// Always show review window
    Always,
    /// Never show review window, direct insert
    Never,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardImplementation {
    Tauri,
    HandyKeys,
}

impl Default for KeyboardImplementation {
    fn default() -> Self {
        // Default to HandyKeys only on macOS where it's well-tested.
        // Windows and Linux use Tauri by default (handy-keys not sufficiently tested yet).
        #[cfg(target_os = "macos")]
        return KeyboardImplementation::HandyKeys;
        #[cfg(not(target_os = "macos"))]
        return KeyboardImplementation::Tauri;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum TitleMatchType {
    /// Simple text contains matching (substring)
    Text,
    /// Regular expression matching
    Regex,
    /// Exact literal equality (used by auto-generated rules from the
    /// suggestion engine — see docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md)
    Exact,
}

impl Default for TitleMatchType {
    fn default() -> Self {
        TitleMatchType::Text
    }
}

pub type PromptOutputMode = SkillOutputMode;

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct TitleRule {
    pub id: String,
    pub pattern: String,
    #[serde(default)]
    pub match_type: TitleMatchType,
    pub policy: AppReviewPolicy,
    pub prompt_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AppProfile {
    pub id: String,
    pub name: String,
    pub policy: AppReviewPolicy,
    pub prompt_id: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub translate_to_english_on_insert: bool,
    #[serde(default)]
    pub disable_selection_clipboard_fallback: bool,
    /// Title-based sub-rules for this app group
    #[serde(default)]
    pub rules: Vec<TitleRule>,
}

impl Default for PasteMethod {
    fn default() -> Self {
        // Default to CtrlV for macOS and Windows, Direct for Linux
        #[cfg(target_os = "linux")]
        return PasteMethod::Direct;
        #[cfg(not(target_os = "linux"))]
        return PasteMethod::CtrlV;
    }
}

impl ModelUnloadTimeout {
    pub fn to_minutes(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0), // Special case for immediate unloading
            ModelUnloadTimeout::Min2 => Some(2),
            ModelUnloadTimeout::Min5 => Some(5),
            ModelUnloadTimeout::Min10 => Some(10),
            ModelUnloadTimeout::Min15 => Some(15),
            ModelUnloadTimeout::Hour1 => Some(60),
            ModelUnloadTimeout::Sec5 => Some(0), // Special case for debug - handled separately
        }
    }

    pub fn to_seconds(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0), // Special case for immediate unloading
            ModelUnloadTimeout::Sec5 => Some(5),
            _ => self.to_minutes().map(|m| m * 60),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SoundTheme {
    Marimba,
    Pop,
    Custom,
}

impl SoundTheme {
    fn as_str(&self) -> &'static str {
        match self {
            SoundTheme::Marimba => "marimba",
            SoundTheme::Pop => "pop",
            SoundTheme::Custom => "custom",
        }
    }

    pub fn to_start_path(self) -> String {
        format!("resources/{}_start.wav", self.as_str())
    }

    pub fn to_stop_path(self) -> String {
        format!("resources/{}_stop.wav", self.as_str())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum TypingTool {
    Auto,
    Wtype,
    Kwtype,
    Dotool,
    Ydotool,
    Xdotool,
}

impl Default for TypingTool {
    fn default() -> Self {
        TypingTool::Auto
    }
}

fn default_show_tray_icon() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    Toggle,
    Hold,
    HoldOrToggle,
}

impl Default for ActivationMode {
    fn default() -> Self {
        Self::Toggle
    }
}

impl<'de> serde::Deserialize<'de> for ActivationMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ActivationModeVisitor;
        impl<'de> serde::de::Visitor<'de> for ActivationModeVisitor {
            type Value = ActivationMode;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string (\"toggle\", \"hold\", \"hold_or_toggle\") or boolean")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(if v {
                    ActivationMode::Hold
                } else {
                    ActivationMode::Toggle
                })
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "toggle" => Ok(ActivationMode::Toggle),
                    "hold" => Ok(ActivationMode::Hold),
                    "hold_or_toggle" => Ok(ActivationMode::HoldOrToggle),
                    _ => Err(E::unknown_variant(v, &["toggle", "hold", "hold_or_toggle"])),
                }
            }
        }
        deserializer.deserialize_any(ActivationModeVisitor)
    }
}

/// A single API key entry with metadata
#[derive(Clone, Serialize, Deserialize, Type)]
pub struct KeyEntry {
    pub key: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub label: Option<String>,
}

/// A map of provider_id → list of API key entries.
/// Backward-compatible: deserializes from old `HashMap<String, String>` format.
#[derive(Clone, Serialize, Type)]
#[serde(transparent)]
pub struct SecretKeyRing(pub HashMap<String, Vec<KeyEntry>>);

impl fmt::Debug for SecretKeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted: HashMap<&String, usize> = self
            .0
            .iter()
            .map(|(k, v)| (k, v.iter().filter(|e| !e.key.is_empty()).count()))
            .collect();
        write!(f, "SecretKeyRing({:?})", redacted)
    }
}

impl std::ops::Deref for SecretKeyRing {
    type Target = HashMap<String, Vec<KeyEntry>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SecretKeyRing {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'de> Deserialize<'de> for SecretKeyRing {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Object(map) => {
                let mut result = HashMap::new();
                for (key, val) in map {
                    match val {
                        serde_json::Value::String(s) => {
                            if s.is_empty() {
                                result.insert(key, Vec::new());
                            } else {
                                result.insert(
                                    key,
                                    vec![KeyEntry {
                                        key: s,
                                        enabled: true,
                                        label: None,
                                    }],
                                );
                            }
                        }
                        serde_json::Value::Array(_) => {
                            let entries: Vec<KeyEntry> =
                                serde_json::from_value(val).map_err(de::Error::custom)?;
                            result.insert(key, entries);
                        }
                        _ => {
                            result.insert(key, Vec::new());
                        }
                    }
                }
                Ok(SecretKeyRing(result))
            }
            _ => Ok(SecretKeyRing(HashMap::new())),
        }
    }
}

impl SecretKeyRing {
    #[allow(dead_code)] // Used by upcoming KeySelector (Task 2)
    pub fn enabled_keys(&self, provider_id: &str) -> Vec<&KeyEntry> {
        self.0
            .get(provider_id)
            .map(|keys| {
                keys.iter()
                    .filter(|k| k.enabled && !k.key.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum WhisperAcceleratorSetting {
    Auto,
    Cpu,
    Gpu,
}

impl Default for WhisperAcceleratorSetting {
    fn default() -> Self {
        WhisperAcceleratorSetting::Auto
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum OrtAcceleratorSetting {
    Auto,
    Cpu,
    Cuda,
    DirectMl,
    Rocm,
}

impl Default for OrtAcceleratorSetting {
    fn default() -> Self {
        OrtAcceleratorSetting::Auto
    }
}

fn default_whisper_gpu_device() -> i32 {
    -1
}

/* still handy for composing the initial JSON in the store ------------- */
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AppSettings {
    pub bindings: HashMap<String, ShortcutBinding>,
    #[serde(default = "default_app_language")]
    pub app_language: String,
    #[serde(default, alias = "push_to_talk")]
    pub activation_mode: ActivationMode,
    pub audio_feedback: bool,
    #[serde(default = "default_audio_feedback_volume")]
    pub audio_feedback_volume: f32,
    #[serde(default = "default_sound_theme")]
    pub sound_theme: SoundTheme,
    #[serde(default = "default_start_hidden")]
    pub start_hidden: bool,
    #[serde(default = "default_autostart_enabled")]
    pub autostart_enabled: bool,
    #[serde(default = "default_update_checks_enabled")]
    pub update_checks_enabled: bool,
    #[serde(default = "default_onboarding_completed")]
    pub onboarding_completed: bool,
    #[serde(default = "default_model")]
    pub selected_model: String,
    #[serde(default = "default_always_on_microphone")]
    pub always_on_microphone: bool,
    #[serde(default)]
    pub selected_microphone: Option<String>,
    #[serde(default)]
    pub clamshell_microphone: Option<String>,
    #[serde(default)]
    pub selected_output_device: Option<String>,
    #[serde(default = "default_translate_to_english")]
    pub translate_to_english: bool,
    #[serde(default = "default_selected_language")]
    pub selected_language: String,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: OverlayPosition,
    #[serde(default = "default_debug_mode")]
    pub debug_mode: bool,
    #[serde(default)]
    pub debug_log_post_process: bool,
    #[serde(default)]
    pub debug_log_skill_routing: bool,
    #[serde(default)]
    pub debug_log_routing: bool,
    #[serde(default)]
    pub debug_log_transcription: bool,
    #[serde(default = "default_log_level")]
    #[specta(type = String)]
    pub log_level: LogLevel,
    #[serde(default)]
    pub model_unload_timeout: ModelUnloadTimeout,
    #[serde(default = "default_word_correction_threshold")]
    pub word_correction_threshold: f64,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default = "default_recording_retention_period")]
    pub recording_retention_period: RecordingRetentionPeriod,
    #[serde(default)]
    pub paste_method: PasteMethod,
    #[serde(default)]
    pub clipboard_handling: ClipboardHandling,
    #[serde(default = "default_auto_submit")]
    pub auto_submit: bool,
    #[serde(default)]
    pub auto_submit_key: AutoSubmitKey,
    #[serde(default = "default_post_process_enabled")]
    pub post_process_enabled: bool,
    #[serde(default = "default_post_process_use_secondary_output")]
    pub post_process_use_secondary_output: bool,
    #[serde(default = "default_post_process_use_local_candidate_when_online_asr")]
    pub post_process_use_local_candidate_when_online_asr: bool,
    #[serde(default)]
    pub post_process_secondary_model_id: Option<String>,
    #[serde(default = "default_post_process_provider_id")]
    pub post_process_provider_id: String,
    /// Global proxy URL (e.g. "http://127.0.0.1:7890", "socks5://...")
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// Whether proxy is enabled globally by default
    #[serde(default)]
    pub proxy_global_enabled: bool,
    #[serde(default = "default_post_process_providers")]
    pub post_process_providers: Vec<PostProcessProvider>,
    #[serde(default = "default_post_process_api_keys")]
    pub post_process_api_keys: SecretKeyRing,
    #[serde(default = "default_post_process_models")]
    pub post_process_models: HashMap<String, String>,
    #[serde(default = "default_post_process_provider_avatar_overrides")]
    pub post_process_provider_avatar_overrides: HashMap<String, String>,
    #[serde(default = "default_post_process_prompts")]
    pub post_process_prompts: Vec<LLMPrompt>,
    #[serde(default)]
    pub builtin_prompt_resource_hashes: HashMap<String, String>,
    #[serde(default)]
    pub custom_words: Vec<String>,
    #[serde(default)]
    pub custom_filler_words: Option<Vec<String>>,
    #[serde(default)]
    pub post_process_selected_prompt_id: Option<String>,
    #[serde(default, alias = "post_process_intent_model_id")]
    pub post_process_intent_model: Option<ModelChain>,
    #[serde(default)]
    pub post_process_translation_model: Option<ModelChain>,
    /// Enable multi-model parallel post-processing
    #[serde(default)]
    pub multi_model_post_process_enabled: bool,
    /// List of models to use for multi-model post-processing
    #[serde(default)]
    pub multi_model_post_process_items: Vec<MultiModelPostProcessItem>,
    /// Selected cached_model IDs for multi-model parallel post-processing (checkbox-based)
    #[serde(default)]
    pub multi_model_selected_ids: Vec<String>,
    /// Multi-model strategy: manual | race | lazy
    #[serde(default = "default_multi_model_strategy")]
    pub multi_model_strategy: String,
    /// Preferred model id for multi-model mode (used by lazy strategy)
    #[serde(default)]
    pub multi_model_preferred_id: Option<String>,
    /// Counts of manual candidate picks by cached_model_id
    #[serde(default)]
    pub multi_model_manual_pick_counts: HashMap<String, u32>,
    #[serde(default)]
    pub cached_models: Vec<CachedModel>,
    #[serde(default)]
    pub online_asr_enabled: bool,
    #[serde(default, alias = "selected_asr_model_id")]
    pub selected_asr_model: Option<ModelChain>,
    #[serde(default, alias = "selected_prompt_model_id")]
    pub selected_prompt_model: Option<ModelChain>,
    #[serde(default)]
    pub selected_clustering_model: Option<ModelChain>,
    #[serde(default)]
    pub mute_while_recording: bool,
    #[serde(default = "default_audio_input_auto_enhance")]
    pub audio_input_auto_enhance: bool,
    /// Per-microphone enhance preference.  Key is the device name (or
    /// "default" for the system default device).
    #[serde(default)]
    pub mic_enhance_preferences: HashMap<String, bool>,
    #[serde(default)]
    pub append_trailing_space: bool,
    #[serde(default = "default_punctuation_enabled")]
    pub punctuation_enabled: bool,
    #[serde(default = "default_punctuation_model")]
    pub punctuation_model: String,
    #[serde(default = "default_favorite_transcription_models")]
    pub favorite_transcription_models: Vec<String>,
    #[serde(default)]
    pub realtime_transcription_enabled: bool,
    #[serde(default = "default_offline_vad_force_interval_ms")]
    pub offline_vad_force_interval_ms: u64,
    #[serde(default = "default_offline_vad_force_window_seconds")]
    pub offline_vad_force_window_seconds: u64,
    /// Application-specific review policies (App Bundle ID or Process Name -> Policy)
    #[serde(default)]
    pub app_review_policies: HashMap<String, AppReviewPolicy>,
    #[serde(default)]
    pub app_profiles: Vec<AppProfile>,
    #[serde(default)]
    pub app_to_profile: HashMap<String, String>,
    #[serde(default = "default_post_process_context_enabled")]
    pub post_process_context_enabled: bool,
    #[serde(default = "default_post_process_context_limit")]
    pub post_process_context_limit: u8,
    #[serde(default = "default_post_process_streaming_output_enabled")]
    pub post_process_streaming_output_enabled: bool,
    #[serde(default = "default_post_process_hotword_injection_enabled")]
    pub post_process_hotword_injection_enabled: bool,
    /// Expert mode enables advanced settings visibility
    #[serde(default)]
    pub expert_mode: bool,
    #[serde(default)]
    pub experimental_enabled: bool,
    #[serde(default)]
    pub keyboard_implementation: KeyboardImplementation,
    #[serde(default = "default_show_tray_icon")]
    pub show_tray_icon: bool,
    #[serde(default = "default_paste_delay_ms")]
    pub paste_delay_ms: u64,
    #[serde(default)]
    pub extra_recording_buffer_ms: u64,
    #[serde(default)]
    pub typing_tool: TypingTool,
    #[serde(default)]
    pub external_script_path: Option<String>,
    /// Length routing: auto-select model based on text length
    #[serde(default)]
    pub length_routing_enabled: bool,
    #[serde(default = "default_length_routing_threshold")]
    pub length_routing_threshold: u32,
    #[serde(default, alias = "length_routing_short_model_id")]
    pub length_routing_short_model: Option<ModelChain>,
    #[serde(default, alias = "length_routing_long_model_id")]
    pub length_routing_long_model: Option<ModelChain>,
    /// Smart routing: pre-process layer that handles history reuse and action routing.
    /// Activated when length_routing_enabled (smart model mode) is on.
    /// This field is kept for potential future independent control but currently
    /// follows length_routing_enabled.
    #[serde(default = "default_true", alias = "smart_routing_history_reuse")]
    pub smart_routing_enabled: bool,
    /// Keep microphone stream open for 30s after recording stops (reduces Bluetooth reconnect latency)
    #[serde(default)]
    pub lazy_stream_close: bool,
    /// Whisper accelerator preference (auto / cpu / gpu)
    #[serde(default)]
    pub whisper_accelerator: WhisperAcceleratorSetting,
    /// ORT accelerator preference (auto / cpu / cuda / directml / rocm)
    #[serde(default)]
    pub ort_accelerator: OrtAcceleratorSetting,
    /// Whisper GPU device index (-1 = auto)
    #[serde(default = "default_whisper_gpu_device")]
    pub whisper_gpu_device: i32,
    #[serde(default = "default_openai_compatible_api_enabled")]
    pub openai_compatible_api_enabled: bool,
    #[serde(default = "default_openai_compatible_api_host")]
    pub openai_compatible_api_host: String,
    #[serde(default = "default_openai_compatible_api_port")]
    pub openai_compatible_api_port: u16,
    #[serde(default)]
    pub openai_compatible_api_allow_lan: bool,
    #[serde(default = "default_openai_compatible_api_base_path")]
    pub openai_compatible_api_base_path: String,
    #[serde(default)]
    pub openai_compatible_api_access_key: String,
}

fn default_model() -> String {
    "".to_string()
}

fn default_app_language() -> String {
    "en".to_string()
}

fn default_always_on_microphone() -> bool {
    false
}

fn default_translate_to_english() -> bool {
    false
}

fn default_openai_compatible_api_enabled() -> bool {
    true
}

fn default_openai_compatible_api_host() -> String {
    "127.0.0.1".to_string()
}

fn default_openai_compatible_api_port() -> u16 {
    33178
}

fn default_openai_compatible_api_base_path() -> String {
    "/v1".to_string()
}

fn default_start_hidden() -> bool {
    false
}

fn default_autostart_enabled() -> bool {
    false
}

fn default_update_checks_enabled() -> bool {
    true
}

fn default_onboarding_completed() -> bool {
    false
}

fn default_selected_language() -> String {
    "auto".to_string()
}

fn default_overlay_position() -> OverlayPosition {
    #[cfg(target_os = "linux")]
    return OverlayPosition::None;
    #[cfg(not(target_os = "linux"))]
    return OverlayPosition::FollowCursor;
}

fn default_debug_mode() -> bool {
    false
}

fn default_log_level() -> LogLevel {
    LogLevel::Debug
}

fn default_word_correction_threshold() -> f64 {
    0.18
}

fn default_paste_delay_ms() -> u64 {
    60
}

fn default_auto_submit() -> bool {
    false
}

fn default_history_limit() -> usize {
    5
}

fn default_recording_retention_period() -> RecordingRetentionPeriod {
    RecordingRetentionPeriod::PreserveLimit
}

fn default_audio_feedback_volume() -> f32 {
    1.0
}

fn default_audio_input_auto_enhance() -> bool {
    true
}

fn default_sound_theme() -> SoundTheme {
    SoundTheme::Marimba
}

fn default_post_process_enabled() -> bool {
    false
}

fn default_post_process_use_secondary_output() -> bool {
    false
}

fn default_post_process_use_local_candidate_when_online_asr() -> bool {
    false
}

fn default_post_process_provider_id() -> String {
    "openai".to_string()
}

fn builtin_post_process_provider(
    id: &str,
    label: &str,
    base_url: &str,
    models_endpoint: Option<&str>,
    supports_structured_output: bool,
    deletable: bool,
) -> PostProcessProvider {
    PostProcessProvider {
        id: id.to_string(),
        label: label.to_string(),
        base_url: base_url.to_string(),
        builtin: true,
        deletable,
        allow_base_url_edit: false,
        models_endpoint: models_endpoint.map(str::to_string),
        supports_structured_output,
        custom_headers: None,
        use_proxy: true,
    }
}

fn default_post_process_providers() -> Vec<PostProcessProvider> {
    let mut providers = vec![
        builtin_post_process_provider(
            "openai",
            "OpenAI",
            "https://api.openai.com/v1",
            Some("/models"),
            true,
            false,
        ),
        builtin_post_process_provider(
            "openrouter",
            "OpenRouter",
            "https://openrouter.ai/api/v1",
            Some("/models"),
            true,
            true,
        ),
        builtin_post_process_provider(
            "anthropic",
            "Anthropic",
            "https://api.anthropic.com/v1",
            Some("/models"),
            false,
            false,
        ),
        PostProcessProvider {
            id: "custom".to_string(),
            label: "Custom".to_string(),
            base_url: "http://localhost:11434/v1".to_string(),
            builtin: true,
            deletable: false,
            allow_base_url_edit: true,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
            custom_headers: None,
            use_proxy: true,
        },
        builtin_post_process_provider(
            "iflow",
            "iflow",
            "https://apis.iflow.cn/v1",
            Some("/models"),
            false,
            false,
        ),
        builtin_post_process_provider(
            "gitee",
            "Gitee",
            "https://ai.gitee.com/v1",
            Some("/models"),
            false,
            false,
        ),
        builtin_post_process_provider(
            "zai",
            "Z.AI",
            "https://api.z.ai/api/paas/v4",
            Some("/models"),
            true,
            true,
        ),
    ];

    // On macOS ARM64, always include Apple Intelligence provider.
    // Availability is checked at runtime in core.rs when actually invoking it.
    // Calling check_apple_intelligence_availability() at startup can crash on macOS 26.x.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        providers.push(PostProcessProvider {
            id: APPLE_INTELLIGENCE_PROVIDER_ID.to_string(),
            label: "Apple Intelligence".to_string(),
            base_url: "apple-intelligence://local".to_string(),
            builtin: true,
            deletable: false,
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: false,
            custom_headers: None,
            use_proxy: true,
        });
    }

    providers
}

fn default_post_process_api_keys() -> SecretKeyRing {
    let mut map = HashMap::new();
    for provider in default_post_process_providers() {
        map.insert(provider.id, Vec::new());
    }
    SecretKeyRing(map)
}

fn default_model_for_provider(provider_id: &str) -> String {
    if provider_id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return APPLE_INTELLIGENCE_DEFAULT_MODEL_ID.to_string();
    }
    String::new()
}

fn default_post_process_models() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for provider in default_post_process_providers() {
        map.insert(
            provider.id.clone(),
            default_model_for_provider(&provider.id),
        );
    }
    map
}

fn default_post_process_provider_avatar_overrides() -> HashMap<String, String> {
    HashMap::new()
}

fn default_punctuation_enabled() -> bool {
    false
}

fn default_multi_model_strategy() -> String {
    "manual".to_string()
}

fn default_punctuation_model() -> String {
    "punct-zh-en-ct-transformer-2024-04-12-int8".to_string()
}

fn default_favorite_transcription_models() -> Vec<String> {
    Vec::new()
}

fn default_offline_vad_force_interval_ms() -> u64 {
    1000
}

fn default_offline_vad_force_window_seconds() -> u64 {
    30
}

fn default_post_process_context_enabled() -> bool {
    false
}

fn default_post_process_context_limit() -> u8 {
    3
}

fn default_post_process_streaming_output_enabled() -> bool {
    true
}

fn default_post_process_hotword_injection_enabled() -> bool {
    true
}

fn default_length_routing_threshold() -> u32 {
    100
}

fn default_post_process_prompts() -> Vec<LLMPrompt> {
    Vec::new()
}

fn ensure_post_process_defaults(settings: &mut AppSettings) -> bool {
    let mut changed = false;
    for provider in default_post_process_providers() {
        // Sync metadata for existing providers from templates.
        if let Some(existing) = settings
            .post_process_providers
            .iter_mut()
            .find(|p| p.id == provider.id)
        {
            if existing.supports_structured_output != provider.supports_structured_output {
                existing.supports_structured_output = provider.supports_structured_output;
                changed = true;
            }
        }

        let mut provider_exists = settings
            .post_process_providers
            .iter()
            .any(|existing| existing.id == provider.id);

        // Required (non-deletable) builtin providers must always be present;
        // deletable builtins stay removed if the user deleted them.
        if !provider_exists && !provider.deletable {
            settings.post_process_providers.push(provider.clone());
            provider_exists = true;
            changed = true;
        }

        if provider_exists && !settings.post_process_api_keys.contains_key(&provider.id) {
            settings
                .post_process_api_keys
                .insert(provider.id.clone(), Vec::new());
            changed = true;
        }

        let default_model = default_model_for_provider(&provider.id);
        if provider_exists {
            match settings.post_process_models.get_mut(&provider.id) {
                Some(existing) => {
                    if existing.is_empty() && !default_model.is_empty() {
                        *existing = default_model.clone();
                        changed = true;
                    }
                }
                None => {
                    settings
                        .post_process_models
                        .insert(provider.id.clone(), default_model);
                    changed = true;
                }
            }
        }
    }

    // Sync builtin skills: only insert missing ones or update changed ones.
    // Skip user-customized builtin skills entirely.
    let builtin_contents: &[&str] = &[
        include_str!("../resources/skills/builtin/default_correction.skill.md"),
        include_str!("../resources/skills/builtin/ai_chat.skill.md"),
        include_str!("../resources/skills/builtin/translation.skill.md"),
        include_str!("../resources/skills/builtin/summarize.skill.md"),
        include_str!("../resources/skills/builtin/memo.skill.md"),
        include_str!("../resources/skills/builtin/code_generate.skill.md"),
        include_str!("../resources/skills/builtin/code_explain.skill.md"),
        include_str!("../resources/skills/builtin/style_reply.skill.md"),
        include_str!("../resources/skills/builtin/reply_suggestion.skill.md"),
        include_str!("../resources/skills/builtin/votype_command.skill.md"),
        include_str!("../resources/skills/builtin/grammar_fix.skill.md"),
        include_str!("../resources/skills/builtin/smart_compose.skill.md"),
    ];
    let mut inserted_count = 0;
    let mut updated_count = 0;
    for content in builtin_contents {
        if let Some(skill) = crate::managers::skill::parse_builtin_skill_content(content) {
            if let Some(existing) = settings
                .post_process_prompts
                .iter()
                .find(|p| p.id == skill.id && p.source == SkillSource::Builtin)
            {
                if existing.customized {
                    log::debug!(
                        "[BuiltinSync] Skipping customized builtin skill: {} ({})",
                        skill.name,
                        skill.id
                    );
                    continue;
                }
                // Check if content has changed
                if existing.name == skill.name
                    && existing.instructions == skill.instructions
                    && existing.prompt == skill.prompt
                    && existing.description == skill.description
                    && existing.skill_type == skill.skill_type
                    && existing.output_mode == skill.output_mode
                    && existing.confidence_check_enabled == skill.confidence_check_enabled
                    && existing.confidence_threshold == skill.confidence_threshold
                {
                    continue; // No changes needed
                }
                // Update in place
                let pos = settings
                    .post_process_prompts
                    .iter()
                    .position(|p| p.id == skill.id && p.source == SkillSource::Builtin)
                    .unwrap();
                log::info!(
                    "[BuiltinSync] Updating builtin skill: {} ({})",
                    skill.name,
                    skill.id
                );
                settings.post_process_prompts[pos] = skill;
                updated_count += 1;
                changed = true;
            } else {
                // Not present — insert at front
                log::info!(
                    "[BuiltinSync] Inserting builtin skill: {} ({})",
                    skill.name,
                    skill.id
                );
                settings.post_process_prompts.insert(0, skill);
                inserted_count += 1;
                changed = true;
            }
        } else {
            log::warn!("[BuiltinSync] Failed to parse a builtin skill content");
        }
    }

    // Remove stale builtin skills that no longer exist in builtin_contents
    let valid_builtin_ids: std::collections::HashSet<String> = builtin_contents
        .iter()
        .filter_map(|content| {
            crate::managers::skill::parse_builtin_skill_content(content).map(|s| s.id)
        })
        .collect();
    let before_len = settings.post_process_prompts.len();
    settings
        .post_process_prompts
        .retain(|p| p.source != SkillSource::Builtin || valid_builtin_ids.contains(&p.id));
    if settings.post_process_prompts.len() != before_len {
        changed = true;
    }

    if inserted_count > 0 || updated_count > 0 {
        log::info!(
            "[BuiltinSync] Inserted {}, updated {} builtin skills, total prompts: {}",
            inserted_count,
            updated_count,
            settings.post_process_prompts.len()
        );
    }

    if !settings.builtin_prompt_resource_hashes.is_empty() {
        settings.builtin_prompt_resource_hashes.clear();
        changed = true;
    }

    if settings
        .post_process_selected_prompt_id
        .as_ref()
        .is_some_and(|selected_id| {
            !settings
                .post_process_prompts
                .iter()
                .any(|prompt| &prompt.id == selected_id)
        })
    {
        settings.post_process_selected_prompt_id = None;
        changed = true;
    }

    changed
}

fn materialize_stored_user_prompts(app: &AppHandle, settings: &mut AppSettings) -> bool {
    let skill_manager = crate::managers::skill::SkillManager::new(app);
    let mut changed = false;

    for prompt in settings
        .post_process_prompts
        .iter()
        .filter(|prompt| prompt.source != SkillSource::Builtin)
        .cloned()
    {
        if skill_manager.find_skill_file_path(&prompt.id).is_some() {
            continue;
        }

        match skill_manager.create_skill_file(&prompt) {
            Ok(created) => {
                log::info!(
                    "Materialized stored prompt '{}' ({}) to file-backed user skill at {:?}",
                    created.name,
                    created.id,
                    created.file_path
                );
                changed = true;
            }
            Err(err) => {
                log::error!(
                    "Failed to materialize stored prompt '{}' ({}): {}",
                    prompt.name,
                    prompt.id,
                    err
                );
            }
        }
    }

    changed
}

fn normalize_app_review_policies(settings: &mut AppSettings) -> bool {
    let mut changed = false;

    for profile in &mut settings.app_profiles {
        if matches!(profile.policy, AppReviewPolicy::Auto) {
            profile.policy = AppReviewPolicy::Always;
            changed = true;
        }

        for rule in &mut profile.rules {
            if matches!(rule.policy, AppReviewPolicy::Auto) {
                rule.policy = AppReviewPolicy::Always;
                changed = true;
            }
        }
    }

    for policy in settings.app_review_policies.values_mut() {
        if matches!(*policy, AppReviewPolicy::Auto) {
            *policy = AppReviewPolicy::Always;
            changed = true;
        }
    }

    changed
}

fn normalize_offline_vad_force_interval(settings: &mut AppSettings) -> bool {
    if settings.offline_vad_force_interval_ms == 2000 {
        settings.offline_vad_force_interval_ms = default_offline_vad_force_interval_ms();
        return true;
    }

    false
}

pub const SETTINGS_STORE_PATH: &str = "settings_store.json";

pub fn get_default_settings() -> AppSettings {
    #[cfg(target_os = "windows")]
    let default_shortcut = "ctrl+space";
    #[cfg(target_os = "macos")]
    let default_shortcut = "option+space";
    #[cfg(target_os = "linux")]
    let default_shortcut = "ctrl+space";
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let default_shortcut = "alt+space";

    let mut bindings = HashMap::new();
    bindings.insert(
        "transcribe".to_string(),
        ShortcutBinding {
            id: "transcribe".to_string(),
            name: "Transcribe".to_string(),
            description: "Converts your speech into text.".to_string(),
            default_binding: default_shortcut.to_string(),
            current_binding: default_shortcut.to_string(),
        },
    );
    bindings.insert(
        "cancel".to_string(),
        ShortcutBinding {
            id: "cancel".to_string(),
            name: "Cancel".to_string(),
            description: "Cancels the current recording.".to_string(),
            default_binding: "escape".to_string(),
            current_binding: "escape".to_string(),
        },
    );
    bindings.insert(
        "open_settings".to_string(),
        ShortcutBinding {
            id: "open_settings".to_string(),
            name: "Open Settings".to_string(),
            description: "Opens the settings window.".to_string(),
            default_binding: if cfg!(target_os = "macos") {
                "command+option+s".to_string()
            } else {
                "ctrl+alt+s".to_string()
            },
            current_binding: if cfg!(target_os = "macos") {
                "command+option+s".to_string()
            } else {
                "ctrl+alt+s".to_string()
            },
        },
    );
    bindings.insert(
        "paste_first_entry".to_string(),
        ShortcutBinding {
            id: "paste_first_entry".to_string(),
            name: "Paste First Entry".to_string(),
            description: "Pastes the first history entry to the active window.".to_string(),
            default_binding: "ctrl+shift+v".to_string(),
            current_binding: "ctrl+shift+v".to_string(),
        },
    );
    bindings.insert(
        "invoke_skill".to_string(),
        ShortcutBinding {
            id: "invoke_skill".to_string(),
            name: "Invoke Skill".to_string(),
            description: "Starts recording in Skill mode with intelligent intent routing."
                .to_string(),
            default_binding: if cfg!(target_os = "macos") {
                "command+shift+space".to_string()
            } else {
                "ctrl+shift+space".to_string()
            },
            current_binding: if cfg!(target_os = "macos") {
                "command+shift+space".to_string()
            } else {
                "ctrl+shift+space".to_string()
            },
        },
    );

    AppSettings {
        bindings,
        app_language: default_app_language(),
        activation_mode: ActivationMode::default(),
        audio_feedback: true,
        audio_feedback_volume: default_audio_feedback_volume(),
        sound_theme: default_sound_theme(),
        start_hidden: default_start_hidden(),
        autostart_enabled: default_autostart_enabled(),
        update_checks_enabled: default_update_checks_enabled(),
        onboarding_completed: default_onboarding_completed(),
        selected_model: "".to_string(),
        always_on_microphone: false,
        selected_microphone: None,
        clamshell_microphone: None,
        selected_output_device: None,
        translate_to_english: false,
        selected_language: "auto".to_string(),
        overlay_position: default_overlay_position(),
        debug_mode: false,
        debug_log_post_process: false,
        debug_log_skill_routing: false,
        debug_log_routing: false,
        debug_log_transcription: false,
        log_level: default_log_level(),
        model_unload_timeout: ModelUnloadTimeout::Never,
        word_correction_threshold: default_word_correction_threshold(),
        history_limit: default_history_limit(),
        recording_retention_period: default_recording_retention_period(),
        paste_method: PasteMethod::default(),
        clipboard_handling: ClipboardHandling::default(),
        auto_submit: default_auto_submit(),
        auto_submit_key: AutoSubmitKey::default(),
        post_process_enabled: default_post_process_enabled(),
        post_process_use_secondary_output: default_post_process_use_secondary_output(),
        post_process_use_local_candidate_when_online_asr:
            default_post_process_use_local_candidate_when_online_asr(),
        post_process_secondary_model_id: None,
        post_process_provider_id: default_post_process_provider_id(),
        proxy_url: None,
        proxy_global_enabled: false,
        post_process_providers: default_post_process_providers(),
        post_process_api_keys: default_post_process_api_keys(),
        post_process_models: default_post_process_models(),
        post_process_provider_avatar_overrides: default_post_process_provider_avatar_overrides(),
        post_process_prompts: default_post_process_prompts(),
        builtin_prompt_resource_hashes: HashMap::new(),
        post_process_selected_prompt_id: None,
        post_process_intent_model: None,
        post_process_translation_model: None,
        multi_model_post_process_enabled: false,
        multi_model_post_process_items: Vec::new(),
        multi_model_selected_ids: Vec::new(),
        multi_model_strategy: default_multi_model_strategy(),
        multi_model_preferred_id: None,
        multi_model_manual_pick_counts: HashMap::new(),
        cached_models: Vec::new(),
        online_asr_enabled: false,
        selected_asr_model: None,
        selected_prompt_model: None,
        selected_clustering_model: None,
        mute_while_recording: false,
        audio_input_auto_enhance: default_audio_input_auto_enhance(),
        mic_enhance_preferences: HashMap::new(),
        append_trailing_space: false,
        punctuation_enabled: default_punctuation_enabled(),
        punctuation_model: default_punctuation_model(),
        favorite_transcription_models: default_favorite_transcription_models(),
        realtime_transcription_enabled: false,
        offline_vad_force_interval_ms: default_offline_vad_force_interval_ms(),
        offline_vad_force_window_seconds: default_offline_vad_force_window_seconds(),
        app_review_policies: HashMap::new(),
        app_profiles: Vec::new(),
        app_to_profile: HashMap::new(),
        post_process_context_enabled: default_post_process_context_enabled(),
        post_process_context_limit: default_post_process_context_limit(),
        post_process_streaming_output_enabled: default_post_process_streaming_output_enabled(),
        post_process_hotword_injection_enabled: default_post_process_hotword_injection_enabled(),
        custom_words: Vec::new(),
        custom_filler_words: None,
        expert_mode: false,
        experimental_enabled: false,
        keyboard_implementation: KeyboardImplementation::default(),
        show_tray_icon: default_show_tray_icon(),
        paste_delay_ms: default_paste_delay_ms(),
        extra_recording_buffer_ms: 0,
        typing_tool: TypingTool::default(),
        external_script_path: None,
        length_routing_enabled: false,
        length_routing_threshold: default_length_routing_threshold(),
        length_routing_short_model: None,
        length_routing_long_model: None,
        smart_routing_enabled: true,
        lazy_stream_close: false,
        whisper_accelerator: WhisperAcceleratorSetting::default(),
        ort_accelerator: OrtAcceleratorSetting::default(),
        whisper_gpu_device: default_whisper_gpu_device(),
        openai_compatible_api_enabled: default_openai_compatible_api_enabled(),
        openai_compatible_api_host: default_openai_compatible_api_host(),
        openai_compatible_api_port: default_openai_compatible_api_port(),
        openai_compatible_api_allow_lan: false,
        openai_compatible_api_base_path: default_openai_compatible_api_base_path(),
        openai_compatible_api_access_key: String::new(),
    }
}

impl AppSettings {
    pub fn active_post_process_provider(&self) -> Option<&PostProcessProvider> {
        // 1. Exact match on persisted provider_id
        if let Some(p) = self
            .post_process_providers
            .iter()
            .find(|provider| provider.id == self.post_process_provider_id)
        {
            return Some(p);
        }

        // 2. Derive from selected_prompt_model → cached_model → provider
        if let Some(chain) = &self.selected_prompt_model {
            if let Some(cached) = self.cached_models.iter().find(|m| m.id == chain.primary_id) {
                if let Some(p) = self.post_process_provider(&cached.provider_id) {
                    return Some(p);
                }
            }
        }

        // 3. Last resort: first available provider
        self.post_process_providers.first()
    }

    pub fn post_process_provider(&self, provider_id: &str) -> Option<&PostProcessProvider> {
        self.post_process_providers
            .iter()
            .find(|provider| provider.id == provider_id)
    }

    pub fn post_process_provider_mut(
        &mut self,
        provider_id: &str,
    ) -> Option<&mut PostProcessProvider> {
        self.post_process_providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
    }

    #[allow(dead_code)]
    /// Get enabled multi-model post-process items
    pub fn enabled_multi_model_items(&self) -> Vec<&MultiModelPostProcessItem> {
        self.multi_model_post_process_items
            .iter()
            .filter(|item| item.enabled)
            .collect()
    }

    #[allow(dead_code)]
    /// Get cached model by ID
    pub fn get_cached_model(&self, model_id: &str) -> Option<&CachedModel> {
        self.cached_models.iter().find(|m| m.id == model_id)
    }

    /// Resolve the effective model ID string for a given provider.
    ///
    /// Resolution order:
    /// 1. `selected_prompt_model` → look up in `cached_models` (must match provider)
    /// 2. `post_process_models` legacy map
    ///
    /// Returns `None` if no non-empty model can be found.
    pub fn resolve_model_for_provider(&self, provider_id: &str) -> Option<String> {
        // Try selected_prompt_model via cached_models first
        if let Some(ref chain) = self.selected_prompt_model {
            let cm_id = &chain.primary_id;
            if let Some(cm) = self.cached_models.iter().find(|m| m.id == *cm_id) {
                if cm.provider_id == provider_id {
                    let model_id = cm.model_id.trim().to_string();
                    if !model_id.is_empty() {
                        return Some(model_id);
                    }
                }
            }
        }

        // Fallback to legacy post_process_models map
        self.post_process_models
            .get(provider_id)
            .filter(|m| !m.trim().is_empty())
            .cloned()
    }

    /// Build MultiModelPostProcessItem list from multi_model_selected_ids.
    /// Uses cached_model info + current selected prompt to dynamically construct items.
    pub fn build_multi_model_items_from_selection(&self) -> Vec<MultiModelPostProcessItem> {
        let prompt_id = self.post_process_selected_prompt_id.clone().or_else(|| {
            self.post_process_prompts
                .first()
                .map(|prompt| prompt.id.clone())
        });

        let Some(prompt_id) = prompt_id else {
            return Vec::new();
        };

        self.multi_model_selected_ids
            .iter()
            .filter_map(|id| {
                let cm = self.get_cached_model(id)?;
                if cm.model_type != ModelType::Text {
                    return None;
                }
                Some(MultiModelPostProcessItem {
                    id: cm.id.clone(),
                    provider_id: cm.provider_id.clone(),
                    model_id: cm.model_id.clone(),
                    prompt_id: prompt_id.clone(),
                    custom_label: cm.custom_label.clone(),
                    enabled: true,
                })
            })
            .collect()
    }

    #[allow(dead_code)]
    /// Get prompt/skill by ID
    pub fn get_prompt(&self, prompt_id: &str) -> Option<&LLMPrompt> {
        self.post_process_prompts.iter().find(|p| p.id == prompt_id)
    }
}

fn store_set_settings(store: &tauri_plugin_store::Store<tauri::Wry>, settings: &AppSettings) {
    if let Ok(val) = serde_json::to_value(settings) {
        store.set("settings", val);
    } else {
        log::error!("Failed to serialize settings to JSON");
    }
}

pub fn load_or_create_app_settings(app: &AppHandle) -> AppSettings {
    // Seed default directory-based skills (e.g. smart_polish with references).
    // Run before store init so seeded skills are visible in the first merge.
    {
        let skill_manager = crate::managers::skill::SkillManager::new(app);
        skill_manager.seed_default_skills();
    }

    // Initialize store
    let store = app
        .store(SETTINGS_STORE_PATH)
        .expect("Failed to initialize store");

    let mut settings = if let Some(settings_value) = store.get("settings") {
        // Parse the entire settings object
        match serde_json::from_value::<AppSettings>(settings_value.clone()) {
            Ok(mut settings) => {
                debug!("Found existing settings: {:?}", settings);
                let default_settings = get_default_settings();
                let mut updated = false;

                // Merge default bindings into existing settings
                for (key, value) in default_settings.bindings {
                    if !settings.bindings.contains_key(&key) {
                        debug!("Adding missing binding: {}", key);
                        settings.bindings.entry(key).or_insert(value);
                        updated = true;
                    }
                }

                if updated {
                    debug!("Settings updated with new bindings");
                    store_set_settings(&store, &settings);
                }

                settings
            }
            Err(e) => {
                warn!("Failed to parse settings: {}", e);

                // Backup the original settings before overwriting
                if let Ok(backup_json) = serde_json::to_string_pretty(&settings_value) {
                    if let Ok(app_data_dir) = app.path().app_data_dir() {
                        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
                        let backup_path =
                            app_data_dir.join(format!("settings_backup_{}.json", timestamp));
                        if let Err(backup_err) = std::fs::write(&backup_path, &backup_json) {
                            warn!(
                                "Failed to backup settings to {:?}: {}",
                                backup_path, backup_err
                            );
                        } else {
                            warn!("Settings backed up to {:?} before reset", backup_path);
                        }
                    }
                }

                // Fall back to default settings if parsing fails
                let default_settings = get_default_settings();
                store_set_settings(&store, &default_settings);
                default_settings
            }
        }
    } else {
        let default_settings = get_default_settings();
        store_set_settings(&store, &default_settings);
        default_settings
    };

    if ensure_post_process_defaults(&mut settings) {
        store_set_settings(&store, &settings);
    }

    if materialize_stored_user_prompts(app, &mut settings) {
        merge_external_skills(app, &mut settings);
        store_set_settings(&store, &settings);
    }

    if normalize_app_review_policies(&mut settings) {
        store_set_settings(&store, &settings);
    }

    if normalize_offline_vad_force_interval(&mut settings) {
        store_set_settings(&store, &settings);
    }

    // Migration: Convert app_review_policies to app_profiles
    if !settings.app_review_policies.is_empty() {
        debug!("Migrating app_review_policies to app_profiles");
        for (app_id, policy) in settings.app_review_policies.clone() {
            if !settings.app_to_profile.contains_key(&app_id) {
                let profile_id = format!("profile_{}", app_id.replace(' ', "_").to_lowercase());
                if !settings.app_profiles.iter().any(|p| p.id == profile_id) {
                    settings.app_profiles.push(AppProfile {
                        id: profile_id.clone(),
                        name: app_id.clone(),
                        policy,
                        prompt_id: None,
                        icon: None,
                        translate_to_english_on_insert: false,
                        disable_selection_clipboard_fallback: false,
                        rules: Vec::new(),
                    });
                }
                settings.app_to_profile.insert(app_id.clone(), profile_id);
            }
        }
        settings.app_review_policies.clear();
        store_set_settings(&store, &settings);
        let _ = store.save();
    }

    settings
}

pub fn get_settings(app: &AppHandle) -> AppSettings {
    let current_version = SETTINGS_VERSION.load(Ordering::Acquire);

    // Check cache
    if let Ok(cache) = CACHED_SETTINGS.lock() {
        if let Some((cached_version, ref cached_settings)) = *cache {
            if cached_version == current_version {
                return cached_settings.clone();
            }
        }
    }

    let store = app
        .store(SETTINGS_STORE_PATH)
        .expect("Failed to initialize store");

    let mut settings = if let Some(settings_value) = store.get("settings") {
        serde_json::from_value::<AppSettings>(settings_value).unwrap_or_else(|_| {
            let default_settings = get_default_settings();
            store_set_settings(&store, &default_settings);
            default_settings
        })
    } else {
        let default_settings = get_default_settings();
        store_set_settings(&store, &default_settings);
        default_settings
    };

    if ensure_post_process_defaults(&mut settings) {
        store_set_settings(&store, &settings);
    }

    if materialize_stored_user_prompts(app, &mut settings) {
        merge_external_skills(app, &mut settings);
        store_set_settings(&store, &settings);
    }

    if normalize_app_review_policies(&mut settings) {
        store_set_settings(&store, &settings);
    }

    if normalize_offline_vad_force_interval(&mut settings) {
        store_set_settings(&store, &settings);
    }

    // Clean up orphaned cached models whose provider no longer exists
    {
        let valid_provider_ids: std::collections::HashSet<&str> = settings
            .post_process_providers
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        let before = settings.cached_models.len();
        settings
            .cached_models
            .retain(|m| valid_provider_ids.contains(m.provider_id.as_str()));
        if settings.cached_models.len() != before {
            store_set_settings(&store, &settings);
        }
    }

    // Clean up stale model references (historical data from previous deletions)
    if cleanup_stale_model_references(&mut settings) {
        store_set_settings(&store, &settings);
    }

    // Merge external skills from ~/.votype/skills/
    merge_external_skills(app, &mut settings);

    if settings.post_process_selected_prompt_id.is_none() {
        if let Some(first) = settings.post_process_prompts.first() {
            settings.post_process_selected_prompt_id = Some(first.id.clone());
            store_set_settings(&store, &settings);
        }
    }

    // Update cache
    if let Ok(mut cache) = CACHED_SETTINGS.lock() {
        *cache = Some((current_version, settings.clone()));
    }

    settings
}

/// Remove stale model references from all settings fields.
/// Returns `true` if any field was modified.
pub fn cleanup_stale_model_references(settings: &mut AppSettings) -> bool {
    let valid_ids: std::collections::HashSet<&str> = settings
        .cached_models
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    let mut changed = false;

    // Option<String> fields that reference a cached_model id
    macro_rules! clear_if_stale {
        ($field:expr) => {
            if $field
                .as_ref()
                .is_some_and(|id| !valid_ids.contains(id.as_str()))
            {
                *$field = None;
                changed = true;
            }
        };
    }

    // Option<ModelChain> fields — clear if primary is stale, strip fallback if stale
    macro_rules! clear_chain_if_stale {
        ($field:expr) => {
            if let Some(ref mut chain) = $field {
                if !valid_ids.contains(chain.primary_id.as_str()) {
                    $field = None;
                    changed = true;
                } else if chain
                    .fallback_id
                    .as_ref()
                    .is_some_and(|fid| !valid_ids.contains(fid.as_str()))
                {
                    chain.fallback_id = None;
                    changed = true;
                }
            }
        };
    }

    clear_chain_if_stale!(settings.selected_prompt_model);
    clear_chain_if_stale!(settings.selected_clustering_model);
    clear_chain_if_stale!(settings.selected_asr_model);
    clear_chain_if_stale!(settings.post_process_intent_model);
    clear_chain_if_stale!(settings.post_process_translation_model);
    // NOTE: post_process_secondary_model_id is a *local STT model id* (from the
    // transcribe-rs registry), not a cached_model UUID. Do NOT run
    // `clear_if_stale` against `valid_ids` here — it would always be "stale"
    // (since STT model ids never appear in `cached_models`) and get wiped on
    // every settings read, silently undoing the user's selection.
    clear_chain_if_stale!(settings.length_routing_short_model);
    clear_chain_if_stale!(settings.length_routing_long_model);
    clear_if_stale!(&mut settings.multi_model_preferred_id);

    // Vec<String> — multi_model_selected_ids
    let before = settings.multi_model_selected_ids.len();
    settings
        .multi_model_selected_ids
        .retain(|id| valid_ids.contains(id.as_str()));
    if settings.multi_model_selected_ids.len() != before {
        changed = true;
        // Auto-disable multi-model if fewer than 2 remain
        if settings.multi_model_selected_ids.len() < 2 {
            settings.multi_model_post_process_enabled = false;
        }
    }

    // HashMap<String, u32> — multi_model_manual_pick_counts
    let before = settings.multi_model_manual_pick_counts.len();
    settings
        .multi_model_manual_pick_counts
        .retain(|id, _| valid_ids.contains(id.as_str()));
    if settings.multi_model_manual_pick_counts.len() != before {
        changed = true;
    }

    // Vec<MultiModelPostProcessItem> — multi_model_post_process_items
    let before = settings.multi_model_post_process_items.len();
    settings
        .multi_model_post_process_items
        .retain(|item| valid_ids.contains(item.model_id.as_str()));
    if settings.multi_model_post_process_items.len() != before {
        changed = true;
    }

    // Per-skill model_id in post_process_prompts
    for prompt in &mut settings.post_process_prompts {
        if prompt
            .model_id
            .as_ref()
            .is_some_and(|id| !valid_ids.contains(id.as_str()))
        {
            prompt.model_id = None;
            changed = true;
        }
    }

    changed
}

fn merge_external_skills(app: &AppHandle, settings: &mut AppSettings) {
    let skill_manager = crate::managers::skill::SkillManager::new(app);
    let external_skills = skill_manager.load_all_external_skills();

    for skill in external_skills {
        // Check if already exists in the list
        if let Some(pos) = settings
            .post_process_prompts
            .iter()
            .position(|p| p.id == skill.id)
        {
            // If it's the SAME source (external), update it with newest content from disk.
            // If the user modified a built-in skill or saved an external one to JSON store,
            // we might want to decide which one wins.
            // For now, external file-based skills always override the memory/store version
            // if they have the same ID, to allow "live" editing of SKILL.md.
            settings.post_process_prompts[pos] = skill;
        } else {
            settings.post_process_prompts.push(skill);
        }
    }
}

pub fn write_settings(app: &AppHandle, settings: AppSettings) {
    let mut settings = settings;
    normalize_app_review_policies(&mut settings);
    normalize_offline_vad_force_interval(&mut settings);

    let store = app
        .store(SETTINGS_STORE_PATH)
        .expect("Failed to initialize store");

    store_set_settings(&store, &settings);
    if let Err(e) = store.save() {
        log::error!("Failed to save settings to disk: {}", e);
    }

    // Invalidate cache
    SETTINGS_VERSION.fetch_add(1, Ordering::Release);

    // Keep the overlay-enabled cache (read on the audio hot path) in sync with
    // any change to overlay_position — covers the dedicated setter, import,
    // and reset paths since they all funnel through write_settings.
    crate::overlay::refresh_overlay_enabled_cache(&settings);
}

pub fn get_bindings(app: &AppHandle) -> HashMap<String, ShortcutBinding> {
    let settings = get_settings(app);

    settings.bindings
}

pub fn get_stored_binding(app: &AppHandle, id: &str) -> Option<ShortcutBinding> {
    let bindings = get_bindings(app);
    bindings.get(id).cloned()
}

pub fn get_history_limit(app: &AppHandle) -> usize {
    let settings = get_settings(app);
    settings.history_limit
}

pub fn get_recording_retention_period(app: &AppHandle) -> RecordingRetentionPeriod {
    let settings = get_settings(app);
    settings.recording_retention_period
}

/// Resolve effective proxy URL for a provider based on global and per-provider settings.
/// Returns Some(url) only when: global proxy is enabled AND provider use_proxy is true AND url is set.
///
/// NOTE: Proxy feature is temporarily disabled — always returns None.
/// The original logic is preserved below for when the feature is re-enabled.
pub fn resolve_proxy(_settings: &AppSettings, _provider: &PostProcessProvider) -> Option<String> {
    // Proxy feature temporarily disabled
    None
    // Original logic:
    // if !settings.proxy_global_enabled || !provider.use_proxy {
    //     return None;
    // }
    // settings
    //     .proxy_url
    //     .as_deref()
    //     .filter(|u| !u.is_empty())
    //     .map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_disable_auto_submit() {
        let settings = get_default_settings();
        assert!(!settings.auto_submit);
        assert_eq!(settings.auto_submit_key, AutoSubmitKey::Enter);
    }

    #[test]
    fn ensure_post_process_defaults_restores_required_builtin_provider() {
        let mut settings = get_default_settings();
        settings
            .post_process_providers
            .retain(|provider| provider.id != "openai");

        let changed = ensure_post_process_defaults(&mut settings);

        assert!(changed);
        assert!(settings
            .post_process_providers
            .iter()
            .any(|provider| provider.id == "openai"));
    }

    #[test]
    fn ensure_post_process_defaults_does_not_restore_deletable_builtin_provider() {
        let mut settings = get_default_settings();
        settings
            .post_process_providers
            .retain(|provider| provider.id != "openrouter" && provider.id != "zai");
        settings.post_process_api_keys.remove("openrouter");
        settings.post_process_api_keys.remove("zai");
        settings.post_process_models.remove("openrouter");
        settings.post_process_models.remove("zai");

        ensure_post_process_defaults(&mut settings);

        assert!(settings
            .post_process_providers
            .iter()
            .all(|provider| provider.id != "openrouter" && provider.id != "zai"));
        assert!(!settings.post_process_api_keys.contains_key("openrouter"));
        assert!(!settings.post_process_api_keys.contains_key("zai"));
        assert!(!settings.post_process_models.contains_key("openrouter"));
        assert!(!settings.post_process_models.contains_key("zai"));
    }

    #[test]
    fn app_profile_clipboard_fallback_defaults_to_enabled() {
        let profile: AppProfile = serde_json::from_value(serde_json::json!({
            "id": "profile_test",
            "name": "Codex",
            "policy": "always",
            "prompt_id": null,
            "icon": null,
            "rules": []
        }))
        .expect("profile should deserialize");

        assert!(!profile.disable_selection_clipboard_fallback);
        assert!(!profile.translate_to_english_on_insert);
    }
}

#[cfg(test)]
mod title_match_type_tests {
    use super::TitleMatchType;
    use serde_json;

    #[test]
    fn roundtrip_text() {
        let v: TitleMatchType = serde_json::from_str("\"text\"").unwrap();
        assert_eq!(v, TitleMatchType::Text);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"text\"");
    }

    #[test]
    fn roundtrip_regex() {
        let v: TitleMatchType = serde_json::from_str("\"regex\"").unwrap();
        assert_eq!(v, TitleMatchType::Regex);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"regex\"");
    }

    #[test]
    fn roundtrip_exact_new_variant() {
        let v: TitleMatchType = serde_json::from_str("\"exact\"").unwrap();
        assert_eq!(v, TitleMatchType::Exact);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"exact\"");
    }
}
