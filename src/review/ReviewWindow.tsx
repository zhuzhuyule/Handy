// ReviewWindow - Independent window for reviewing low-confidence transcriptions
// This provides a floating window UI for editing and inserting transcribed text

import { Button, ScrollArea, Tooltip } from "@radix-ui/themes";
import { IconCheck, IconClipboard, IconTextPlus } from "@tabler/icons-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Extension } from "@tiptap/core";
import CodeBlock from "@tiptap/extension-code-block";
import Placeholder from "@tiptap/extension-placeholder";
import { TextSelection } from "@tiptap/pm/state";
import {
  EditorContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  useEditor,
} from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  REVIEW_WINDOW_INLINE_APPLY,
  REVIEW_WINDOW_REWRITE_APPLY,
  VOTYPE_REFOCUS_ACTIVE_INPUT,
} from "../lib/events";
import { escapeHtml } from "../lib/utils/html";
import { MultiModelCandidate } from "./CandidatePanel";
import { DiffMark } from "./diff-mark";
import {
  buildDiffViews,
  buildPlainViews,
  computeChangePercent,
  computeChangeStats,
} from "./diff-utils";
import { DiffViewPanel } from "./DiffViewPanel";
import { hljs } from "./highlight";
import { simpleMarkdownToHtml } from "./markdown-utils";
import { MultiCandidateView } from "./MultiCandidateView";
import { NeonBorder } from "./NeonBorder";
import { ReviewFooter } from "./ReviewFooter";
import { PromptInfo, ReviewHeader, ReviewModelOption } from "./ReviewHeader";
import "./ReviewWindow.css";

const DIFF_THRESHOLD = 50;
const SPEED_RANK_STATS_STORAGE_KEY = "votype.multiModelSpeedRankStats";
const MULTI_SORT_MODE_STORAGE_KEY = "votype.multiModelSortMode";

type RankPosition = 1 | 2 | 3;
type SpeedRankStats = Record<string, Partial<Record<RankPosition, number>>>;
type MultiSortMode = "default" | "speed" | "change";

function readSpeedRankStats(): SpeedRankStats {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(SPEED_RANK_STATS_STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as SpeedRankStats;
  } catch {
    return {};
  }
}

function writeSpeedRankStats(stats: SpeedRankStats) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(
    SPEED_RANK_STATS_STORAGE_KEY,
    JSON.stringify(stats),
  );
}

function readMultiSortMode(): MultiSortMode {
  if (typeof window === "undefined") return "default";
  const raw = window.localStorage.getItem(MULTI_SORT_MODE_STORAGE_KEY);
  if (raw === "speed" || raw === "change") return raw;
  return "default";
}

function writeMultiSortMode(mode: MultiSortMode) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(MULTI_SORT_MODE_STORAGE_KEY, mode);
}

interface ReviewData {
  source_text: string;
  final_text: string;
  change_percent: number;
  history_id: number | null;
  reason?: string | null;
  output_mode?: "polish" | "chat";
  skill_name?: string | null;
  prompt_id?: string | null;
  model_id?: string | null;
}

interface ReviewWindowProps {
  initialData: ReviewData;
  multiCandidates?: MultiModelCandidate[];
  autoSelectedId?: string | null;
  onClose: () => void;
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof Node)) return false;

  const element = target instanceof HTMLElement ? target : target.parentElement;
  if (!element) return false;

  return Boolean(
    element.closest(
      'input, textarea, [contenteditable=""], [contenteditable="true"], [contenteditable="plaintext-only"], .ProseMirror',
    ),
  );
}

const CodeBlockComponent = ({
  node: { textContent },
  extension,
}: {
  node: { textContent: string };
  extension: { options: { HTMLAttributes?: Record<string, string> } };
}) => {
  const { t } = useTranslation();
  const [insertState, setInsertState] = useState<"idle" | "success">("idle");
  const [copyState, setCopyState] = useState<"idle" | "success">("idle");

  const handleInsert = useCallback(async () => {
    try {
      await invoke("paste_to_previous_window", { text: textContent });
      setInsertState("success");
      setTimeout(() => setInsertState("idle"), 2000);
    } catch (e) {
      console.error("Failed to insert code:", e);
    }
  }, [textContent]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(textContent);
      setCopyState("success");
      setTimeout(() => setCopyState("idle"), 2000);
    } catch (e) {
      console.error("Failed to copy code:", e);
    }
  }, [textContent]);

  const language =
    extension.options?.HTMLAttributes?.class?.match(/language-(\w+)/)?.[1] ||
    "";

  const highlightedCode = useCallback(() => {
    if (!textContent) return "";
    try {
      if (language && hljs.getLanguage(language)) {
        return hljs.highlight(textContent.trim(), { language }).value;
      }
      return hljs.highlightAuto(textContent.trim()).value;
    } catch {
      return escapeHtml(textContent.trim());
    }
  }, [textContent, language]);

  return (
    <NodeViewWrapper className="code-block-wrapper relative group my-2">
      <div className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity z-10 flex gap-1">
        <Tooltip content={t("common.copy", "Copy")}>
          <Button
            size="1"
            variant="soft"
            color={copyState === "success" ? "green" : "gray"}
            onClick={handleCopy}
            className="cursor-pointer shadow-sm backdrop-blur-sm px-1.5!"
          >
            {copyState === "success" ? (
              <IconCheck size={14} />
            ) : (
              <IconClipboard size={14} />
            )}
          </Button>
        </Tooltip>
        <Tooltip content={t("transcription.review.insertCode", "Insert")}>
          <Button
            size="1"
            variant="soft"
            color={insertState === "success" ? "green" : "gray"}
            onClick={handleInsert}
            className="cursor-pointer shadow-sm backdrop-blur-sm px-1.5!"
          >
            {insertState === "success" ? (
              <IconCheck size={14} />
            ) : (
              <IconTextPlus size={14} />
            )}
          </Button>
        </Tooltip>
      </div>
      <pre className="bg-(--gray-3) p-3 rounded-md overflow-x-auto text-sm font-mono border border-(--gray-4)">
        <code
          className={`hljs ${language ? `language-${language}` : ""}`}
          dangerouslySetInnerHTML={{ __html: highlightedCode() }}
        />
      </pre>
    </NodeViewWrapper>
  );
};

const ReviewWindow: React.FC<ReviewWindowProps> = ({
  initialData,
  multiCandidates,
  autoSelectedId,
  onClose,
}) => {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [selectedCandidateId, setSelectedCandidateId] = useState<string | null>(
    autoSelectedId && multiCandidates?.some((c) => c.id === autoSelectedId)
      ? autoSelectedId
      : multiCandidates && multiCandidates.length > 0
        ? multiCandidates[0].id
        : null,
  );
  const selectedCandidateIdRef = useRef(selectedCandidateId);
  useEffect(() => {
    selectedCandidateIdRef.current = selectedCandidateId;
  }, [selectedCandidateId]);

  // Auto-select candidate when backend picks one (race/lazy mode)
  useEffect(() => {
    if (!multiCandidates) return;
    let unlisten: (() => void) | null = null;
    listen<{ id: string }>("multi-post-process-auto-selected", (event) => {
      setSelectedCandidateId((prev) => {
        // Only auto-select if user hasn't manually picked a different candidate
        const initialId = multiCandidates[0]?.id;
        if (prev !== null && prev !== initialId) return prev;
        return event.payload.id;
      });
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [!!multiCandidates]);

  const [editingCandidateId, setEditingCandidateId] = useState<string | null>(
    null,
  );

  // Prompt selector state for multi-candidate mode
  const [prompts, setPrompts] = useState<PromptInfo[]>([]);
  const [selectedPromptId, setSelectedPromptId] = useState<string>("");
  const [localCandidates, setLocalCandidates] = useState<
    MultiModelCandidate[] | undefined
  >(multiCandidates);
  const [isRerunning, setIsRerunning] = useState(false);
  const [editedTexts, setEditedTexts] = useState<Record<string, string>>({});
  const localCandidatesRef = useRef(localCandidates);
  useEffect(() => {
    localCandidatesRef.current = localCandidates;
  }, [localCandidates]);
  const editedTextsRef = useRef(editedTexts);
  useEffect(() => {
    editedTextsRef.current = editedTexts;
  }, [editedTexts]);
  const [showCandidateShortcutHints, setShowCandidateShortcutHints] =
    useState(false);
  const [multiSortMode, setMultiSortMode] = useState<MultiSortMode>(() =>
    readMultiSortMode(),
  );
  const [speedRankStats, setSpeedRankStats] = useState<SpeedRankStats>(() =>
    readSpeedRankStats(),
  );
  const lastRecordedRaceKeyRef = useRef<string | null>(null);
  const pendingInlineRangeRef = useRef<{ from: number; to: number } | null>(
    null,
  );

  useEffect(() => {
    writeMultiSortMode(multiSortMode);
  }, [multiSortMode]);

  // Model selector state (single-model polish mode only)
  const [modelOptions, setModelOptions] = useState<ReviewModelOption[]>([]);
  const [selectedModelId, setSelectedModelId] = useState<string>("");
  const [defaultModelLabel, setDefaultModelLabel] = useState<string>("");
  const [currentModelName, setCurrentModelName] = useState<string>("");

  // Translation state
  const [translationEnabled, setTranslationEnabled] = useState(false);
  const [translatedText, setTranslatedText] = useState<string | null>(null);
  const [isTranslating, setIsTranslating] = useState(false);
  const [translationSourceText, setTranslationSourceText] = useState(
    initialData.final_text,
  );
  const [translatedSourceText, setTranslatedSourceText] = useState<
    string | null
  >(null);
  const [translationStatus, setTranslationStatus] = useState<
    "idle" | "loading" | "ready" | "error"
  >("idle");
  const [translationError, setTranslationError] = useState<string | null>(null);
  const translationRequestIdRef = useRef(0);
  const [revisionHistory, setRevisionHistory] = useState<string[]>([
    initialData.final_text,
  ]);
  const [revisionIndex, setRevisionIndex] = useState(0);
  const revisionIndexRef = useRef(0);

  // Fetch prompts and model options on mount
  // NOTE: dependency is [] because ReviewWindow is re-mounted via key when new data arrives.
  // Using [initialData] would cause re-runs on every progress event (object reference changes).
  useEffect(() => {
    invoke<{ prompts: PromptInfo[]; selected_id: string | null }>(
      "get_post_process_prompts",
    ).then((resp) => {
      setPrompts(resp.prompts);
      // Prioritize the prompt_id from initialData (may be overridden by app rules)
      if (initialData.prompt_id) {
        setSelectedPromptId(initialData.prompt_id);
      } else if (resp.selected_id) {
        setSelectedPromptId(resp.selected_id);
      } else if (resp.prompts.length > 0) {
        setSelectedPromptId(resp.prompts[0].id);
      }
    });
    // Fetch text-type models for single-model mode selector
    if (!multiCandidates || multiCandidates.length === 0) {
      invoke<{
        models: ReviewModelOption[];
        default_model_id: string | null;
      }>("get_review_model_options").then((resp) => {
        setModelOptions(resp.models);
        // Prioritize the model_id from initialData
        // Backend may pass a CachedModel UUID or a model_id string (e.g. "gpt-4o")
        if (initialData.model_id) {
          const selectedModel =
            resp.models.find((m) => m.id === initialData.model_id) ||
            resp.models.find((m) => m.model_id === initialData.model_id);
          if (selectedModel) {
            setSelectedModelId(selectedModel.id);
            setCurrentModelName(selectedModel.label);
          } else {
            // No matching cached model — use the raw value as display name
            setCurrentModelName(initialData.model_id);
          }
        } else if (resp.default_model_id) {
          const dm = resp.models.find((m) => m.id === resp.default_model_id);
          if (dm) setDefaultModelLabel(dm.label);
        }
      });
    }

    invoke<{ enabled: boolean }>("get_review_translation_settings")
      .then((resp) => {
        setTranslationEnabled(resp.enabled);
      })
      .catch((e) => {
        console.error("Failed to load review translation settings:", e);
      });
  }, []);

  // Sync external multiCandidates prop into local state
  // Skip during rerun — ReviewWindow owns local state while rerunning
  useEffect(() => {
    if (multiCandidates && !isRerunning) {
      setLocalCandidates(multiCandidates);
    }
  }, [multiCandidates]);

  // Listen for rerun reset and progress events
  useEffect(() => {
    if (!multiCandidates) return;
    let unlistenReset: (() => void) | null = null;
    let unlistenProgress: (() => void) | null = null;

    listen<{ candidates: MultiModelCandidate[] }>(
      "multi-model-rerun-reset",
      (event) => {
        setLocalCandidates(event.payload.candidates);
        setSelectedCandidateId(null);
        setEditingCandidateId(null);
        setEditedTexts({});
        setIsRerunning(true);
      },
    ).then((fn) => {
      unlistenReset = fn;
    });

    // Listen for progress events to update local candidates during rerun
    listen<{
      results: MultiModelCandidate[];
    }>("multi-post-process-progress", (event) => {
      const progress = event.payload;
      setLocalCandidates((prev) => {
        if (!prev) return prev;
        return prev.map((candidate) => {
          const completed = progress.results.find((r) => r.id === candidate.id);
          if (completed) {
            return {
              ...candidate,
              text: completed.text,
              confidence: completed.confidence,
              processing_time_ms: completed.processing_time_ms,
              error: completed.error,
              ready: completed.ready ?? true,
              output_speed: completed.output_speed,
            };
          }
          return candidate;
        });
      });
    }).then((fn) => {
      unlistenProgress = fn;
    });

    return () => {
      unlistenReset?.();
      unlistenProgress?.();
    };
  }, [!!multiCandidates]);

  // Mark rerun done when all local candidates are ready
  useEffect(() => {
    if (
      isRerunning &&
      localCandidates &&
      localCandidates.length > 0 &&
      localCandidates.every((c) => c.ready)
    ) {
      setIsRerunning(false);
      // Auto-select first successful candidate
      const first = localCandidates.find((c) => !c.error);
      if (first) setSelectedCandidateId(first.id);
    }
  }, [isRerunning, localCandidates]);

  // Derive the label of the currently selected candidate for the header
  const selectedCandidateLabel = useMemo(() => {
    if (!selectedCandidateId) return null;
    const all = localCandidates || multiCandidates;
    const found = all?.find((c) => c.id === selectedCandidateId);
    return found?.label ?? null;
  }, [selectedCandidateId, localCandidates, multiCandidates]);

  // The candidates to render (local state, updated by progress events)
  const displayCandidates = localCandidates || multiCandidates;
  const sortedCandidates = useMemo(() => {
    if (!displayCandidates || multiSortMode === "default") {
      return displayCandidates;
    }

    const originalOrder = new Map(
      displayCandidates.map((candidate, index) => [candidate.id, index]),
    );

    return [...displayCandidates].sort((a, b) => {
      const aReady = a.ready && !a.error;
      const bReady = b.ready && !b.error;

      if (aReady && bReady) {
        if (multiSortMode === "speed") {
          const aHasTime = a.processing_time_ms > 0;
          const bHasTime = b.processing_time_ms > 0;
          if (
            aHasTime &&
            bHasTime &&
            a.processing_time_ms !== b.processing_time_ms
          ) {
            return a.processing_time_ms - b.processing_time_ms;
          }
        } else if (multiSortMode === "change") {
          const aChange = Math.abs(
            computeChangePercent(initialData.source_text, a.text),
          );
          const bChange = Math.abs(
            computeChangePercent(initialData.source_text, b.text),
          );
          if (aChange !== bChange) return aChange - bChange;
        }
      }

      if (aReady !== bReady) return aReady ? -1 : 1;

      const aError = Boolean(a.error);
      const bError = Boolean(b.error);
      if (aError !== bError) return aError ? 1 : -1;

      return (
        (originalOrder.get(a.id) ?? Number.MAX_SAFE_INTEGER) -
        (originalOrder.get(b.id) ?? Number.MAX_SAFE_INTEGER)
      );
    });
  }, [displayCandidates, multiSortMode, initialData.source_text]);

  useEffect(() => {
    if (!displayCandidates || multiSortMode !== "speed") return;
    if (
      displayCandidates.length === 0 ||
      !displayCandidates.every((c) => c.ready)
    ) {
      return;
    }

    const podium = [...displayCandidates]
      .filter((c) => !c.error && c.processing_time_ms > 0)
      .sort((a, b) => a.processing_time_ms - b.processing_time_ms)
      .slice(0, 3);

    if (podium.length === 0) return;

    const raceKey = JSON.stringify(
      podium.map((candidate, index) => ({
        id: candidate.id,
        rank: index + 1,
        processing_time_ms: candidate.processing_time_ms,
      })),
    );
    if (lastRecordedRaceKeyRef.current === raceKey) {
      return;
    }
    lastRecordedRaceKeyRef.current = raceKey;

    setSpeedRankStats((prev) => {
      const next: SpeedRankStats = { ...prev };
      podium.forEach((candidate, index) => {
        const rank = (index + 1) as RankPosition;
        next[candidate.id] = {
          ...next[candidate.id],
          [rank]: (next[candidate.id]?.[rank] ?? 0) + 1,
        };
      });
      writeSpeedRankStats(next);
      return next;
    });
  }, [displayCandidates, multiSortMode]);

  // Use selected candidate text if in multi-candidate mode
  const currentText =
    selectedCandidateId && sortedCandidates
      ? sortedCandidates.find((c) => c.id === selectedCandidateId)?.text ||
        initialData.final_text
      : initialData.final_text;

  // Diff toggle: auto-off when change_percent >= threshold
  const [showDiff, setShowDiff] = useState(() => {
    if (initialData.output_mode === "chat") return false;
    return Math.abs(initialData.change_percent ?? 0) < DIFF_THRESHOLD;
  });

  // Track current final text (updated by rerun results)
  const [currentFinalText, setCurrentFinalText] = useState(
    initialData.final_text,
  );

  // Compute change stats for single-model view
  const singleModelChangeStats = useMemo(() => {
    if (initialData.output_mode === "chat") return null;
    return computeChangeStats(initialData.source_text, currentFinalText);
  }, [initialData.source_text, currentFinalText, initialData.output_mode]);

  const [sourceHtml, setSourceHtml] = useState(() => {
    if (initialData.output_mode === "chat") return "";
    const build = showDiff ? buildDiffViews : buildPlainViews;
    return build(initialData.source_text, currentText).sourceHtml;
  });
  const isMac =
    typeof navigator !== "undefined" &&
    navigator.platform.toLowerCase().includes("mac");

  // Use refs to access latest callbacks inside Tiptap extension
  const insertPolishedRef = useRef<() => void>(() => {});
  const insertEnglishRef = useRef<() => void>(() => {});
  const insertOriginalRef = useRef<() => void>(() => {});
  const cancelRef = useRef<() => void>(() => {});
  const translateRef = useRef<() => void>(() => {});
  // Cmd+Enter routes here: inserts English when translation is available,
  // otherwise falls into "activate editing" (focus polish editor).
  const metaEnterRef = useRef<() => void>(() => {});

  // Tracks which shortcut modifier the user is currently holding so we can
  // preview the insert action: marquee-border the target button, highlight
  // the shortcut glyph, and flash a border around the content that button
  // would insert. Cleared on keyup or when the window loses focus.
  const [pressedModifier, setPressedModifier] = useState<
    "meta" | "ctrl" | null
  >(null);

  useEffect(() => {
    const isEditableTargetNow = (el: EventTarget | null) =>
      el instanceof HTMLElement &&
      (el.tagName === "INPUT" ||
        el.tagName === "TEXTAREA" ||
        el.isContentEditable);

    const handleDown = (e: KeyboardEvent) => {
      if (e.repeat) return;
      if (e.key === "Meta") setPressedModifier("meta");
      else if (e.key === "Control") setPressedModifier("ctrl");
      // If another key is typed in an editor, clear the preview — it means
      // the user is just typing, not arming a shortcut.
      else if (isEditableTargetNow(e.target) && !e.metaKey && !e.ctrlKey) {
        setPressedModifier(null);
      }
    };
    const handleUp = (e: KeyboardEvent) => {
      if (e.key === "Meta" || e.key === "Control") {
        setPressedModifier(null);
      }
    };
    const clear = () => setPressedModifier(null);

    window.addEventListener("keydown", handleDown);
    window.addEventListener("keyup", handleUp);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", handleDown);
      window.removeEventListener("keyup", handleUp);
      window.removeEventListener("blur", clear);
    };
  }, []);
  // Manual-sizing state. Once the user drags the window, we stop auto-fitting
  // the window to content. Editor area becomes flex:1 (fills leftover), and
  // we only grow the window when the preview/translation region grows.
  const manualSizeRef = useRef({
    active: false,
    lastPreviewH: 0,
  });
  const programmaticResizeUntilRef = useRef(0);
  // Last size WE asked Tauri for — incoming resize events matching this
  // are ours, not a user drag.
  const lastIssuedSizeRef = useRef<{ w: number; h: number } | null>(null);
  const dragSettleTimerRef = useRef<number | null>(null);
  const [manualSize, setManualSize] = useState(false);

  const applyProgrammaticResize = useCallback(
    async (width: number, height: number, reposition: boolean) => {
      programmaticResizeUntilRef.current = Math.max(
        programmaticResizeUntilRef.current,
        Date.now() + 500,
      );
      lastIssuedSizeRef.current = {
        w: Math.round(width),
        h: Math.round(height),
      };
      try {
        await invoke("resize_review_window", {
          width,
          height,
          reposition,
        });
      } catch (e) {
        console.error("[ReviewWindow] resize_review_window failed:", e);
      }
    },
    [],
  );

  const measureAndResize = useCallback(
    async (reposition: boolean) => {
      const container = containerRef.current;
      if (!container) return;

      const screenH = window.screen?.availHeight ?? 900;
      const SCREEN_SAFETY = 80;
      const screenMaxH = Math.max(400, screenH - SCREEN_SAFETY);
      document.documentElement.style.setProperty(
        "--review-output-max",
        `${Math.floor(screenH * 0.6)}px`,
      );
      document.documentElement.style.setProperty(
        "--review-preview-max",
        `${Math.floor(screenH * 0.3)}px`,
      );

      const regionHeight = (sel: string) =>
        container.querySelector(sel)?.getBoundingClientRect().height ?? 0;
      const previewH = regionHeight(".review-preview-dock");

      // Manual mode: window size is user-controlled. Only grow if preview
      // (translation) grew and no longer fits. Editor area stays fixed via
      // CSS min-height; leftover space naturally flows to it via flex: 1.
      if (manualSizeRef.current.active) {
        const delta = previewH - manualSizeRef.current.lastPreviewH;
        manualSizeRef.current.lastPreviewH = previewH;
        if (delta > 0) {
          const newH = Math.min(window.innerHeight + delta, screenMaxH);
          await applyProgrammaticResize(window.innerWidth, newH, false);
        }
        return;
      }

      // Auto mode: window height == sum of rendered region heights.
      const headerH = regionHeight(".review-header");
      const footerH = regionHeight(".review-footer");
      // Measure the wrapper that includes its own padding so the window
      // height accounts for the content-area's 12px top/bottom padding +
      // any margin between the chat card and the footer. For polish mode,
      // .review-panels-layout IS the same element as .review-content-area
      // (two classes on one div), so this is a no-op there. Falling back
      // to the bare .review-section-no-title would undercount by ~24px and
      // clip the footer.
      const contentH =
        regionHeight(".review-panels-layout") ||
        regionHeight(".review-multi-content") ||
        regionHeight(".review-content-area");
      // Must match `.review-window-container { padding-bottom }`.
      const containerPadding = 16;

      const rawTotalH =
        headerH + contentH + previewH + footerH + containerPadding;
      const totalH = Math.min(rawTotalH, screenMaxH);
      const currentW = window.innerWidth;

      await applyProgrammaticResize(currentW, totalH, reposition);
    },
    [applyProgrammaticResize],
  );

  // Detect user-driven window resize → enter manual mode. Programmatic
  // resizes (our own `resize_review_window` call) are filtered out via
  // the short-lived `programmaticResizeUntilRef` window. We also grace
  // the first ~1.5s after mount so the Rust-side initial sizing + our
  // first auto measureAndResize don't get misclassified as a user drag.
  useEffect(() => {
    // Grace for ~2s covering Rust initial set_size, webview paint, and first
    // auto measureAndResize. Nothing within that window is "user drag".
    programmaticResizeUntilRef.current = Date.now() + 2000;
    const handleResize = () => {
      const w = window.innerWidth;
      const h = window.innerHeight;
      // If the new size matches what we last asked for (within 4px slop),
      // it's our own resize round-tripping — never a drag.
      if (lastIssuedSizeRef.current) {
        const dw = Math.abs(w - lastIssuedSizeRef.current.w);
        const dh = Math.abs(h - lastIssuedSizeRef.current.h);
        if (dw <= 4 && dh <= 4) return;
      }
      if (Date.now() < programmaticResizeUntilRef.current) return;
      // Real user drag. Flip manual mode, capture editor height on settle.
      if (!manualSizeRef.current.active) {
        const previewEl = containerRef.current?.querySelector(
          ".review-preview-dock",
        );
        manualSizeRef.current.lastPreviewH =
          previewEl?.getBoundingClientRect().height ?? 0;
        manualSizeRef.current.active = true;
      }
      if (dragSettleTimerRef.current !== null) {
        window.clearTimeout(dragSettleTimerRef.current);
      }
      dragSettleTimerRef.current = window.setTimeout(() => {
        const editorEl = containerRef.current?.querySelector<HTMLElement>(
          ".review-panel-output-compact",
        );
        if (editorEl) {
          const editorH = Math.floor(editorEl.getBoundingClientRect().height);
          document.documentElement.style.setProperty(
            "--review-editor-min",
            `${editorH}px`,
          );
          // Only flip the CSS mode once we have a real editor-min, so the
          // transition from auto → manual doesn't momentarily let editor
          // expand via flex:1 with min=0.
          setManualSize(true);
        }
      }, 180);
    };
    window.addEventListener("resize", handleResize);
    return () => {
      window.removeEventListener("resize", handleResize);
      if (dragSettleTimerRef.current !== null) {
        window.clearTimeout(dragSettleTimerRef.current);
      }
    };
  }, []);

  const editor = useEditor(
    {
      extensions: [
        StarterKit.configure({
          codeBlock: false,
        }),
        CodeBlock.extend({
          addNodeView() {
            return ReactNodeViewRenderer(CodeBlockComponent);
          },
        }),
        DiffMark,
        Placeholder.configure({
          placeholder: t(
            "transcription.review.placeholder",
            "Edit transcription...",
          ),
        }),
        Extension.create({
          name: "shortcuts",
          addKeyboardShortcuts() {
            return {
              "Meta-Enter": () => {
                metaEnterRef.current();
                return true;
              },
              "Ctrl-Enter": () => {
                insertPolishedRef.current();
                return true;
              },
              "Mod-t": () => {
                translateRef.current();
                return true;
              },
              Tab: () => {
                insertOriginalRef.current();
                return true;
              },
              Escape: () => {
                cancelRef.current();
                return true;
              },
            };
          },
        }),
      ],
      content:
        initialData.output_mode === "chat"
          ? simpleMarkdownToHtml(initialData.final_text)
          : (showDiff ? buildDiffViews : buildPlainViews)(
              initialData.source_text,
              initialData.final_text,
            ).targetHtml,
      editorProps: {
        attributes: {
          class: "review-editor",
        },
      },
    },
    [t],
  );

  // Track whether this instance was mounted in multi-candidate mode.
  // Stable across re-renders so it doesn't trigger effects on progress events.
  const isMultiCandidateMode = useRef(
    !!(multiCandidates && multiCandidates.length > 0),
  );

  useEffect(() => {
    if (!editor) return;
    // In multi-candidate mode the Tiptap editor is hidden behind
    // MultiCandidateView. Skip content rebuilds to avoid wasted work
    // on every progress event.
    if (isMultiCandidateMode.current) return;

    let content = "";
    let nextSourceHtml = "";

    if (initialData.output_mode === "chat") {
      content = simpleMarkdownToHtml(currentFinalText.trim());
    } else {
      const build = showDiff ? buildDiffViews : buildPlainViews;
      const views = build(initialData.source_text, currentFinalText);
      content = views.targetHtml;
      nextSourceHtml = views.sourceHtml;
    }

    editor.commands.setContent(content, { emitUpdate: false });
    const nextEditorText = editor.getText({ blockSeparator: "\n" });
    setTranslationSourceText(nextEditorText);
    void invoke("set_review_editor_content_state", {
      text: nextEditorText,
    }).catch((e) => {
      console.error("Failed to sync review editor content state:", e);
    });
    setSourceHtml(nextSourceHtml);
    setTimeout(() => measureAndResize(false), 16);
  }, [
    editor,
    initialData.source_text,
    currentFinalText,
    initialData.output_mode,
    showDiff,
  ]);

  useEffect(() => {
    if (!editor) return;
    let disposed = false;
    // In multi-candidate mode the Tiptap editor is hidden; skip focus
    const focusTimer = isMultiCandidateMode.current
      ? undefined
      : window.setTimeout(() => {
          if (disposed || editor.isDestroyed) return;
          editor.commands.focus("end");
        }, 50);

    // Multi-model mode: backend already sized and showed the window directly,
    // skip frontend measure/resize to avoid overriding the correct size.
    // Single-model/chat mode: measure DOM, resize, then notify backend to show.
    const isMultiModel = isMultiCandidateMode.current;
    const readyTimer = window.setTimeout(async () => {
      if (disposed) return;
      if (!isMultiModel) {
        try {
          await measureAndResize(true);
        } catch (e) {
          console.error("Failed to measure/resize:", e);
        }
        invoke("review_window_content_ready").catch((e) => {
          console.error("Failed to notify content ready:", e);
        });
      }
    }, 16);

    return () => {
      disposed = true;
      window.clearTimeout(focusTimer);
      window.clearTimeout(readyTimer);
    };
  }, [editor]);

  const getEditorText = useCallback(() => {
    // In multi-candidate mode, return edited or original candidate text
    if (sortedCandidates && selectedCandidateId) {
      const edited = editedTexts[selectedCandidateId];
      if (edited !== undefined) return edited;
      const candidate = sortedCandidates.find(
        (c) => c.id === selectedCandidateId,
      );
      return candidate?.text || "";
    }
    if (!editor) return "";
    return editor.getText({ blockSeparator: "\n" });
  }, [editor, sortedCandidates, selectedCandidateId, editedTexts]);

  const applyInlineTextToEditor = useCallback(
    (text: string) => {
      if (!editor || isMultiCandidateMode.current) return;

      const currentSelection = editor.state.selection;
      const pendingRange = pendingInlineRangeRef.current ?? {
        from: currentSelection.from,
        to: currentSelection.to,
      };

      editor
        .chain()
        .focus()
        .command(({ tr, dispatch }) => {
          tr.insertText(text, pendingRange.from, pendingRange.to);
          const cursor = pendingRange.from + text.length;
          tr.setSelection(TextSelection.create(tr.doc, cursor));
          dispatch?.(tr);
          return true;
        })
        .run();

      pendingInlineRangeRef.current = null;
      window.setTimeout(() => measureAndResize(false), 16);
    },
    [editor, measureAndResize],
  );

  const replaceEditorDocument = useCallback(
    (text: string) => {
      if (isMultiCandidateMode.current) return;
      setCurrentFinalText(text);
      setRevisionHistory((prev) => {
        const base = prev.slice(0, revisionIndexRef.current + 1);
        if (base[base.length - 1] === text) {
          return base;
        }
        const next = [...base, text];
        setRevisionIndex(next.length - 1);
        return next;
      });
      pendingInlineRangeRef.current = null;
      window.setTimeout(() => measureAndResize(false), 16);
    },
    [measureAndResize],
  );

  useEffect(() => {
    revisionIndexRef.current = revisionIndex;
  }, [revisionIndex]);

  useEffect(() => {
    if (isMultiCandidateMode.current) return;
    const nextText = revisionHistory[revisionIndex];
    if (typeof nextText === "string" && nextText !== currentFinalText) {
      setCurrentFinalText(nextText);
    }
  }, [currentFinalText, revisionHistory, revisionIndex]);

  const handleUndoRevision = useCallback(() => {
    setRevisionIndex((prev) => Math.max(0, prev - 1));
  }, []);

  const handleRedoRevision = useCallback(() => {
    setRevisionIndex((prev) => Math.min(revisionHistory.length - 1, prev + 1));
  }, [revisionHistory.length]);

  const getOriginalReviewText = useCallback(() => {
    if (sortedCandidates && selectedCandidateId) {
      return (
        sortedCandidates
          .find((c) => c.id === selectedCandidateId)
          ?.text.trim() || ""
      );
    }
    return currentFinalText.trim();
  }, [currentFinalText, selectedCandidateId, sortedCandidates]);

  const didUserEditReviewedText = useCallback(() => {
    const currentText = getEditorText().trim();
    const originalReviewText = getOriginalReviewText();

    if (sortedCandidates && selectedCandidateId) {
      return (
        editedTexts[selectedCandidateId] !== undefined &&
        currentText.length > 0 &&
        currentText !== originalReviewText
      );
    }

    return currentText.length > 0 && currentText !== originalReviewText;
  }, [
    editedTexts,
    getEditorText,
    getOriginalReviewText,
    selectedCandidateId,
    sortedCandidates,
  ]);

  const buildTranslationInsertPayload = useCallback(
    (text: string) => {
      const trimmedText = text.trim();
      if (
        translationEnabled &&
        translatedText &&
        translatedSourceText &&
        translatedSourceText === trimmedText
      ) {
        return {
          translatedTextForInsert: translatedText,
          translationSourceText: translatedSourceText,
        };
      }

      return {};
    },
    [translatedSourceText, translatedText, translationEnabled],
  );

  const handleInsertPolished = useCallback(async () => {
    const currentText = getEditorText();
    if (isSubmitting || !currentText.trim()) return;

    setIsSubmitting(true);

    try {
      await invoke("confirm_reviewed_transcription", {
        text: currentText.trim(),
        historyId: initialData.history_id,
        cachedModelId: selectedCandidateId || undefined,
        learnFromEdit: didUserEditReviewedText(),
        originalTextForLearning: didUserEditReviewedText()
          ? getOriginalReviewText()
          : undefined,
        insertTarget: "polished",
      });
      onClose();
    } catch (e) {
      console.error("Failed to insert polished text:", e);
    } finally {
      setIsSubmitting(false);
    }
  }, [
    getEditorText,
    initialData.history_id,
    onClose,
    isSubmitting,
    selectedCandidateId,
    didUserEditReviewedText,
    getOriginalReviewText,
  ]);

  // Insert English translation (reuses cached preview if source matches, otherwise backend re-translates)
  const handleInsertEnglish = useCallback(async () => {
    // Skill / AI Q&A outputs follow the skill's own insertion semantics —
    // never run them through the polish-flow English translation path.
    // Fall back to plain polished insert so Cmd+Enter still does *something*
    // useful instead of silently no-op'ing.
    const isSkillOutput =
      initialData.output_mode === "chat" || !!initialData.skill_name;
    if (isSkillOutput) {
      void handleInsertPolished();
      return;
    }

    const t0 = performance.now();
    const flog = (msg: string) => {
      void invoke("log_from_frontend", {
        source: "review",
        message: `[overlay-trace] ${msg} +${(performance.now() - t0).toFixed(1)}ms`,
      }).catch(() => {});
    };
    flog("handleInsertEnglish ENTER");
    const currentText = getEditorText();
    if (isSubmitting || !currentText.trim()) return;

    setIsSubmitting(true);

    // Overlay only appears while translation is actually happening. If
    // translation isn't enabled for the target app, the insert is silent
    // (matching "pop-up exists only during active translation").
    if (translationEnabled) {
      flog("invoking show_review_translation_overlay");
      void invoke("show_review_translation_overlay")
        .then(() => flog("show_review_translation_overlay RESOLVED"))
        .catch(() => {});
    } else {
      flog("skipping overlay (translationEnabled=false)");
    }

    try {
      flog("invoking confirm_reviewed_transcription");
      await invoke("confirm_reviewed_transcription", {
        text: currentText.trim(),
        historyId: initialData.history_id,
        cachedModelId: selectedCandidateId || undefined,
        learnFromEdit: didUserEditReviewedText(),
        originalTextForLearning: didUserEditReviewedText()
          ? getOriginalReviewText()
          : undefined,
        ...buildTranslationInsertPayload(currentText.trim()),
        insertTarget: "english",
      });
      flog("confirm_reviewed_transcription RESOLVED");
      onClose();
    } catch (e) {
      console.error("Failed to insert english text:", e);
    } finally {
      setIsSubmitting(false);
    }
  }, [
    getEditorText,
    initialData.history_id,
    initialData.output_mode,
    initialData.skill_name,
    handleInsertPolished,
    onClose,
    isSubmitting,
    selectedCandidateId,
    didUserEditReviewedText,
    getOriginalReviewText,
    buildTranslationInsertPayload,
  ]);

  // Insert original ASR text directly
  const handleInsertOriginal = useCallback(async () => {
    if (isSubmitting || !initialData.source_text.trim()) return;

    setIsSubmitting(true);

    try {
      await invoke("confirm_reviewed_transcription", {
        text: initialData.source_text.trim(),
        historyId: initialData.history_id,
        cachedModelId: undefined,
        learnFromEdit: false,
        insertTarget: "asr_original",
      });
      onClose();
    } catch (e) {
      console.error("Failed to insert original text:", e);
    } finally {
      setIsSubmitting(false);
    }
  }, [initialData.source_text, initialData.history_id, onClose, isSubmitting]);

  // Direct insert for a specific candidate text (one-click from hover button)
  const handleDirectInsert = useCallback(
    async (text: string, candidateId?: string) => {
      if (isSubmitting || !text.trim()) return;
      setIsSubmitting(true);
      try {
        await invoke("confirm_reviewed_transcription", {
          text: text.trim(),
          historyId: initialData.history_id,
          cachedModelId: candidateId || undefined,
          learnFromEdit: false,
          insertTarget: "polished",
        });
        onClose();
      } catch (e) {
        console.error("Failed to insert text:", e);
      } finally {
        setIsSubmitting(false);
      }
    },
    [initialData.history_id, onClose, isSubmitting],
  );

  const requestEnglishTranslation = useCallback(
    async (sourceText: string) => {
      const trimmedText = sourceText.trim();
      if (!trimmedText) {
        setTranslatedText(null);
        setTranslatedSourceText(null);
        setTranslationError(null);
        setTranslationStatus("idle");
        return;
      }

      const requestId = translationRequestIdRef.current + 1;
      translationRequestIdRef.current = requestId;
      setIsTranslating(true);
      setTranslationStatus("loading");
      setTranslationError(null);

      try {
        const result = await invoke<{ translated_text: string }>(
          "translate_text_to_english_command",
          {
            text: trimmedText,
          },
        );

        if (translationRequestIdRef.current !== requestId) return;

        const nextTranslated = result.translated_text.trim();
        if (!nextTranslated) {
          setTranslatedText(null);
          setTranslatedSourceText(null);
          setTranslationError(
            t(
              "transcription.review.translationFailedFallback",
              "翻译失败，插入时将回退原文",
            ),
          );
          setTranslationStatus("error");
          return;
        }

        setTranslatedText(nextTranslated);
        setTranslatedSourceText(trimmedText);
        setTranslationError(null);
        setTranslationStatus("ready");
      } catch (e) {
        if (translationRequestIdRef.current !== requestId) return;
        console.error("Failed to translate text:", e);
        setTranslatedText(null);
        setTranslatedSourceText(null);
        setTranslationError(
          t(
            "transcription.review.translationFailedFallback",
            "翻译失败，插入时将回退原文",
          ),
        );
        setTranslationStatus("error");
      } finally {
        if (translationRequestIdRef.current === requestId) {
          setIsTranslating(false);
        }
      }
    },
    [t],
  );

  const handleInsertCandidateByIndex = useCallback(
    (shortcutIndex: number) => {
      if (!sortedCandidates || shortcutIndex < 1 || shortcutIndex > 5) return;
      const candidate = sortedCandidates[shortcutIndex - 1];
      if (!candidate || !candidate.ready || candidate.error) return;
      const text = (editedTexts[candidate.id] ?? candidate.text).trim();
      if (!text) return;
      void handleDirectInsert(text, candidate.id);
    },
    [editedTexts, handleDirectInsert, sortedCandidates],
  );

  // Translate current text with auto language detection and analysis
  const handleTranslate = useCallback(async () => {
    const currentText = getEditorText();
    if (isTranslating || !currentText.trim()) return;

    await requestEnglishTranslation(currentText.trim());
  }, [getEditorText, isTranslating, requestEnglishTranslation]);

  // Keep refs updated. Shortcut semantics:
  //   Ctrl+Enter  → insert polished ("Insert" — fixed bottom-right button)
  //   Cmd+Enter   → insert English when translation is enabled and ready;
  //                 otherwise focus the polish editor (activate editing).
  useEffect(() => {
    insertOriginalRef.current = handleInsertOriginal;
    insertEnglishRef.current = handleInsertEnglish;
    insertPolishedRef.current = handleInsertPolished;
    metaEnterRef.current = () => {
      // Cmd+Enter inserts English when a translation is actually available
      // to insert (either via app-profile intent or manual Cmd+T). Otherwise
      // Cmd+Enter IS the polish insert shortcut — no English block exists,
      // so Cmd owns the main insert.
      const translationUsable =
        translationEnabled || translationStatus !== "idle" || !!translatedText;
      if (translationUsable) {
        handleInsertEnglish();
      } else {
        handleInsertPolished();
      }
    };
  }, [
    handleInsertPolished,
    handleInsertEnglish,
    handleInsertOriginal,
    translationEnabled,
    translationStatus,
    translatedText,
  ]);

  // Cmd+T / Ctrl+T — manual on-demand translation. When translationIntended
  // is false, this is the only way to produce the English preview + unlock
  // Ctrl+Enter for inserting the translation.
  useEffect(() => {
    translateRef.current = handleTranslate;
  }, [handleTranslate]);

  const [hasRewriteApplied, setHasRewriteApplied] = useState(false);

  // Track whether user has made any edits (manual or voice rewrite)
  const hasEdits = useMemo(() => {
    if (hasRewriteApplied) return true;
    if (Object.keys(editedTexts).length > 0) return true;
    // Check if new candidates were added (voice rewrite)
    const originalCount = multiCandidates?.length ?? 0;
    const currentCount = localCandidates?.length ?? 0;
    return currentCount > originalCount;
  }, [hasRewriteApplied, editedTexts, multiCandidates, localCandidates]);

  const [pendingClose, setPendingClose] = useState(false);

  // Auto-reset pendingClose after timeout
  useEffect(() => {
    if (!pendingClose) return;
    const timer = setTimeout(() => setPendingClose(false), 2000);
    return () => clearTimeout(timer);
  }, [pendingClose]);

  const doClose = useCallback(() => {
    if (isSubmitting) return;
    const trimmed = getEditorText().trim();
    const historyId = initialData.history_id;
    onClose();

    void (async () => {
      try {
        await invoke("cancel_transcription_review", {
          text: trimmed.length > 0 ? trimmed : null,
          historyId,
        });
      } catch (e) {
        console.error("Failed to cancel review:", e);
      }
    })();
  }, [onClose, isSubmitting, initialData.history_id, getEditorText]);

  const handleCancel = useCallback(() => {
    if (isSubmitting) return;
    if (hasEdits && !pendingClose) {
      setPendingClose(true);
      return;
    }
    doClose();
  }, [isSubmitting, hasEdits, pendingClose, doClose]);

  // Keep refs updated
  useEffect(() => {
    cancelRef.current = handleCancel;
  }, [handleCancel]);

  // Navigate between ready candidates in multi-model mode
  const getNextReadyCandidate = useCallback(
    (direction: 1 | -1): string | null => {
      if (!sortedCandidates) return null;
      const ready = sortedCandidates.filter((c) => c.ready && !c.error);
      if (ready.length === 0) return null;
      const currentIdx = ready.findIndex((c) => c.id === selectedCandidateId);
      const nextIdx = (currentIdx + direction + ready.length) % ready.length;
      return ready[nextIdx].id;
    },
    [sortedCandidates, selectedCandidateId],
  );

  // In multi-model mode, Tiptap editor is not rendered so its keyboard
  // shortcuts don't fire. Register a global keydown listener with
  // two-level focus model (List Mode / Edit Mode).
  useEffect(() => {
    if (!sortedCandidates || sortedCandidates.length === 0) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.metaKey) {
        setShowCandidateShortcutHints(true);
      }

      if (e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
        const digit = Number.parseInt(e.key, 10);
        if (Number.isInteger(digit) && digit >= 1 && digit <= 5) {
          e.preventDefault();
          handleInsertCandidateByIndex(digit);
          return;
        }
      }

      // Tab: insert original ASR text
      if (e.key === "Tab" && !e.shiftKey) {
        e.preventDefault();
        insertOriginalRef.current();
        return;
      }

      // Cmd+Enter: route through the same "is translation in play?" decision
      // as single-model mode. When the app profile doesn't request English
      // and the user hasn't manually triggered a translation, ⌘⏎ is the
      // main insert (polished / selected candidate) — no translation pass.
      // When translation IS in play, it inserts English.
      if (e.metaKey && !e.ctrlKey && e.key === "Enter") {
        e.preventDefault();
        metaEnterRef.current();
        return;
      }

      // Ctrl+Enter: insert polished text
      if (e.ctrlKey && !e.metaKey && e.key === "Enter") {
        e.preventDefault();
        insertPolishedRef.current();
        return;
      }

      if (editingCandidateId === null) {
        // === List Mode ===
        if (e.key === "ArrowDown") {
          e.preventDefault();
          const next = getNextReadyCandidate(1);
          if (next) setSelectedCandidateId(next);
        } else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
          e.preventDefault();
          const prev = getNextReadyCandidate(-1);
          if (prev) setSelectedCandidateId(prev);
        } else if (e.key === "Enter") {
          e.preventDefault();
          if (selectedCandidateId) {
            setEditingCandidateId(selectedCandidateId);
          }
        } else if (e.key === "Escape") {
          e.preventDefault();
          cancelRef.current();
        }
      } else {
        // === Edit Mode ===
        if (e.key === "Escape") {
          e.preventDefault();
          setEditingCandidateId(null);
        }
        // All other keys pass through to the focused textarea
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Meta") {
        setShowCandidateShortcutHints(false);
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("keyup", handleKeyUp);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("keyup", handleKeyUp);
    };
  }, [
    handleInsertCandidateByIndex,
    sortedCandidates,
    editingCandidateId,
    selectedCandidateId,
    getNextReadyCandidate,
  ]);

  useEffect(() => {
    const handleWindowBlur = () => setShowCandidateShortcutHints(false);
    window.addEventListener("blur", handleWindowBlur);
    return () => window.removeEventListener("blur", handleWindowBlur);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const setupListener = async () => {
      const detach = await listen<string>(
        REVIEW_WINDOW_INLINE_APPLY,
        (event) => {
          applyInlineTextToEditor(event.payload);
        },
      );

      if (disposed) {
        detach();
        return;
      }

      unlisten = detach;
    };

    void setupListener();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyInlineTextToEditor]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const setupListener = async () => {
      const detach = await listen<{ text: string; model?: string }>(
        REVIEW_WINDOW_REWRITE_APPLY,
        (event) => {
          const { text, model } = event.payload;
          setShowDiff(true);
          setHasRewriteApplied(true);
          if (isMultiCandidateMode.current) {
            // In multi-candidate mode: replace the selected candidate's text with the rewrite result
            const targetId = selectedCandidateIdRef.current;
            if (targetId) {
              setLocalCandidates((prev) => {
                if (!prev) return prev;
                return prev.map((c) =>
                  c.id === targetId
                    ? {
                        ...c,
                        text,
                        label: model || c.label,
                        ready: true,
                      }
                    : c,
                );
              });
              // Clear any manual edits for this candidate
              setEditedTexts((prev) => {
                const next = { ...prev };
                delete next[targetId];
                return next;
              });
              // Sync updated text to backend for next rewrite
              void invoke("set_review_editor_content_state", { text }).catch(
                (e) => {
                  console.error("Failed to sync rewrite result to backend:", e);
                },
              );
              setTranslationSourceText(text);
            }
          } else {
            replaceEditorDocument(text);
            // Sync the new content to backend immediately so next rewrite uses it as target
            void invoke("set_review_editor_content_state", { text }).catch(
              (e) => {
                console.error("Failed to sync rewrite result to backend:", e);
              },
            );
            setTranslationSourceText(text);
          }
        },
      );

      if (disposed) {
        detach();
        return;
      }

      unlisten = detach;
    };

    void setupListener();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [replaceEditorDocument]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const setupListener = async () => {
      const detach = await listen(VOTYPE_REFOCUS_ACTIVE_INPUT, () => {
        if (!editor || isMultiCandidateMode.current) return;
        editor.commands.focus();
      });

      if (disposed) {
        detach();
        return;
      }

      unlisten = detach;
    };

    void setupListener();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [editor]);

  useEffect(() => {
    const syncEditorActive = (active: boolean) => {
      void invoke("set_review_editor_active_state", { active }).catch((e) => {
        console.error("Failed to sync review editor active state:", e);
      });
    };

    const syncEditorContent = () => {
      let text: string | undefined;
      if (isMultiCandidateMode.current) {
        // In multi-candidate mode: sync the selected candidate's text
        const id = selectedCandidateIdRef.current;
        if (id) {
          const candidates = localCandidatesRef.current;
          const candidate = candidates?.find((c) => c.id === id);
          if (candidate) {
            text = editedTextsRef.current[id] ?? candidate.text;
          }
        }
      } else if (editor) {
        text = editor.getText({ blockSeparator: "\n" });
      }
      if (text !== undefined) {
        setTranslationSourceText(text);
        void invoke("set_review_editor_content_state", { text }).catch((e) => {
          console.error("Failed to sync review editor content state:", e);
        });
      }
    };

    const updateEditorStateFromTarget = (target: EventTarget | null) => {
      const editorActive = isEditableTarget(target);

      if (editorActive && editor) {
        pendingInlineRangeRef.current = {
          from: editor.state.selection.from,
          to: editor.state.selection.to,
        };
      }

      syncEditorActive(editorActive);
    };

    const handleFocusIn = (event: FocusEvent) => {
      updateEditorStateFromTarget(event.target);
    };

    const handleSelectionChange = () => {
      updateEditorStateFromTarget(document.activeElement);
    };

    const handleWindowBlur = () => {
      syncEditorActive(false);
    };

    updateEditorStateFromTarget(document.activeElement);
    syncEditorContent();

    document.addEventListener("focusin", handleFocusIn, true);
    document.addEventListener("selectionchange", handleSelectionChange);
    window.addEventListener("blur", handleWindowBlur);

    let offUpdate: (() => void) | undefined;
    if (editor) {
      const handler = () => {
        syncEditorContent();
      };
      editor.on("update", handler);
      offUpdate = () => editor.off("update", handler);
    }

    return () => {
      syncEditorActive(false);
      offUpdate?.();
      document.removeEventListener("focusin", handleFocusIn, true);
      document.removeEventListener("selectionchange", handleSelectionChange);
      window.removeEventListener("blur", handleWindowBlur);
    };
  }, [editor]);

  // Sync selected candidate content to backend when selection or candidates change.
  // This covers: initial selection, candidate switching, and candidate completion
  // (when localCandidates updates with ready text but selectedCandidateId stays the same).
  useEffect(() => {
    if (!isMultiCandidateMode.current || !selectedCandidateId) return;
    const candidate = localCandidates?.find(
      (c) => c.id === selectedCandidateId,
    );
    if (candidate?.ready && candidate.text) {
      const text =
        editedTextsRef.current[selectedCandidateId] ?? candidate.text;
      setTranslationSourceText(text);
      void invoke("set_review_editor_content_state", { text }).catch((e) => {
        console.error("Failed to sync candidate content:", e);
      });
    }
  }, [selectedCandidateId, localCandidates]);

  // Auto-translate on editor blur (not on every keystroke). The previous 1s
  // debounce fired while the user was still mid-edit, which felt too eager
  // and generated wasted LLM calls. Now translation kicks in only when the
  // user's intent has settled — either they clicked away from the editor or
  // they explicitly pressed Cmd+T.
  useEffect(() => {
    const isSkillOutput =
      initialData.output_mode === "chat" || !!initialData.skill_name;
    if (isSkillOutput) {
      setTranslatedText(null);
      setTranslatedSourceText(null);
      setTranslationError(null);
      setTranslationStatus("idle");
      setIsTranslating(false);
      return;
    }
    if (!translationEnabled) return;
    if (!editor) return;

    const runIfChanged = () => {
      const trimmed = editor.getText().trim();
      if (!trimmed) return;
      if (trimmed === translatedSourceText) return;
      setTranslationStatus("loading");
      void requestEnglishTranslation(trimmed);
    };

    editor.on("blur", runIfChanged);
    return () => {
      editor.off("blur", runIfChanged);
    };
  }, [
    editor,
    requestEnglishTranslation,
    translationEnabled,
    translatedSourceText,
    initialData.output_mode,
    initialData.skill_name,
  ]);

  // When the source text becomes empty (e.g. user clears the editor), reset
  // translation state immediately — no waiting for blur.
  useEffect(() => {
    if (!translationEnabled) return;
    if (translationSourceText.trim()) return;
    setTranslatedText(null);
    setTranslatedSourceText(null);
    setTranslationError(null);
    setTranslationStatus("idle");
    setIsTranslating(false);
  }, [translationEnabled, translationSourceText]);

  // Kick off auto-translate on window open AND after a programmatic reset
  // (prompt rerun / model change) — the blur listener above only fires once
  // the user has focused AND left the editor, which doesn't happen for these
  // non-interactive transitions. The ref makes the fire one-shot per reset
  // cycle; the companion effect below re-arms it whenever translation state
  // is cleared so subsequent reruns re-translate automatically.
  const didInitialTranslateRef = useRef(false);
  useEffect(() => {
    if (translationStatus === "idle" && !translatedText) {
      didInitialTranslateRef.current = false;
    }
  }, [translationStatus, translatedText]);

  // Multi-candidate: switching the selected candidate invalidates the
  // current translation (it was for the previous candidate's text). Clear
  // translation state so the auto-translate effect picks up the new
  // candidate's source on the next tick.
  useEffect(() => {
    if (!isMultiCandidateMode.current) return;
    setTranslatedText(null);
    setTranslatedSourceText(null);
    setTranslationError(null);
    setTranslationStatus("idle");
    setIsTranslating(false);
  }, [selectedCandidateId]);
  useEffect(() => {
    if (didInitialTranslateRef.current) return;
    if (!translationEnabled) return;
    const isSkillOutput =
      initialData.output_mode === "chat" || !!initialData.skill_name;
    if (isSkillOutput) return;
    const trimmed = translationSourceText.trim();
    if (!trimmed) return;
    didInitialTranslateRef.current = true;
    setTranslationStatus("loading");
    void requestEnglishTranslation(trimmed);
  }, [
    translationEnabled,
    translationSourceText,
    requestEnglishTranslation,
    initialData.output_mode,
    initialData.skill_name,
  ]);

  // Re-measure whenever the preview card's content changes. Use a double rAF
  // so layout from the React commit has fully settled — a plain setTimeout(16)
  // sometimes fired before the translation block's first paint landed, which
  // made the resize under-shoot on the first render (preview border hugging
  // the window's bottom edge until the next translation event corrected it).
  useEffect(() => {
    let raf1 = 0;
    let raf2 = 0;
    raf1 = window.requestAnimationFrame(() => {
      raf2 = window.requestAnimationFrame(() => {
        void measureAndResize(false);
      });
    });
    return () => {
      if (raf1) window.cancelAnimationFrame(raf1);
      if (raf2) window.cancelAnimationFrame(raf2);
    };
  }, [
    measureAndResize,
    translationEnabled,
    translatedText,
    translationError,
    translationStatus,
  ]);

  // Observe layout-relevant containers so any user edit (typing, pasting,
  // candidate switch, diff toggle) triggers an immediate window resize, keeping
  // all content visible without leaving transparent-but-clickable dead space.
  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === "undefined") return;

    let rafId: number | null = null;
    const schedule = () => {
      if (rafId !== null) return;
      rafId = window.requestAnimationFrame(() => {
        rafId = null;
        void measureAndResize(false);
      });
    };

    const observer = new ResizeObserver(schedule);
    const selectors = [
      ".review-header",
      ".review-footer",
      ".review-panel-source",
      ".review-panel-output",
      ".review-content-area",
      ".review-section-no-title",
      ".review-multi-content",
      ".review-preview-dock",
      ".review-translation-float",
    ];
    for (const selector of selectors) {
      const el = container.querySelector(selector);
      if (el) observer.observe(el);
    }

    return () => {
      if (rafId !== null) window.cancelAnimationFrame(rafId);
      observer.disconnect();
    };
  }, [
    measureAndResize,
    initialData.output_mode,
    translationEnabled,
    sortedCandidates?.length,
  ]);

  const handleDrag = useCallback(async () => {
    try {
      const appWindow = getCurrentWindow();
      await appWindow.startDragging();
    } catch (e) {
      console.error("Failed to start dragging:", e);
    }
  }, []);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(getEditorText());
    } catch (err) {
      console.error("Failed to copy text: ", err);
    }
  }, [getEditorText]);

  const handlePromptRerunReset = useCallback(
    (promptId: string, candidates: MultiModelCandidate[]) => {
      setSelectedPromptId(promptId);
      setLocalCandidates(candidates);
      setSelectedCandidateId(null);
      setEditingCandidateId(null);
      setEditedTexts({});
      setIsRerunning(true);
    },
    [],
  );

  const isRealMultiModel =
    multiCandidates != null && multiCandidates.length > 1;

  // Skill/chat outputs route Cmd+Enter through the polish-insert path (see
  // handleInsertEnglish's skill fallback). The translation-enabled setting
  // can still be true from the user's profile, but English preview / English
  // insert aren't meaningful in this mode. Use this derived flag to drive
  // rendering so the footer Insert (which Cmd+Enter actually triggers)
  // styles as primary and no spurious "ENGLISH" preview appears.
  const isSkillOutput =
    initialData.output_mode === "chat" || !!initialData.skill_name;
  const translationActive =
    !isSkillOutput &&
    (translationEnabled || translationStatus !== "idle" || !!translatedText);

  const getHeaderMode = (): "multi" | "polish" | "chat" => {
    // Only use "multi" header for actual multi-model results (2+ candidates)
    if (isRealMultiModel) return "multi";
    if (initialData.output_mode === "chat") return "chat";
    return "polish";
  };

  // Per-mode root class so CSS can target a single mode without relying on
  // implicit markers like the presence of `.review-section-no-title`.
  // Yields: `polish-window`, `chat-window`, or `multi-window`.
  const modeClass = `${getHeaderMode()}-window`;

  return (
    <div
      className="w-screen h-screen flex flex-col items-stretch p-0 box-border overflow-hidden bg-transparent"
      ref={containerRef}
    >
      <div
        className={`review-window-container ${modeClass}`}
        data-review-manual={manualSize ? "true" : undefined}
      >
        <div
          className="cursor-grab select-none"
          onPointerDown={(e) => {
            if (e.button !== 0) return;
            // Only trigger dragging if the target (or its parent) isn't the close button
            if (!(e.target as HTMLElement).closest(".review-close-button")) {
              e.preventDefault();
              handleDrag();
            }
          }}
        >
          <ReviewHeader
            mode={getHeaderMode()}
            skillName={initialData.skill_name}
            prompts={prompts}
            selectedPromptId={selectedPromptId}
            modelOptions={modelOptions}
            selectedModelId={selectedModelId}
            defaultModelLabel={defaultModelLabel}
            sourceText={initialData.source_text}
            historyId={initialData.history_id}
            editor={editor}
            showDiff={showDiff}
            onShowDiffChange={setShowDiff}
            onPromptChange={(promptId) => {
              // Update local state to sync dropdown display
              setSelectedPromptId(promptId);

              // Clear translation panel when changing prompt
              setTranslatedText(null);
              setTranslatedSourceText(null);
              setTranslationError(null);
              setTranslationStatus("idle");

              if (isRealMultiModel) {
                // Multi mode (2+ candidates): clear old results immediately
                handlePromptRerunReset(
                  promptId,
                  multiCandidates.map((c) => ({
                    ...c,
                    text: "",
                    confidence: undefined,
                    processing_time_ms: 0,
                    error: undefined,
                    ready: false,
                  })),
                );
              }
            }}
            onModelChange={setSelectedModelId}
            onCancel={handleCancel}
            onInsertOriginal={handleInsertOriginal}
            onTranslate={handleTranslate}
            isTranslating={isTranslating}
            onSourceHtmlChange={setSourceHtml}
            onModelNameChange={setCurrentModelName}
            onRerunStart={() => {
              setIsRerunning(true);
              // Clear translation panel when rerunning
              setTranslatedText(null);
              setTranslatedSourceText(null);
              setTranslationError(null);
              setTranslationStatus("idle");
            }}
            onRerunEnd={() => setIsRerunning(false)}
            onMeasureAndResize={measureAndResize}
            onRerunResult={(text: string) => {
              const cp = computeChangePercent(initialData.source_text, text);
              setShowDiff(Math.abs(cp) < DIFF_THRESHOLD);
              replaceEditorDocument(text);
            }}
            multiSortMode={multiSortMode}
            onMultiSortModeChange={setMultiSortMode}
            selectedCandidateLabel={selectedCandidateLabel}
            pressedModifier={pressedModifier}
          />
        </div>

        {sortedCandidates && sortedCandidates.length > 0 ? (
          <MultiCandidateView
            sourceText={initialData.source_text}
            showDiff={showDiff}
            candidates={sortedCandidates}
            showShortcutHints={showCandidateShortcutHints}
            rankStats={speedRankStats}
            selectedCandidateId={selectedCandidateId}
            selectedCandidateText={
              selectedCandidateId
                ? (editedTexts[selectedCandidateId] ??
                  sortedCandidates.find((c) => c.id === selectedCandidateId)
                    ?.text ??
                  null)
                : null
            }
            editingCandidateId={editingCandidateId}
            editedTexts={editedTexts}
            onCandidateSelect={setSelectedCandidateId}
            onEditEnd={() => setEditingCandidateId(null)}
            onTextChange={(candidateId, text) => {
              setEditedTexts((prev) => ({ ...prev, [candidateId]: text }));
              if (candidateId === selectedCandidateId) {
                setTranslationSourceText(text);
              }
            }}
            onInsert={handleDirectInsert}
            onInsertOriginal={handleInsertOriginal}
            pressedModifier={pressedModifier}
            insertModifier={translationActive ? "ctrl" : "meta"}
          />
        ) : initialData.output_mode !== "chat" ? (
          <DiffViewPanel
            sourceHtml={sourceHtml}
            editor={editor}
            isRerunning={isRerunning}
            currentModelName={currentModelName}
            changeStats={singleModelChangeStats}
            onInsertOriginal={handleInsertOriginal}
            onInsertPolished={handleInsertPolished}
            // Polish insert shortcut flips once a translation is in play
            // (either by app-profile intent or after the user pressed ⌘T).
            // In that world ⌘⏎ is reserved for English, so polish uses
            // Ctrl+Enter; otherwise ⌘⏎ IS the polish insert.
            insertShortcut={
              translationActive ? (isMac ? "⌃⏎" : "Ctrl⏎") : isMac ? "⌘⏎" : "⊞⏎"
            }
            isSubmitting={isSubmitting}
            translationIntended={translationActive}
            pressedModifier={pressedModifier}
          />
        ) : (
          <div className="review-content-area">
            <div className="review-section review-section-final review-section-no-title">
              <ScrollArea
                type="hover"
                scrollbars="vertical"
                className="review-section-scroll"
              >
                <div
                  className="review-section-content"
                  onMouseDown={(event) => {
                    if (!(event.target instanceof HTMLElement)) {
                      return;
                    }

                    if (event.target.closest("button")) {
                      return;
                    }

                    if (event.target.closest(".ProseMirror")) {
                      return;
                    }
                    event.preventDefault();
                    editor?.commands.focus();
                  }}
                >
                  <EditorContent editor={editor} className="flex-1 min-h-0" />
                </div>
              </ScrollArea>
            </div>
          </div>
        )}

        {pendingClose && (
          <div className="review-pending-close-toast">
            {t(
              "transcription.review.pressEscAgain",
              "内容已修改，再次按 ESC 关闭",
            )}
          </div>
        )}

        <ReviewFooter
          reason={initialData.reason}
          outputMode={initialData.output_mode}
          isSubmitting={isSubmitting}
          hasText={!!getEditorText().trim()}
          // Footer Insert shortcut flips by translation presence — same rule
          // as the polish card: ⌘⏎ when there is no English (footer owns the
          // main insert), ⌃⏎ when English is in play (⌘⏎ reserved for the
          // English preview's Insert).
          insertShortcut={
            translationActive ? (isMac ? "⌃⏎" : "Ctrl⏎") : isMac ? "⌘⏎" : "⊞⏎"
          }
          // ⌃⏎ → secondary styling (no solid fill). ⌘⏎ (no-translation
          // case) keeps primary because it IS the main insert action.
          insertVariant={translationActive ? "secondary" : "primary"}
          // NeonBorder preview fires on the modifier actually bound to the
          // footer: ⌃ when translation is live, ⌘ otherwise.
          insertArmed={
            pressedModifier === (translationActive ? "ctrl" : "meta")
          }
          // Polish single-model view: Insert lives inside DiffViewPanel, so the
          // footer must not render a duplicate Insert button.
          hidePrimaryInsert={
            initialData.output_mode !== "chat" &&
            !(sortedCandidates && sortedCandidates.length > 0)
          }
          onCopy={handleCopy}
          onInsert={handleInsertPolished}
        />

        {translationActive && (
          <div className="review-preview-dock">
            <div
              className="review-translation-float"
              data-mod-armed={
                pressedModifier === "meta" &&
                translationStatus === "ready" &&
                !!translatedText
                  ? "true"
                  : undefined
              }
            >
              {pressedModifier === "meta" &&
                translationStatus === "ready" &&
                !!translatedText && (
                  <NeonBorder
                    radius={10}
                    gradientId="review-neon-gradient-translation-card"
                    strokeWidth={2.8}
                    durationSec={3.2}
                  />
                )}
              <div className="review-translation-float-header">
                <span className="review-translation-title-row">
                  <span
                    className={`review-translation-status-dot review-translation-status-dot--${translationStatus}`}
                    aria-hidden="true"
                  />
                  <span className="review-translation-title">
                    {t(
                      "transcription.review.translationPreviewLangLabel",
                      "ENGLISH",
                    )}
                  </span>
                  <span
                    className="review-translation-title-sep"
                    aria-hidden="true"
                  >
                    ·
                  </span>
                  <span className="review-translation-title-mode">
                    {translationEnabled
                      ? t("transcription.review.translationModeAuto", "AUTO")
                      : t(
                          "transcription.review.translationModeManual",
                          "MANUAL",
                        )}
                  </span>
                </span>
                <button
                  type="button"
                  className="review-btn-primary review-translation-insert-btn"
                  onClick={handleInsertEnglish}
                  title={t("transcription.review.insert", "插入")}
                  data-mod-armed={
                    pressedModifier === "meta" ? "true" : undefined
                  }
                >
                  {pressedModifier === "meta" && (
                    <NeonBorder
                      radius={6}
                      gradientId="review-neon-gradient-translation"
                      strokeWidth={2.0}
                    />
                  )}
                  {t("transcription.review.insert", "插入")}
                  <span
                    className="review-shortcut-hint"
                    data-mod-armed={
                      pressedModifier === "meta" ? "true" : undefined
                    }
                  >
                    {isMac ? "⌘⏎" : "⊞⏎"}
                  </span>
                </button>
              </div>
              <div className="review-translation-float-content">
                {translatedText ||
                  translationError ||
                  t("transcription.review.translationUpdating", "翻译中...")}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};

export default ReviewWindow;
