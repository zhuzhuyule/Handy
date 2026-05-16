import { AlertDialog, Button, Flex } from "@radix-ui/themes";

interface DeleteClusterConfirmProps {
  open: boolean;
  clusterTitle: string;
  onCancel: () => void;
  onConfirm: () => void;
}

export function DeleteClusterConfirm({
  open,
  clusterTitle,
  onCancel,
  onConfirm,
}: DeleteClusterConfirmProps) {
  return (
    <AlertDialog.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <AlertDialog.Content style={{ maxWidth: 400 }}>
        <AlertDialog.Title>删除 cluster</AlertDialog.Title>
        <AlertDialog.Description size="2">
          确认删除「{clusterTitle}」？源转录记录不会被删除。下次重新生成时 AI
          可能再次提出类似聚类。
        </AlertDialog.Description>
        <Flex gap="3" mt="4" justify="end">
          <AlertDialog.Cancel>
            <Button variant="soft" color="gray">
              取消
            </Button>
          </AlertDialog.Cancel>
          <AlertDialog.Action>
            <Button color="red" onClick={onConfirm}>
              删除
            </Button>
          </AlertDialog.Action>
        </Flex>
      </AlertDialog.Content>
    </AlertDialog.Root>
  );
}
