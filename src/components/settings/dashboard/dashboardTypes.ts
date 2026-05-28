export interface PostProcessStep {
  prompt_id?: string | null;
  prompt_name: string;
  model?: string | null;
  result: string;
}

export interface HistoryError {
  stage: "polish" | "asr";
  error_type: string;
  detail?: string | null;
  model?: string | null;
  provider?: string | null;
}

export interface HistoryEntry {
  id: number;
  file_name: string;
  timestamp: number;
  saved: boolean;
  title: string;
  transcription_text: string;
  streaming_text?: string | null;
  streaming_asr_model?: string | null;
  post_processed_text?: string | null;
  post_process_prompt?: string | null;
  post_process_prompt_id?: string | null;
  post_process_model?: string | null;
  duration_ms?: number | null;
  char_count?: number | null;
  corrected_char_count?: number | null;
  transcription_ms?: number | null;
  language?: string | null;
  asr_model?: string | null;
  app_name?: string | null;
  window_title?: string | null;
  post_process_history?: string | null; // JSON string of PostProcessStep[]
  token_count?: number | null;
  llm_call_count?: number | null;
  deleted: boolean;
  error_summary?: HistoryError | null;
}

export type DashboardSelection =
  | { type: "preset"; preset: "7d" | "30d" | "40d" | "all" }
  | { type: "day"; day: string };

export interface PaginatedHistoryResult {
  entries: HistoryEntry[];
  total_count: number;
  offset: number;
  limit: number;
}

export interface AppStats {
  count: number;
  chars: number;
}

export interface SummaryStats {
  entry_count: number;
  total_duration_ms: number;
  total_chars: number;
  llm_calls: number;
  by_app: Record<string, AppStats>;
  by_hour: number[];
  top_skills: string[];
}

export interface AiAnalysisEntry {
  timestamp: number;
  model: string;
  summary: string;
  reflection: string;
}

export interface Summary {
  id: number;
  period_type: string;
  period_start: number;
  period_end: number;
  stats: SummaryStats;
  ai_summary?: string | null;
  ai_reflection?: string | null;
  ai_generated_at?: number | null;
  ai_model_used?: string | null;
  ai_history?: AiAnalysisEntry[]; // History of AI analysis
  created_at: number;
  updated_at: number;
}
