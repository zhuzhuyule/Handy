import React from "react";
import { getAppIconUrl, useInstalledApps } from "../../hooks/useInstalledApps";

interface AppIconProps {
  appName: string | null | undefined;
  size?: number;
  className?: string;
  style?: React.CSSProperties;
}

/**
 * Render the native icon of an installed macOS application by name.
 * Falls back to a letter-circle placeholder if the icon is unavailable
 * (e.g., app not installed, or cache not yet loaded).
 *
 * Automatically subscribes to the installed apps cache so it re-renders
 * when the cache is populated or refreshed.
 */
export const AppIcon: React.FC<AppIconProps> = ({
  appName,
  size = 16,
  className,
  style,
}) => {
  // Subscribe to cache updates
  useInstalledApps();

  const iconUrl = appName ? getAppIconUrl(appName) : null;
  const letter = (appName || "?").charAt(0).toUpperCase();
  const radius = Math.max(2, size / 8);

  // Thin line border so white/light icons don't blend into the row
  const chrome: React.CSSProperties = {
    border: "1px solid var(--gray-a5)",
    boxSizing: "border-box",
  };

  if (iconUrl) {
    return (
      <img
        src={iconUrl}
        alt={appName || ""}
        width={size}
        height={size}
        className={className}
        style={{
          width: size,
          height: size,
          objectFit: "contain",
          flexShrink: 0,
          borderRadius: radius,
          ...chrome,
          ...style,
        }}
        loading="lazy"
      />
    );
  }

  return (
    <div
      className={className}
      style={{
        width: size,
        height: size,
        borderRadius: radius,
        background: "var(--gray-a3)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        flexShrink: 0,
        fontSize: Math.max(8, Math.floor(size * 0.55)),
        fontWeight: 600,
        color: "var(--gray-11)",
        ...chrome,
        ...style,
      }}
    >
      {letter}
    </div>
  );
};
