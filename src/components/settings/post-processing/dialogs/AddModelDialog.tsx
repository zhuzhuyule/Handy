import {
  Badge,
  Box,
  Button,
  Checkbox,
  Dialog,
  Flex,
  Grid,
  ScrollArea,
  SegmentedControl,
  Text,
  TextField,
  Tooltip,
} from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import {
  IconActivity,
  IconFlame,
  IconPlus,
  IconSearch,
} from "@tabler/icons-react";
import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import {
  useModelSpeedStats,
  type ModelSpeedStats,
} from "../../../../hooks/useModelSpeedStats";
import { useSettings } from "../../../../hooks/useSettings";
import { inferModelType } from "../../../../lib/modelTypeUtils";
import type { CachedModel } from "../../../../lib/types";
import type { PostProcessProviderState } from "../../PostProcessingSettingsApi/usePostProcessProviderState";
import { PROVIDER_TEMPLATES } from "../providerTemplates";
import { CustomAddModelDialog } from "./CustomAddModelDialog";

function resolveTemplateMeta(providerId: string) {
  return PROVIDER_TEMPLATES.find((t) => t.id === providerId) ?? null;
}

interface FreeModel {
  id: string;
  name: string;
  capabilities: string;
  price: number;
  provider: string;
  vendor: string;
}

// Unified model option for display
interface ModelOption {
  id: string;
  name: string;
  source: "free" | "api";
  capabilities?: string;
  vendor?: string;
}

function formatSpeed(speed: number): string {
  if (speed >= 1000) return `${(speed / 1000).toFixed(1)}k`;
  if (speed >= 100) return Math.round(speed).toString();
  if (speed >= 10) return speed.toFixed(1);
  return speed.toFixed(2);
}

function formatCalls(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return n.toString();
}

function getModelStats(
  modelId: string,
  providerId: string,
  stats: ModelSpeedStats[],
): { totalCalls: number; totalErrors: number; avgSpeed: number } | null {
  const matched = stats.filter(
    (s) => s.model_id === modelId && s.provider === providerId,
  );
  if (matched.length === 0) return null;
  const totalCalls = matched.reduce((sum, s) => sum + s.total_calls, 0);
  const totalErrors = matched.reduce(
    (sum, s) => sum + (s.total_errors ?? 0),
    0,
  );
  const weightedSpeed = matched.reduce(
    (sum, s) => sum + s.avg_speed * s.total_calls,
    0,
  );
  return {
    totalCalls,
    totalErrors,
    avgSpeed: totalCalls > 0 ? weightedSpeed / totalCalls : 0,
  };
}

const buildCacheId = (modelId: string, providerId: string) => {
  if (globalThis.crypto?.randomUUID) {
    return globalThis.crypto.randomUUID();
  }
  return `${providerId}-${modelId}-${Date.now()}`;
};

interface AddModelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerState: PostProcessProviderState;
  isFetchingModels: boolean;
}

export const AddModelDialog: React.FC<AddModelDialogProps> = ({
  open,
  onOpenChange,
  providerState,
  isFetchingModels,
}) => {
  const { t } = useTranslation();
  const { settings, addCachedModel } = useSettings();
  const { stats: speedStats } = useModelSpeedStats();

  const [source, setSource] = useState<"free" | "api">("free");
  const [query, setQuery] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [freeModels, setFreeModels] = useState<FreeModel[]>([]);
  const [freeLoading, setFreeLoading] = useState(false);
  const [adding, setAdding] = useState(false);
  const [customAddOpen, setCustomAddOpen] = useState(false);

  const templateMeta = resolveTemplateMeta(providerState.selectedProviderId);
  const freeModelProviderKey = templateMeta?.freeModelProvider ?? null;
  const hasFreeModels = freeModelProviderKey !== null;

  const cachedModels = settings?.cached_models ?? [];
  const configuredIds = useMemo(
    () => new Set(cachedModels.map((m) => m.model_id)),
    [cachedModels],
  );

  // Load free models when dialog opens
  useEffect(() => {
    if (open) {
      setSelectedIds(new Set());
      setQuery("");
      setSource(freeModelProviderKey ? "free" : "api");

      // Load free models
      setFreeLoading(true);
      invoke<FreeModel[]>("get_free_models", { provider: freeModelProviderKey })
        .then(setFreeModels)
        .catch(() => setFreeModels([]))
        .finally(() => setFreeLoading(false));

      // Fetch API models if needed
      if (source === "api" && !isFetchingModels) {
        providerState.handleRefreshModels();
      }
    }
  }, [open, providerState.selectedProviderId]);

  // Fetch API models when switching to API source
  useEffect(() => {
    if (open && source === "api" && !isFetchingModels) {
      providerState.handleRefreshModels();
    }
  }, [source]);

  // Build unified model options based on source
  const modelOptions: ModelOption[] = useMemo(() => {
    if (source === "free") {
      return freeModels
        .filter(
          (m) =>
            m.capabilities === "文本生成" ||
            m.capabilities === "speech2text" ||
            m.capabilities === "多模态",
        )
        .map((m) => ({
          id: m.id,
          name: m.name,
          source: "free" as const,
          capabilities: m.capabilities,
          vendor: m.vendor,
        }));
    }
    // API source — use the provider-supplied display label when available
    // (backend fills it from `display_name`/`name`/`title` fields on the
    // provider's /models response). Falls back to the id when missing.
    return providerState.modelOptions.map((o) => ({
      id: o.value,
      name: o.label?.trim() || o.value,
      source: "api" as const,
    }));
  }, [source, freeModels, providerState.modelOptions]);

  // Filter by search
  const filteredOptions = useMemo(() => {
    const lq = query.toLowerCase().trim();
    if (!lq) return modelOptions;
    return modelOptions.filter((m) => {
      const target = `${m.id} ${m.name} ${m.vendor ?? ""}`.toLowerCase();
      return target.includes(lq);
    });
  }, [modelOptions, query]);

  const toggleModel = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleAddSelected = async () => {
    if (selectedIds.size === 0 || !providerState.selectedProviderId) return;
    setAdding(true);
    try {
      for (const modelId of selectedIds) {
        const freeModel = freeModels.find((m) => m.id === modelId);
        const modelType = inferModelType({
          modelId,
          name: freeModel?.name ?? modelId,
          capabilities: freeModel?.capabilities ?? null,
        });

        // Prefer free-catalog name → API display label → bare id as fallback.
        const apiOption = modelOptions.find((o) => o.id === modelId);
        const friendlyName =
          freeModel?.name?.trim() ||
          (apiOption?.name && apiOption.name !== modelId
            ? apiOption.name
            : "") ||
          modelId;
        const newModel: CachedModel = {
          id: buildCacheId(modelId, providerState.selectedProviderId),
          name: friendlyName,
          model_type: modelType,
          provider_id: providerState.selectedProviderId,
          model_id: modelId,
          added_at: new Date().toISOString(),
          is_thinking_model: false,
          prompt_message_role: "system",
        };
        await addCachedModel(newModel);
      }
      toast.success(
        t("settings.postProcessing.models.addedCount", {
          count: selectedIds.size,
          defaultValue: `Added ${selectedIds.size} model(s)`,
        }),
      );
      onOpenChange(false);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setAdding(false);
    }
  };

  const isLoading = source === "free" ? freeLoading : isFetchingModels;

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Content maxWidth="860px">
        <Dialog.Title>
          {t("settings.postProcessing.models.selectModel.title")}
        </Dialog.Title>

        <Flex direction="column" gap="3" className="mt-3">
          {/* Toolbar: source toggle + search */}
          <Flex gap="2" align="center" wrap="wrap">
            {hasFreeModels && (
              <SegmentedControl.Root
                size="1"
                value={source}
                onValueChange={(v) => {
                  setSource(v as "free" | "api");
                  setSelectedIds(new Set());
                }}
              >
                <SegmentedControl.Item value="free">
                  {t(
                    "settings.postProcessing.models.selectModel.sourceBuiltin",
                    "Free",
                  )}
                </SegmentedControl.Item>
                <SegmentedControl.Item value="api">
                  {t(
                    "settings.postProcessing.models.selectModel.sourceOfficial",
                    "API",
                  )}
                </SegmentedControl.Item>
              </SegmentedControl.Root>
            )}

            <Box className="min-w-[160px] flex-1">
              <TextField.Root
                size="2"
                placeholder={t(
                  "settings.postProcessing.models.searchPlaceholder",
                  "Search...",
                )}
                value={query}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) =>
                  setQuery(e.target.value)
                }
              >
                <TextField.Slot>
                  <IconSearch size={14} className="text-(--gray-9)" />
                </TextField.Slot>
              </TextField.Root>
            </Box>

            <Tooltip content="添加自定义模型 ID">
              <Button
                size="1"
                variant="soft"
                color="gray"
                onClick={() => setCustomAddOpen(true)}
              >
                <IconPlus size={14} />
                自定义
              </Button>
            </Tooltip>

            {selectedIds.size > 0 && (
              <Badge variant="solid" size="1">
                {selectedIds.size}
              </Badge>
            )}
          </Flex>

          {/* Model grid */}
          <ScrollArea
            scrollbars="vertical"
            type="hover"
            style={{ maxHeight: 520 }}
            className="[&>.rt-ScrollAreaViewport]:rounded-[var(--radius-3)] [&>.rt-ScrollAreaViewport]:border [&>.rt-ScrollAreaViewport]:border-(--gray-a4) [&>.rt-ScrollAreaViewport]:p-2"
          >
            {isLoading ? (
              <Flex align="center" justify="center" py="8">
                <Text size="2" color="gray">
                  {t("common.loading")}
                </Text>
              </Flex>
            ) : filteredOptions.length === 0 ? (
              <Flex align="center" justify="center" py="8">
                <Text size="2" color="gray">
                  {query
                    ? t(
                        "settings.postProcessing.models.empty.noMatch",
                        "No models match your search.",
                      )
                    : t(
                        "settings.postProcessing.models.selectModel.noFreeModels",
                        "No models available.",
                      )}
                </Text>
              </Flex>
            ) : (
              <Grid columns="3" gap="2">
                {filteredOptions.map((model) => {
                  const isSelected = selectedIds.has(model.id);
                  const alreadyAdded = configuredIds.has(model.id);
                  const stats = getModelStats(
                    model.id,
                    providerState.selectedProviderId,
                    speedStats,
                  );

                  return (
                    <Box
                      key={model.id}
                      onClick={() => toggleModel(model.id)}
                      className={[
                        "relative cursor-pointer select-none rounded-[var(--radius-2)] border px-3 py-2 transition-all duration-100",
                        isSelected
                          ? "border-(--accent-a7) bg-(--accent-a2)"
                          : alreadyAdded
                            ? "border-(--gray-a3) bg-(--gray-a2) hover:border-(--gray-a6)"
                            : "border-(--gray-a4) hover:border-(--gray-a6) hover:bg-(--gray-a2)",
                      ].join(" ")}
                    >
                      <Flex direction="column" gap="1">
                        {/* Row 1: checkbox + name */}
                        <Flex align="center" gap="2" className="min-w-0">
                          <Checkbox
                            size="1"
                            checked={isSelected}
                            tabIndex={-1}
                            className="shrink-0 pointer-events-none"
                          />
                          <Text size="2" weight="medium" className="truncate">
                            {model.name}
                          </Text>
                        </Flex>

                        {/* Row 2: meta info */}
                        <Flex
                          align="center"
                          gap="2"
                          className="min-h-[16px] pl-5"
                        >
                          {model.capabilities && (
                            <Text size="1" className="text-(--gray-8)">
                              {model.capabilities}
                            </Text>
                          )}
                          {alreadyAdded && (
                            <Badge variant="soft" color="blue" size="1">
                              {t(
                                "settings.postProcessing.models.selectModel.alreadyAdded",
                                "Added",
                              )}
                            </Badge>
                          )}
                        </Flex>

                        {/* Row 3: usage stats (if available) */}
                        {stats && stats.totalCalls > 0 && (
                          <Flex align="center" gap="2.5" className="pl-5">
                            <Flex align="center" gap="0.5">
                              <IconActivity
                                size={10}
                                strokeWidth={2.5}
                                className="text-(--gray-8)"
                              />
                              <Text
                                size="1"
                                className="text-(--gray-9) tabular-nums"
                              >
                                {formatCalls(stats.totalCalls)}
                              </Text>
                            </Flex>
                            {stats.avgSpeed > 0 && (
                              <Flex align="center" gap="0.5">
                                <IconFlame
                                  size={10}
                                  strokeWidth={2.5}
                                  className="text-amber-500/50"
                                />
                                <Text
                                  size="1"
                                  className="text-(--gray-9) tabular-nums"
                                >
                                  {formatSpeed(stats.avgSpeed)}
                                  <span className="ml-0.5 opacity-40">t/s</span>
                                </Text>
                              </Flex>
                            )}
                          </Flex>
                        )}
                      </Flex>
                    </Box>
                  );
                })}
              </Grid>
            )}
          </ScrollArea>

          {/* Footer: cancel + add */}
          <Flex justify="end" gap="3">
            <Dialog.Close>
              <Button variant="soft" color="gray">
                {t("common.cancel")}
              </Button>
            </Dialog.Close>
            <Button
              variant="solid"
              onClick={handleAddSelected}
              disabled={selectedIds.size === 0 || adding}
            >
              {t("common.add")}{" "}
              {selectedIds.size > 0 && `(${selectedIds.size})`}
            </Button>
          </Flex>
        </Flex>
      </Dialog.Content>
      <CustomAddModelDialog
        open={customAddOpen}
        onOpenChange={setCustomAddOpen}
        providerId={providerState.selectedProviderId}
        testInferenceInline={providerState.testInferenceInline}
      />
    </Dialog.Root>
  );
};
