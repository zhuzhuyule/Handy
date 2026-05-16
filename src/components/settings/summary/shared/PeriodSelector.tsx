import { Button, Flex, SegmentedControl, Text } from "@radix-ui/themes";
import { IconChevronLeft, IconChevronRight } from "@tabler/icons-react";
import { useSummaryStore } from "../stores/summaryStore";
import type { ViewMode } from "../summaryTypes";

function shiftDate(iso: string, mode: ViewMode, delta: number): string {
  const d = new Date(iso);
  if (mode === "day") d.setDate(d.getDate() + delta);
  else if (mode === "week") d.setDate(d.getDate() + delta * 7);
  else d.setMonth(d.getMonth() + delta);
  return d.toISOString().slice(0, 10);
}

function formatRange(iso: string, mode: ViewMode): string {
  if (mode === "day") return iso;
  const d = new Date(iso);
  if (mode === "week") {
    const start = new Date(d);
    start.setDate(d.getDate() - d.getDay());
    const end = new Date(start);
    end.setDate(start.getDate() + 6);
    return `${start.toISOString().slice(0, 10)} ~ ${end.toISOString().slice(0, 10)}`;
  }
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

export function PeriodSelector() {
  const { viewMode, selectedDate, setViewMode, setSelectedDate } =
    useSummaryStore();

  return (
    <Flex align="center" gap="3">
      <Flex align="center" gap="1">
        <Button
          variant="ghost"
          size="1"
          onClick={() => setSelectedDate(shiftDate(selectedDate, viewMode, -1))}
          aria-label="上一段"
        >
          <IconChevronLeft size={16} />
        </Button>
        <Text
          size="2"
          weight="medium"
          style={{ minWidth: 180, textAlign: "center" }}
        >
          {formatRange(selectedDate, viewMode)}
        </Text>
        <Button
          variant="ghost"
          size="1"
          onClick={() => setSelectedDate(shiftDate(selectedDate, viewMode, 1))}
          aria-label="下一段"
        >
          <IconChevronRight size={16} />
        </Button>
      </Flex>
      <SegmentedControl.Root
        value={viewMode}
        onValueChange={(v) => setViewMode(v as ViewMode)}
        size="1"
      >
        <SegmentedControl.Item value="day">day</SegmentedControl.Item>
        <SegmentedControl.Item value="week">week</SegmentedControl.Item>
        <SegmentedControl.Item value="month">month</SegmentedControl.Item>
      </SegmentedControl.Root>
    </Flex>
  );
}
