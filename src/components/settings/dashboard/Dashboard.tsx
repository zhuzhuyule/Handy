import { Box, Flex, Heading, Switch, Text } from "@radix-ui/themes";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { DashboardActivityChart } from "./DashboardActivityChart";
import { DashboardSummaryCards } from "./DashboardSummaryCards";
import { VirtualDetailsList } from "./VirtualDetailsList";
import type { DashboardSelection, HistoryEntry } from "./dashboardTypes";
import {
  countUnicodeChars,
  formatDurationMs,
  toLocalYmd,
} from "./dashboardUtils";

export const Dashboard: React.FC = () => {
  const { t } = useTranslation();
  const PAGE_SIZE = 10;

  // All entries for charts/summary (loaded once)
  const [allEntries, setAllEntries] = useState<HistoryEntry[]>([]);
  // Paginated entries for detail list only
  const [detailEntries, setDetailEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [selection, setSelection] = useState<DashboardSelection>(() => ({
    type: "day",
    day: toLocalYmd(new Date()),
  }));
  const audioUrlCacheRef = useRef<Map<string, string | null>>(new Map());
  const [errorsOnly, setErrorsOnly] = useState(false);

  const numberFormat = useMemo(() => new Intl.NumberFormat(), []);

  // Track today's date and auto-update when day changes
  const [todayYmd, setTodayYmd] = useState(() => toLocalYmd(new Date()));

  // Check for date change periodically (every minute)
  useEffect(() => {
    const checkDateChange = () => {
      const newToday = toLocalYmd(new Date());
      if (newToday !== todayYmd) {
        setTodayYmd(newToday);
        // If user was viewing "today", update selection to the new today
        if (selection.type === "day" && selection.day === todayYmd) {
          setSelection({ type: "day", day: newToday });
        }
      }
    };

    const interval = setInterval(checkDateChange, 60000); // Check every minute
    return () => clearInterval(interval);
  }, [todayYmd, selection]);

  // Pagination state for detail list
  const [detailTotalCount, setDetailTotalCount] = useState(0);
  const [hasMore, setHasMore] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);

  // Calculate timestamp range based on selection
  const getTimestampRange = useCallback((sel: DashboardSelection) => {
    const now = new Date();
    let startTs: number | undefined;
    let endTs: number | undefined;

    if (sel.type === "day") {
      const dayDate = new Date(sel.day + "T00:00:00");
      startTs = Math.floor(dayDate.getTime() / 1000);
      const nextDay = new Date(dayDate);
      nextDay.setDate(nextDay.getDate() + 1);
      endTs = Math.floor(nextDay.getTime() / 1000) - 1;
    } else if (sel.preset !== "all") {
      const days = sel.preset === "7d" ? 7 : sel.preset === "30d" ? 30 : 40;
      const startDate = new Date(now);
      startDate.setDate(startDate.getDate() - days);
      startDate.setHours(0, 0, 0, 0);
      startTs = Math.floor(startDate.getTime() / 1000);
    }

    return { startTs, endTs };
  }, []);

  // Load paginated detail entries
  const loadDetailPage = useCallback(
    async (offset: number, sel: DashboardSelection, reset = false) => {
      // Only show loading indicator for non-reset (load more) requests
      if (!reset) setIsLoadingMore(true);
      try {
        const { startTs, endTs } = getTimestampRange(sel);
        const result = await invoke<{
          entries: HistoryEntry[];
          total_count: number;
          offset: number;
          limit: number;
        }>("get_history_entries_paginated", {
          offset,
          limit: PAGE_SIZE,
          startTimestamp: startTs ?? null,
          endTimestamp: endTs ?? null,
        });

        setDetailTotalCount(result.total_count);
        setHasMore(result.offset + result.entries.length < result.total_count);

        if (reset) {
          setDetailEntries(result.entries);
        } else {
          setDetailEntries((prev) => {
            const existingIds = new Set(prev.map((e) => e.id));
            const newEntries = result.entries.filter(
              (e) => !existingIds.has(e.id),
            );
            return [...prev, ...newEntries];
          });
        }
      } catch (e) {
        console.error("Failed to load detail entries:", e);
      } finally {
        setIsLoadingMore(false);
      }
    },
    [getTimestampRange],
  );

  // Keep a ref to the current selection for use in event listeners
  const selectionRef = useRef(selection);
  useEffect(() => {
    selectionRef.current = selection;
  }, [selection]);

  // Load all entries for charts (once on mount and on history-updated)
  useEffect(() => {
    let cancelled = false;
    const loadAllEntries = async () => {
      try {
        setLoading(true);
        const res = await invoke<HistoryEntry[]>("get_history_entries");
        if (!cancelled) setAllEntries(res);
      } catch (e) {
        console.error("Failed to load all history entries:", e);
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    loadAllEntries();

    let unlisten: (() => void) | null = null;
    (async () => {
      try {
        unlisten = await listen("history-updated", () => {
          loadAllEntries();
          // Also reload first page of details using current selection from ref
          loadDetailPage(0, selectionRef.current, true);
        });
      } catch (e) {
        console.error("Failed to listen for history-updated:", e);
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [loadDetailPage]);

  // Load first page of details when selection changes
  useEffect(() => {
    setDetailEntries([]);
    setHasMore(true);
    loadDetailPage(0, selection, true);
  }, [selection, loadDetailPage]);

  // Load more callback for VirtualDetailsList
  const handleLoadMore = useCallback(() => {
    if (!hasMore || loading) return;
    loadDetailPage(detailEntries.length, selection, false);
  }, [detailEntries.length, hasMore, loading, loadDetailPage, selection]);

  const chartDays = useMemo(() => {
    const days: string[] = [];
    for (let i = 39; i >= 0; i--) {
      const d = new Date();
      d.setDate(d.getDate() - i);
      days.push(toLocalYmd(d));
    }
    return days;
  }, []);

  const bucketsByDay = useMemo(() => {
    const map = new Map<string, { entries: number }>();
    for (const entry of allEntries) {
      const day = toLocalYmd(new Date(entry.timestamp * 1000));
      const bucket = map.get(day) ?? { entries: 0 };
      bucket.entries += 1;
      map.set(day, bucket);
    }
    return map;
  }, [allEntries]);

  const selectedDays = useMemo(() => {
    if (selection.type === "day") return new Set([selection.day]);
    if (selection.preset === "40d" || selection.preset === "all") {
      return new Set(chartDays);
    }
    if (selection.preset === "30d") {
      return new Set(chartDays.slice(-30));
    }
    return new Set(chartDays.slice(-7));
  }, [chartDays, selection]);

  const bars = useMemo(() => {
    const days = chartDays.map((day) => ({
      day,
      entries: bucketsByDay.get(day)?.entries ?? 0,
    }));
    const max = Math.max(1, ...days.map((d) => d.entries));
    return days.map((d) => ({
      ...d,
      heightPct: Math.round((d.entries / max) * 100),
      selected: selectedDays.has(d.day),
      isToday: d.day === todayYmd,
    }));
  }, [bucketsByDay, chartDays, selectedDays, todayYmd]);

  const selectionTitle = useMemo(() => {
    if (selection.type === "day") {
      if (selection.day === todayYmd) return t("dashboard.range.today");
      return selection.day;
    }
    if (selection.preset === "7d")
      return t("dashboard.range.lastNDays", { days: 7 });
    if (selection.preset === "30d")
      return t("dashboard.range.lastNDays", { days: 30 });
    if (selection.preset === "40d")
      return t("dashboard.range.lastNDays", { days: 40 });
    return t("dashboard.range.allTime");
  }, [selection, t, todayYmd]);

  // Selection title is derived from selection, no need for separate effect

  const selectedEntries = useMemo(() => {
    if (selection.type === "preset" && selection.preset === "all")
      return allEntries;
    const getDay = (entry: HistoryEntry) =>
      toLocalYmd(new Date(entry.timestamp * 1000));
    if (selection.type === "day") {
      return allEntries.filter(
        (e: HistoryEntry) => getDay(e) === selection.day,
      );
    }
    return allEntries.filter((e: HistoryEntry) => selectedDays.has(getDay(e)));
  }, [allEntries, selectedDays, selection]);

  const selectedDayTotals = useMemo(() => {
    const map = new Map<string, number>();
    for (const entry of selectedEntries) {
      const day = toLocalYmd(new Date(entry.timestamp * 1000));
      map.set(day, (map.get(day) ?? 0) + 1);
    }
    return map;
  }, [selectedEntries]);

  // Calculate previous period entries for trend comparison
  const previousPeriodEntries = useMemo(() => {
    if (selection.type === "preset" && selection.preset === "all") {
      return []; // No comparison for "all time"
    }

    const getDay = (entry: HistoryEntry) =>
      toLocalYmd(new Date(entry.timestamp * 1000));

    if (selection.type === "day") {
      // Compare with previous day
      const currentDate = new Date(selection.day + "T00:00:00");
      const prevDate = new Date(currentDate);
      prevDate.setDate(prevDate.getDate() - 1);
      const prevDay = toLocalYmd(prevDate);
      return allEntries.filter((e) => getDay(e) === prevDay);
    }

    // For preset ranges, compare with previous period of same length
    const days =
      selection.preset === "7d" ? 7 : selection.preset === "30d" ? 30 : 40;
    const now = new Date();
    const periodStart = new Date(now);
    periodStart.setDate(periodStart.getDate() - days);
    const prevPeriodStart = new Date(periodStart);
    prevPeriodStart.setDate(prevPeriodStart.getDate() - days);
    const prevPeriodEnd = new Date(periodStart);
    prevPeriodEnd.setDate(prevPeriodEnd.getDate() - 1);

    return allEntries.filter((e) => {
      const entryDate = new Date(e.timestamp * 1000);
      return entryDate >= prevPeriodStart && entryDate <= prevPeriodEnd;
    });
  }, [allEntries, selection]);

  // LLM usage stats from llm_call_log (replaces per-entry token_count/llm_call_count)
  const [llmUsagePeriod, setLlmUsagePeriod] = useState({
    total_calls: 0,
    total_tokens: 0,
  });
  const [llmUsagePrev, setLlmUsagePrev] = useState({
    total_calls: 0,
    total_tokens: 0,
  });

  type LlmUsage = { total_calls: number; total_tokens: number };
  const fetchLlmUsage = (since: number | null, until: number | null) =>
    invoke<LlmUsage>("get_llm_usage_stats", {
      sinceTimestamp: since,
      untilTimestamp: until,
    });

  // Fetch LLM usage stats when selection changes
  useEffect(() => {
    if (selection.type === "preset" && selection.preset === "all") {
      fetchLlmUsage(null, null)
        .then(setLlmUsagePeriod)
        .catch(() => {});
      setLlmUsagePrev({ total_calls: 0, total_tokens: 0 });
    } else {
      const now = Math.floor(Date.now() / 1000);

      if (selection.type === "day") {
        const dayDate = new Date(selection.day + "T00:00:00");
        const periodStart = Math.floor(dayDate.getTime() / 1000);
        const periodEnd = periodStart + 86400;
        const prevStart = periodStart - 86400;

        fetchLlmUsage(periodStart, periodEnd)
          .then(setLlmUsagePeriod)
          .catch(() => {});
        fetchLlmUsage(prevStart, periodStart)
          .then(setLlmUsagePrev)
          .catch(() => {});
      } else {
        const days =
          selection.preset === "7d" ? 7 : selection.preset === "30d" ? 30 : 40;
        const periodStart = now - days * 86400;
        const prevStart = periodStart - days * 86400;

        fetchLlmUsage(periodStart, null)
          .then(setLlmUsagePeriod)
          .catch(() => {});
        fetchLlmUsage(prevStart, periodStart)
          .then(setLlmUsagePrev)
          .catch(() => {});
      }
    }
  }, [selection]);

  // Helper to calculate summary from entries
  const calculateSummary = useCallback(
    (
      entries: HistoryEntry[],
      llmUsage: { total_calls: number; total_tokens: number },
    ) => {
      let entryCount = 0;
      let durationMs = 0;
      let charCount = 0;
      let transcriptionMs = 0;
      let savedCount = 0;
      let llmHits = 0;
      const appCounts = new Map<string, number>();

      for (const entry of entries) {
        entryCount += 1;
        durationMs += entry.duration_ms ?? 0;
        transcriptionMs += entry.transcription_ms ?? 0;

        if (entry.saved) savedCount += 1;
        if (entry.post_processed_text?.trim()) llmHits += 1;

        const appName = entry.app_name?.trim();
        if (appName) {
          appCounts.set(appName, (appCounts.get(appName) ?? 0) + 1);
        }

        if (typeof entry.char_count === "number") {
          charCount += entry.char_count;
        } else if (entry.transcription_text) {
          charCount += countUnicodeChars(entry.transcription_text);
        }
      }

      const llmCalls = llmUsage.total_calls;
      const totalTokens = llmUsage.total_tokens;
      const rtf = durationMs > 0 ? transcriptionMs / durationMs : 0;
      const llmHitRate = llmCalls > 0 ? llmHits / llmCalls : 0;
      const charsPerMinute =
        durationMs > 0 ? (charCount / durationMs) * 60_000 : 0;
      const topApps = Array.from(appCounts.entries())
        .sort((a, b) => b[1] - a[1])
        .slice(0, 3);

      return {
        entryCount,
        durationMs,
        charCount,
        transcriptionMs,
        rtf,
        savedCount,
        llmCalls,
        llmHits,
        llmHitRate,
        charsPerMinute,
        topApps,
        totalTokens,
      };
    },
    [],
  );

  const summary = useMemo(
    () => calculateSummary(selectedEntries, llmUsagePeriod),
    [calculateSummary, selectedEntries, llmUsagePeriod],
  );

  const previousSummary = useMemo(
    () => calculateSummary(previousPeriodEntries, llmUsagePrev),
    [calculateSummary, previousPeriodEntries, llmUsagePrev],
  );

  // Calculate trend percentages
  const trends = useMemo(() => {
    const calcTrend = (current: number, previous: number) => {
      if (previous === 0) return current > 0 ? 100 : 0;
      return Math.round(((current - previous) / previous) * 100);
    };

    // Don't show trends for "all time" selection
    if (selection.type === "preset" && selection.preset === "all") {
      return null;
    }

    return {
      entryCount: calcTrend(summary.entryCount, previousSummary.entryCount),
      durationMs: calcTrend(summary.durationMs, previousSummary.durationMs),
      charCount: calcTrend(summary.charCount, previousSummary.charCount),
      llmCalls: calcTrend(summary.llmCalls, previousSummary.llmCalls),
      totalTokens: calcTrend(summary.totalTokens, previousSummary.totalTokens),
    };
  }, [summary, previousSummary, selection]);

  const getAudioUrl = useCallback(async (fileName: string) => {
    try {
      if (audioUrlCacheRef.current.has(fileName)) {
        return audioUrlCacheRef.current.get(fileName) ?? null;
      }
      const filePath = await invoke<string>("get_audio_file_path", {
        fileName,
        file_name: fileName,
      });
      const url = convertFileSrc(`${filePath}`, "asset");
      audioUrlCacheRef.current.set(fileName, url);
      return url;
    } catch (error) {
      console.error("Failed to get audio file path:", error);
      return null;
    }
  }, []);

  const onCopy = useCallback(async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch (error) {
      console.error("Failed to copy to clipboard:", error);
    }
  }, []);

  const onToggleSaved = useCallback(async (id: number) => {
    try {
      await invoke("toggle_history_entry_saved", { id });
    } catch (error) {
      console.error("Failed to toggle saved status:", error);
    }
  }, []);

  const onDelete = useCallback(
    async (id: number) => {
      try {
        await invoke("delete_history_entry", { id });
      } catch (error) {
        console.error("Failed to delete entry:", error);
        alert(t("settings.history.deleteError"));
      }
    },
    [t],
  );

  const detailGroups = useMemo(() => {
    const map = new Map<string, HistoryEntry[]>();
    for (const entry of detailEntries) {
      const day = toLocalYmd(new Date(entry.timestamp * 1000));
      const group = map.get(day) ?? [];
      group.push(entry);
      map.set(day, group);
    }
    return Array.from(map.entries()).sort(([a], [b]) => b.localeCompare(a));
  }, [detailEntries]);

  // Client-side filter: when errorsOnly is on, restrict the visible list to
  // entries with a non-null error_summary. The summary stats above stay
  // unaffected. Spec marks backend-side filtering as V2 work.
  const visibleDetailEntries = useMemo(
    () =>
      errorsOnly
        ? detailEntries.filter((e) => e.error_summary != null)
        : detailEntries,
    [detailEntries, errorsOnly],
  );
  const visibleDetailTotal = errorsOnly
    ? visibleDetailEntries.length
    : detailTotalCount;

  return (
    <Box className="w-full max-w-5xl mx-auto space-y-8">
      <DashboardActivityChart
        bars={bars}
        selection={selection}
        loading={loading}
        onSelectDay={(day) => setSelection({ type: "day", day })}
        onSelectPreset={(preset) => setSelection({ type: "preset", preset })}
      />

      <Flex justify="between" align="center" mb="4">
        <Heading size="5">{selectionTitle}</Heading>
        <Flex
          asChild
          gap="2"
          align="center"
          className="cursor-pointer select-none"
        >
          <label>
            <Switch
              size="1"
              checked={errorsOnly}
              onCheckedChange={setErrorsOnly}
            />
            <Text size="2" color="gray">
              {t("dashboard.errorIndicator.errorsOnlyToggle")}
            </Text>
          </label>
        </Flex>
      </Flex>
      <DashboardSummaryCards
        summary={summary}
        trends={trends}
        formatDurationMs={formatDurationMs}
      />

      <VirtualDetailsList
        entries={visibleDetailEntries}
        totalCount={visibleDetailTotal}
        selectedDayTotals={selectedDayTotals}
        getAudioUrl={getAudioUrl}
        onCopy={onCopy}
        onToggleSaved={onToggleSaved}
        onDelete={onDelete}
        onRetranscribe={async (id) => {
          try {
            await invoke("retranscribe_history_entry", { id });
          } catch (e) {
            console.error("Retranscribe invocation failed", e);
            alert(t("dashboard.actions.retranscribeFailed"));
          }
        }}
        onReprocess={async (id, promptId, inputText) => {
          console.log(
            `[Dashboard] Invoking reprocess_history_entry for ID: ${id}, Prompt: ${promptId}`,
          );
          try {
            await invoke("reprocess_history_entry", {
              id,
              promptId,
              inputText,
            });
            console.log("[Dashboard] Reprocess invocation successful");
          } catch (e) {
            console.error("[Dashboard] Reprocess invocation failed", e);
            alert(t("dashboard.actions.reprocessFailed"));
          }
        }}
        onLoadMore={handleLoadMore}
        isLoadingMore={isLoadingMore}
        hasMore={hasMore}
        formatDurationMs={formatDurationMs}
        numberFormat={numberFormat}
        t={t}
      />
    </Box>
  );
};
