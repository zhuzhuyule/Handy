import {
  Dialog,
  Flex,
  IconButton,
  ScrollArea,
  Text,
  TextField,
  Tooltip,
} from "@radix-ui/themes";
import { convertFileSrc } from "@tauri-apps/api/core";
import { IconRefresh, IconSearch } from "@tabler/icons-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  InstalledApp,
  useInstalledApps,
} from "../../../../hooks/useInstalledApps";

interface AppPickerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (appName: string) => void;
  /** App names already assigned to profiles (excluded from the list) */
  excludedApps: string[];
  /** Recently used app names from history (shown in "Recent" section) */
  recentApps: string[];
}

const GRID_COLUMNS = 6;

export function AppPickerDialog({
  open,
  onOpenChange,
  onSelect,
  excludedApps,
  recentApps,
}: AppPickerDialogProps) {
  const { t } = useTranslation();
  const { apps, loading, refresh } = useInstalledApps();
  const [search, setSearch] = useState("");

  useEffect(() => {
    if (open) {
      setSearch("");
      // Quietly refresh — the backend short-circuits if dirs haven't changed.
      refresh().catch((e) =>
        console.error("Failed to refresh installed apps", e),
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const excludedSet = useMemo(
    () => new Set(excludedApps.map((n) => n.toLowerCase())),
    [excludedApps],
  );

  const filteredApps = useMemo(() => {
    if (!apps) return [];
    return apps.filter((app) => {
      if (excludedSet.has(app.name.toLowerCase())) return false;
      if (!search) return true;
      const q = search.toLowerCase();
      return app.name.toLowerCase().includes(q);
    });
  }, [apps, search, excludedSet]);

  // Split into Recent (pinned to top) + all others (already alphabetically sorted by backend)
  const { recentList, allList } = useMemo(() => {
    const recentLower = recentApps.map((n) => n.toLowerCase());
    const recent: InstalledApp[] = [];
    const rest: InstalledApp[] = [];

    for (const app of filteredApps) {
      if (
        !search &&
        recent.length < GRID_COLUMNS &&
        recentLower.includes(app.name.toLowerCase())
      ) {
        recent.push(app);
      } else {
        rest.push(app);
      }
    }

    return { recentList: recent, allList: rest };
  }, [filteredApps, recentApps, search]);

  const handleSelect = useCallback(
    (app: InstalledApp) => {
      onSelect(app.name);
      onOpenChange(false);
    },
    [onSelect, onOpenChange],
  );

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Content style={{ maxWidth: 640, padding: 0, overflow: "hidden" }}>
        <Flex direction="column" style={{ height: 540 }}>
          {/* Header: search + refresh */}
          <Flex
            p="3"
            pb="2"
            gap="2"
            align="center"
            style={{ borderBottom: "1px solid var(--gray-a4)" }}
          >
            <TextField.Root
              placeholder={t(
                "settings.postProcessing.appRules.searchApps",
                "Search apps...",
              )}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              style={{ flex: 1 }}
              autoFocus
            >
              <TextField.Slot>
                <IconSearch size={16} />
              </TextField.Slot>
            </TextField.Root>
            <Tooltip
              content={t(
                "settings.postProcessing.appRules.refresh",
                "Rescan installed apps",
              )}
            >
              <IconButton
                variant="soft"
                onClick={() => refresh()}
                disabled={loading}
              >
                <IconRefresh
                  size={16}
                  style={{
                    animation: loading ? "spin 1s linear infinite" : undefined,
                  }}
                />
              </IconButton>
            </Tooltip>
          </Flex>

          {/* App grid */}
          <ScrollArea style={{ flex: 1 }}>
            <Flex direction="column" p="3" gap="3">
              {loading && !apps && (
                <Flex align="center" justify="center" py="6">
                  <Text size="2" color="gray">
                    {t("common.loading", "Loading...")}
                  </Text>
                </Flex>
              )}

              {!loading && filteredApps.length === 0 && apps && (
                <Flex align="center" justify="center" py="6">
                  <Text size="2" color="gray">
                    {t("common.noOptionsFound", "No apps found")}
                  </Text>
                </Flex>
              )}

              {/* Recent section (only when not searching) */}
              {recentList.length > 0 && (
                <Section
                  label={t(
                    "settings.postProcessing.appRules.recentApps",
                    "Recent",
                  )}
                >
                  <AppGrid apps={recentList} onSelect={handleSelect} />
                </Section>
              )}

              {/* All apps (alphabetical) */}
              {allList.length > 0 && (
                <Section
                  label={t(
                    "settings.postProcessing.appRules.allApps",
                    "All Apps",
                  )}
                >
                  <AppGrid apps={allList} onSelect={handleSelect} />
                </Section>
              )}
            </Flex>
          </ScrollArea>
        </Flex>
      </Dialog.Content>

      {/* keyframes for refresh spinner */}
      <style>{`
        @keyframes spin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
      `}</style>
    </Dialog.Root>
  );
}

function Section({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <Flex direction="column" gap="2">
      <Text
        size="1"
        color="gray"
        weight="bold"
        style={{
          textTransform: "uppercase",
          letterSpacing: "0.05em",
          padding: "0 2px",
        }}
      >
        {label}
      </Text>
      {children}
    </Flex>
  );
}

function AppGrid({
  apps,
  onSelect,
}: {
  apps: InstalledApp[];
  onSelect: (app: InstalledApp) => void;
}) {
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: `repeat(${GRID_COLUMNS}, 1fr)`,
        gap: 4,
      }}
    >
      {apps.map((app) => (
        <AppCell
          key={app.bundle_path}
          app={app}
          onClick={() => onSelect(app)}
        />
      ))}
    </div>
  );
}

function AppCell({ app, onClick }: { app: InstalledApp; onClick: () => void }) {
  const [hovered, setHovered] = useState(false);
  const iconUrl = useMemo(() => {
    if (!app.icon_path) return null;
    return convertFileSrc(app.icon_path, "asset");
  }, [app.icon_path]);

  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "flex-start",
        gap: 6,
        padding: "10px 6px 8px",
        border: "none",
        borderRadius: 8,
        cursor: "pointer",
        background: hovered ? "var(--gray-a3)" : "transparent",
        transition: "background 0.12s",
        minHeight: 92,
      }}
    >
      <div
        style={{
          width: 44,
          height: 44,
          borderRadius: 10,
          overflow: "hidden",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: iconUrl ? "transparent" : "var(--gray-a3)",
          border: "1px solid var(--gray-a5)",
          boxSizing: "border-box",
        }}
      >
        {iconUrl ? (
          <img
            src={iconUrl}
            alt={app.name}
            style={{ width: 44, height: 44, objectFit: "contain" }}
            loading="lazy"
          />
        ) : (
          <Text size="3" color="gray" weight="bold">
            {app.name.charAt(0)}
          </Text>
        )}
      </div>
      <Text
        size="1"
        align="center"
        style={{
          lineHeight: 1.2,
          display: "-webkit-box",
          WebkitLineClamp: 2,
          WebkitBoxOrient: "vertical",
          overflow: "hidden",
          width: "100%",
          wordBreak: "break-word",
        }}
      >
        {app.name}
      </Text>
    </button>
  );
}
