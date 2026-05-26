import { Box, Button, DataList, Flex, Heading, Text } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import React, { useCallback, useEffect, useRef, useState } from "react";

export interface RuleSuggestionPayload {
  appName: string;
  title: string;
  promptName: string;
  promptId: string;
  count: number;
  threshold: number;
}

type Decision = "accepted" | "dismissed" | "never_again";

interface Props {
  payload: RuleSuggestionPayload;
}

const truncate = (s: string, max: number) =>
  s.length > max ? s.slice(0, max) + "…" : s;

export const RuleSuggestionWindow: React.FC<Props> = ({ payload }) => {
  const [busy, setBusy] = useState(false);
  const decisionApplied = useRef(false);

  // Swallow all keystrokes (defense-in-depth — the user explicitly does not
  // want this dialog to ever interpret Enter/Space/Esc). React's synthetic
  // event system handles button clicks; this only kills key activation.
  useEffect(() => {
    const swallow = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", swallow, { capture: true });
    window.addEventListener("keyup", swallow, { capture: true });
    window.addEventListener("keypress", swallow, { capture: true });
    return () => {
      window.removeEventListener("keydown", swallow, { capture: true });
      window.removeEventListener("keyup", swallow, { capture: true });
      window.removeEventListener("keypress", swallow, { capture: true });
    };
  }, []);

  // Blur whatever the platform autofocused on first paint (e.g., the first
  // focusable button) so there's no visible "default action" affordance and
  // an accidental Enter cannot trigger it via any path we missed.
  useEffect(() => {
    const t = requestAnimationFrame(() => {
      const el = document.activeElement as HTMLElement | null;
      if (el && el !== document.body) {
        el.blur();
      }
    });
    return () => cancelAnimationFrame(t);
  }, []);

  const respond = useCallback(
    (decision: Decision) => {
      if (decisionApplied.current) return;
      decisionApplied.current = true;
      setBusy(true);
      console.log("[RuleSuggestion] respond", { decision, payload });

      // Fire-and-forget invoke. The backend writes settings + records the
      // decision in the database; we don't make the user wait for that
      // round-trip. If it fails the JS console error surfaces in DevTools;
      // any inconsistency will self-heal next time the engine runs.
      void invoke("respond_rule_suggestion", {
        decision,
        appName: payload.appName,
        title: payload.title,
        promptId: payload.promptId,
        threshold: payload.threshold,
      }).catch((e) => {
        console.error("[RuleSuggestion] invoke failed:", e);
      });

      // Close the window immediately so the click feels instant.
      getCurrentWindow()
        .close()
        .catch((e) => console.error("[RuleSuggestion] close failed:", e));
    },
    [payload],
  );

  // If the user closes the window via the title-bar X, signal Dismissed.
  useEffect(() => {
    const handler = (_e: BeforeUnloadEvent) => {
      if (decisionApplied.current) return;
      void invoke("respond_rule_suggestion", {
        decision: "dismissed",
        appName: payload.appName,
        title: payload.title,
        promptId: payload.promptId,
        threshold: payload.threshold,
      }).catch(() => {});
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [payload]);

  const titleDisplay = truncate(payload.title, 40);

  return (
    <Box
      p="4"
      style={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        gap: "var(--space-3)",
      }}
    >
      <Heading size="3" weight="medium">
        是否为该窗口添加规则？
      </Heading>
      <DataList.Root size="2" orientation="horizontal">
        <DataList.Item>
          <DataList.Label minWidth="64px">应用</DataList.Label>
          <DataList.Value>{payload.appName}</DataList.Value>
        </DataList.Item>
        <DataList.Item>
          <DataList.Label minWidth="64px">窗口</DataList.Label>
          <DataList.Value style={{ wordBreak: "break-all" }}>
            {titleDisplay}
          </DataList.Value>
        </DataList.Item>
        <DataList.Item>
          <DataList.Label minWidth="64px">提示词</DataList.Label>
          <DataList.Value>{payload.promptName}</DataList.Value>
        </DataList.Item>
        <DataList.Item>
          <DataList.Label minWidth="64px">已使用</DataList.Label>
          <DataList.Value>{payload.count} 次</DataList.Value>
        </DataList.Item>
      </DataList.Root>
      <Text size="1" color="gray">
        添加后，下次在该窗口录音将自动应用此提示词。
      </Text>
      <Flex
        gap="2"
        justify="end"
        mt="auto"
        style={{ paddingTop: "var(--space-2)" }}
      >
        <Button
          variant="soft"
          color="gray"
          tabIndex={-1}
          disabled={busy}
          onClick={() => respond("dismissed")}
        >
          这次不要
        </Button>
        <Button
          variant="ghost"
          color="gray"
          tabIndex={-1}
          disabled={busy}
          onClick={() => respond("never_again")}
        >
          别再问
        </Button>
        <Button
          variant="solid"
          tabIndex={-1}
          disabled={busy}
          onClick={() => respond("accepted")}
        >
          添加规则
        </Button>
      </Flex>
    </Box>
  );
};
