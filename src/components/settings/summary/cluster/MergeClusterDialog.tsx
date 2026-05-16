import {
  Dialog,
  Button,
  Checkbox,
  Flex,
  Text,
  ScrollArea,
} from "@radix-ui/themes";
import { useState } from "react";
import type { TaskCluster } from "../summaryTypes";

interface MergeClusterDialogProps {
  open: boolean;
  targetCluster: TaskCluster | null;
  otherClusters: TaskCluster[]; // candidates same date
  onCancel: () => void;
  onConfirm: (sourceClusterIds: string[]) => Promise<void>;
}

export function MergeClusterDialog({
  open,
  targetCluster,
  otherClusters,
  onCancel,
  onConfirm,
}: MergeClusterDialogProps) {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);

  if (!targetCluster) return null;

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const submit = async () => {
    if (selected.size === 0) return;
    setSubmitting(true);
    try {
      await onConfirm(Array.from(selected));
      onCancel();
    } finally {
      setSubmitting(false);
    }
  };

  const candidates = otherClusters.filter((c) => c.id !== targetCluster.id);

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <Dialog.Content style={{ maxWidth: 520 }}>
        <Dialog.Title>合并到「{targetCluster.title}」</Dialog.Title>
        <Dialog.Description size="2" mb="3">
          选择要并入的 cluster。被并入的 cluster 会被删除，其 source_ids
          与关键词合并到当前 cluster。
        </Dialog.Description>
        <ScrollArea type="auto" style={{ maxHeight: 320 }}>
          <Flex direction="column" gap="2">
            {candidates.length === 0 && (
              <Text size="2" color="gray">
                当日没有其他 cluster 可合并。
              </Text>
            )}
            {candidates.map((c) => (
              <Flex key={c.id} align="center" gap="2" asChild>
                <label>
                  <Checkbox
                    checked={selected.has(c.id)}
                    onCheckedChange={() => toggle(c.id)}
                  />
                  <Flex direction="column" className="flex-1">
                    <Text size="2" weight="medium">
                      {c.title}
                    </Text>
                    <Text size="1" color="gray">
                      {c.entry_count} entries · {c.status}
                    </Text>
                  </Flex>
                </label>
              </Flex>
            ))}
          </Flex>
        </ScrollArea>
        <Flex gap="3" mt="4" justify="end">
          <Dialog.Close>
            <Button variant="soft" color="gray">
              取消
            </Button>
          </Dialog.Close>
          <Button disabled={selected.size === 0 || submitting} onClick={submit}>
            合并 {selected.size > 0 && `(${selected.size})`}
          </Button>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
}
