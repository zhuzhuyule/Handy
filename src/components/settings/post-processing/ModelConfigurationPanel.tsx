import { invoke } from "@tauri-apps/api/core";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { EditModelDialog } from "./dialogs/EditModelDialog";
import { ModelCardContent } from "../../ui/ModelCard";

import {
  AlertDialog,
  Box,
  Button,
  Flex,
  Grid,
  IconButton,
  SegmentedControl,
  Text,
  TextField,
  Tooltip,
} from "@radix-ui/themes";
import {
  IconEdit,
  IconLayoutList,
  IconPlayerPlay,
  IconSearch,
  IconTrash,
} from "@tabler/icons-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  useModelSpeedStats,
  type ModelSpeedStats,
} from "../../../hooks/useModelSpeedStats";
import { useSettings } from "../../../hooks/useSettings";
import {
  DEFAULT_MODEL_LIST_VIEW_STATE,
  readModelListViewState,
  writeModelListViewState,
} from "../../../lib/modelListViewState";
import { getModelDisplayName } from "../../../lib/modelDisplay";
import { getModelTypeLabel } from "../../../lib/modelTypeUtils";
import type { CachedModel, ModelType } from "../../../lib/types";

// ─── Helpers ────────────────────────────────────────────────────────────────

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

type SortKey = "name" | "calls" | "speed" | "provider";

function getStoredModelListViewState() {
  if (typeof window === "undefined") {
    return DEFAULT_MODEL_LIST_VIEW_STATE;
  }

  return readModelListViewState(window.localStorage);
}

// ─── Model Card ─────────────────────────────────────────────────────────────

const ModelCard: React.FC<{
  model: CachedModel;
  onEditModel: (model: CachedModel) => void;
  handleRemove: (id: string) => void;
  isRemoving: boolean;
  providerName: string;
  showProvider: boolean;
  stats: { totalCalls: number; totalErrors: number; avgSpeed: number } | null;
  t: any;
}> = ({
  model,
  onEditModel,
  handleRemove,
  isRemoving,
  providerName,
  showProvider,
  stats,
  t,
}) => {
  const isAsr = model.model_type === "asr";

  return (
    <Box className="group/card relative rounded-lg bg-(--gray-a2) border-2 border-transparent hover:border-(--gray-a6) transition-all duration-100 overflow-hidden">
      <div className="px-3 py-2.5">
        <ModelCardContent
          name={getModelDisplayName(model)}
          modelId={
            getModelDisplayName(model) !== model.model_id
              ? model.model_id
              : undefined
          }
          subtitle={
            showProvider ? providerName : getModelTypeLabel(model.model_type)
          }
          isAsr={isAsr}
          isThinking={model.is_thinking_model}
          stats={stats}
        />
      </div>

      {/* Hover overlay: actions */}
      <Flex
        align="center"
        justify="center"
        gap="1"
        className="absolute inset-0 opacity-0 group-hover/card:opacity-100 transition-opacity duration-100 bg-(--color-panel-solid)/80 backdrop-blur-[2px]"
      >
        <IconButton
          size="1"
          variant="soft"
          color="green"
          onClick={async (e) => {
            e.stopPropagation();
            const toastId = toast.loading(
              t("settings.postProcessing.api.providers.api.testing"),
            );
            try {
              const isAsrModel = model.model_type === "asr";
              const result = isAsrModel
                ? await invoke<string>("test_asr_model_inference", {
                    modelId: model.model_id,
                    providerId: model.provider_id,
                  })
                : await invoke<any>("test_post_process_model_inference", {
                    providerId: model.provider_id,
                    modelId: model.model_id,
                    cachedModelId: model.id,
                    input: "OK",
                  });

              const resultObj = isAsrModel
                ? ({ content: result as string } as const)
                : (result as {
                    content: string;
                    reasoning_content?: string;
                    duration_ms?: number;
                    total_tokens?: number;
                  });

              // Strip <think>...</think> tags from content
              const rawContent = resultObj.content || "";
              const mainContent = rawContent
                .replace(/<think>[\s\S]*?<\/think>/g, "")
                .trim();
              const hasThinking =
                ("reasoning_content" in resultObj &&
                  !!resultObj.reasoning_content) ||
                rawContent.includes("<think>");

              toast.dismiss(toastId);
              const modelLabel = getModelDisplayName(model);

              // Build stats suffix for non-ASR models
              const statsStr = !isAsrModel
                ? [
                    "duration_ms" in resultObj &&
                      resultObj.duration_ms != null &&
                      `${(resultObj.duration_ms / 1000).toFixed(1)}s`,
                    "total_tokens" in resultObj &&
                      resultObj.total_tokens != null &&
                      `${resultObj.total_tokens} tokens`,
                  ]
                    .filter(Boolean)
                    .join(" · ")
                : "";

              const msg = t(
                "settings.postProcessing.api.providers.api.testSuccess",
                {
                  result: hasThinking
                    ? `[Thinking] ${mainContent}`
                    : mainContent,
                },
              );
              toast.success(
                `${msg} (${modelLabel})${statsStr ? ` · ${statsStr}` : ""}`,
                {
                  duration: 5000,
                  closeButton: true,
                },
              );
            } catch (error) {
              toast.dismiss(toastId);
              const errorMessage =
                error instanceof Error ? error.message : String(error);
              toast.error(
                t("settings.postProcessing.api.providers.testFailed", {
                  error: errorMessage,
                }),
                {
                  duration: Infinity,
                  closeButton: true,
                  style: { color: "red" },
                },
              );
            }
          }}
          title={t("settings.postProcessing.api.providers.testConnection")}
        >
          <IconPlayerPlay size={14} />
        </IconButton>

        <IconButton
          size="1"
          variant="soft"
          color="gray"
          onClick={(e) => {
            e.stopPropagation();
            onEditModel(model);
          }}
          title={t("common.edit", "Edit")}
        >
          <IconEdit size={14} />
        </IconButton>

        <AlertDialog.Root>
          <AlertDialog.Trigger>
            <IconButton
              size="1"
              variant="soft"
              color="red"
              onClick={(e) => e.stopPropagation()}
              disabled={isRemoving}
              title={t("common.delete")}
            >
              <IconTrash size={14} />
            </IconButton>
          </AlertDialog.Trigger>
          <AlertDialog.Content
            maxWidth="450px"
            onClick={(e) => e.stopPropagation()}
          >
            <AlertDialog.Title>
              {t("settings.postProcessing.models.deleteConfirm.title")}
            </AlertDialog.Title>
            <AlertDialog.Description size="2">
              {t("settings.postProcessing.models.deleteConfirm.description")}
            </AlertDialog.Description>
            <Flex gap="3" mt="4" justify="end">
              <AlertDialog.Cancel>
                <Button variant="soft" color="gray">
                  {t("common.cancel")}
                </Button>
              </AlertDialog.Cancel>
              <AlertDialog.Action>
                <Button
                  variant="solid"
                  color="red"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleRemove(model.id);
                  }}
                >
                  {t("common.delete")}
                </Button>
              </AlertDialog.Action>
            </Flex>
          </AlertDialog.Content>
        </AlertDialog.Root>
      </Flex>
    </Box>
  );
};

// ─── Panel ──────────────────────────────────────────────────────────────────

export interface ModelListPanelProps {
  targetType: ModelType | ModelType[];
  /** null = show all, string = filter by this provider */
  providerFilter: string | null;
  onProviderFilterChange: (providerId: string | null) => void;
}

export const ModelListPanel: React.FC<ModelListPanelProps> = ({
  targetType,
  providerFilter,
  onProviderFilterChange,
}) => {
  const { settings, removeCachedModel, isUpdating, refreshSettings } =
    useSettings();
  const { stats: speedStats } = useModelSpeedStats();
  const [editingModel, setEditingModel] = useState<CachedModel | null>(null);
  const [sortKey, setSortKey] = useState<SortKey>(
    () => getStoredModelListViewState().sortKey,
  );
  const [typeFilter, setTypeFilter] = useState<"all" | ModelType>(
    () => getStoredModelListViewState().typeFilter,
  );
  const [query, setQuery] = useState(() => getStoredModelListViewState().query);
  const { t } = useTranslation();

  const cachedModels = settings?.cached_models ?? [];
  const allTypes = useMemo(
    () => (Array.isArray(targetType) ? targetType : [targetType]),
    [targetType],
  );

  const providerNameMap = useMemo(() => {
    const map: Record<string, string> = {};
    settings?.post_process_providers.forEach((p) => {
      map[p.id] = p.label;
    });
    return map;
  }, [settings?.post_process_providers]);

  // Filter + sort
  const filteredModels = useMemo(() => {
    const lowerQuery = query.toLowerCase().trim();

    // Determine active types
    let activeTypes: ModelType[];
    if (typeFilter === "all") {
      activeTypes = allTypes;
    } else if (typeFilter === "asr") {
      activeTypes = allTypes.filter((t) => t === "asr" || t === "other");
    } else {
      activeTypes = allTypes.filter((t) => t === typeFilter);
    }

    let models = cachedModels.filter((m) => {
      if (!activeTypes.includes(m.model_type)) return false;
      if (!providerNameMap[m.provider_id]) return false;
      if (providerFilter && m.provider_id !== providerFilter) return false;
      if (lowerQuery) {
        const target =
          `${m.model_id} ${m.custom_label ?? ""} ${m.name}`.toLowerCase();
        if (!target.includes(lowerQuery)) return false;
      }
      return true;
    });

    models = [...models].sort((a, b) => {
      switch (sortKey) {
        case "name": {
          const nameA = getModelDisplayName(a).toLowerCase();
          const nameB = getModelDisplayName(b).toLowerCase();
          return nameA.localeCompare(nameB);
        }
        case "calls": {
          const sa = getModelStats(a.model_id, a.provider_id, speedStats);
          const sb = getModelStats(b.model_id, b.provider_id, speedStats);
          return (sb?.totalCalls ?? 0) - (sa?.totalCalls ?? 0);
        }
        case "speed": {
          const sa = getModelStats(a.model_id, a.provider_id, speedStats);
          const sb = getModelStats(b.model_id, b.provider_id, speedStats);
          return (sb?.avgSpeed ?? 0) - (sa?.avgSpeed ?? 0);
        }
        case "provider": {
          const provCmp = (providerNameMap[a.provider_id] ?? "").localeCompare(
            providerNameMap[b.provider_id] ?? "",
          );
          if (provCmp !== 0) return provCmp;
          return getModelDisplayName(a).localeCompare(getModelDisplayName(b));
        }
        default:
          return 0;
      }
    });

    return models;
  }, [
    cachedModels,
    allTypes,
    typeFilter,
    providerNameMap,
    providerFilter,
    query,
    sortKey,
    speedStats,
  ]);

  const handleRemoveModel = useCallback(
    async (modelId: string) => {
      await removeCachedModel(modelId);
    },
    [removeCachedModel],
  );

  const isShowingAll = !providerFilter;
  const [grouped, setGrouped] = useState(
    () => getStoredModelListViewState().grouped,
  );
  const effectiveGrouped = isShowingAll ? grouped : false;

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }

    writeModelListViewState(window.localStorage, {
      ...getStoredModelListViewState(),
      providerFilter,
      grouped,
      sortKey,
      typeFilter,
      query,
    });
  }, [grouped, providerFilter, query, sortKey, typeFilter]);

  // Group models by provider
  const groupedModels = useMemo(() => {
    if (!effectiveGrouped) return null;
    const groups: Record<string, CachedModel[]> = {};
    filteredModels.forEach((m) => {
      if (!groups[m.provider_id]) groups[m.provider_id] = [];
      groups[m.provider_id].push(m);
    });
    return Object.entries(groups).sort(([a], [b]) =>
      (providerNameMap[a] ?? a).localeCompare(providerNameMap[b] ?? b),
    );
  }, [effectiveGrouped, filteredModels, providerNameMap]);

  // In flat "all" mode, show provider name on each card
  const showProviderOnCard = isShowingAll && !effectiveGrouped;

  return (
    <Box>
      {/* Toolbar: search + type filter + group toggle + sort */}
      <Flex gap="2" align="center" wrap="wrap" className="mb-3">
        <Box className="w-[200px] min-w-[140px]">
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

        <SegmentedControl.Root
          size="1"
          value={typeFilter}
          onValueChange={(v) => setTypeFilter(v as "all" | ModelType)}
        >
          <SegmentedControl.Item value="all">
            {t("settings.postProcessing.models.filter.all", "All")}
          </SegmentedControl.Item>
          <SegmentedControl.Item value="text">
            {t("settings.postProcessing.models.modelTypes.text.label")}
          </SegmentedControl.Item>
          <SegmentedControl.Item value="asr">
            {t("settings.postProcessing.models.modelTypes.asr.label")}
          </SegmentedControl.Item>
        </SegmentedControl.Root>

        {/* Group toggle — only in "all" mode */}
        {isShowingAll && (
          <Tooltip
            content={t(
              "settings.postProcessing.models.sort.group",
              "Group by provider",
            )}
            delayDuration={200}
          >
            <IconButton
              size="1"
              variant={grouped ? "solid" : "soft"}
              color={grouped ? "indigo" : "gray"}
              onClick={() => setGrouped(!grouped)}
            >
              <IconLayoutList size={14} />
            </IconButton>
          </Tooltip>
        )}

        <Box className="flex-1" />

        <SegmentedControl.Root
          size="1"
          value={sortKey}
          onValueChange={(v) => setSortKey(v as SortKey)}
        >
          <SegmentedControl.Item value="name">
            {t("settings.postProcessing.models.sort.name", "Name")}
          </SegmentedControl.Item>
          <SegmentedControl.Item value="calls">
            {t("settings.postProcessing.models.sort.calls", "Calls")}
          </SegmentedControl.Item>
          <SegmentedControl.Item value="speed">
            {t("settings.postProcessing.models.sort.speed", "Speed")}
          </SegmentedControl.Item>
          {/* Provider sort — useful in flat all mode */}
          {isShowingAll && !effectiveGrouped && (
            <SegmentedControl.Item value="provider">
              {t("settings.postProcessing.models.sort.provider", "Provider")}
            </SegmentedControl.Item>
          )}
        </SegmentedControl.Root>
      </Flex>

      {/* Model content */}
      {filteredModels.length > 0 ? (
        groupedModels ? (
          // Grouped mode
          <Flex direction="column" gap="4">
            {groupedModels.map(([providerId, models]) => (
              <Box key={providerId}>
                <Flex align="center" gap="2" className="mb-2 px-0.5">
                  <Text
                    size="1"
                    weight="bold"
                    className="uppercase tracking-widest text-(--gray-9)"
                  >
                    {providerNameMap[providerId] ?? providerId}
                  </Text>
                  <Text size="1" className="text-(--gray-8) tabular-nums">
                    {models.length}
                  </Text>
                </Flex>
                <Grid columns={{ initial: "2", sm: "3" }} gap="2">
                  {models.map((model) => (
                    <ModelCard
                      key={model.id}
                      model={model}
                      onEditModel={setEditingModel}
                      handleRemove={handleRemoveModel}
                      isRemoving={
                        !!isUpdating(`cached_model_remove:${model.id}`)
                      }
                      providerName={
                        providerNameMap[model.provider_id] ?? model.provider_id
                      }
                      showProvider={false}
                      stats={getModelStats(
                        model.model_id,
                        model.provider_id,
                        speedStats,
                      )}
                      t={t}
                    />
                  ))}
                </Grid>
              </Box>
            ))}
          </Flex>
        ) : (
          // Flat mode
          <Grid columns={{ initial: "2", sm: "3" }} gap="2">
            {filteredModels.map((model) => (
              <ModelCard
                key={model.id}
                model={model}
                onEditModel={setEditingModel}
                handleRemove={handleRemoveModel}
                isRemoving={!!isUpdating(`cached_model_remove:${model.id}`)}
                providerName={
                  providerNameMap[model.provider_id] ?? model.provider_id
                }
                showProvider={showProviderOnCard}
                stats={getModelStats(
                  model.model_id,
                  model.provider_id,
                  speedStats,
                )}
                t={t}
              />
            ))}
          </Grid>
        )
      ) : (
        <Flex
          align="center"
          justify="center"
          py="8"
          className="rounded-[var(--radius-4)] border border-dashed border-(--gray-a5)"
        >
          <Text size="2" className="text-(--gray-8)">
            {query
              ? t(
                  "settings.postProcessing.models.empty.noMatch",
                  "No models match your search.",
                )
              : t("settings.postProcessing.models.empty.description")}
          </Text>
        </Flex>
      )}

      {editingModel && (
        <EditModelDialog
          model={editingModel}
          onClose={() => setEditingModel(null)}
          onSave={async () => {
            await refreshSettings();
            setEditingModel(null);
          }}
        />
      )}
    </Box>
  );
};

ModelListPanel.displayName = "ModelListPanel";
