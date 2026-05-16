import { Box, Flex } from "@radix-ui/themes";
import { useEffect, useMemo } from "react";
import { AuxPanel } from "./aux/AuxPanel";
import { ExportSection } from "./aux/ExportSection";
import { FeedbackHistorySection } from "./aux/FeedbackHistorySection";
import { HotwordSection } from "./aux/HotwordSection";
import { ProfileSection } from "./aux/ProfileSection";
import { RecapSection } from "./aux/RecapSection";
import { StatsSection } from "./aux/StatsSection";
import { useSummary } from "./hooks/useSummary";
import { useTaskClusters } from "./hooks/useTaskClusters";
import { ClusteringModelSelector } from "./shared/ClusteringModelSelector";
import { PeriodSelector } from "./shared/PeriodSelector";
import { RegenerateButton } from "./shared/RegenerateButton";
import { useSummaryStore } from "./stores/summaryStore";
import type {
  AuxSection,
  PeriodSelection,
  PeriodType,
  ViewMode,
} from "./summaryTypes";
import { DayView } from "./views/DayView";
import { MonthView } from "./views/MonthView";
import { WeekView } from "./views/WeekView";

/**
 * Convert (viewMode, selectedDate) into a PeriodSelection consumed by the
 * legacy `useSummary` hook. Mirrors the date math the previous SummaryPage
 * used for day/week/month selections.
 */
function selectionFor(mode: ViewMode, isoDate: string): PeriodSelection {
  const anchor = new Date(isoDate);
  let startTs = 0;
  let endTs = 0;
  let type: PeriodType = "day";
  let label = isoDate;

  switch (mode) {
    case "day": {
      const start = new Date(anchor);
      start.setHours(0, 0, 0, 0);
      const end = new Date(anchor);
      end.setHours(23, 59, 59, 999);
      startTs = Math.floor(start.getTime() / 1000);
      endTs = Math.floor(end.getTime() / 1000);
      type = "day";
      label = start.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
        weekday: "short",
      });
      break;
    }
    case "week": {
      const start = new Date(anchor);
      start.setDate(start.getDate() - start.getDay()); // Sunday
      start.setHours(0, 0, 0, 0);
      const end = new Date(start);
      end.setDate(end.getDate() + 6);
      end.setHours(23, 59, 59, 999);
      startTs = Math.floor(start.getTime() / 1000);
      endTs = Math.floor(end.getTime() / 1000);
      type = "week";
      label = `${start.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      })} - ${end.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      })}`;
      break;
    }
    case "month": {
      const start = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
      const end = new Date(
        anchor.getFullYear(),
        anchor.getMonth() + 1,
        0,
        23,
        59,
        59,
        999,
      );
      startTs = Math.floor(start.getTime() / 1000);
      endTs = Math.floor(end.getTime() / 1000);
      type = "month";
      label = start.toLocaleDateString(undefined, {
        month: "long",
        year: "numeric",
      });
      break;
    }
  }

  return { type, startTs, endTs, label };
}

export const SummaryPage: React.FC = () => {
  const { viewMode, selectedDate } = useSummaryStore();
  const { stats, summary, userProfile, loading, loadSummary } = useSummary();
  const { generate, generating } = useTaskClusters(selectedDate);

  // Derive the legacy PeriodSelection from store state, and refresh the
  // summary record whenever it changes.
  const selection = useMemo(
    () => selectionFor(viewMode, selectedDate),
    [viewMode, selectedDate],
  );

  useEffect(() => {
    loadSummary(selection);
  }, [selection, loadSummary]);

  const auxSections: Record<AuxSection, React.ReactNode> = {
    stats: <StatsSection stats={stats} loading={loading} />,
    recap: <RecapSection summary={summary} />,
    profile: <ProfileSection userProfile={userProfile} />,
    hotword: <HotwordSection summary={summary} />,
    export: <ExportSection summaryId={summary?.id ?? null} />,
    feedback: <FeedbackHistorySection />,
  };

  // TODO(T22+): wire to Dashboard navigation once the parent (Sidebar) exposes
  // an onNavigate hook. For now the click is a no-op stub so cluster cards do
  // not crash when invoking the callback.
  const navigateToHistory = (_entryId: number) => {
    /* no-op until cross-section navigation lands */
  };

  return (
    <Box className="w-full max-w-6xl mx-auto p-4">
      <Flex justify="between" align="center" mb="3">
        <PeriodSelector />
        {viewMode === "day" && (
          <Flex align="center" gap="2">
            <ClusteringModelSelector />
            <RegenerateButton
              onRegenerate={() => generate(true)}
              loading={generating}
            />
          </Flex>
        )}
      </Flex>

      <AuxPanel sectionsContent={auxSections} />

      {viewMode === "day" && (
        <DayView onNavigateToHistory={navigateToHistory} />
      )}
      {viewMode === "week" && <WeekView />}
      {viewMode === "month" && <MonthView />}
    </Box>
  );
};

export default SummaryPage;
