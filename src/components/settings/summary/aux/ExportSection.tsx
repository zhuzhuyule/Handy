import { Button, Flex, Text } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { toast } from "sonner";

interface ExportSectionProps {
  summaryId: number | null;
}

export function ExportSection({ summaryId }: ExportSectionProps) {
  if (!summaryId) {
    return (
      <Text size="2" color="gray">
        请先选择 Summary
      </Text>
    );
  }

  const exportAs = async (format: "markdown" | "json") => {
    try {
      const content = await invoke<string>("export_summary", {
        summaryId,
        format,
      });
      await writeText(content);
      toast.success(`${format.toUpperCase()} 已复制到剪贴板`);
    } catch (e) {
      toast.error(`导出失败: ${e}`);
    }
  };

  return (
    <Flex direction="column" gap="2">
      <Button size="2" variant="soft" onClick={() => exportAs("markdown")}>
        导出 Markdown
      </Button>
      <Button size="2" variant="soft" onClick={() => exportAs("json")}>
        导出 JSON
      </Button>
    </Flex>
  );
}
