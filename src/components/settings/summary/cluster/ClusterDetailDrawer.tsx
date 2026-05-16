import { Box, Flex, IconButton, Text } from "@radix-ui/themes";
import { IconArrowUpRight } from "@tabler/icons-react";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { TaskCluster } from "../summaryTypes";

interface HistoryEntryLite {
  id: number;
  timestamp: number;
  app_name: string | null;
  window_title: string | null;
  transcription_text: string;
  post_processed_text: string | null;
  duration_ms: number | null;
}

interface ClusterDetailDrawerProps {
  cluster: TaskCluster;
  onNavigateToHistory: (entryId: number) => void;
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function ClusterDetailDrawer({
  cluster,
  onNavigateToHistory,
}: ClusterDetailDrawerProps) {
  const [entries, setEntries] = useState<HistoryEntryLite[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    if (cluster.source_history_ids.length === 0) {
      setEntries([]);
      return;
    }
    setLoading(true);
    (async () => {
      try {
        const all = await invoke<HistoryEntryLite[]>(
          "get_history_entries_by_ids",
          {
            ids: cluster.source_history_ids,
          },
        );
        if (!cancelled) {
          all.sort((a, b) => a.timestamp - b.timestamp);
          setEntries(all);
        }
      } catch (e) {
        console.warn("failed to load cluster entries", e);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cluster.source_history_ids]);

  if (loading) {
    return (
      <Text size="2" color="gray">
        加载源转录...
      </Text>
    );
  }

  if (entries.length === 0) {
    return (
      <Text size="2" color="gray">
        该 cluster 已失效，建议重新生成
      </Text>
    );
  }

  return (
    <Box className="border-l-2 border-gray-200 dark:border-gray-700 pl-3">
      <Flex direction="column" gap="2">
        {entries.map((e) => (
          <Flex key={e.id} align="start" gap="2">
            <Text size="1" color="gray" className="min-w-12">
              {formatTime(e.timestamp)}
            </Text>
            <Box className="flex-1">
              <Flex align="center" gap="1">
                <Text size="1" weight="medium">
                  {e.app_name ?? "?"}
                </Text>
                {e.window_title && (
                  <Text size="1" color="gray" className="truncate max-w-xs">
                    · {e.window_title}
                  </Text>
                )}
                <IconButton
                  size="1"
                  variant="ghost"
                  onClick={() => onNavigateToHistory(e.id)}
                  aria-label="跳转到 Dashboard 对应条目"
                >
                  <IconArrowUpRight size={12} />
                </IconButton>
              </Flex>
              <Text size="2" className="line-clamp-3">
                {e.post_processed_text || e.transcription_text}
              </Text>
            </Box>
          </Flex>
        ))}
      </Flex>
    </Box>
  );
}
