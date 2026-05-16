import { Button } from "@radix-ui/themes";
import { IconRotateClockwise } from "@tabler/icons-react";

interface RegenerateButtonProps {
  onRegenerate: () => Promise<void>;
  loading: boolean;
}

export function RegenerateButton({
  onRegenerate,
  loading,
}: RegenerateButtonProps) {
  return (
    <Button variant="soft" size="1" onClick={onRegenerate} disabled={loading}>
      <IconRotateClockwise
        size={14}
        className={loading ? "animate-spin" : ""}
      />
      {loading ? "重新生成中..." : "重新生成"}
    </Button>
  );
}
