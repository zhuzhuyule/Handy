import {
  Box,
  DropdownMenu,
  Flex,
  IconButton,
  Tabs,
  Text,
  Tooltip,
} from "@radix-ui/themes";
import {
  IconAlertTriangle,
  IconCopy,
  IconMicrophone,
  IconPencil,
  IconPlayerPlay,
  IconSend,
  IconStar,
  IconThumbDown,
  IconTrash,
  IconWand,
} from "@tabler/icons-react";
import { invoke } from "@tauri-apps/api/core";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useQuickInsertTarget } from "../../../hooks/useQuickInsertTarget";
import { useSettings } from "../../../hooks/useSettings";
import { AppIcon } from "../../shared/AppIcon";
import { DynamicIcon } from "../../shared/IconPicker";
import { AudioPlayer } from "../../ui/AudioPlayer";
import { Card } from "../../ui/Card";
import type { HistoryEntry, PostProcessStep } from "./dashboardTypes";
import { EditHistoryDialog, type EditableField } from "./EditHistoryDialog";

interface DashboardEntryCardProps {
  entry: HistoryEntry;
  getAudioUrl: (fileName: string) => Promise<string | null>;
  metaText: string;
  timeText: string;
  appName: string | null;
  onCopy: (text: string) => void;
  onToggleSaved: (id: number) => void;
  onDelete: (id: number) => void;
  onRetranscribe: (id: number) => Promise<void>;
  onReprocess: (
    id: number,
    promptId: string,
    inputText?: string,
  ) => Promise<void>;
}

export const DashboardEntryCard = React.memo<DashboardEntryCardProps>(
  ({
    entry,
    getAudioUrl,
    metaText,
    timeText,
    appName,
    onCopy,
    onToggleSaved,
    onDelete,
    onRetranscribe,
    onReprocess,
  }) => {
    const { t } = useTranslation();
    const { settings } = useSettings();
    const quickInsertTarget = useQuickInsertTarget();

    const handleQuickInsert = useCallback(
      async (text: string) => {
        try {
          await invoke("quick_insert_to_target", { text });
        } catch (err) {
          const msg = typeof err === "string" ? err : String(err);
          if (msg.startsWith("focus_failed")) {
            toast.error(t("dashboard.actions.quickInsertErrorFocus"));
          } else if (msg.startsWith("paste_failed")) {
            toast.error(t("dashboard.actions.quickInsertErrorPaste"));
          } else {
            toast.error(t("dashboard.actions.quickInsertErrorNoTarget"));
          }
        }
      },
      [t],
    );

    const [audioUrl, setAudioUrl] = useState<string | null>(null);
    const [audioMissing, setAudioMissing] = useState(false);
    const [isLoadingAudio, setIsLoadingAudio] = useState(false);
    const [shouldAutoPlay, setShouldAutoPlay] = useState(false);
    const [activeTab, setActiveTab] = useState<string>("original");

    const [retranscribing, setRetranscribing] = useState(false);
    const [reprocessing, setReprocessing] = useState(false);
    const [errorPanelOpen, setErrorPanelOpen] = useState(false);

    // Edit dialog state
    const [editDialogOpen, setEditDialogOpen] = useState(false);
    const [editField, setEditField] =
      useState<EditableField>("transcription_text");
    const [editInitialText, setEditInitialText] = useState("");
    const [editStepIndex, setEditStepIndex] = useState<number | undefined>(
      undefined,
    );
    const [editStepLabel, setEditStepLabel] = useState<string | undefined>(
      undefined,
    );

    const historySteps = useMemo(() => {
      if (!entry.post_process_history) return [];
      try {
        return JSON.parse(entry.post_process_history) as PostProcessStep[];
      } catch (e) {
        console.error("Failed to parse post_process_history", e);
        return [];
      }
    }, [entry.post_process_history]);

    const hasSteps = historySteps.length > 0;
    const hasImprovement = !!entry.post_processed_text?.trim() && !hasSteps;
    const hasStreaming = !!entry.streaming_text?.trim();

    const finalStepIdx = hasSteps ? historySteps.length - 1 : -1;
    const intermediateSteps = hasSteps ? historySteps.slice(0, -1) : [];

    // Auto-reset tab to the first/best one when entry changes or gets processed
    useEffect(() => {
      if (hasSteps) {
        setActiveTab(`step:${finalStepIdx}`);
      } else if (hasImprovement) {
        setActiveTab("improved");
      } else {
        setActiveTab("original");
      }
    }, [entry.id, hasImprovement, hasSteps, historySteps.length]);

    const onRetranscribeClick = async () => {
      console.log(
        `[DashboardEntryCard] onRetranscribeClick triggered for entry: ${entry.id}`,
      );
      if (retranscribing) return;
      setRetranscribing(true);
      try {
        await onRetranscribe(entry.id);
      } catch (e) {
        console.error("[DashboardEntryCard] Retranscribe failed", e);
      } finally {
        setRetranscribing(false);
      }
    };

    const onReprocessClick = async (promptId: string) => {
      console.log(
        `[DashboardEntryCard] onReprocessClick triggered for entry: ${entry.id}, prompt: ${promptId}`,
      );
      if (reprocessing) return;
      setReprocessing(true);
      try {
        await onReprocess(entry.id, promptId);
      } catch (e) {
        console.error("[DashboardEntryCard] Reprocess failed", e);
      } finally {
        setReprocessing(false);
      }
    };

    const handleRejectPolish = useCallback(async () => {
      try {
        await invoke("reject_post_process_result", { id: entry.id });
        if (entry.post_processed_text && entry.transcription_text) {
          await invoke("cascade_reject_post_process", {
            transcriptionText: entry.transcription_text,
            postProcessedText: entry.post_processed_text,
          });
        }
      } catch (e) {
        console.error("Failed to reject polish result:", e);
      }
    }, [entry.id, entry.post_processed_text, entry.transcription_text]);

    // Open edit dialog for a specific field
    const openEditDialog = useCallback(
      (
        field: EditableField,
        text: string,
        stepIndex?: number,
        stepLabel?: string,
      ) => {
        setEditField(field);
        setEditInitialText(text);
        setEditStepIndex(stepIndex);
        setEditStepLabel(stepLabel);
        setEditDialogOpen(true);
      },
      [],
    );

    const handleGlobalEdit = useCallback(() => {
      if (activeTab === "original") {
        openEditDialog("transcription_text", entry.transcription_text);
      } else if (activeTab === "streaming") {
        openEditDialog("streaming_text", entry.streaming_text || "");
      } else if (activeTab === "improved") {
        openEditDialog("post_processed_text", entry.post_processed_text || "");
      } else if (activeTab.startsWith("step:")) {
        const idx = parseInt(activeTab.split(":")[1]);
        const step = historySteps[idx];
        if (step) {
          openEditDialog(
            "post_process_history_step",
            step.result,
            idx,
            step.prompt_name,
          );
        }
      }
    }, [activeTab, entry, historySteps, openEditDialog]);

    // Load audio only when requested (on play click)
    const loadAudio = useCallback(async () => {
      if (audioUrl || isLoadingAudio) return; // Already loaded or loading
      setIsLoadingAudio(true);
      setAudioMissing(false);
      try {
        const url = await getAudioUrl(entry.file_name);
        if (url) {
          setAudioUrl(url);
          setShouldAutoPlay(true); // Auto-play after loading
        } else {
          setAudioMissing(true);
        }
      } finally {
        setIsLoadingAudio(false);
      }
    }, [audioUrl, isLoadingAudio, entry.file_name, getAudioUrl]);

    // Logic: Find the prompt object used for this entry
    const usedPrompt = useMemo(() => {
      // 1. Try matching by ID first (Most reliable and dynamic)
      if (entry.post_process_prompt_id) {
        const found = settings?.post_process_prompts.find(
          (p) => p.id === entry.post_process_prompt_id,
        );
        if (found) return found;
      }

      // 2. Fallback to matching by prompt text (for old entries or deleted prompts)
      if (entry.post_process_prompt) {
        return settings?.post_process_prompts.find(
          (p) => p.prompt === entry.post_process_prompt,
        );
      }
      return null;
    }, [
      entry.post_process_prompt_id,
      entry.post_process_prompt,
      settings?.post_process_prompts,
    ]);

    const improvedTabLabel =
      usedPrompt?.name || t("settings.history.content.improved");
    const improvedTabIcon = usedPrompt?.icon || "IconWand";

    // Format duration from entry.duration_ms
    const formatDuration = (ms: number) => {
      const totalSeconds = Math.floor(ms / 1000);
      const minutes = Math.floor(totalSeconds / 60);
      const seconds = totalSeconds % 60;
      return `${minutes}:${seconds.toString().padStart(2, "0")}`;
    };

    const durationText = entry.duration_ms
      ? formatDuration(entry.duration_ms)
      : null;

    const isCancelled = !entry.transcription_text?.trim();

    const availablePrompts = (settings?.post_process_prompts || []).filter(
      (p) => p.source !== "builtin",
    );

    // Build a short, human-readable summary for the warning tooltip. The
    // spec keeps this terse: full diagnostics live in the expanded panel.
    const errorTooltip = useMemo(() => {
      const err = entry.error_summary;
      if (!err) return "";
      if (err.stage === "asr") {
        return t("dashboard.errorIndicator.tooltipAsrEmpty");
      }
      const type = err.error_type.toLowerCase();
      if (type.includes("timeout")) {
        return t("dashboard.errorIndicator.tooltipPolishTimeout");
      }
      const statusMatch = err.detail?.match(/status=?\s*(\d{3})/i);
      if (statusMatch) {
        return t("dashboard.errorIndicator.tooltipPolishProviderError", {
          status: statusMatch[1],
        });
      }
      return t("dashboard.errorIndicator.tooltipPolishFailed");
    }, [entry.error_summary, t]);

    const activeClass = "text-logo-primary! font-medium";

    return (
      <Card
        className="group/card px-1 py-0.5 border-mid-gray/20 border-1 hover:border-mid-gray/40 transition-colors"
        shadow="none"
      >
        <Flex direction="column" className="p-4">
          <Flex justify="between" align="center" className="pb-3">
            <Flex gap="2" align="center" className="flex-wrap">
              <Text size="1" color="gray">
                {timeText}
              </Text>

              {/* Processing Chain Badges - Only shown if step count > 1 */}
              {(entry.streaming_asr_model || hasImprovement || hasSteps) && (
                <Flex
                  gap="1"
                  align="center"
                  className="bg-mid-gray/10 px-1.5 py-0.5 rounded border border-mid-gray/10"
                >
                  <Tooltip content={`ASR: ${entry.asr_model}`}>
                    <IconMicrophone size={12} className="opacity-60" />
                  </Tooltip>

                  {entry.streaming_asr_model && (
                    <>
                      <Text size="1" className="opacity-30">
                        /
                      </Text>
                      <Tooltip
                        content={`Streaming: ${entry.streaming_asr_model}`}
                      >
                        <Box className="w-1.5 h-1.5 rounded-full bg-logo-primary/60 animate-pulse" />
                      </Tooltip>
                    </>
                  )}

                  {(hasImprovement || hasSteps) && (
                    <>
                      <Text size="1" className="opacity-30">
                        →
                      </Text>
                      <Tooltip
                        content={
                          hasSteps
                            ? `${t("settings.history.content.chained")}: ${t("dashboard.details.count", { count: historySteps.length })}`
                            : `AI: ${entry.post_process_model || "Unknown"}`
                        }
                      >
                        <Flex gap="1" align="center">
                          <DynamicIcon
                            name={improvedTabIcon}
                            size={12}
                            className="text-logo-primary opacity-80"
                          />
                          {hasSteps && historySteps.length > 1 && (
                            <Text size="1" color="iris" weight="bold">
                              {historySteps.length}
                            </Text>
                          )}
                        </Flex>
                      </Tooltip>
                    </>
                  )}
                </Flex>
              )}

              <Flex gap="1" align="center">
                {appName && <AppIcon appName={appName} size={14} />}
                <Text size="1" color="gray" weight="medium">
                  {appName || t("common.unknown")}
                </Text>
                {entry.window_title && entry.window_title !== appName && (
                  <>
                    <Text size="1" color="gray" className="opacity-40">
                      ·
                    </Text>
                    <Tooltip content={entry.window_title}>
                      <Text
                        size="1"
                        color="gray"
                        className="max-w-[180px] truncate opacity-70"
                      >
                        {entry.window_title}
                      </Text>
                    </Tooltip>
                  </>
                )}
              </Flex>
              {metaText ? (
                <Text size="1" color="gray">
                  {metaText}
                </Text>
              ) : null}
            </Flex>

            <Flex align="center" gap="2">
              {entry.error_summary && (
                <Tooltip content={errorTooltip}>
                  <IconButton
                    variant="ghost"
                    size="2"
                    color="amber"
                    onClick={() => setErrorPanelOpen((v) => !v)}
                    aria-expanded={errorPanelOpen}
                    aria-label={errorTooltip}
                    className="text-amber-500 hover:bg-amber-500/10 transition-colors"
                  >
                    <IconAlertTriangle className="w-4 h-4" />
                  </IconButton>
                </Tooltip>
              )}

              <Flex
                align="center"
                className="flex gap-2 opacity-0 group-hover/card:opacity-100 transition-opacity duration-200"
              >
                {retranscribing || reprocessing ? (
                  <Text size="1" color="gray" className="animate-pulse mr-2">
                    {t("common.loading")}
                  </Text>
                ) : null}

                <DropdownMenu.Root>
                  <DropdownMenu.Trigger
                    disabled={retranscribing || reprocessing}
                  >
                    <IconButton
                      variant="ghost"
                      size="2"
                      className="text-text/60 hover:text-logo-primary hover:bg-logo-primary/10 transition-colors"
                    >
                      <IconWand
                        className={`w-4 h-4 ${reprocessing ? "animate-pulse" : ""}`}
                      />
                    </IconButton>
                  </DropdownMenu.Trigger>
                  <DropdownMenu.Content variant="soft" color="gray" size="2">
                    {availablePrompts.map((p) => (
                      <DropdownMenu.Item
                        key={p.id}
                        onClick={() => {
                          console.log(
                            `[UI] Clicked prompt item: ${p.name} (${p.id})`,
                          );
                          onReprocessClick(p.id);
                        }}
                        className="cursor-pointer"
                      >
                        <Flex align="center" gap="2" className="w-full">
                          <DynamicIcon
                            name={p.icon || "IconWand"}
                            size={14}
                            className="opacity-70"
                          />
                          <Text className="truncate">{p.name}</Text>
                          {p.output_mode === "chat" && (
                            <span className="shrink-0 text-[8px] px-1 py-0.5 rounded bg-(--accent-a3) text-(--accent-11) font-medium leading-none ml-auto">
                              {t(
                                "settings.postProcessing.prompts.outputMode.chat",
                                "AI Chat",
                              )}
                            </span>
                          )}
                        </Flex>
                      </DropdownMenu.Item>
                    ))}
                    {availablePrompts.length > 0 && <DropdownMenu.Separator />}
                    <DropdownMenu.Item
                      color="red"
                      onClick={() => {
                        console.log("[UI] Clicked retranscribe item");
                        onRetranscribeClick();
                      }}
                      className="cursor-pointer"
                    >
                      {t("dashboard.actions.retranscribe")}
                    </DropdownMenu.Item>
                  </DropdownMenu.Content>{" "}
                </DropdownMenu.Root>

                <Tooltip content={t("settings.history.copyToClipboard")}>
                  <IconButton
                    variant="ghost"
                    size="2"
                    onClick={() => {
                      let text = entry.transcription_text;
                      if (activeTab.startsWith("step:")) {
                        const idx = parseInt(activeTab.split(":")[1]);
                        text =
                          historySteps[idx]?.result ?? entry.transcription_text;
                      } else if (activeTab === "improved") {
                        text =
                          entry.post_processed_text ?? entry.transcription_text;
                      } else if (activeTab === "streaming") {
                        text = entry.streaming_text ?? entry.transcription_text;
                      }
                      onCopy(text ?? "");
                    }}
                    className="text-text/60 hover:text-logo-primary hover:bg-logo-primary/10 transition-colors"
                  >
                    <IconCopy className="w-4 h-4" />
                  </IconButton>
                </Tooltip>

                <Tooltip
                  content={
                    entry.saved
                      ? t("settings.history.unsave")
                      : t("settings.history.save")
                  }
                >
                  <IconButton
                    variant="ghost"
                    size="2"
                    onClick={() => onToggleSaved(entry.id)}
                    className={`transition-all duration-200 active:scale-125 ${
                      entry.saved
                        ? "text-orange-400 hover:text-orange-500 hover:bg-orange-400/10"
                        : "text-text/60 hover:text-orange-400 hover:bg-orange-400/10"
                    }`}
                  >
                    <IconStar
                      className={`w-4 h-4 transition-transform duration-200 ${entry.saved ? "scale-110" : ""}`}
                      fill={entry.saved ? "currentColor" : "none"}
                    />
                  </IconButton>
                </Tooltip>

                {entry.post_processed_text && (
                  <Tooltip content={t("dashboard.actions.rejectPolish")}>
                    <IconButton
                      variant="ghost"
                      size="2"
                      onClick={handleRejectPolish}
                      className="text-text/60 hover:text-red-500 hover:bg-red-500/10 transition-colors"
                    >
                      <IconThumbDown className="w-4 h-4" />
                    </IconButton>
                  </Tooltip>
                )}

                <Tooltip content={t("settings.history.delete")}>
                  <IconButton
                    variant="ghost"
                    size="2"
                    color="red"
                    onClick={() => onDelete(entry.id)}
                    className="text-text/60 hover:text-red-500 hover:bg-red-500/10 transition-colors"
                  >
                    <IconTrash className="w-4 h-4" />
                  </IconButton>
                </Tooltip>
              </Flex>
            </Flex>
          </Flex>

          {entry.error_summary && errorPanelOpen && (
            <Box className="mb-3 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3">
              <Flex direction="column" gap="2">
                <Flex gap="2" align="center">
                  <IconAlertTriangle className="w-4 h-4 text-amber-500" />
                  <Text size="2" weight="medium" className="text-amber-500">
                    {entry.error_summary.stage === "polish"
                      ? t("dashboard.errorIndicator.stagePolish")
                      : t("dashboard.errorIndicator.stageAsr")}
                  </Text>
                </Flex>
                <Box className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
                  <Text size="1" color="gray">
                    {t("dashboard.errorIndicator.panelErrorType")}
                  </Text>
                  <Text size="1">
                    {t(
                      `dashboard.errorIndicator.errorTypeLabel.${entry.error_summary.error_type}`,
                      {
                        defaultValue: entry.error_summary.error_type,
                      },
                    )}
                    <Text size="1" color="gray" className="ml-2 font-mono">
                      ({entry.error_summary.error_type})
                    </Text>
                  </Text>
                  {entry.error_summary.provider && (
                    <>
                      <Text size="1" color="gray">
                        {t("dashboard.errorIndicator.panelProvider")}
                      </Text>
                      <Text size="1" className="break-all">
                        {settings?.post_process_providers?.find(
                          (p) => p.id === entry.error_summary?.provider,
                        )?.label || entry.error_summary.provider}
                      </Text>
                    </>
                  )}
                  {entry.error_summary.model && (
                    <>
                      <Text size="1" color="gray">
                        {t("dashboard.errorIndicator.panelModel")}
                      </Text>
                      <Text size="1" className="font-mono break-all">
                        {entry.error_summary.model}
                      </Text>
                    </>
                  )}
                  {entry.error_summary.detail && (
                    <>
                      <Text size="1" color="gray">
                        {t("dashboard.errorIndicator.panelDetail")}
                      </Text>
                      <Text
                        size="1"
                        className="font-mono whitespace-pre-wrap break-all"
                      >
                        {entry.error_summary.detail}
                      </Text>
                    </>
                  )}
                  <Text size="1" color="gray">
                    {t("dashboard.errorIndicator.panelTimestamp")}
                  </Text>
                  <Text size="1" className="font-mono">
                    {new Date(entry.timestamp * 1000).toLocaleString()}
                  </Text>
                </Box>
              </Flex>
            </Box>
          )}

          {hasSteps || hasImprovement || hasStreaming ? (
            <Tabs.Root
              value={activeTab}
              onValueChange={(v) => setActiveTab(v as any)}
            >
              <Tabs.List size="1" className="flex-wrap gap-y-1 mb-3">
                {/* 1. All Processing Steps (Reverse chronological order - most recent first) */}
                {[...historySteps].reverse().map((step, revIdx) => {
                  const actualIdx = historySteps.length - 1 - revIdx;
                  return (
                    <Tooltip
                      key={actualIdx}
                      content={`${t("common.models")}: ${step.model || "Unknown"}`}
                    >
                      <Tabs.Trigger
                        value={`step:${actualIdx}`}
                        className={`transition-all border-b-2! ${activeTab === `step:${actualIdx}` ? `${activeClass} border-logo-primary/60` : "border-b-transparent!"}`}
                      >
                        <Flex align="center" gap="2">
                          <DynamicIcon
                            name={
                              (step.prompt_id &&
                                settings?.post_process_prompts.find(
                                  (p) => p.id === step.prompt_id,
                                )?.icon) ||
                              "IconWand"
                            }
                            size={14}
                          />
                          {step.prompt_name}
                        </Flex>
                      </Tabs.Trigger>
                    </Tooltip>
                  );
                })}

                {/* 2. Legacy Improved Text (if exists) */}
                {hasImprovement && (
                  <Tooltip
                    content={`${t("common.models")}: ${entry.post_process_model || "Unknown"}`}
                  >
                    <Tabs.Trigger
                      value="improved"
                      className={`transition-all ${activeTab === "improved" ? activeClass : ""}`}
                    >
                      <Flex align="center" gap="2">
                        <DynamicIcon name={improvedTabIcon} size={14} />
                        {improvedTabLabel}
                      </Flex>
                    </Tabs.Trigger>
                  </Tooltip>
                )}

                {/* 3. Original Transcription (The source of truth) */}
                <Tabs.Trigger
                  value="original"
                  className={`transition-all ${activeTab === "original" ? activeClass : ""}`}
                >
                  {t("settings.history.content.original")}
                </Tabs.Trigger>

                {/* 4. Streaming (Realtime) - At the end as it's the most transient data */}
                {hasStreaming && (
                  <Tabs.Trigger
                    value="streaming"
                    className={`transition-all ${activeTab === "streaming" ? activeClass : ""}`}
                  >
                    {t("settings.history.content.streaming")}
                  </Tabs.Trigger>
                )}
              </Tabs.List>
              <Box className="relative group mb-3 bg-mid-gray/5 rounded-lg p-3 border border-mid-gray/10">
                {hasSteps &&
                  historySteps.map((step, idx) => (
                    <Tabs.Content key={idx} value={`step:${idx}`}>
                      <Text className="text-text/90 text-sm leading-relaxed whitespace-pre-wrap wrap-break-word font-mono">
                        {step.result}
                      </Text>
                    </Tabs.Content>
                  ))}
                <Tabs.Content value="improved">
                  <Text className="text-text/90 text-sm leading-relaxed whitespace-pre-wrap wrap-break-word font-mono">
                    {entry.post_processed_text}
                  </Text>
                </Tabs.Content>
                <Tabs.Content value="original">
                  {isCancelled ? (
                    <Text className="text-text/40 text-sm italic">
                      {t("dashboard.details.cancelled")}
                    </Text>
                  ) : (
                    <Text className="text-text/80 text-sm leading-relaxed whitespace-pre-wrap wrap-break-word font-mono">
                      {entry.transcription_text}
                    </Text>
                  )}
                </Tabs.Content>
                <Tabs.Content value="streaming">
                  <Text className="text-text/80 text-sm leading-relaxed whitespace-pre-wrap wrap-break-word font-mono italic">
                    {entry.streaming_text}
                  </Text>
                </Tabs.Content>
                {/* Quick Insert + Edit Buttons for Tabs */}
                <Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-all z-20">
                  <Flex gap="1">
                    <Tooltip
                      content={
                        quickInsertTarget
                          ? t("dashboard.actions.quickInsert", {
                              app: quickInsertTarget.app_name,
                            })
                          : t("dashboard.actions.quickInsertEmpty")
                      }
                    >
                      <IconButton
                        variant="ghost"
                        size="1"
                        aria-disabled={!quickInsertTarget}
                        onClick={() => {
                          if (!quickInsertTarget) return;
                          handleQuickInsert(entry.transcription_text);
                        }}
                        className={
                          "text-logo-primary hover:bg-logo-primary/10 cursor-pointer " +
                          (quickInsertTarget
                            ? ""
                            : "opacity-50 cursor-not-allowed hover:bg-transparent")
                        }
                      >
                        <IconSend size={14} />
                      </IconButton>
                    </Tooltip>
                    <Tooltip content={t("dashboard.actions.edit")}>
                      <IconButton
                        variant="ghost"
                        size="1"
                        onClick={handleGlobalEdit}
                        className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
                      >
                        <IconPencil size={14} />
                      </IconButton>
                    </Tooltip>
                  </Flex>
                </Box>
              </Box>
            </Tabs.Root>
          ) : (
            <Box className="relative group mb-3 bg-mid-gray/5 rounded-lg p-3 border border-mid-gray/10">
              {isCancelled ? (
                <Text className="text-text/40 text-sm italic">
                  {t("dashboard.details.cancelled")}
                </Text>
              ) : (
                <Text className="text-text/80 text-sm leading-relaxed whitespace-pre-wrap wrap-break-word font-mono">
                  {entry.transcription_text}
                </Text>
              )}
              {!isCancelled && (
                <Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-all z-20">
                  <Flex gap="1">
                    <Tooltip
                      content={
                        quickInsertTarget
                          ? t("dashboard.actions.quickInsert", {
                              app: quickInsertTarget.app_name,
                            })
                          : t("dashboard.actions.quickInsertEmpty")
                      }
                    >
                      <IconButton
                        variant="ghost"
                        size="1"
                        aria-disabled={!quickInsertTarget}
                        onClick={() => {
                          if (!quickInsertTarget) return;
                          handleQuickInsert(entry.transcription_text);
                        }}
                        className={
                          "text-logo-primary hover:bg-logo-primary/10 cursor-pointer " +
                          (quickInsertTarget
                            ? ""
                            : "opacity-50 cursor-not-allowed hover:bg-transparent")
                        }
                      >
                        <IconSend size={14} />
                      </IconButton>
                    </Tooltip>
                    <Tooltip content={t("dashboard.actions.edit")}>
                      <IconButton
                        variant="ghost"
                        size="1"
                        onClick={() =>
                          openEditDialog(
                            "transcription_text",
                            entry.transcription_text,
                          )
                        }
                        className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
                      >
                        <IconPencil size={14} />
                      </IconButton>
                    </Tooltip>
                  </Flex>
                </Box>
              )}
            </Box>
          )}

          {/* Audio section */}
          {audioUrl ? (
            <Box className="pt-3 border-t border-white/5">
              <AudioPlayer
                src={audioUrl}
                autoPlay={shouldAutoPlay}
                className="w-full"
                onError={() => {
                  setAudioUrl(null);
                  setAudioMissing(true);
                }}
              />
            </Box>
          ) : audioMissing ? (
            <Box className="pt-3 border-t border-white/5">
              <Text size="2" color="gray">
                {t("dashboard.details.audioRemoved")}
              </Text>
            </Box>
          ) : durationText ? (
            <Box className="pt-3 border-t border-white/5">
              <Flex align="center" gap="3">
                <IconButton
                  variant="ghost"
                  size="2"
                  onClick={loadAudio}
                  disabled={isLoadingAudio}
                  className="text-text/60 hover:text-logo-primary hover:bg-logo-primary/10 transition-colors"
                >
                  <IconPlayerPlay
                    className={`w-4 h-4 ${isLoadingAudio ? "animate-pulse" : ""}`}
                  />
                </IconButton>
                <Box className="flex-1 h-2 bg-mid-gray/15 rounded" />
                <Text size="1" color="gray">
                  {durationText}
                </Text>
              </Flex>
            </Box>
          ) : null}
        </Flex>

        {/* Edit History Dialog */}
        <EditHistoryDialog
          entryId={entry.id}
          field={editField}
          initialText={editInitialText}
          isOpen={editDialogOpen}
          onOpenChange={setEditDialogOpen}
          stepIndex={editStepIndex}
          stepLabel={editStepLabel}
          appName={entry.app_name || undefined}
        />
      </Card>
    );
  },
  (prevProps, nextProps) => {
    return (
      prevProps.entry.id === nextProps.entry.id &&
      prevProps.entry.saved === nextProps.entry.saved &&
      prevProps.entry.transcription_text ===
        nextProps.entry.transcription_text &&
      prevProps.entry.post_processed_text ===
        nextProps.entry.post_processed_text &&
      prevProps.entry.streaming_text === nextProps.entry.streaming_text &&
      prevProps.entry.error_summary?.error_type ===
        nextProps.entry.error_summary?.error_type
    );
  },
);

DashboardEntryCard.displayName = "DashboardEntryCard";
