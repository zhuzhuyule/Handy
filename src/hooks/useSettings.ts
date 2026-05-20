import { useEffect } from "react";
import { FetchedModel } from "../bindings";
import {
  AudioDevice,
  CachedModel,
  KeyEntry,
  ModelChain,
  ModelType,
  PostProcessProvider,
  Settings,
} from "../lib/types";
import { useSettingsStore } from "../stores/settingsStore";

interface UseSettingsReturn {
  // State
  settings: Settings | null;
  isLoading: boolean;
  isUpdating: (key: string) => boolean;
  audioDevices: AudioDevice[];
  outputDevices: AudioDevice[];
  audioFeedbackEnabled: boolean;
  expertMode: boolean;
  postProcessModelOptions: Record<string, FetchedModel[]>;

  // Actions
  updateSetting: <K extends keyof Settings>(
    key: K,
    value: Settings[K],
  ) => Promise<void>;
  resetSetting: (key: keyof Settings) => Promise<void>;
  refreshSettings: () => Promise<void>;
  refreshAudioDevices: () => Promise<void>;
  refreshOutputDevices: () => Promise<void>;

  // Binding-specific actions
  updateBinding: (id: string, binding: string) => Promise<void>;
  resetBinding: (id: string) => Promise<void>;

  // Convenience getters
  getSetting: <K extends keyof Settings>(key: K) => Settings[K] | undefined;

  // Post-processing helpers
  setPostProcessProvider: (providerId: string) => Promise<void>;
  updatePostProcessBaseUrl: (
    providerId: string,
    baseUrl: string,
  ) => Promise<void>;
  updatePostProcessApiKey: (
    providerId: string,
    apiKey: string,
  ) => Promise<void>;
  updatePostProcessModel: (providerId: string, model: string) => Promise<void>;
  fetchPostProcessModels: (providerId: string) => Promise<FetchedModel[]>;
  addCustomProvider: (payload: {
    label: string;
    baseUrl: string;
    modelsEndpoint?: string;
  }) => Promise<PostProcessProvider>;
  updateCustomProvider: (payload: {
    providerId: string;
    label?: string;
    baseUrl?: string;
    modelsEndpoint?: string;
  }) => Promise<void>;
  removeCustomProvider: (providerId: string) => Promise<void>;
  reorderPostProcessProviders: (providerIds: string[]) => Promise<void>;
  toggleMultiModelSelection: (
    cachedModelId: string,
    selected: boolean,
  ) => Promise<void>;
  addCachedModel: (model: CachedModel) => Promise<void>;
  updateCachedModelType: (
    modelId: string,
    modelType: ModelType,
  ) => Promise<void>;
  updateCachedModelPromptMessageRole: (
    modelId: string,
    role: "system" | "developer",
  ) => Promise<void>;
  toggleCachedModelThinking: (
    modelId: string,
    enabled: boolean,
  ) => Promise<void>;
  removeCachedModel: (modelId: string) => Promise<void>;
  toggleOnlineAsr: (enabled: boolean) => Promise<void>;
  selectAsrModel: (modelId: string | null) => Promise<void>;
  selectPromptModel: (modelId: string | null) => Promise<void>;
  testPostProcessInference: (
    providerId: string,
    modelId: string,
    overrides?: {
      extraParams?: Record<string, unknown> | null;
      extraHeaders?: Record<string, string> | null;
    },
  ) => Promise<{
    content?: string;
    reasoning_content?: string;
    duration_ms?: number;
    total_tokens?: number;
  }>;
  updateModelChain: (field: string, chain: ModelChain | null) => Promise<void>;
  getPostProcessApiKeys: (providerId: string) => Promise<KeyEntry[]>;
  setPostProcessApiKeys: (
    providerId: string,
    keys: KeyEntry[],
  ) => Promise<void>;
  setProxySettings: (
    url: string | null,
    globalEnabled: boolean,
  ) => Promise<void>;
  setProviderUseProxy: (providerId: string, useProxy: boolean) => Promise<void>;
}

export const useSettings = (): UseSettingsReturn => {
  const store = useSettingsStore();

  // Initialize on first mount
  useEffect(() => {
    if (store.isLoading) {
      store.initialize();
    }
  }, [store.initialize, store.isLoading]);

  return {
    settings: store.settings,
    isLoading: store.isLoading,
    isUpdating: store.isUpdatingKey,
    audioDevices: store.audioDevices,
    outputDevices: store.outputDevices,
    audioFeedbackEnabled: store.settings?.audio_feedback || false,
    expertMode: store.settings?.expert_mode || false,
    postProcessModelOptions: store.postProcessModelOptions,
    updateSetting: store.updateSetting,
    resetSetting: store.resetSetting,
    refreshSettings: store.refreshSettings,
    refreshAudioDevices: store.refreshAudioDevices,
    refreshOutputDevices: store.refreshOutputDevices,
    updateBinding: store.updateBinding,
    resetBinding: store.resetBinding,
    getSetting: store.getSetting,
    setPostProcessProvider: store.setPostProcessProvider,
    updatePostProcessBaseUrl: store.updatePostProcessBaseUrl,
    updatePostProcessApiKey: store.updatePostProcessApiKey,
    updatePostProcessModel: store.updatePostProcessModel,
    fetchPostProcessModels: store.fetchPostProcessModels,
    addCachedModel: store.addCachedModel,
    updateCachedModelType: store.updateCachedModelType,
    updateCachedModelPromptMessageRole:
      store.updateCachedModelPromptMessageRole,
    toggleCachedModelThinking: store.toggleCachedModelThinking,
    removeCachedModel: store.removeCachedModel,
    toggleOnlineAsr: store.toggleOnlineAsr,
    selectAsrModel: store.selectAsrModel,
    selectPromptModel: store.selectPostProcessModel,
    addCustomProvider: store.addCustomProvider,
    updateCustomProvider: store.updateCustomProvider,
    removeCustomProvider: store.removeCustomProvider,
    reorderPostProcessProviders: store.reorderPostProcessProviders,
    toggleMultiModelSelection: store.toggleMultiModelSelection,
    testPostProcessInference: store.testPostProcessInference,
    updateModelChain: store.updateModelChain,
    getPostProcessApiKeys: store.getPostProcessApiKeys,
    setPostProcessApiKeys: store.setPostProcessApiKeys,
    setProxySettings: store.setProxySettings,
    setProviderUseProxy: store.setProviderUseProxy,
  };
};
