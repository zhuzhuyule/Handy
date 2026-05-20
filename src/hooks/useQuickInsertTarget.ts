import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export interface QuickInsertTarget {
  app_name: string;
  pid: number;
}

const POLL_INTERVAL_MS = 1000;

/**
 * Polls the backend every 1s for the most recent non-Votype frontmost app.
 * Polling only runs while the hook is mounted; cleanup clears the interval.
 *
 * Returns `null` when the backend has no target yet (app just started,
 * Wayland session, or accessibility permission missing).
 */
export function useQuickInsertTarget(): QuickInsertTarget | null {
  const [target, setTarget] = useState<QuickInsertTarget | null>(null);

  useEffect(() => {
    let cancelled = false;

    const tick = async () => {
      try {
        const next = await invoke<QuickInsertTarget | null>(
          "get_quick_insert_target",
        );
        if (!cancelled) setTarget(next);
      } catch {
        // Backend error → fall back to "no target" rather than crashing UI.
        if (!cancelled) setTarget(null);
      }
    };

    tick(); // immediate first read so first paint is accurate
    const id = setInterval(tick, POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  return target;
}
