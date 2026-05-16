import { invoke } from "@tauri-apps/api/core";
import { Box, Flex, Grid, Text } from "@radix-ui/themes";
import { useEffect, useState } from "react";
import { useSummaryStore } from "../stores/summaryStore";
import type { TaskCluster } from "../summaryTypes";
import { Card } from "../../../ui/Card";

function monthDates(anchor: string): string[] {
  const d = new Date(anchor);
  const year = d.getFullYear();
  const month = d.getMonth();
  const last = new Date(year, month + 1, 0).getDate();
  return Array.from({ length: last }, (_, i) => {
    const x = new Date(year, month, i + 1);
    return x.toISOString().slice(0, 10);
  });
}

export function MonthView() {
  const { selectedDate, setSelectedDate, setViewMode } = useSummaryStore();
  const [byDate, setByDate] = useState<Record<string, TaskCluster[]>>({});

  useEffect(() => {
    const dates = monthDates(selectedDate);
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

  const dates = monthDates(selectedDate);

  return (
    <Box>
      <Grid columns="7" gap="1">
        {dates.map((d) => {
          const clusters = byDate[d] ?? [];
          const total = clusters.reduce((s, c) => s + c.total_duration_ms, 0);
          const minutes = Math.round(total / 60000);
          return (
            <Card
              key={d}
              className="p-1 cursor-pointer hover:bg-gray-50 dark:hover:bg-gray-900"
            >
              <button
                type="button"
                onClick={() => {
                  setSelectedDate(d);
                  setViewMode("day");
                }}
                className="w-full text-left"
              >
                <Text size="1" weight="medium">
                  {d.slice(-2)}
                </Text>
                {clusters.length > 0 ? (
                  <Flex direction="column" gap="0">
                    <Text size="1" color="gray">
                      {clusters.length} clusters
                    </Text>
                    <Text size="1" color="gray">
                      {minutes}m
                    </Text>
                  </Flex>
                ) : (
                  <Text size="1" color="gray">
                    —
                  </Text>
                )}
              </button>
            </Card>
          );
        })}
      </Grid>
    </Box>
  );
}
