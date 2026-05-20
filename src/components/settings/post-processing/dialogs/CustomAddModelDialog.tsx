import {
  Button,
  Callout,
  Dialog,
  Flex,
  IconButton,
  SegmentedControl,
  Text,
  TextField,
  Tooltip,
} from "@radix-ui/themes";
import { IconBrain, IconInfoCircle, IconPlus } from "@tabler/icons-react";
import React, { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useSettings } from "../../../../hooks/useSettings";
import type { CachedModel, ModelType } from "../../../../lib/types";
import {
  KeyValueEditor,
  type KeyValueEditorHandle,
} from "../../../ui/KeyValueEditor";

interface CustomAddModelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerId: string;
  testInferenceInline: (
    modelId: string,
    extraParams: Record<string, unknown> | null,
    extraHeaders: Record<string, string> | null,
  ) => Promise<
    | {
        content: string;
        hasThinking: boolean;
        durationMs: number | null;
        totalTokens: number | null;
        error?: undefined;
      }
    | { error: string }
  >;
}

export const CustomAddModelDialog: React.FC<CustomAddModelDialogProps> = ({
  open,
  onOpenChange,
  providerId,
  testInferenceInline,
}) => {
  const { t } = useTranslation();
  const { settings, addCachedModel } = useSettings();

  const [modelId, setModelId] = useState("");
  const [modelType, setModelType] = useState<ModelType>("text");
  const [label, setLabel] = useState("");
  const [extraParams, setExtraParams] = useState<Record<string, unknown>>({});
  const [extraHeaders, setExtraHeaders] = useState<Record<string, unknown>>({});
  const bodyEditorRef = useRef<KeyValueEditorHandle>(null);
  const headersEditorRef = useRef<KeyValueEditorHandle>(null);
  const [bodyEntryCount, setBodyEntryCount] = useState(0);
  const [headerEntryCount, setHeaderEntryCount] = useState(0);

  type TestState =
    | { kind: "idle" }
    | { kind: "testing" }
    | {
        kind: "passed";
        content: string;
        hasThinking: boolean;
        durationMs: number | null;
        totalTokens: number | null;
      }
    | { kind: "failed"; error: string };

  const [testState, setTestState] = useState<TestState>({ kind: "idle" });
  const [skipped, setSkipped] = useState(false);
  const [adding, setAdding] = useState(false);

  const tokensPerSec = (() => {
    if (testState.kind !== "passed") return null;
    const { totalTokens, durationMs } = testState;
    if (!totalTokens || !durationMs || durationMs <= 0) return null;
    return (totalTokens / durationMs) * 1000;
  })();

  const headersAsStringMap = (): Record<string, string> | null => {
    const entries = Object.entries(extraHeaders);
    if (entries.length === 0) return null;
    const out: Record<string, string> = {};
    for (const [k, v] of entries) {
      out[k] = typeof v === "string" ? v : JSON.stringify(v);
    }
    return out;
  };

  const handleTest = async () => {
    const id = modelId.trim();
    if (!id) return;
    setSkipped(false);
    setTestState({ kind: "testing" });
    const result = await testInferenceInline(
      id,
      Object.keys(extraParams).length > 0 ? extraParams : null,
      headersAsStringMap(),
    );
    if ("error" in result && result.error) {
      setTestState({ kind: "failed", error: result.error });
    } else if (!("error" in result)) {
      setTestState({
        kind: "passed",
        content: result.content,
        hasThinking: result.hasThinking,
        durationMs: result.durationMs,
        totalTokens: result.totalTokens,
      });
    }
  };

  const cachedModels = settings?.cached_models ?? [];
  const isDuplicate = useMemo(() => {
    const trimmed = modelId.trim();
    if (trimmed.length === 0) return false;
    return cachedModels.some(
      (m) => m.provider_id === providerId && m.model_id === trimmed,
    );
  }, [cachedModels, providerId, modelId]);

  const handleClose = () => {
    setModelId("");
    setModelType("text");
    setLabel("");
    setExtraParams({});
    setExtraHeaders({});
    setTestState({ kind: "idle" });
    setSkipped(false);
    setAdding(false);
    onOpenChange(false);
  };

  const buildCacheId = () => {
    if (globalThis.crypto?.randomUUID) {
      return globalThis.crypto.randomUUID();
    }
    return `${providerId}-${modelId.trim()}-${Date.now()}`;
  };

  const handleAdd = async () => {
    const id = modelId.trim();
    if (!id) return;
    setAdding(true);
    try {
      const headersOut: Record<string, string> = {};
      for (const [k, v] of Object.entries(extraHeaders)) {
        headersOut[k] = typeof v === "string" ? v : JSON.stringify(v);
      }
      const newModel: CachedModel = {
        id: buildCacheId(),
        name: label.trim() || id,
        model_type: modelType,
        provider_id: providerId,
        model_id: id,
        added_at: new Date().toISOString(),
        is_thinking_model: false,
        prompt_message_role: "system",
        extra_params:
          Object.keys(extraParams).length > 0 ? extraParams : undefined,
        extra_headers:
          Object.keys(headersOut).length > 0 ? headersOut : undefined,
      };
      await addCachedModel(newModel);
      handleClose();
    } catch (e) {
      setTestState({
        kind: "failed",
        error: `添加失败: ${typeof e === "string" ? e : JSON.stringify(e)}`,
      });
    } finally {
      setAdding(false);
    }
  };

  const canAdd =
    modelId.trim().length > 0 &&
    (testState.kind === "passed" || skipped) &&
    !adding;

  const showSkipLink =
    modelId.trim().length > 0 &&
    !skipped &&
    (testState.kind === "idle" || testState.kind === "failed");

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && handleClose()}>
      <Dialog.Content maxWidth="540px">
        <Dialog.Title>
          {t(
            "settings.postProcessing.models.customAdd.title",
            "Add Custom Model",
          )}
        </Dialog.Title>

        <Flex direction="column" gap="4" mt="3">
          {isDuplicate && (
            <Callout.Root color="amber" size="1">
              <Callout.Icon>
                <IconInfoCircle size={14} />
              </Callout.Icon>
              <Callout.Text>
                {t(
                  "settings.postProcessing.models.customAdd.duplicateWarning",
                  "A model with this ID already exists for this provider. Adding will create a copy.",
                )}
              </Callout.Text>
            </Callout.Root>
          )}

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.customAdd.modelId",
                "Model ID",
              )}
            </Text>
            <TextField.Root
              value={modelId}
              onChange={(e) => {
                setModelId(e.target.value);
                if (skipped) setSkipped(false);
                if (
                  testState.kind === "passed" ||
                  testState.kind === "failed"
                ) {
                  setTestState({ kind: "idle" });
                }
              }}
              placeholder="e.g. gpt-4o, my-custom-llama"
              autoFocus
            />
          </Flex>

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.selectModel.usageTypeTitle",
                "Usage Type",
              )}
            </Text>
            <SegmentedControl.Root
              value={modelType}
              onValueChange={(v) => setModelType(v as ModelType)}
              size="1"
            >
              <SegmentedControl.Item value="text">
                {t(
                  "settings.postProcessing.models.modelTypes.text.label",
                  "Text",
                )}
              </SegmentedControl.Item>
              <SegmentedControl.Item value="asr">
                {t(
                  "settings.postProcessing.models.modelTypes.asr.label",
                  "ASR",
                )}
              </SegmentedControl.Item>
              <SegmentedControl.Item value="other">
                {t(
                  "settings.postProcessing.models.modelTypes.other.label",
                  "Other",
                )}
              </SegmentedControl.Item>
            </SegmentedControl.Root>
          </Flex>

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.selectModel.customLabel",
                "Display Name",
              )}
            </Text>
            <TextField.Root
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={modelId || t("common.optional", "Optional")}
            />
          </Flex>

          <Flex direction="column" gap="1">
            <Flex align="center" gap="2" wrap="wrap">
              <Text size="2" weight="medium" color="gray">
                Body 参数
              </Text>
              {bodyEntryCount > 0 && (
                <Tooltip content="添加参数">
                  <IconButton
                    size="1"
                    variant="outline"
                    className="h-5! w-5!"
                    color="gray"
                    onClick={() => bodyEditorRef.current?.addEntry()}
                  >
                    <IconPlus size={12} />
                  </IconButton>
                </Tooltip>
              )}
              <Button
                size="1"
                variant="soft"
                color="blue"
                onClick={() =>
                  setExtraParams((prev) => ({
                    ...prev,
                    thinking: { type: "enabled" },
                  }))
                }
              >
                <IconBrain size={12} />
                启用思考
              </Button>
              <Button
                size="1"
                variant="soft"
                color="orange"
                onClick={() =>
                  setExtraParams((prev) => ({
                    ...prev,
                    thinking: { type: "disabled" },
                  }))
                }
              >
                <IconBrain size={12} />
                禁用思考
              </Button>
            </Flex>
            <KeyValueEditor
              value={extraParams}
              onChange={setExtraParams}
              addLabel="添加 Body 参数"
              addTooltip="添加参数"
              addRef={bodyEditorRef}
              onEntryCountChange={setBodyEntryCount}
            />
          </Flex>

          <Flex direction="column" gap="1">
            <Flex align="center" gap="2">
              <Text size="2" weight="medium" color="gray">
                Headers
              </Text>
              {headerEntryCount > 0 && (
                <Tooltip content="添加 Header">
                  <IconButton
                    size="1"
                    variant="outline"
                    color="gray"
                    className="h-5! w-5!"
                    onClick={() => headersEditorRef.current?.addEntry()}
                  >
                    <IconPlus size={12} />
                  </IconButton>
                </Tooltip>
              )}
            </Flex>
            <KeyValueEditor
              value={extraHeaders}
              onChange={setExtraHeaders}
              addLabel="添加 Header"
              addTooltip="添加 Header"
              addRef={headersEditorRef}
              onEntryCountChange={setHeaderEntryCount}
            />
          </Flex>

          <Flex direction="column" gap="2">
            <Flex align="center" gap="2">
              <Tooltip
                content={
                  modelId.trim().length === 0
                    ? "请先填入模型 ID"
                    : "发送一句话 ping 测试"
                }
              >
                <Button
                  variant="soft"
                  color="blue"
                  disabled={
                    modelId.trim().length === 0 || testState.kind === "testing"
                  }
                  onClick={handleTest}
                >
                  {testState.kind === "testing" ? "测试中…" : "测试"}
                </Button>
              </Tooltip>
            </Flex>

            {testState.kind === "passed" && (
              <Callout.Root color="green" size="1">
                <Callout.Icon>
                  <IconInfoCircle size={14} />
                </Callout.Icon>
                <Callout.Text>
                  <Flex direction="column" gap="1">
                    <Text size="2">
                      {testState.content.length > 200
                        ? testState.content.slice(0, 200) + "…"
                        : testState.content}
                    </Text>
                    <Flex gap="3">
                      {tokensPerSec !== null && (
                        <Text size="1" color="gray">
                          {tokensPerSec.toFixed(1)} t/s
                        </Text>
                      )}
                      {testState.hasThinking && (
                        <Text size="1" color="blue">
                          🧠 Thinking
                        </Text>
                      )}
                    </Flex>
                  </Flex>
                </Callout.Text>
              </Callout.Root>
            )}

            {testState.kind === "failed" && (
              <Callout.Root color="red" size="1">
                <Callout.Icon>
                  <IconInfoCircle size={14} />
                </Callout.Icon>
                <Callout.Text>{testState.error}</Callout.Text>
              </Callout.Root>
            )}
          </Flex>

          <Flex justify="end" align="center" gap="3" mt="2">
            {showSkipLink && (
              <Button
                variant="ghost"
                color="gray"
                size="1"
                onClick={() => setSkipped(true)}
              >
                跳过测试直接添加
              </Button>
            )}
            <Button variant="soft" color="gray" onClick={handleClose}>
              {t("common.cancel")}
            </Button>
            <Button variant="solid" onClick={handleAdd} disabled={!canAdd}>
              {t("common.add")}
            </Button>
          </Flex>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
};
