import { Badge, Box, Flex, Text } from "@radix-ui/themes";
import { IconCheck } from "@tabler/icons-react";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import type { Hotword, HotwordCategory } from "../../../../types/hotword";
import { parseAiAnalysis, type Summary } from "../summaryTypes";

interface HotwordSectionProps {
  summary: Summary | null;
}

type BadgeColor = "purple" | "blue" | "green" | "orange" | "gray";

function getTypeColor(
  hint: string | null,
  isAlreadyAdded: boolean,
): BadgeColor {
  if (isAlreadyAdded) return "gray";
  if (!hint) return "purple";
  if (hint.includes("项目") || hint.includes("模块") || hint.includes("工作"))
    return "blue";
  if (hint.includes("人名") || hint.includes("同事")) return "green";
  if (hint.includes("技术") || hint.includes("术语")) return "orange";
  return "purple";
}

export function HotwordSection({ summary }: HotwordSectionProps) {
  const [existingHotwords, setExistingHotwords] = useState<Set<string>>(
    new Set(),
  );
  const [addedWords, setAddedWords] = useState<Set<string>>(new Set());

  const loadExistingHotwords = useCallback(async () => {
    try {
      const hotwords = await invoke<Hotword[]>("get_hotwords");
      setExistingHotwords(new Set(hotwords.map((h) => h.target.toLowerCase())));
    } catch (e) {
      console.error("[HotwordSection] Failed to load hotwords:", e);
    }
  }, []);

  useEffect(() => {
    loadExistingHotwords();
  }, [loadExistingHotwords]);

  const analysis = parseAiAnalysis(summary?.ai_summary ?? null);
  const items = analysis?.vocabulary_extracted?.items ?? [];

  if (items.length === 0) {
    return (
      <Text size="2" color="gray">
        尚无提取词汇
      </Text>
    );
  }

  return (
    <Box>
      <Text
        size="1"
        color="gray"
        className="uppercase tracking-wide block mb-3"
      >
        {analysis?.vocabulary_extracted?.title ?? "词汇提取"}
      </Text>
      <Flex wrap="wrap" gap="2">
        {items.map((item, i) => {
          const match = item.match(/^(.+?)\s*\(([^)]+)\)\s*$/);
          const word = match ? match[1].trim() : item;
          const typeHint = match ? match[2].trim() : null;

          const isAlreadyAdded =
            existingHotwords.has(word.toLowerCase()) ||
            addedWords.has(word.toLowerCase());

          const handleAddToHotword = async () => {
            if (isAlreadyAdded) return;
            try {
              const category = await invoke<HotwordCategory>(
                "infer_hotword_category",
                { target: word },
              );
              await invoke("add_hotword", {
                target: word,
                originals: [],
                category,
                scenarios: ["work", "casual"],
              });
              setAddedWords((prev) => new Set(prev).add(word.toLowerCase()));
            } catch (e) {
              console.error(
                `[HotwordSection] Failed to add hotword "${word}":`,
                e,
              );
            }
          };

          return (
            <Flex key={i} gap="1" align="center">
              <Badge
                size="2"
                variant="soft"
                color={getTypeColor(typeHint, isAlreadyAdded)}
                className={`px-3 py-1 ${isAlreadyAdded ? "opacity-60" : "cursor-pointer hover:opacity-80"}`}
                onClick={handleAddToHotword}
                title={isAlreadyAdded ? "已添加到热词" : "点击添加到热词"}
              >
                {isAlreadyAdded && (
                  <IconCheck size={12} className="mr-1 inline" />
                )}
                {word}
              </Badge>
              {typeHint && (
                <Text size="1" color="gray" className="opacity-60">
                  {typeHint}
                </Text>
              )}
            </Flex>
          );
        })}
      </Flex>
    </Box>
  );
}
