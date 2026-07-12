// Constants for ASR Models Settings

import type { LanguageKey, TypeKey } from "./types";

// Recommended model IDs (catalog slugs) for highlighting. Sourced from the
// vendored catalog's `recommended` entries (recommended_rank order).
export const RECOMMENDED_MODEL_IDS = new Set([
  "parakeet-unified-en-0.6b",
  "nemotron-3.5-asr-streaming-0.6b",
  "canary-180m-flash",
  "cohere-transcribe-03-2026",
  "whisper-medium",
]);

// All available language keys in display order
export const ALL_LANGUAGE_KEYS: LanguageKey[] = [
  "zh",
  "en",
  "yue",
  "ja",
  "ko",
  "de",
  "es",
  "fr",
  "ru",
  "multilingual",
  "other",
];

// All available type keys
export const TYPE_KEYS: TypeKey[] = [
  "whisper",
  "parakeet",
  "moonshine",
  "sensevoice",
  "zipformer",
  "paraformer",
  "other",
];

// Known language keys for parsing
export const KNOWN_LANGUAGE_KEYS: LanguageKey[] = [
  "zh",
  "yue",
  "en",
  "ja",
  "ko",
  "de",
  "es",
  "fr",
  "ru",
  "multilingual",
  "other",
];
