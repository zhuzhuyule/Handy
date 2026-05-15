/**
 * Centralized Tauri Event Constants
 *
 * All Tauri event names should be defined here to avoid magic strings
 * and ensure type safety across the application.
 */

// =============================================================================
// Overlay Events
// =============================================================================

/** Show the recording overlay with a specific state */
export const SHOW_OVERLAY = "show-overlay";

/** Hide the recording overlay */
export const HIDE_OVERLAY = "hide-overlay";

/** Report an error to the overlay */
export const OVERLAY_ERROR = "overlay-error";

/** Online ASR timed out — overlay should show action buttons */
export const ASR_ONLINE_TIMEOUT = "asr-online-timeout";

// =============================================================================
// Audio Events
// =============================================================================

/** Real-time microphone level updates (array of frequency levels) */
export const MIC_LEVEL = "mic-level";

// =============================================================================
// Transcription Events
// =============================================================================

/** Partial transcription result from online (streaming) recognizer */
export const SHERPA_ONLINE_PARTIAL = "sherpa-online-partial";

/** Partial transcription result from offline recognizer */
export const SHERPA_OFFLINE_PARTIAL = "sherpa-offline-partial";

// =============================================================================
// Model Events
// =============================================================================

/** Model state has changed (downloaded, deleted, etc.) */
export const MODEL_STATE_CHANGED = "model-state-changed";

/** Model download progress update */
export const DOWNLOAD_PROGRESS = "model-download-progress";

/** Model download completed successfully */
export const DOWNLOAD_COMPLETED = "model-download-complete";

/** Model extraction started */
export const EXTRACTION_STARTED = "model-extraction-started";

/** Model extraction completed successfully */
export const EXTRACTION_COMPLETED = "model-extraction-completed";

/** Model extraction failed */
export const EXTRACTION_FAILED = "model-extraction-failed";

// =============================================================================
// Navigation Events
// =============================================================================

/** Navigate to a specific settings section */
export const NAVIGATE_TO_SETTINGS = "navigate-to-settings";

// =============================================================================
// History Events
// =============================================================================

/** Transcription history has been updated */
export const HISTORY_UPDATED = "history-updated";

// =============================================================================
// Application Events
// =============================================================================

/** Trigger update check */
export const CHECK_FOR_UPDATES = "check-for-updates";

/** Insert processed text back into the focused element inside the main Votype window */
export const VOTYPE_LOCAL_INSERT = "votype-local-insert";

/** Apply processed text directly to the Review window editor */
export const REVIEW_WINDOW_INLINE_APPLY = "review-window-inline-apply";

/** Replace the entire Review window document with rewritten text */
export const REVIEW_WINDOW_REWRITE_APPLY = "review-window-rewrite-apply";

/** Boolean payload — voice rewrite recording + pipeline is in progress.
 *  Emitted true at recording start, false when its FinishGuard ends.
 *  Review window listens to suppress ESC-close behaviour while active. */
export const REVIEW_REWRITE_STATE = "review-rewrite-state";

/** Refocus the currently active editable control inside a Votype-owned window */
export const VOTYPE_REFOCUS_ACTIVE_INPUT = "votype-refocus-active-input";

// =============================================================================
// Event Type Map (for generic use)
// =============================================================================

export const TauriEvents = {
  // Overlay
  SHOW_OVERLAY,
  HIDE_OVERLAY,
  OVERLAY_ERROR,
  ASR_ONLINE_TIMEOUT,

  // Audio
  MIC_LEVEL,

  // Transcription
  SHERPA_ONLINE_PARTIAL,
  SHERPA_OFFLINE_PARTIAL,

  // Model
  MODEL_STATE_CHANGED,
  DOWNLOAD_PROGRESS,
  DOWNLOAD_COMPLETED,
  EXTRACTION_STARTED,
  EXTRACTION_COMPLETED,
  EXTRACTION_FAILED,

  // Navigation
  NAVIGATE_TO_SETTINGS,

  // History
  HISTORY_UPDATED,

  // Application
  CHECK_FOR_UPDATES,
  VOTYPE_LOCAL_INSERT,
  REVIEW_WINDOW_INLINE_APPLY,
  REVIEW_WINDOW_REWRITE_APPLY,
  REVIEW_REWRITE_STATE,
  VOTYPE_REFOCUS_ACTIVE_INPUT,
} as const;

export type TauriEventName = (typeof TauriEvents)[keyof typeof TauriEvents];
