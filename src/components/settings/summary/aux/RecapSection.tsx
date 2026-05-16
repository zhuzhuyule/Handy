import { Box, Flex, Text } from "@radix-ui/themes";
import type { Summary } from "../summaryTypes";

interface RecapSectionProps {
  summary: Summary | null;
}

export function RecapSection({ summary }: RecapSectionProps) {
  const recap = summary?.stats?.daily_overview;
  if (!recap) {
    return (
      <Text size="2" color="gray">
        尚无 Recap
      </Text>
    );
  }

  const hasKeyProgress = recap.key_progress && recap.key_progress.length > 0;
  const hasFrictionPoints =
    recap.friction_points && recap.friction_points.length > 0;
  const hasNextFocus = recap.next_focus && recap.next_focus.trim() !== "";

  return (
    <Flex direction="column" gap="4">
      {recap.headline && (
        <Box>
          <Text size="1" color="gray" className="uppercase tracking-wide">
            Headline
          </Text>
          <Text size="3" weight="bold" className="block mt-1 text-(--gray-12)">
            {recap.headline}
          </Text>
        </Box>
      )}

      {hasKeyProgress && (
        <Box>
          <Text size="1" color="gray" className="uppercase tracking-wide mb-1">
            Key Progress
          </Text>
          <ul className="list-disc list-inside space-y-1 mt-1">
            {recap.key_progress.map((item, i) => (
              <li key={i} className="pl-1">
                <Text size="2" color="gray">
                  {item}
                </Text>
              </li>
            ))}
          </ul>
        </Box>
      )}

      {hasFrictionPoints && (
        <Box>
          <Text size="1" color="gray" className="uppercase tracking-wide mb-1">
            Friction Points
          </Text>
          <ul className="list-disc list-inside space-y-1 mt-1">
            {recap.friction_points.map((item, i) => (
              <li key={i} className="pl-1">
                <Text size="2" color="gray">
                  {item}
                </Text>
              </li>
            ))}
          </ul>
        </Box>
      )}

      {hasNextFocus && (
        <Box>
          <Text size="1" color="gray" className="uppercase tracking-wide">
            Next Focus
          </Text>
          <Text size="2" className="block mt-1 text-(--gray-12)">
            {recap.next_focus}
          </Text>
        </Box>
      )}
    </Flex>
  );
}
