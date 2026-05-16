import { invoke } from "@tauri-apps/api/core";
import { Box, Flex, Grid, Text, Badge } from "@radix-ui/themes";
import { useEffect, useState } from "react";
import { useSummaryStore } from "../stores/summaryStore";
import type { TaskCluster } from "../summaryTypes";
import { Card } from "../../../ui/Card";

function weekDates(anchor: string): string[] {
  const d = new Date(anchor);
  const start = new Date(d);
  start.setDate(d.getDate() - d.getDay());
  return Array.from({ length: 7 }, (_, i) => {
    const x = new Date(start);
    x.setDate(start.getDate() + i);
    return x.toISOString().slice(0, 10);
  });
}

export function WeekView() {
  const { selectedDate, setSelectedDate, setViewMode } = useSummaryStore();
  const [byDate, setByDate] = useState<Record<string, TaskCluster[]>>({});

  useEffect(() => {
    const dates = weekDates(selectedDate);
    let cancelled = false;
    (async () => {
      const acc: Record<string, TaskCluster[]> = {};
      for (const d of dates) {
        try {
          const list = await invoke<TaskCluster[]>(
            "get_task_clusters_by_date",
            { date: d },
          );
          acc[d] = list;
        } catch {
          acc[d] = [];
        }
      }
      if (!cancelled) setByDate(acc);
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedDate]);

  const dates = weekDates(selectedDate);
  const allKeywords = Object.values(byDate)
    .flat()
    .flatMap((c) => c.keywords);
  const kwCounts = new Map<string, number>();
  for (const k of allKeywords) kwCounts.set(k, (kwCounts.get(k) ?? 0) + 1);
  const topKw = Array.from(kwCounts.entries())
    .sort((a, b) => b[1] - a[1])
    .slice(0, 12);

  return (
    <Box>
      {topKw.length > 0 && (
        <Card className="mb-3">
          <Text size="1" color="gray" mb="1">
            本周热点
          </Text>
          <Flex gap="1" wrap="wrap">
            {topKw.map(([k, n]) => (
              <Badge
                key={k}
                size="1"
                color={n >= 3 ? "blue" : "gray"}
                variant="soft"
              >
                {k} {n > 1 && `×${n}`}
              </Badge>
            ))}
          </Flex>
        </Card>
      )}
      <Grid columns="7" gap="2">
        {dates.map((d) => (
          <Box key={d}>
            <Flex align="center" justify="between" mb="1">
              <Text size="1" weight="medium">
                {d.slice(5)}
              </Text>
              <button
                type="button"
                onClick={() => {
                  setSelectedDate(d);
                  setViewMode("day");
                }}
                className="text-xs text-blue-600 hover:underline"
              >
                打开
              </button>
            </Flex>
            <Flex direction="column" gap="1">
              {(byDate[d] ?? []).map((c) => (
                <Card key={c.id} className="p-2">
                  <Text size="1" weight="medium" className="truncate">
                    {c.title}
                  </Text>
                  <Text size="1" color="gray">
                    {c.entry_count}e · {Math.round(c.total_duration_ms / 60000)}
                    m
                  </Text>
                </Card>
              ))}
              {(byDate[d] ?? []).length === 0 && (
                <Text size="1" color="gray">
                  —
                </Text>
              )}
            </Flex>
          </Box>
        ))}
      </Grid>
    </Box>
  );
}
