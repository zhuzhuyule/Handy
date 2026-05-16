import {
  Button,
  Flex,
  IconButton,
  Popover,
  Text,
  TextArea,
} from "@radix-ui/themes";
import { IconThumbDown, IconThumbUp } from "@tabler/icons-react";
import { useState } from "react";
import { useClusterFeedback } from "../hooks/useClusterFeedback";

interface ClusterFeedbackButtonsProps {
  clusterId: string;
}

export function ClusterFeedbackButtons({
  clusterId,
}: ClusterFeedbackButtonsProps) {
  const { add } = useClusterFeedback(clusterId);
  const [openThumb, setOpenThumb] = useState<"up" | "down" | null>(null);
  const [note, setNote] = useState("");

  const submit = async () => {
    if (!openThumb) return;
    await add(openThumb, note.trim() ? note.trim() : undefined);
    setNote("");
    setOpenThumb(null);
  };

  const cancel = () => {
    setNote("");
    setOpenThumb(null);
  };

  return (
    <Popover.Root
      open={openThumb !== null}
      onOpenChange={(open) => {
        if (!open) cancel();
      }}
    >
      <Flex gap="1">
        <Popover.Trigger>
          <IconButton
            variant="ghost"
            size="1"
            onClick={() => setOpenThumb("up")}
            aria-label="赞同"
          >
            <IconThumbUp size={14} />
          </IconButton>
        </Popover.Trigger>
        <Popover.Trigger>
          <IconButton
            variant="ghost"
            size="1"
            onClick={() => setOpenThumb("down")}
            aria-label="反对"
          >
            <IconThumbDown size={14} />
          </IconButton>
        </Popover.Trigger>
      </Flex>
      <Popover.Content>
        <Flex direction="column" gap="2" style={{ minWidth: 240 }}>
          <Text size="2">
            {openThumb === "down" ? "反馈：哪里不对？" : "可选备注"}
          </Text>
          <TextArea
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder={
              openThumb === "down" ? "例：Slack 消息应该单独成簇..." : "可选"
            }
            rows={3}
          />
          <Flex gap="2" justify="end">
            <Button variant="soft" onClick={cancel} size="1">
              取消
            </Button>
            <Button onClick={submit} size="1">
              {note.trim() ? "提交" : "提交（无备注）"}
            </Button>
          </Flex>
        </Flex>
      </Popover.Content>
    </Popover.Root>
  );
}
