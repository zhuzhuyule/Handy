import { Badge, Box, Button, Flex, Text } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { ClusterFeedback } from "../summaryTypes";

export function FeedbackHistorySection() {
  const [items, setItems] = useState<ClusterFeedback[]>([]);
  const [loading, setLoading] = useState(false);

  const refresh = async () => {
    setLoading(true);
    try {
      const list = await invoke<ClusterFeedback[]>(
        "list_recent_negative_cluster_feedback",
        { days: 30, limit: 50 },
      );
      setItems(list);
    } catch (e) {
      toast.error(`加载失败: ${e}`);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const remove = async (id: number) => {
    try {
      await invoke("delete_cluster_feedback", { id });
      await refresh();
    } catch (e) {
      toast.error(`删除失败: ${e}`);
    }
  };

  if (loading)
    return (
      <Text size="2" color="gray">
        加载中...
      </Text>
    );
  if (items.length === 0)
    return (
      <Text size="2" color="gray">
        最近 30 天暂无负反馈记录
      </Text>
    );

  return (
    <Flex direction="column" gap="2">
      <Text size="1" color="gray">
        最近 30 天的 👎+备注 反馈（这些备注会注入下次 AI 聚类的 prompt）
      </Text>
      {items.map((f) => (
        <Box
          key={f.id}
          className="rounded-md border border-gray-200 dark:border-gray-800 p-2"
        >
          <Flex justify="between" align="start" gap="2">
            <Box className="flex-1">
              <Flex gap="1" align="center" mb="1">
                <Badge color="red" size="1">
                  👎
                </Badge>
                <Text size="1" color="gray">
                  {new Date(f.created_at).toLocaleString()}
                </Text>
              </Flex>
              <Text size="2">{f.note ?? "(空备注)"}</Text>
            </Box>
            <Button
              size="1"
              variant="ghost"
              color="gray"
              onClick={() => remove(f.id)}
            >
              删除
            </Button>
          </Flex>
        </Box>
      ))}
    </Flex>
  );
}
