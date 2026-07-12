import { z } from "zod";

export const ShortcutBindingSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string(),
  default_binding: z.string(),
  current_binding: z.string(),
});

export const ShortcutBindingsMapSchema = z.record(
  z.string(),
  ShortcutBindingSchema,
);

export const AudioDeviceSchema = z.object({
  index: z.string(),
  name: z.string(),
  is_default: z.boolean(),
});

export const OverlayPositionSchema = z.enum([
  "none",
  "top",
  "bottom",
  "follow",
]);
export type OverlayPosition = z.infer<typeof OverlayPositionSchema>;

export const ModelUnloadTimeoutSchema = z.enum([
  "never",
  "immediately",
  "min2",
  "min5",
  "min10",
  "min15",
  "hour1",
  "sec5",
]);
export type ModelUnloadTimeout = z.infer<typeof ModelUnloadTimeoutSchema>;

export const PasteMethodSchema = z.enum([
  "ctrl_v",
  "ctrl_shift_v",
  "direct",
  "shift_insert",
  "none",
]);
export type PasteMethod = z.infer<typeof PasteMethodSchema>;

export const ClipboardHandlingSchema = z.enum([
  "dont_modify",
  "copy_to_clipboard",
]);
export type ClipboardHandling = z.infer<typeof ClipboardHandlingSchema>;

export const AutoSubmitKeySchema = z.enum(["enter", "ctrl_enter", "cmd_enter"]);
export type AutoSubmitKey = z.infer<typeof AutoSubmitKeySchema>;

export const LogLevelSchema = z.number().int().min(1).max(5).default(2);
export type LogLevelValue = z.infer<typeof LogLevelSchema>;

export const RecordingRetentionPeriodSchema = z.enum([
  "never",
  "preserve_limit",
  "days3",
  "weeks2",
  "months3",
]);
export type RecordingRetentionPeriod = z.infer<
  typeof RecordingRetentionPeriodSchema
>;

export const AppReviewPolicySchema = z.enum(["auto", "always", "never"]);
export type AppReviewPolicy = z.infer<typeof AppReviewPolicySchema>;

export const TitleMatchTypeSchema = z.enum(["text", "regex", "exact"]);
export type TitleMatchType = z.infer<typeof TitleMatchTypeSchema>;

export const TitleRuleSchema = z.object({
  id: z.string(),
  pattern: z.string(),
  match_type: TitleMatchTypeSchema.default("text"),
  policy: AppReviewPolicySchema,
  prompt_id: z.string().nullable().optional(),
});
export type TitleRule = z.infer<typeof TitleRuleSchema>;

export const AppProfileSchema = z.object({
  id: z.string(),
  name: z.string(),
  policy: AppReviewPolicySchema,
  prompt_id: z.string().nullable().optional(),
  icon: z.string().nullable().optional(),
  translate_to_english_on_insert: z.boolean().default(false),
  disable_selection_clipboard_fallback: z.boolean().default(false),
  rules: z.array(TitleRuleSchema).default([]),
});
export type AppProfile = z.infer<typeof AppProfileSchema>;

export const PromptOutputModeSchema = z.enum(["polish", "chat", "silent"]);
export type PromptOutputMode = z.infer<typeof PromptOutputModeSchema>;

export const SkillTypeSchema = z.enum(["text", "action"]);
export type SkillType = z.infer<typeof SkillTypeSchema>;

export const SkillSourceSchema = z.union([
  z.enum(["builtin", "user", "imported"]),
  z.object({ external: z.object({ path: z.string() }) }),
]);
export type SkillSource = z.infer<typeof SkillSourceSchema>;

export const LLMPromptSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string().optional().default(""),
  instructions: z.string(),
  model_id: z.string().optional().nullable(),
  icon: z.string().optional().nullable(),
  skill_type: SkillTypeSchema.default("text"),
  source: SkillSourceSchema.default("builtin"),
  confidence_check_enabled: z.boolean().default(false),
  confidence_threshold: z.number().optional().default(70),
  output_mode: PromptOutputModeSchema.default("chat"),
  enabled: z.boolean().default(true),
  customized: z.boolean().optional().default(false),
  locked: z.boolean().default(false),
  // Keep legacy field for a bit of safety during transition if any code still uses it
  prompt: z.string().optional(),
});

export type LLMPrompt = z.infer<typeof LLMPromptSchema>;

export const ModelTypeSchema = z.enum(["text", "asr", "other"]);
export type ModelType = z.infer<typeof ModelTypeSchema>;

export const CachedModelSchema = z.object({
  id: z.string(),
  name: z.string(),
  model_type: ModelTypeSchema,
  provider_id: z.string(),
  model_id: z.string(),
  added_at: z.string(),
  custom_label: z.string().optional(),
  // Thinking Mode 标记
  is_thinking_model: z.boolean().optional().default(false),
  model_family: z.string().optional(),
  prompt_message_role: z
    .enum(["system", "developer"])
    .optional()
    .default("system"),
  // 额外的请求参数（JSON 格式，会合并到 LLM 请求体中）
  extra_params: z.record(z.string(), z.unknown()).optional(),
  // 额外的请求头（会合并到 HTTP 请求头中）
  extra_headers: z.record(z.string(), z.string()).optional(),
});

export type CachedModel = z.infer<typeof CachedModelSchema>;

export const MultiModelPostProcessItemSchema = z.object({
  id: z.string(),
  provider_id: z.string(),
  model_id: z.string(),
  prompt_id: z.string(),
  custom_label: z.string().optional(),
  enabled: z.boolean().default(true),
});

export type MultiModelPostProcessItem = z.infer<
  typeof MultiModelPostProcessItemSchema
>;

export const KeyEntrySchema = z.object({
  key: z.string(),
  enabled: z.boolean().default(true),
  label: z.string().nullable().optional(),
});

export type KeyEntry = z.infer<typeof KeyEntrySchema>;

export const PostProcessProviderSchema = z.object({
  id: z.string(),
  label: z.string(),
  base_url: z.string(),
  builtin: z.boolean().optional().default(false),
  deletable: z.boolean().optional().default(true),
  allow_base_url_edit: z.boolean().optional().default(false),
  models_endpoint: z.string().nullable().optional(),
  supports_structured_output: z.boolean().optional().default(false),
  kind: z
    .enum(["openai_compatible", "anthropic"])
    .optional()
    .default("openai_compatible"),
  use_proxy: z.boolean().optional().default(true),
});

export type PostProcessProvider = z.infer<typeof PostProcessProviderSchema>;

export const ModelChainStrategySchema = z.enum(["serial", "staggered", "race"]);
export type ModelChainStrategy = z.infer<typeof ModelChainStrategySchema>;

export const ModelChainSchema = z.object({
  primary_id: z.string(),
  fallback_id: z.string().nullable().optional().default(null),
  strategy: ModelChainStrategySchema.optional().default("serial"),
});
export type ModelChain = z.infer<typeof ModelChainSchema>;

export const SettingsSchema = z.object({
  bindings: ShortcutBindingsMapSchema,
  activation_mode: z
    .enum(["toggle", "hold", "hold_or_toggle"])
    .default("toggle"),
  audio_feedback: z.boolean(),
  audio_feedback_volume: z.number().optional().default(1.0),
  sound_theme: z
    .enum(["marimba", "pop", "custom"])
    .optional()
    .default("marimba"),
  start_hidden: z.boolean().optional().default(false),
  autostart_enabled: z.boolean().optional().default(false),
  update_checks_enabled: z.boolean().optional().default(true),
  onboarding_completed: z.boolean().optional().default(false),
  selected_model: z.string(),
  always_on_microphone: z.boolean(),
  selected_microphone: z.string().nullable().optional(),
  clamshell_microphone: z.string().nullable().optional(),
  selected_output_device: z.string().nullable().optional(),
  translate_to_english: z.boolean(),
  selected_language: z.string(),
  overlay_position: OverlayPositionSchema,
  debug_mode: z.boolean(),
  debug_log_post_process: z.boolean().optional().default(false),
  debug_log_skill_routing: z.boolean().optional().default(false),
  debug_log_routing: z.boolean().optional().default(false),
  debug_log_transcription: z.boolean().optional().default(false),
  log_level: LogLevelSchema.optional().default(2),
  custom_words: z.array(z.string()).optional().default([]),
  model_unload_timeout: ModelUnloadTimeoutSchema.optional().default("never"),
  word_correction_threshold: z.number().optional().default(0.18),
  history_limit: z.number().optional().default(5),
  recording_retention_period:
    RecordingRetentionPeriodSchema.optional().default("preserve_limit"),
  paste_method: PasteMethodSchema.optional().default("ctrl_v"),
  clipboard_handling: ClipboardHandlingSchema.optional().default("dont_modify"),
  auto_submit: z.boolean().optional().default(false),
  auto_submit_key: AutoSubmitKeySchema.optional().default("enter"),
  post_process_enabled: z.boolean().optional().default(false),
  post_process_use_secondary_output: z.boolean().optional().default(false),
  post_process_use_local_candidate_when_online_asr: z
    .boolean()
    .optional()
    .default(false),
  post_process_secondary_model_id: z
    .string()
    .nullable()
    .optional()
    .default(null),
  post_process_provider_id: z.string().optional().default("openai"),
  post_process_providers: z
    .array(PostProcessProviderSchema)
    .optional()
    .default([]),
  post_process_api_keys: z
    .record(z.array(KeyEntrySchema))
    .optional()
    .default({}),
  post_process_models: z.record(z.string()).optional().default({}),
  post_process_provider_avatar_overrides: z
    .record(z.string())
    .optional()
    .default({}),
  post_process_prompts: z.array(LLMPromptSchema).optional().default([]),
  post_process_selected_prompt_id: z.string().nullable().optional(),
  post_process_intent_model: ModelChainSchema.nullable()
    .optional()
    .default(null),
  post_process_translation_model: ModelChainSchema.nullable()
    .optional()
    .default(null),
  proxy_url: z.string().nullable().optional().default(null),
  proxy_global_enabled: z.boolean().optional().default(false),
  multi_model_post_process_enabled: z.boolean().optional().default(false),
  multi_model_post_process_items: z
    .array(MultiModelPostProcessItemSchema)
    .optional()
    .default([]),
  multi_model_selected_ids: z.array(z.string()).optional().default([]),
  multi_model_strategy: z
    .enum(["manual", "race", "lazy"])
    .optional()
    .default("manual"),
  multi_model_preferred_id: z.string().nullable().optional().default(null),
  multi_model_manual_pick_counts: z.record(z.number()).optional().default({}),
  cached_models: z.array(CachedModelSchema).optional().default([]),
  online_asr_enabled: z.boolean().optional().default(false),
  selected_asr_model: ModelChainSchema.nullable().optional().default(null),
  selected_prompt_model: ModelChainSchema.nullable().optional().default(null),
  selected_clustering_model: ModelChainSchema.nullable()
    .optional()
    .default(null),
  mute_while_recording: z.boolean().optional().default(false),
  audio_input_auto_enhance: z.boolean().optional().default(true),
  mic_enhance_preferences: z.record(z.boolean()).optional().default({}),
  append_trailing_space: z.boolean().optional().default(false),
  punctuation_enabled: z.boolean().optional().default(false),
  punctuation_model: z
    .string()
    .optional()
    .default("punct-zh-en-ct-transformer-2024-04-12-int8"),
  favorite_transcription_models: z.array(z.string()).optional().default([]),
  realtime_transcription_enabled: z.boolean().optional().default(false),
  offline_vad_force_interval_ms: z.number().optional().default(1000),
  offline_vad_force_window_seconds: z.number().optional().default(30),
  app_review_policies: z
    .record(z.string(), AppReviewPolicySchema)
    .optional()
    .default({}),
  app_profiles: z.array(AppProfileSchema).optional().default([]),
  app_to_profile: z.record(z.string(), z.string()).optional().default({}),
  post_process_context_enabled: z.boolean().optional().default(false),
  post_process_context_limit: z.number().min(1).max(30).optional().default(3),
  post_process_streaming_output_enabled: z.boolean().optional().default(true),
  post_process_hotword_injection_enabled: z.boolean().optional().default(true),
  openai_compatible_api_enabled: z.boolean().optional().default(true),
  openai_compatible_api_host: z.string().optional().default("127.0.0.1"),
  openai_compatible_api_port: z
    .number()
    .int()
    .nonnegative()
    .optional()
    .default(33178),
  openai_compatible_api_allow_lan: z.boolean().optional().default(false),
  openai_compatible_api_base_path: z.string().optional().default("/v1"),
  openai_compatible_api_access_key: z.string().optional().default(""),
  expert_mode: z.boolean().optional().default(false),
  keyboard_implementation: z
    .enum(["tauri", "handy_keys"])
    .optional()
    .default("tauri"),
  show_tray_icon: z.boolean().optional().default(true),
  length_routing_enabled: z.boolean().optional().default(false),
  length_routing_threshold: z.number().min(10).max(500).optional().default(100),
  length_routing_short_model: ModelChainSchema.nullable()
    .optional()
    .default(null),
  length_routing_long_model: ModelChainSchema.nullable()
    .optional()
    .default(null),
});

export const BindingResponseSchema = z.object({
  success: z.boolean(),
  binding: ShortcutBindingSchema.nullable(),
  error: z.string().nullable(),
});

export type AudioDevice = z.infer<typeof AudioDeviceSchema>;
export type BindingResponse = z.infer<typeof BindingResponseSchema>;
export type ShortcutBinding = z.infer<typeof ShortcutBindingSchema>;
export type ShortcutBindingsMap = z.infer<typeof ShortcutBindingsMapSchema>;
export type Settings = z.infer<typeof SettingsSchema>;

export const EngineTypeSchema = z.enum(["TranscribeCpp"]);
export type EngineType = z.infer<typeof EngineTypeSchema>;

export const ModelInfoSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string(),
  filename: z.string(),
  url: z.string().optional().nullable(),
  size_mb: z.number(),
  is_downloaded: z.boolean(),
  is_downloading: z.boolean(),
  partial_size: z.number(),
  is_directory: z.boolean(),
  engine_type: EngineTypeSchema,
  accuracy_score: z.number(),
  speed_score: z.number(),
  supports_translation: z.boolean().default(false),
  is_recommended: z.boolean().default(false),
  supported_languages: z.array(z.string()).default([]),
  is_custom: z.boolean().default(false),
  tags: z.array(z.string()).optional(),
  is_default: z.boolean().default(false),
});

export type ModelInfo = z.infer<typeof ModelInfoSchema>;

// =============================================================================
// Model Download/State Types (shared between hooks and components)
// =============================================================================

export interface DownloadProgress {
  model_id: string;
  downloaded: number;
  total: number;
  percentage: number;
}

export interface DownloadStats {
  startTime: number;
  lastUpdate: number;
  totalDownloaded: number;
  speed: number;
}

export interface ModelStateEvent {
  event_type: string;
  model_id?: string;
  model_name?: string;
  error?: string;
}

export type ModelStatus =
  | "ready"
  | "loading"
  | "downloading"
  | "extracting"
  | "error"
  | "unloaded"
  | "none";
