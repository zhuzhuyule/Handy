import { Box, Flex, Switch, Text, Tooltip } from "@radix-ui/themes";
import {
  IconAbc,
  IconAdjustments,
  IconApps,
  IconBrain,
  IconChartBar,
  IconInfoCircle,
  IconKeyboard,
  IconLayoutDashboard,
  IconSchool,
  IconSettings,
  IconNotes,
} from "@tabler/icons-react";
import React, { lazy, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "../hooks/useSettings";
import VotypeHand from "./icons/VotypeHand";

// 使用懒加载导入所有设置组件，减少初始 bundle 大小
const Dashboard = lazy(() =>
  import("./settings/dashboard/Dashboard").then((m) => ({
    default: m.Dashboard,
  })),
);
const GeneralSettings = lazy(() =>
  import("./settings/general/GeneralSettings").then((m) => ({
    default: m.GeneralSettings,
  })),
);
const ShortcutsSettings = lazy(() =>
  import("./settings/shortcuts/ShortcutsSettings").then((m) => ({
    default: m.ShortcutsSettings,
  })),
);
const AdvancedSettings = lazy(() =>
  import("./settings/advanced/AdvancedSettings").then((m) => ({
    default: m.AdvancedSettings,
  })),
);
const ModelsConfiguration = lazy(() =>
  import("./settings/ModelsSettings").then((m) => ({
    default: m.ModelsSettings,
  })),
);
const PromptsConfiguration = lazy(() =>
  import("./settings/post-processing/PromptsConfiguration").then((m) => ({
    default: m.PromptsConfiguration,
  })),
);
const AboutSettings = lazy(() =>
  import("./settings/about/AboutSettings").then((m) => ({
    default: m.AboutSettings,
  })),
);
const AppProfilesSettings = lazy(() =>
  import("./settings/AppProfilesSettings").then((m) => ({
    default: m.AppProfilesSettings,
  })),
);
const VocabularySettings = lazy(() =>
  import("./settings/VocabularySettings").then((m) => ({
    default: m.VocabularySettings,
  })),
);
const SummaryPage = lazy(() =>
  import("./settings/summary/SummaryPage").then((m) => ({
    default: m.SummaryPage,
  })),
);

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: any;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
  enabled: (settings: any) => boolean;
  shortcutKey: string;
}

export const SECTIONS_CONFIG = {
  dashboard: {
    labelKey: "sidebar.dashboard",
    icon: IconLayoutDashboard,
    component: Dashboard,
    enabled: () => true,
    shortcutKey: "1",
  },
  summary: {
    labelKey: "sidebar.summary",
    icon: IconChartBar,
    component: SummaryPage,
    enabled: () => true,
    shortcutKey: "2",
  },
  general: {
    labelKey: "sidebar.general",
    icon: IconSettings,
    component: GeneralSettings,
    enabled: () => true,
    shortcutKey: "3",
  },
  advanced: {
    labelKey: "sidebar.advanced",
    icon: IconAdjustments,
    component: AdvancedSettings,
    enabled: (expertMode: boolean) => expertMode,
    shortcutKey: "9",
  },
  models: {
    labelKey: "sidebar.models",
    icon: IconBrain,
    component: ModelsConfiguration,
    enabled: () => true,
    shortcutKey: "4",
  },
  prompts: {
    labelKey: "sidebar.postProcessing",
    icon: IconNotes,
    component: PromptsConfiguration,
    enabled: () => true,
    shortcutKey: "5",
  },
  shortcuts: {
    labelKey: "sidebar.shortcuts",
    icon: IconKeyboard,
    component: ShortcutsSettings,
    enabled: () => true,
    shortcutKey: "6",
  },
  appProfiles: {
    labelKey: "sidebar.appProfiles",
    icon: IconApps,
    component: AppProfilesSettings,
    enabled: () => true,
    shortcutKey: "7",
  },
  vocabulary: {
    labelKey: "sidebar.vocabulary",
    icon: IconAbc,
    component: VocabularySettings,
    enabled: () => true,
    shortcutKey: "8",
  },
  about: {
    labelKey: "sidebar.about",
    icon: IconInfoCircle,
    component: AboutSettings,
    enabled: () => true,
    shortcutKey: "",
  },
} as const satisfies Record<string, SectionConfig>;

// Canonical section order
export const SECTION_ORDER: SidebarSection[] = [
  "dashboard",
  "summary",
  "general",
  "models",
  "prompts",
  "shortcuts",
  "appProfiles",
  "vocabulary",
  "advanced",
  "about",
];

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

const SidebarItem: React.FC<{
  section: SectionConfig & { id: string; label: string };
  isActive: boolean;
  onClick: () => void;
  collapsed: boolean;
}> = ({ section, isActive, onClick, collapsed }) => {
  const Icon = section.icon;

  const handleClick = useCallback(() => {
    onClick();
  }, [onClick]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onClick();
      }
    },
    [onClick],
  );

  return (
    <Flex
      align="center"
      justify={collapsed ? "center" : "between"}
      px={collapsed ? "0" : "3"}
      py="2"
      role="button"
      tabIndex={0}
      aria-selected={isActive}
      className={`w-full cursor-pointer rounded-xl transition-all duration-200 group relative ${
        isActive
          ? "bg-(--accent-a3) shadow-sm border border-(--accent-a4)"
          : "hover:bg-(--gray-a3) opacity-70 hover:opacity-100 border border-transparent"
      } ${collapsed ? "h-10 w-10 mx-auto" : ""}`}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
    >
      <Flex
        gap="3"
        align="center"
        justify={collapsed ? "center" : "start"}
        className={collapsed ? "w-full justify-center" : "w-full"}
      >
        <Box className="flex items-center justify-center">
          <Icon
            size={20}
            stroke={isActive ? 2 : 1.5}
            color={isActive ? "var(--accent-9)" : "currentColor"}
          />
        </Box>
        {!collapsed && (
          <Text
            size="2"
            weight={isActive ? "bold" : "medium"}
            className={`${isActive ? "text-(--accent-11)" : "text-(--gray-11)"} whitespace-nowrap overflow-hidden text-ellipsis`}
          >
            {section.label}
          </Text>
        )}
      </Flex>
      {!collapsed && section.shortcutKey && (
        <Flex
          align="center"
          justify="center"
          className={`w-5 h-5 rounded-md border text-[10px] font-mono transition-colors ${
            isActive
              ? "bg-(--accent-9) border-transparent text-white shadow-sm"
              : "bg-(--gray-2) border-(--gray-5) text-(--gray-9)"
          }`}
        >
          {section.shortcutKey}
        </Flex>
      )}
    </Flex>
  );
};

export const Sidebar: React.FC<SidebarProps & { collapsed: boolean }> = ({
  activeSection,
  onSectionChange,
  collapsed,
}) => {
  const { t } = useTranslation();
  const { expertMode, updateSetting } = useSettings();

  const sections = SECTION_ORDER.map((id) => ({ id, ...SECTIONS_CONFIG[id] }))
    .filter((config) => config.enabled(expertMode))
    .map((config) => ({
      ...config,
      label: t(config.labelKey),
    }));

  const handleExpertModeToggle = useCallback(
    (checked: boolean) => {
      updateSetting("expert_mode", checked);
    },
    [updateSetting],
  );

  return (
    <Flex
      direction="column"
      className={`${collapsed ? "w-[72px]" : "w-56"} h-full border-r border-(--gray-5) bg-(--gray-1) select-none transition-all duration-300 ease-in-out`}
    >
      {/* Logo Area */}
      {/* Logo Area */}
      <Flex align="center" justify="center" className="pt-5 pb-1 min-h-[60px]">
        {collapsed ? (
          <img
            src="/src/assets/logo.png"
            alt="App Logo"
            className="w-8 h-8 object-contain drop-shadow-sm"
          />
        ) : (
          <VotypeHand size={30} />
        )}
      </Flex>

      {/* Divider with center dot */}
      <Flex
        align="center"
        justify="center"
        className={`relative mb-5 mt-2 transition-all duration-300 ${collapsed ? "opacity-0" : "opacity-100 mx-8"}`}
      >
        <Box className="absolute inset-x-0 h-px bg-(--gray-3)" />
        <Box className="relative z-10 px-2 bg-(--gray-1)">
          <Box className="w-1.5 h-1.5 rounded-full bg-(--gray-3)" />
        </Box>
      </Flex>

      <Flex
        direction="column"
        gap="5"
        px="2"
        className="flex-1 overflow-y-auto overflow-x-hidden"
      >
        <Flex direction="column" gap="1">
          {sections.map((section) =>
            collapsed ? (
              <Tooltip key={section.id} content={section.label} side="right">
                <div className="w-full">
                  <SidebarItem
                    section={section}
                    isActive={activeSection === section.id}
                    onClick={() =>
                      onSectionChange(section.id as SidebarSection)
                    }
                    collapsed={collapsed}
                  />
                </div>
              </Tooltip>
            ) : (
              <SidebarItem
                key={section.id}
                section={section}
                isActive={activeSection === section.id}
                onClick={() => onSectionChange(section.id as SidebarSection)}
                collapsed={collapsed}
              />
            ),
          )}
        </Flex>
      </Flex>

      {/* Expert Mode Toggle */}
      <Flex
        direction="column"
        gap="2"
        className={`border-t border-(--gray-4) transition-all duration-300 ${collapsed ? "px-2 py-4" : "px-4 py-3"}`}
      >
        <Flex
          align="center"
          justify={collapsed ? "center" : "between"}
          className="w-full"
        >
          {!collapsed && (
            <Tooltip content={t("sidebar.expertModeHint")}>
              <Flex align="center" gap="2" className="cursor-help">
                <IconSchool size={16} className="text-(--gray-9)" />
                <Text size="1" color="gray">
                  {t("sidebar.expertMode")}
                </Text>
              </Flex>
            </Tooltip>
          )}

          {collapsed ? (
            <Tooltip
              content={`${t("sidebar.expertMode")} (${expertMode ? "On" : "Off"})`}
              side="right"
            >
              <div>
                <Switch
                  size="1"
                  checked={expertMode}
                  onCheckedChange={handleExpertModeToggle}
                />
              </div>
            </Tooltip>
          ) : (
            <Switch
              size="1"
              checked={expertMode}
              onCheckedChange={handleExpertModeToggle}
            />
          )}
        </Flex>
      </Flex>
    </Flex>
  );
};
