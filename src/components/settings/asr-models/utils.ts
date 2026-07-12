// Utility functions for ASR Models Settings

import type { ModelInfo } from "../../../lib/types";
import { KNOWN_LANGUAGE_KEYS } from "./constants";
import type { LanguageKey, ModeKey, SizeBucket, TypeKey } from "./types";

/**
 * Parse language keys from a model's tags or ID
 */
export const parseLanguageKeys = (model: ModelInfo): LanguageKey[] => {
  // Check explicit tags first
  if (model.tags && model.tags.length > 0) {
    const explicit = model.tags.filter((t) =>
      KNOWN_LANGUAGE_KEYS.includes(t as LanguageKey),
    ) as LanguageKey[];
    if (explicit.length > 0) return explicit;
  }

  const id = (model.id ?? "").toLowerCase();
  const tokenSet = new Set<LanguageKey>();

  // Parse language tokens from ID
  const re = /(^|[-_])(zh|yue|ct|cantonese|en|ja|ko|de|es|fr|ru)(?=([-_]|$))/g;
  for (const match of id.matchAll(re)) {
    const tok = match[2];
    if (tok === "ct" || tok === "cantonese") {
      tokenSet.add("yue");
    } else {
      tokenSet.add(tok as LanguageKey);
    }
  }

  const found = Array.from(tokenSet);
  if (found.length >= 2) return ["multilingual", ...found];
  if (found.length === 1) return found;
  return ["other"];
};

/**
 * Get the mode key for a model (asr or punctuation)
 */
export const getModeKey = (m: ModelInfo): ModeKey =>
  m.id.startsWith("punct-") ? "punctuation" : "asr";

/**
 * Get the type key for a model.
 *
 * All local models now run through the unified transcribe.cpp engine, so the
 * family is inferred from the model id/name rather than the engine type.
 */
export const getTypeKey = (m: ModelInfo): TypeKey => {
  const id = (m.id ?? "").toLowerCase();
  if (id.startsWith("punct-")) return "other";
  if (id.includes("whisper")) return "whisper";
  if (id.includes("parakeet")) return "parakeet";
  if (id.includes("moonshine")) return "moonshine";
  if (id.includes("sensevoice") || id.includes("sense-voice"))
    return "sensevoice";
  if (id.includes("zipformer")) return "zipformer";
  if (id.includes("paraformer")) return "paraformer";
  return "other";
};

/**
 * Ordering function for mode keys
 */
export const orderMode = (k: ModeKey): number => (k === "asr" ? 0 : 1);

/**
 * Ordering function for type keys
 */
export const orderType = (k: TypeKey): number => {
  const order: Record<TypeKey, number> = {
    whisper: 0,
    parakeet: 1,
    moonshine: 2,
    sensevoice: 3,
    zipformer: 4,
    paraformer: 5,
    other: 99,
  };
  return order[k] ?? 99;
};

/**
 * Ordering function for language keys
 */
export const orderLanguage = (k: LanguageKey): number => {
  switch (k) {
    case "multilingual":
      return 0;
    case "zh":
      return 1;
    case "en":
      return 2;
    case "yue":
      return 3;
    case "ja":
      return 4;
    case "ko":
      return 5;
    case "de":
      return 6;
    case "es":
      return 7;
    case "fr":
      return 8;
    case "ru":
      return 9;
    case "other":
      return 99;
  }
};

/**
 * Categorize model size into bucket
 */
export const sizeBucket = (sizeMb?: number): SizeBucket => {
  if (sizeMb == null || !Number.isFinite(sizeMb)) return "unknown";
  if (sizeMb < 100) return "small";
  if (sizeMb < 500) return "medium";
  return "large";
};

/**
 * Toggle a value in a Set (add if not present, remove if present)
 */
export const toggleSetValue = <T>(set: Set<T>, value: T): Set<T> => {
  const next = new Set(set);
  if (next.has(value)) {
    next.delete(value);
  } else {
    next.add(value);
  }
  return next;
};
