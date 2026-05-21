import {
  Box,
  Button,
  Flex,
  IconButton,
  SegmentedControl,
  Select,
  Switch,
  Text,
  Tooltip,
} from "@radix-ui/themes";
import {
  IconAlertTriangle,
  IconChevronDown,
  IconChevronRight,
  IconGripVertical,
  IconPlus,
  IconPlayerSkipForward,
  IconTarget,
  IconTrash,
} from "@tabler/icons-react";
import { invoke } from "@tauri-apps/api/core";
import React, { useCallback, useEffect, useState } from "react";
import { AppPickerDialog } from "./dialogs/AppPickerDialog";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../../hooks/useSettings";
import {
  getAppIconUrl,
  isAppInstalled,
  useInstalledApps,
} from "../../../hooks/useInstalledApps";
import {
  AppProfile,
  AppReviewPolicy,
  TitleMatchType,
  TitleRule,
} from "../../../lib/types";
import { DynamicIcon, IconPicker } from "../../shared/IconPicker";
import { Card } from "../../ui/Card";
import { useSkills } from "./prompts/hooks/useSkills";

// dnd-kit imports
import {
  closestCenter,
  DndContext,
  DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

// Sub-rule row component
interface TitleRuleRowProps {
  rule: TitleRule;
  prompts: any[];
  onUpdate: (updates: Partial<TitleRule>) => void;
  onRemove: () => void;
  t: any;
}

const TitleRuleRow: React.FC<TitleRuleRowProps> = ({
  rule,
  prompts,
  onUpdate,
  onRemove,
  t,
}) => {
  const [matchStatus, setMatchStatus] = useState<boolean | null>(null);
  const [isTesting, setIsTesting] = useState(false);
  const [countdown, setCountdown] = useState(0);

  // Capture active window: if pattern exists, test match; otherwise fill pattern
  const handleCapture = useCallback(async () => {
    setIsTesting(true);
    setCountdown(3);
    setMatchStatus(null);

    const timer = setInterval(() => {
      setCountdown((prev) => {
        if (prev <= 1) {
          clearInterval(timer);
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    setTimeout(async () => {
      try {
        const info: any = await invoke("get_active_window_info");
        console.log(
          "[TitleRule] Active window info:",
          JSON.stringify(info, null, 2),
        );
        if (info && info.title) {
          if (rule.pattern.trim()) {
            // Has pattern: test match and show result
            let matched = false;
            try {
              if (rule.match_type === "text") {
                matched = info.title
                  .toLowerCase()
                  .includes(rule.pattern.toLowerCase());
              } else if (rule.match_type === "exact") {
                matched =
                  info.title.toLowerCase() === rule.pattern.toLowerCase();
              } else {
                const re = new RegExp(rule.pattern);
                matched = re.test(info.title);
              }
            } catch {
              matched = false;
            }
            setMatchStatus(matched);
            // Auto-hide after 3 seconds
            setTimeout(() => setMatchStatus(null), 3000);
          } else {
            // No pattern: fill with window title
            onUpdate({ pattern: info.title });
          }
        } else {
          console.warn(
            "[TitleRule] No title in window info, using app_name as fallback",
          );
          if (!rule.pattern.trim() && info?.app_name) {
            onUpdate({ pattern: info.app_name });
          }
        }
      } catch (e) {
        console.error("Failed to get active window", e);
      } finally {
        setIsTesting(false);
      }
    }, 3000);
  }, [rule.pattern, rule.match_type, onUpdate]);

  return (
    <>
      <Flex
        align="center"
        justify="between"
        gap="2"
        className="pl-10 py-1.5 border-l-2 border-[var(--gray-4)] ml-4"
      >
        {/* Left: pattern input and match type */}
        <Flex align="center" gap="2" style={{ flex: 1 }}>
          <input
            type="text"
            value={rule.pattern}
            onChange={(e) => onUpdate({ pattern: e.target.value })}
            placeholder={rule.match_type === "regex" ? "(?i)github" : "github"}
            className="w-40 px-2 py-1 text-xs rounded border border-[var(--gray-6)] bg-[var(--gray-1)] text-[var(--gray-12)] placeholder:text-[var(--gray-9)] focus:outline-none focus:border-[var(--accent-8)]"
          />
          <SegmentedControl.Root
            size="1"
            value={rule.match_type}
            onValueChange={(val) =>
              onUpdate({ match_type: val as TitleMatchType })
            }
          >
            <SegmentedControl.Item value="text">Text</SegmentedControl.Item>
            <SegmentedControl.Item value="exact">Exact</SegmentedControl.Item>
            <SegmentedControl.Item value="regex">Regex</SegmentedControl.Item>
          </SegmentedControl.Root>

          {/* Capture/Test Button */}
          <Tooltip
            content={
              rule.pattern.trim()
                ? t("settings.postProcessing.appRules.testMatch")
                : t("settings.postProcessing.appRules.captureTitle")
            }
          >
            <IconButton
              variant="ghost"
              size="1"
              onClick={handleCapture}
              disabled={isTesting}
            >
              {isTesting ? (
                <Text size="1">{countdown}s</Text>
              ) : matchStatus !== null ? (
                <Text
                  size="1"
                  color={matchStatus ? "green" : "red"}
                  weight="bold"
                >
                  {matchStatus ? "✓" : "✗"}
                </Text>
              ) : (
                <IconTarget size={12} />
              )}
            </IconButton>
          </Tooltip>
        </Flex>

        {/* Right: policy/prompt aligned with parent */}
        <Flex align="center" gap="2">
          <Select.Root
            size="1"
            value={rule.policy}
            onValueChange={(val) =>
              onUpdate({ policy: val as AppReviewPolicy })
            }
          >
            <Select.Trigger variant="surface" style={{ width: 100 }} />
            <Select.Content position="popper">
              <Select.Item value="always">
                {t("settings.postProcessing.appRules.policy.always")}
              </Select.Item>
              <Select.Item value="never">
                {t("settings.postProcessing.appRules.policy.never")}
              </Select.Item>
            </Select.Content>
          </Select.Root>
          <Select.Root
            size="1"
            value={rule.prompt_id || "default"}
            onValueChange={(val) =>
              onUpdate({ prompt_id: val === "default" ? null : val })
            }
          >
            <Select.Trigger variant="surface" style={{ width: 120 }} />
            <Select.Content position="popper" style={{ maxHeight: 360 }}>
              <Select.Item value="default">{t("common.default")}</Select.Item>
              {prompts.map((p) => (
                <Select.Item key={p.id} value={p.id}>
                  <Flex gap="2" align="center">
                    <DynamicIcon name={p.icon || "IconWand"} size={14} />
                    <span>{p.name}</span>
                  </Flex>
                </Select.Item>
              ))}
              <Select.Separator />
              <Select.Item value="__SKIP_POST_PROCESS__">
                <Flex gap="2" align="center">
                  <IconPlayerSkipForward size={14} />
                  <span>
                    {t("settings.postProcessing.appRules.skipPostProcess")}
                  </span>
                </Flex>
              </Select.Item>
            </Select.Content>
          </Select.Root>
          <IconButton variant="ghost" color="red" size="1" onClick={onRemove}>
            <IconTrash size={12} />
          </IconButton>
        </Flex>
      </Flex>
    </>
  );
};

// Profile group card component
interface ProfileGroupCardProps {
  appName: string;
  profile: AppProfile;
  prompts: any[];
  onUpdateProfile: (profileId: string, updates: Partial<AppProfile>) => void;
  onRemoveProfile: (appName: string, profileId: string) => void;
  t: any;
}

const ProfileGroupCard: React.FC<ProfileGroupCardProps> = ({
  appName,
  profile,
  prompts,
  onUpdateProfile,
  onRemoveProfile,
  t,
}) => {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: profile.id });

  // Subscribe to installed apps cache so this card re-renders when it updates
  useInstalledApps();

  // Ensure rules is always an array (handle legacy data)
  const rules = profile.rules || [];
  const [expanded, setExpanded] = useState(rules.length > 0);

  // Resolve display icon: user custom override > native app icon > fallback
  const nativeIcon = getAppIconUrl(appName);
  const displayIcon = profile.icon ?? nativeIcon;
  const installedStatus = isAppInstalled(appName);
  const isMissing = installedStatus === false;

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 10 : 1,
    opacity: isDragging ? 0.5 : 1,
  };

  const handleAddRule = () => {
    const newRule: TitleRule = {
      id: `rule_${Date.now()}_${Math.random().toString(36).substr(2, 5)}`,
      pattern: "",
      match_type: "text",
      policy: "always",
      prompt_id: null,
    };
    onUpdateProfile(profile.id, { rules: [...rules, newRule] });
    setExpanded(true);
  };

  const handleUpdateRule = (ruleId: string, updates: Partial<TitleRule>) => {
    const newRules = rules.map((r) =>
      r.id === ruleId ? { ...r, ...updates } : r,
    );
    onUpdateProfile(profile.id, { rules: newRules });
  };

  const handleRemoveRule = (ruleId: string) => {
    const newRules = rules.filter((r) => r.id !== ruleId);
    onUpdateProfile(profile.id, { rules: newRules });
  };

  return (
    <div ref={setNodeRef} style={style}>
      <Card className="p-3 shadow-sm hover:shadow-md transition-shadow">
        <Flex direction="column" gap="2">
          {/* Group Header */}
          <Flex align="center" justify="between" gap="3">
            <Flex align="center" gap="2" style={{ flex: 1, minWidth: 0 }}>
              <Box
                {...attributes}
                {...listeners}
                className="cursor-grab active:cursor-grabbing p-1 text-[var(--gray-8)] hover:text-[var(--gray-11)] transition-colors"
              >
                <IconGripVertical size={18} />
              </Box>
              {rules.length > 0 && (
                <IconButton
                  variant="ghost"
                  size="1"
                  onClick={() => setExpanded(!expanded)}
                >
                  {expanded ? (
                    <IconChevronDown size={14} />
                  ) : (
                    <IconChevronRight size={14} />
                  )}
                </IconButton>
              )}
              <IconPicker
                value={displayIcon}
                onChange={(icon) => onUpdateProfile(profile.id, { icon })}
              />
              <IconButton variant="ghost" size="1" onClick={handleAddRule}>
                <IconPlus size={14} />
              </IconButton>
              <Text
                weight="bold"
                size="2"
                color={isMissing ? "gray" : undefined}
                style={
                  isMissing ? { textDecoration: "line-through" } : undefined
                }
              >
                {appName}
              </Text>
              {isMissing && (
                <Tooltip
                  content={t(
                    "settings.postProcessing.appRules.appNotInstalled",
                    "This app is no longer installed",
                  )}
                >
                  <IconAlertTriangle
                    size={14}
                    color="var(--amber-9)"
                    style={{ flexShrink: 0 }}
                  />
                </Tooltip>
              )}
              {rules.length > 0 && (
                <Text size="1" color="gray">
                  ({rules.length})
                </Text>
              )}
            </Flex>

            {/* Default settings */}
            <Flex align="center" gap="2">
              <Tooltip
                content={t(
                  "settings.postProcessing.appRules.selectionClipboardFallback.description",
                )}
              >
                <Flex align="center" gap="2" px="1">
                  <Text size="1" color="gray">
                    {t(
                      "settings.postProcessing.appRules.selectionClipboardFallback.label",
                    )}
                  </Text>
                  <Switch
                    checked={!profile.disable_selection_clipboard_fallback}
                    onCheckedChange={(checked) =>
                      onUpdateProfile(profile.id, {
                        disable_selection_clipboard_fallback: !checked,
                      })
                    }
                    className="data-[state=checked]:bg-logo-primary/90"
                  />
                </Flex>
              </Tooltip>

              <Tooltip
                content={t(
                  "settings.postProcessing.appRules.translateToEnglishOnInsert.description",
                )}
              >
                <Flex align="center" gap="2" px="1">
                  <Text size="1" color="gray">
                    {t(
                      "settings.postProcessing.appRules.translateToEnglishOnInsert.label",
                    )}
                  </Text>
                  <Switch
                    checked={profile.translate_to_english_on_insert ?? false}
                    onCheckedChange={(checked) =>
                      onUpdateProfile(profile.id, {
                        translate_to_english_on_insert: checked,
                      })
                    }
                    className="data-[state=checked]:bg-logo-primary/90"
                  />
                </Flex>
              </Tooltip>

              <Select.Root
                size="1"
                value={profile.policy}
                onValueChange={(val) =>
                  onUpdateProfile(profile.id, {
                    policy: val as AppReviewPolicy,
                  })
                }
              >
                <Select.Trigger variant="surface" style={{ width: 100 }} />
                <Select.Content position="popper">
                  <Select.Item value="always">
                    {t("settings.postProcessing.appRules.policy.always")}
                  </Select.Item>
                  <Select.Item value="never">
                    {t("settings.postProcessing.appRules.policy.never")}
                  </Select.Item>
                </Select.Content>
              </Select.Root>

              <Select.Root
                size="1"
                value={profile.prompt_id || "default"}
                onValueChange={(val) =>
                  onUpdateProfile(profile.id, {
                    prompt_id: val === "default" ? null : val,
                  })
                }
              >
                <Select.Trigger variant="surface" style={{ width: 120 }} />
                <Select.Content position="popper" style={{ maxHeight: 360 }}>
                  <Select.Item value="default">
                    {t("common.default")}
                  </Select.Item>
                  {prompts.map((p) => (
                    <Select.Item key={p.id} value={p.id}>
                      <Flex gap="2" align="center">
                        <DynamicIcon name={p.icon || "IconWand"} size={14} />
                        <span>{p.name}</span>
                      </Flex>
                    </Select.Item>
                  ))}
                  <Select.Separator />
                  <Select.Item value="__SKIP_POST_PROCESS__">
                    <Flex gap="2" align="center">
                      <IconPlayerSkipForward size={14} />
                      <span>
                        {t("settings.postProcessing.appRules.skipPostProcess")}
                      </span>
                    </Flex>
                  </Select.Item>
                </Select.Content>
              </Select.Root>

              <IconButton
                variant="ghost"
                color="red"
                size="1"
                onClick={() => onRemoveProfile(appName, profile.id)}
              >
                <IconTrash size={12} />
              </IconButton>
            </Flex>
          </Flex>

          {/* Sub-rules */}
          {expanded && rules.length > 0 && (
            <Flex direction="column" gap="1">
              {rules.map((rule) => (
                <TitleRuleRow
                  key={rule.id}
                  rule={rule}
                  prompts={prompts}
                  onUpdate={(updates) => handleUpdateRule(rule.id, updates)}
                  onRemove={() => handleRemoveRule(rule.id)}
                  t={t}
                />
              ))}
            </Flex>
          )}
        </Flex>
      </Card>
    </div>
  );
};

export const AppProfilesManager: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const { skills } = useSkills();
  const [isCapturing, setIsCapturing] = useState(false);
  const [countdown, setCountdown] = useState(0);
  const [suggestedApps, setSuggestedApps] = useState<string[]>([]);
  const [appPickerOpen, setAppPickerOpen] = useState(false);
  // Warm installed apps cache on mount; also re-renders when cache updates.
  useInstalledApps();

  const profiles = settings?.app_profiles || [];
  const appToProfile = settings?.app_to_profile || {};
  const prompts = skills;

  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const fetchSuggestions = useCallback(async () => {
    try {
      const entries: any[] = await invoke("get_history_entries");
      const counts = new Map<string, number>();
      entries.forEach((e) => {
        const name = e.app_name?.trim();
        if (name && name !== "Votype" && name !== "Handy") {
          counts.set(name, (counts.get(name) ?? 0) + 1);
        }
      });
      const assignedApps = new Set(Object.keys(appToProfile));
      const top = Array.from(counts.entries())
        .sort((a, b) => b[1] - a[1])
        .filter(([name]) => !assignedApps.has(name))
        .slice(0, 10)
        .map(([name]) => name);
      setSuggestedApps(top);
    } catch (e) {
      console.error("Failed to fetch app suggestions", e);
    }
  }, [appToProfile]);

  useEffect(() => {
    fetchSuggestions();
  }, [fetchSuggestions]);

  const handleUpdateProfile = useCallback(
    async (profileId: string, updates: Partial<AppProfile>) => {
      const newProfiles = profiles.map((p) =>
        p.id === profileId ? { ...p, ...updates } : p,
      );
      await updateSetting("app_profiles", newProfiles);
    },
    [profiles, updateSetting],
  );

  const handleAssignApp = useCallback(
    async (appName: string) => {
      if (!appName.trim()) return;
      if (appToProfile[appName]) return;

      const profileId = `profile_${Date.now()}_${Math.random().toString(36).substr(2, 5)}`;
      // Icon is null by default — native icon is resolved at render time
      // from the installed apps cache. User can override via IconPicker.
      const newProfile: AppProfile = {
        id: profileId,
        name: appName,
        policy: "always",
        prompt_id: null,
        icon: null,
        translate_to_english_on_insert: false,
        disable_selection_clipboard_fallback: false,
        rules: [],
      };

      const newProfiles = [...profiles, newProfile];
      const newAppToProfile = { ...appToProfile, [appName]: profileId };

      await updateSetting("app_profiles", newProfiles);
      await updateSetting("app_to_profile", newAppToProfile);

      setCountdown(0);
      setIsCapturing(false);
    },
    [profiles, appToProfile, updateSetting],
  );

  const handleRemoveProfile = useCallback(
    async (appName: string, profileId: string) => {
      const newAppToProfile = { ...appToProfile };
      delete newAppToProfile[appName];

      const newProfiles = profiles.filter((p) => p.id !== profileId);

      await updateSetting("app_profiles", newProfiles);
      await updateSetting("app_to_profile", newAppToProfile);
    },
    [profiles, appToProfile, updateSetting],
  );

  const handleDragEnd = useCallback(
    async (event: DragEndEvent) => {
      const { active, over } = event;
      if (over && active.id !== over.id) {
        const oldIndex = profiles.findIndex((p) => p.id === active.id);
        const newIndex = profiles.findIndex((p) => p.id === over.id);
        const newProfiles = arrayMove(profiles, oldIndex, newIndex);
        await updateSetting("app_profiles", newProfiles);
      }
    },
    [profiles, updateSetting],
  );

  const captureAndAssign = useCallback(async () => {
    setIsCapturing(true);
    setCountdown(3);

    const timer = setInterval(() => {
      setCountdown((prev) => {
        if (prev <= 1) {
          clearInterval(timer);
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    setTimeout(async () => {
      try {
        const info: any = await invoke("get_active_window_info");
        if (info && info.app_name) {
          if (info.app_name !== "Votype" && info.app_name !== "Handy") {
            await handleAssignApp(info.app_name);
          }
        }
      } catch (e) {
        console.error("Failed to capture active window", e);
      } finally {
        setIsCapturing(false);
      }
    }, 3000);
  }, [handleAssignApp]);

  const rules = profiles
    .map((profile) => {
      const appName = Object.entries(appToProfile).find(
        ([_, pid]) => pid === profile.id,
      )?.[0];
      if (!appName) return null;
      return { appName, profile };
    })
    .filter((r): r is { appName: string; profile: AppProfile } => r !== null);

  return (
    <Flex direction="column" gap="4">
      {/* Header Actions */}
      <Flex justify="between" align="center" px="1">
        <Text size="1" color="gray" weight="medium">
          {rules.length > 0
            ? t("settings.postProcessing.appRules.activeProfiles", {
                count: rules.length,
              })
            : t("settings.postProcessing.appRules.noProfiles")}
        </Text>
        <Flex gap="2">
          <Button
            variant="soft"
            size="2"
            style={{ minWidth: "fit-content" }}
            onClick={() => setAppPickerOpen(true)}
          >
            <IconPlus size={16} />
            {t("settings.postProcessing.appRules.addApp")}
          </Button>

          <Button
            variant="solid"
            size="2"
            onClick={captureAndAssign}
            disabled={isCapturing}
          >
            {isCapturing
              ? `${countdown}s`
              : t("settings.postProcessing.appRules.capture")}
          </Button>
        </Flex>

        <AppPickerDialog
          open={appPickerOpen}
          onOpenChange={setAppPickerOpen}
          onSelect={(name) => handleAssignApp(name)}
          excludedApps={Object.keys(appToProfile)}
          recentApps={suggestedApps}
        />
      </Flex>

      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext
          items={rules.map((r) => r.profile.id)}
          strategy={verticalListSortingStrategy}
        >
          <Flex direction="column" gap="2">
            {rules.map(({ appName, profile }) => (
              <ProfileGroupCard
                key={profile.id}
                appName={appName}
                profile={profile}
                prompts={prompts}
                onUpdateProfile={handleUpdateProfile}
                onRemoveProfile={handleRemoveProfile}
                t={t}
              />
            ))}

            {rules.length === 0 && (
              <Flex
                direction="column"
                align="center"
                justify="center"
                py="8"
                className="border-2 border-dashed border-[var(--gray-4)] rounded-xl bg-[var(--gray-2)]/50"
                gap="2"
              >
                <Text size="2" color="gray">
                  {t("settings.postProcessing.appRules.noAppsAssigned")}
                </Text>
                <Text size="1" color="gray" className="opacity-60">
                  {t("settings.postProcessing.appRules.createPromptHint")}
                </Text>
              </Flex>
            )}
          </Flex>
        </SortableContext>
      </DndContext>
    </Flex>
  );
};
