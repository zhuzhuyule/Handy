import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import type { AuxSection, ViewMode } from "../summaryTypes";

interface SummaryStore {
  viewMode: ViewMode;
  selectedDate: string; // ISO date yyyy-mm-dd
  expandedClusterIds: Set<string>;
  auxPanelOpen: boolean;
  auxActiveSection: AuxSection;

  setViewMode: (mode: ViewMode) => void;
  setSelectedDate: (date: string) => void;
  toggleClusterExpanded: (id: string) => void;
  expandCluster: (id: string) => void;
  collapseCluster: (id: string) => void;
  openAuxPanel: (section?: AuxSection) => void;
  closeAuxPanel: () => void;
  setAuxSection: (section: AuxSection) => void;
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

export const useSummaryStore = create<SummaryStore>()(
  subscribeWithSelector((set) => ({
    viewMode: "day",
    selectedDate: todayIso(),
    expandedClusterIds: new Set<string>(),
    auxPanelOpen: false,
    auxActiveSection: "stats",

    setViewMode: (mode) => set({ viewMode: mode }),
    setSelectedDate: (date) =>
      set({ selectedDate: date, expandedClusterIds: new Set() }),
    toggleClusterExpanded: (id) =>
      set((state) => {
        const next = new Set(state.expandedClusterIds);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return { expandedClusterIds: next };
      }),
    expandCluster: (id) =>
      set((state) => {
        const next = new Set(state.expandedClusterIds);
        next.add(id);
        return { expandedClusterIds: next };
      }),
    collapseCluster: (id) =>
      set((state) => {
        const next = new Set(state.expandedClusterIds);
        next.delete(id);
        return { expandedClusterIds: next };
      }),
    openAuxPanel: (section) =>
      set((state) => ({
        auxPanelOpen: true,
        auxActiveSection: section ?? state.auxActiveSection,
      })),
    closeAuxPanel: () => set({ auxPanelOpen: false }),
    setAuxSection: (section) => set({ auxActiveSection: section }),
  })),
);
