import { Box, Grid, Text } from "@radix-ui/themes";
import type { SummaryStats } from "../summaryTypes";

interface StatsSectionProps {
  stats: SummaryStats | null;
  loading: boolean;
}

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h${m}m`;
  return `${m}m`;
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <Box className="rounded-lg p-3 bg-gray-50 dark:bg-gray-900">
      <Text size="1" color="gray" className="uppercase tracking-wide">
        {label}
      </Text>
      <Text size="5" weight="bold" className="block mt-1">
        {value}
      </Text>
    </Box>
  );
}

export function StatsSection({ stats, loading }: StatsSectionProps) {
  if (loading)
    return (
      <Text size="2" color="gray">
        加载中...
      </Text>
    );
  if (!stats)
    return (
      <Text size="2" color="gray">
        无统计数据
      </Text>
    );

  return (
    <Grid columns="2" gap="2">
      <StatCard label="Entries" value={String(stats.entry_count)} />
      <StatCard
        label="Duration"
        value={formatDuration(stats.total_duration_ms)}
      />
      <StatCard label="Chars" value={String(stats.total_chars)} />
      <StatCard label="LLM Calls" value={String(stats.llm_calls)} />
    </Grid>
  );
}
