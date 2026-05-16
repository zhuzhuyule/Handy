import { Box, Flex, Tabs, Text } from "@radix-ui/themes";
import { IconSettings } from "@tabler/icons-react";
import type { UserProfile } from "../summaryTypes";

interface ProfileSectionProps {
  userProfile: UserProfile | null;
}

function StatBlock({ label, value }: { label: string; value: string | null }) {
  if (!value || value.trim() === "") {
    return (
      <Text size="2" color="gray">
        暂无 {label} 数据
      </Text>
    );
  }
  return (
    <Text
      size="2"
      className="whitespace-pre-wrap leading-relaxed text-(--gray-12)"
    >
      {value}
    </Text>
  );
}

export function ProfileSection({ userProfile }: ProfileSectionProps) {
  if (!userProfile) {
    return (
      <Text size="2" color="gray">
        尚无 Profile 数据
      </Text>
    );
  }

  return (
    <Flex direction="column" gap="3">
      {userProfile.style_prompt && (
        <Box className="rounded-xl border border-(--accent-a4) bg-linear-to-br from-(--accent-a2) to-(--accent-a3) p-3 shadow-sm">
          <Flex gap="2" align="start">
            <Box className="mt-0.5">
              <IconSettings size={16} className="text-(--accent-11)" />
            </Box>
            <Box>
              <Text
                size="1"
                weight="bold"
                className="block text-(--accent-11) uppercase tracking-wide"
              >
                Current Style
              </Text>
              <Text
                size="2"
                className="italic text-(--gray-12) opacity-80 block mt-1"
              >
                {userProfile.style_prompt}
              </Text>
            </Box>
          </Flex>
        </Box>
      )}

      <Tabs.Root defaultValue="vocab">
        <Tabs.List>
          <Tabs.Trigger value="vocab">词汇</Tabs.Trigger>
          <Tabs.Trigger value="expr">表达</Tabs.Trigger>
          <Tabs.Trigger value="time">时间</Tabs.Trigger>
        </Tabs.List>
        <Box pt="3">
          <Tabs.Content value="vocab">
            <StatBlock label="词汇" value={userProfile.vocabulary_stats} />
          </Tabs.Content>
          <Tabs.Content value="expr">
            <StatBlock label="表达" value={userProfile.expression_stats} />
          </Tabs.Content>
          <Tabs.Content value="time">
            <StatBlock label="时间" value={userProfile.time_pattern_stats} />
          </Tabs.Content>
        </Box>
      </Tabs.Root>
    </Flex>
  );
}
