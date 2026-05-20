import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import type { FetchedModel } from "../bindings";
import type { BindingResponse } from "../lib/types";
import {
  AudioDevice,
  CachedModel,
  KeyEntry,
  ModelChain,
  ModelType,
  MultiModelPostProcessItem,
  PostProcessProvider,
  Settings,
} from "../lib/types";

interface SettingsStore {
  settings: Settings | null;
  isLoading: boolean;
  isUpdating: Record<string, boolean>;
  audioDevices: AudioDevice[];
  outputDevices: AudioDevice[];
  customSounds: { start: boolean; stop: boolean };
  /** Cached per-provider model lists — now keeps the full FetchedModel
   *  shape (id + display_name) so UIs can render a friendly label even
   *  when the id is an opaque string. */
  postProcessModelOptions: Record<string, FetchedModel[]>;

  // Actions
  initialize: () => Promise<void>;
  updateSetting: <K extends keyof Settings>(
    key: K,
    value: Settings[K],
  ) => Promise<void>;
  resetSetting: (key: keyof Settings) => Promise<void>;
  refreshSettings: () => Promise<void>;
  refreshAudioDevices: () => Promise<void>;
  refreshOutputDevices: () => Promise<void>;
  updateBinding: (id: string, binding: string) => Promise<void>;
  resetBinding: (id: string) => Promise<void>;
  getSetting: <K extends keyof Settings>(key: K) => Settings[K] | undefined;
  isUpdatingKey: (key: string) => boolean;
  playTestSound: (soundType: "start" | "stop") => Promise<void>;
  checkCustomSounds: () => Promise<void>;
  setPostProcessProvider: (providerId: string) => Promise<void>;
  updatePostProcessSetting: (
    settingType: "base_url" | "api_key" | "model",
    providerId: string,
    value: string,
  ) => Promise<void>;
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
  setPostProcessModelOptions: (
    providerId: string,
    models: FetchedModel[],
  ) => void;
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
  selectPostProcessModel: (modelId: string | null) => Promise<void>;
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
  addMultiModelPostProcessItem: (
    item: MultiModelPostProcessItem,
  ) => Promise<void>;
  updateMultiModelPostProcessItem: (
    item: MultiModelPostProcessItem,
  ) => Promise<void>;
  removeMultiModelPostProcessItem: (itemId: string) => Promise<void>;
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

  // Internal state setters
  setSettings: (settings: Settings | null) => void;
  setLoading: (loading: boolean) => void;
  setUpdating: (key: string, updating: boolean) => void;
  setAudioDevices: (devices: AudioDevice[]) => void;
  setOutputDevices: (devices: AudioDevice[]) => void;
  setCustomSounds: (sounds: { start: boolean; stop: boolean }) => void;
}

// Note: Default post-processing settings are now managed in Rust
// Settings will always have providers after migration
const DEFAULT_SETTINGS: Partial<Settings> = {
  always_on_microphone: false,
  audio_feedback: true,
  audio_feedback_volume: 1.0,
  sound_theme: "marimba",
  start_hidden: false,
  autostart_enabled: false,
  activation_mode: "toggle",
  selected_microphone: "Default",
  clamshell_microphone: "Default",
  selected_output_device: "Default",
  translate_to_english: false,
  selected_language: "auto",
  overlay_position: "follow",
  debug_mode: false,
  debug_log_post_process: false,
  debug_log_skill_routing: false,
  debug_log_routing: false,
  debug_log_transcription: false,
  log_level: 2,
  custom_words: [],
  history_limit: 5,
  recording_retention_period: "preserve_limit",
  mute_while_recording: false,
  audio_input_auto_enhance: true,
  mic_enhance_preferences: {},
  append_trailing_space: false,
  cached_models: [],
  online_asr_enabled: false,
  selected_asr_model: null,
  selected_prompt_model: null,
  punctuation_enabled: false,
  punctuation_model: "punct-zh-en-ct-transformer-2024-04-12-int8",
  favorite_transcription_models: [],
  realtime_transcription_enabled: false,
  offline_vad_force_interval_ms: 1000,
  offline_vad_force_window_seconds: 30,
  post_process_use_secondary_output: false,
  post_process_use_local_candidate_when_online_asr: false,
  post_process_secondary_model_id: null,
  post_process_intent_model: null,
  post_process_translation_model: null,
  app_review_policies: {},
  openai_compatible_api_enabled: true,
  openai_compatible_api_host: "127.0.0.1",
  openai_compatible_api_port: 33178,
  openai_compatible_api_allow_lan: false,
  openai_compatible_api_base_path: "/v1",
  openai_compatible_api_access_key: "",
  expert_mode: false,
};

const DEFAULT_AUDIO_DEVICE: AudioDevice = {
  index: "default",
  name: "Default",
  is_default: true,
};

const normalizeDeviceName = (name?: string | null) => {
  if (!name) return "Default";
  return name.toLowerCase() === "default" ? "Default" : name;
};

const settingUpdaters: {
  [K in keyof Settings]?: (value: Settings[K]) => Promise<unknown>;
} = {
  always_on_microphone: (value) =>
    invoke("update_microphone_mode", { alwaysOn: value }),
  audio_feedback: (value) =>
    invoke("change_audio_feedback_setting", { enabled: value }),
  audio_feedback_volume: (value) =>
    invoke("change_audio_feedback_volume_setting", { volume: value }),
  sound_theme: (value) =>
    invoke("change_sound_theme_setting", { theme: value }),
  start_hidden: (value) =>
    invoke("change_start_hidden_setting", { enabled: value }),
  autostart_enabled: (value) =>
    invoke("change_autostart_setting", { enabled: value }),
  activation_mode: (value) =>
    invoke("change_activation_mode_setting", { mode: value }),
  selected_microphone: async (value) => {
    const result = (await invoke("set_selected_microphone", {
      deviceName: value === "Default" ? "default" : value,
    })) as { audio_input_auto_enhance: boolean };
    // Apply the per-mic enhance preference returned by the backend.
    useSettingsStore.setState((state) => ({
      settings: state.settings
        ? {
            ...state.settings,
            audio_input_auto_enhance: result.audio_input_auto_enhance,
          }
        : null,
    }));
  },
  clamshell_microphone: (value) =>
    invoke("set_clamshell_microphone", {
      deviceName: value === "Default" ? "default" : value,
    }),
  selected_output_device: (value) =>
    invoke("set_selected_output_device", {
      deviceName: value === "Default" ? "default" : value,
    }),
  recording_retention_period: (value) =>
    invoke("update_recording_retention_period", { period: value }),
  translate_to_english: (value) =>
    invoke("change_translate_to_english_setting", { enabled: value }),
  selected_language: (value) =>
    invoke("change_selected_language_setting", { language: value }),
  overlay_position: (value) =>
    invoke("change_overlay_position_setting", { position: value }),
  debug_mode: (value) =>
    invoke("change_debug_mode_setting", { enabled: value }),
  debug_log_post_process: (value) =>
    invoke("change_debug_log_channel", {
      channel: "post_process",
      enabled: value,
    }),
  debug_log_skill_routing: (value) =>
    invoke("change_debug_log_channel", {
      channel: "skill_routing",
      enabled: value,
    }),
  debug_log_routing: (value) =>
    invoke("change_debug_log_channel", { channel: "routing", enabled: value }),
  debug_log_transcription: (value) =>
    invoke("change_debug_log_channel", {
      channel: "transcription",
      enabled: value,
    }),
  custom_words: (value) => invoke("update_custom_words", { words: value }),
  word_correction_threshold: (value) =>
    invoke("change_word_correction_threshold_setting", { threshold: value }),
  paste_method: (value) =>
    invoke("change_paste_method_setting", { method: value }),
  clipboard_handling: (value) =>
    invoke("change_clipboard_handling_setting", { handling: value }),
  auto_submit: (value) =>
    invoke("change_auto_submit_setting", { enabled: value }),
  auto_submit_key: (value) =>
    invoke("change_auto_submit_key_setting", { key: value }),
  history_limit: (value) => invoke("update_history_limit", { limit: value }),
  post_process_enabled: (value) =>
    invoke("change_post_process_enabled_setting", { enabled: value }),
  post_process_context_enabled: (value) =>
    invoke("change_post_process_context_enabled_setting", { enabled: value }),
  post_process_context_limit: (value) =>
    invoke("change_post_process_context_limit_setting", { limit: value }),
  post_process_streaming_output_enabled: (value) =>
    invoke("change_post_process_streaming_output_enabled_setting", {
      enabled: value,
    }),
  post_process_hotword_injection_enabled: (value) =>
    invoke("change_post_process_hotword_injection_enabled_setting", {
      enabled: value,
    }),
  openai_compatible_api_enabled: (value) =>
    invoke("change_openai_compatible_api_enabled_setting", { enabled: value }),
  openai_compatible_api_access_key: (value) =>
    invoke("change_openai_compatible_api_access_key_setting", {
      accessKey: value,
    }),
  openai_compatible_api_allow_lan: (value) =>
    invoke("change_openai_compatible_api_allow_lan_setting", {
      enabled: value,
    }),
  openai_compatible_api_port: (value) =>
    invoke("change_openai_compatible_api_port_setting", { port: value }),
  openai_compatible_api_base_path: (value) =>
    invoke("change_openai_compatible_api_base_path_setting", {
      basePath: value,
    }),
  post_process_use_secondary_output: (value) =>
    invoke("change_post_process_use_secondary_output_setting", {
      enabled: value,
    }),
  post_process_use_local_candidate_when_online_asr: (value) =>
    invoke("change_post_process_use_local_candidate_when_online_asr_setting", {
      enabled: value,
    }),
  post_process_secondary_model_id: (value) =>
    invoke("change_post_process_secondary_model_id_setting", {
      modelId: value,
    }),
  post_process_intent_model: (value) =>
    invoke("update_model_chain", {
      field: "post_process_intent_model",
      chain: value,
    }),
  post_process_translation_model: (value) =>
    invoke("update_model_chain", {
      field: "post_process_translation_model",
      chain: value,
    }),
  multi_model_post_process_enabled: (value) =>
    invoke("change_multi_model_post_process_enabled_setting", {
      enabled: value,
    }),
  multi_model_strategy: (value) =>
    invoke("change_multi_model_strategy_setting", {
      strategy: value,
    }),
  multi_model_preferred_id: (value) =>
    invoke("set_multi_model_preferred_id", { id: value }),
  multi_model_post_process_items: () => Promise.resolve(), // Handled separately
  post_process_selected_prompt_id: (value) =>
    invoke("set_post_process_selected_prompt", { id: value }),
  mute_while_recording: (value) =>
    invoke("change_mute_while_recording_setting", { enabled: value }),
  audio_input_auto_enhance: (value) =>
    invoke("change_audio_input_auto_enhance_setting", { enabled: value }),
  append_trailing_space: (value) =>
    invoke("change_append_trailing_space_setting", { enabled: value }),
  punctuation_enabled: (value) =>
    invoke("change_punctuation_enabled_setting", { enabled: value }),
  punctuation_model: (value) =>
    invoke("change_punctuation_model_setting", { model: value }),
  favorite_transcription_models: (value) =>
    invoke("change_favorite_transcription_models_setting", { models: value }),
  realtime_transcription_enabled: (value) =>
    invoke("change_realtime_transcription_enabled_setting", { enabled: value }),
  offline_vad_force_interval_ms: (value) =>
    invoke("change_offline_vad_force_interval_ms_setting", { interval: value }),
  offline_vad_force_window_seconds: (value) =>
    invoke("change_offline_vad_force_window_seconds_setting", {
      window: value,
    }),
  log_level: (value) => invoke("set_log_level", { level: value }),
  onboarding_completed: (value) =>
    invoke("change_onboarding_completed_setting", { completed: value }),
  app_review_policies: (value) =>
    invoke("set_app_review_policies", { policies: value }),
  app_profiles: (value) => invoke("set_app_profiles", { profiles: value }),
  app_to_profile: (value) =>
    invoke("set_app_to_profile", { appToProfile: value }),
  expert_mode: (value) =>
    invoke("change_expert_mode_setting", { enabled: value }),
  length_routing_enabled: (value) =>
    invoke("change_length_routing_enabled_setting", { enabled: value }),
  length_routing_threshold: (value) =>
    invoke("change_length_routing_threshold_setting", { threshold: value }),
  length_routing_short_model: (value) =>
    invoke("update_model_chain", {
      field: "length_routing_short_model",
      chain: value,
    }),
  length_routing_long_model: (value) =>
    invoke("update_model_chain", {
      field: "length_routing_long_model",
      chain: value,
    }),
};

export const useSettingsStore = create<SettingsStore>()(
  subscribeWithSelector((set, get) => ({
    settings: null,
    isLoading: true,
    isUpdating: {},
    audioDevices: [],
    outputDevices: [],
    customSounds: { start: false, stop: false },
    postProcessModelOptions: {},

    // Internal setters
    setSettings: (settings) => set({ settings }),
    setLoading: (isLoading) => set({ isLoading }),
    setUpdating: (key, updating) =>
      set((state) => ({
        isUpdating: { ...state.isUpdating, [key]: updating },
      })),
    setAudioDevices: (audioDevices) => set({ audioDevices }),
    setOutputDevices: (outputDevices) => set({ outputDevices }),
    setCustomSounds: (customSounds) => set({ customSounds }),

    // Getters
    getSetting: (key) => get().settings?.[key],
    isUpdatingKey: (key) => get().isUpdating[key] || false,

    // Load settings from store
    refreshSettings: async () => {
      try {
        const settings = (await invoke("get_app_settings")) as Settings;

        // Load additional settings that come from invoke calls
        const [
          microphoneMode,
          selectedMicrophone,
          clamshellMicrophone,
          selectedOutputDevice,
        ] = await Promise.allSettled([
          invoke("get_microphone_mode"),
          invoke("get_selected_microphone"),
          invoke("get_clamshell_microphone"),
          invoke("get_selected_output_device"),
        ]);

        // Merge all settings
        const mergedSettings: Settings = {
          ...settings,
          always_on_microphone:
            microphoneMode.status === "fulfilled"
              ? (microphoneMode.value as boolean)
              : false,
          selected_microphone:
            selectedMicrophone.status === "fulfilled"
              ? normalizeDeviceName(selectedMicrophone.value as string)
              : "Default",
          clamshell_microphone:
            clamshellMicrophone.status === "fulfilled"
              ? normalizeDeviceName(clamshellMicrophone.value as string)
              : "Default",
          selected_output_device:
            selectedOutputDevice.status === "fulfilled"
              ? normalizeDeviceName(selectedOutputDevice.value as string)
              : "Default",
        };

        set({ settings: mergedSettings, isLoading: false });
      } catch (error) {
        console.error("Failed to load settings:", error);
        set({ isLoading: false });
      }
    },

    // Load audio devices
    refreshAudioDevices: async () => {
      try {
        const devices: AudioDevice[] = await invoke(
          "get_available_microphones",
        );
        const devicesWithDefault = [
          DEFAULT_AUDIO_DEVICE,
          ...devices.filter(
            (d) => d.name !== "Default" && d.name !== "default",
          ),
        ];
        set({ audioDevices: devicesWithDefault });
      } catch (error) {
        console.error("Failed to load audio devices:", error);
        set({ audioDevices: [DEFAULT_AUDIO_DEVICE] });
      }
    },

    // Load output devices
    refreshOutputDevices: async () => {
      try {
        const devices: AudioDevice[] = await invoke(
          "get_available_output_devices",
        );
        const devicesWithDefault = [
          DEFAULT_AUDIO_DEVICE,
          ...devices.filter(
            (d) => d.name !== "Default" && d.name !== "default",
          ),
        ];
        set({ outputDevices: devicesWithDefault });
      } catch (error) {
        console.error("Failed to load output devices:", error);
        set({ outputDevices: [DEFAULT_AUDIO_DEVICE] });
      }
    },

    // Play a test sound
    playTestSound: async (soundType: "start" | "stop") => {
      try {
        await invoke("play_test_sound", { soundType });
      } catch (error) {
        console.error(`Failed to play test sound (${soundType}):`, error);
      }
    },

    checkCustomSounds: async () => {
      try {
        const sounds = await invoke("check_custom_sounds");
        get().setCustomSounds(sounds as { start: boolean; stop: boolean });
      } catch (error) {
        console.error("Failed to check custom sounds:", error);
      }
    },

    // Update a specific setting
    updateSetting: async <K extends keyof Settings>(
      key: K,
      value: Settings[K],
    ) => {
      const { settings, setUpdating } = get();
      const updateKey = String(key);
      const originalValue = settings?.[key];

      setUpdating(updateKey, true);

      try {
        set((state) => ({
          settings: state.settings ? { ...state.settings, [key]: value } : null,
        }));

        const updater = settingUpdaters[key];
        if (updater) {
          await updater(value);
        } else if (key !== "bindings" && key !== "selected_model") {
          console.warn(`No handler for setting: ${String(key)}`);
        }
      } catch (error) {
        console.error(`Failed to update setting ${String(key)}:`, error);
        if (settings) {
          set({ settings: { ...settings, [key]: originalValue } });
        }
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Reset a setting to its default value
    resetSetting: async (key) => {
      const defaultValue = DEFAULT_SETTINGS[key];
      if (defaultValue !== undefined) {
        await get().updateSetting(key, defaultValue as any);
      }
    },

    // Update a specific binding
    updateBinding: async (id, binding) => {
      const { settings, setUpdating } = get();
      const updateKey = `binding_${id}`;
      const originalBinding = settings?.bindings?.[id]?.current_binding;

      setUpdating(updateKey, true);

      try {
        // Optimistic update
        set((state) => ({
          settings: state.settings
            ? {
                ...state.settings,
                bindings: {
                  ...state.settings.bindings,
                  [id]: {
                    ...state.settings.bindings[id],
                    current_binding: binding,
                  },
                },
              }
            : null,
        }));

        const response = await invoke<BindingResponse>("change_binding", {
          id,
          binding,
        });

        if (!response?.success) {
          throw new Error(response?.error || "Failed to change binding");
        }

        const canonicalBinding = response.binding?.current_binding ?? binding;

        // Sync with backend-canonicalized binding (e.g. whitespace/alias normalization)
        set((state) => ({
          settings: state.settings
            ? {
                ...state.settings,
                bindings: {
                  ...state.settings.bindings,
                  [id]: {
                    ...state.settings.bindings[id],
                    current_binding: canonicalBinding,
                  },
                },
              }
            : null,
        }));
      } catch (error) {
        console.error(`Failed to update binding ${id}:`, error);

        // Rollback on error
        if (originalBinding && get().settings) {
          set((state) => ({
            settings: state.settings
              ? {
                  ...state.settings,
                  bindings: {
                    ...state.settings.bindings,
                    [id]: {
                      ...state.settings.bindings[id],
                      current_binding: originalBinding,
                    },
                  },
                }
              : null,
          }));
        }
        throw error;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Reset a specific binding
    resetBinding: async (id) => {
      const { setUpdating, refreshSettings } = get();
      const updateKey = `binding_${id}`;

      setUpdating(updateKey, true);

      try {
        await invoke("reset_binding", { id });
        await refreshSettings();
      } catch (error) {
        console.error(`Failed to reset binding ${id}:`, error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    setPostProcessProvider: async (providerId) => {
      const {
        settings,
        setUpdating,
        refreshSettings,
        setPostProcessModelOptions,
      } = get();
      const updateKey = "post_process_provider_id";
      const previousId = settings?.post_process_provider_id ?? null;

      setUpdating(updateKey, true);

      if (settings) {
        set((state) => ({
          settings: state.settings
            ? { ...state.settings, post_process_provider_id: providerId }
            : null,
        }));
      }

      // Clear cached model options for the new provider so the dropdown
      // doesn't show stale models from a previous fetch or base_url.
      setPostProcessModelOptions(providerId, []);

      try {
        await invoke("set_post_process_provider", { providerId });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to set post-process provider:", error);
        if (previousId !== null) {
          set((state) => ({
            settings: state.settings
              ? { ...state.settings, post_process_provider_id: previousId }
              : null,
          }));
        }
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Generic updater for post-processing provider settings
    updatePostProcessSetting: async (
      settingType: "base_url" | "api_key" | "model",
      providerId: string,
      value: string,
    ) => {
      const { setUpdating, refreshSettings } = get();
      const updateKey = `post_process_${settingType}:${providerId}`;

      // Map setting types to command names
      const commandMap = {
        base_url: "change_post_process_base_url_setting",
        api_key: "change_post_process_api_key_setting",
        model: "change_post_process_model_setting",
      };

      // Map setting types to param names
      const paramMap = {
        base_url: "baseUrl",
        api_key: "apiKey",
        model: "model",
      };

      setUpdating(updateKey, true);

      try {
        await invoke(commandMap[settingType], {
          providerId,
          [paramMap[settingType]]: value,
        });
        await refreshSettings();
      } catch (error) {
        console.error(
          `Failed to update post-process ${settingType.replace("_", " ")}:`,
          error,
        );
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updatePostProcessBaseUrl: async (providerId, baseUrl) => {
      const { setUpdating, refreshSettings } = get();
      const updateKey = `post_process_base_url:${providerId}`;

      setUpdating(updateKey, true);

      try {
        // Persist the new base URL first.
        await invoke("change_post_process_base_url_setting", {
          providerId,
          baseUrl,
        });

        // Reset the stored model since the previous value is almost certainly
        // invalid for the new endpoint.
        await invoke("change_post_process_model_setting", {
          providerId,
          model: "",
        });

        // Clear cached model options after both backend writes succeed.
        set((state) => ({
          postProcessModelOptions: {
            ...state.postProcessModelOptions,
            [providerId]: [],
          },
        }));

        // Single refresh after both backend writes.
        await refreshSettings();
      } catch (error) {
        console.error("Failed to update post-process base URL:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updatePostProcessApiKey: async (providerId, apiKey) => {
      // Clear cached models when API key changes - user should click refresh after
      set((state) => ({
        postProcessModelOptions: {
          ...state.postProcessModelOptions,
          [providerId]: [],
        },
      }));
      return get().updatePostProcessSetting("api_key", providerId, apiKey);
    },

    updatePostProcessModel: async (providerId, model) => {
      return get().updatePostProcessSetting("model", providerId, model);
    },

    fetchPostProcessModels: async (providerId) => {
      const updateKey = `post_process_models_fetch:${providerId}`;
      const { setUpdating, setPostProcessModelOptions } = get();

      setUpdating(updateKey, true);

      try {
        // Call Tauri backend command instead of fetch
        const models: FetchedModel[] = await invoke(
          "fetch_post_process_models",
          { providerId },
        );

        setPostProcessModelOptions(providerId, models);
        return models;
      } catch (error) {
        console.error("Failed to fetch models:", error);
        // Don't cache empty array on error - let user retry
        throw error;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    testPostProcessInference: async (
      providerId: string,
      modelId: string,
      overrides?: {
        extraParams?: Record<string, unknown> | null;
        extraHeaders?: Record<string, string> | null;
      },
    ) => {
      const updateKey = `test_post_process_inference:${providerId}`;
      const { setUpdating } = get();
      setUpdating(updateKey, true);
      try {
        const result = (await invoke("test_post_process_model_inference", {
          providerId,
          modelId,
          extraParamsOverride: overrides?.extraParams ?? null,
          extraHeadersOverride: overrides?.extraHeaders ?? null,
        })) as {
          content?: string;
          reasoning_content?: string;
          duration_ms?: number;
          total_tokens?: number;
        };
        return result;
      } catch (error) {
        console.error("Failed to test post-process inference:", error);
        throw error;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    setPostProcessModelOptions: (providerId, models) =>
      set((state) => ({
        postProcessModelOptions: {
          ...state.postProcessModelOptions,
          [providerId]: models,
        },
      })),

    addCustomProvider: async ({ label, baseUrl, modelsEndpoint }) => {
      const updateKey = "add_custom_provider";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        const provider = (await invoke("add_custom_provider", {
          label: label.trim(),
          baseUrl: baseUrl.trim(),
          modelsEndpoint: modelsEndpoint?.trim() || null,
        })) as PostProcessProvider;
        await refreshSettings();
        return provider;
      } catch (error) {
        console.error("Failed to add custom provider:", error);
        throw error;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updateCustomProvider: async ({
      providerId,
      label,
      baseUrl,
      modelsEndpoint,
    }) => {
      const updateKey = `update_custom_provider:${providerId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        await invoke("update_custom_provider", {
          providerId,
          label: label?.trim(),
          baseUrl: baseUrl?.trim(),
          modelsEndpoint: modelsEndpoint?.trim(),
        });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to update provider:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    removeCustomProvider: async (providerId) => {
      const updateKey = `remove_custom_provider:${providerId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        await invoke("remove_custom_provider", { providerId });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to remove custom provider:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    reorderPostProcessProviders: async (providerIds) => {
      const updateKey = "reorder_post_process_providers";
      const { settings, setUpdating, refreshSettings } = get();
      const originalProviders = settings?.post_process_providers ?? [];
      const providerMap = new Map(
        originalProviders.map((provider) => [provider.id, provider]),
      );
      const reorderedProviders = providerIds
        .map((providerId) => providerMap.get(providerId))
        .filter((provider): provider is PostProcessProvider => !!provider);

      setUpdating(updateKey, true);

      try {
        set((state) => ({
          settings: state.settings
            ? {
                ...state.settings,
                post_process_providers: reorderedProviders,
              }
            : null,
        }));

        await invoke("reorder_post_process_providers", {
          providerIds,
        });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to reorder post-process providers:", error);
        set((state) => ({
          settings: state.settings
            ? {
                ...state.settings,
                post_process_providers: originalProviders,
              }
            : null,
        }));
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Multi-model checkbox selection
    toggleMultiModelSelection: async (
      cachedModelId: string,
      selected: boolean,
    ) => {
      const updateKey = "toggle_multi_model_selection";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        await invoke("toggle_multi_model_selection", {
          cachedModelId,
          selected,
        });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to toggle multi-model selection:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Multi-model post-process CRUD operations
    addMultiModelPostProcessItem: async (item: MultiModelPostProcessItem) => {
      const updateKey = "add_multi_model_item";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        await invoke("add_multi_model_post_process_item", { item });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to add multi-model item:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updateMultiModelPostProcessItem: async (
      item: MultiModelPostProcessItem,
    ) => {
      const updateKey = `update_multi_model_item:${item.id}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        await invoke("update_multi_model_post_process_item", { item });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to update multi-model item:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    removeMultiModelPostProcessItem: async (itemId: string) => {
      const updateKey = `remove_multi_model_item:${itemId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);

      try {
        await invoke("remove_multi_model_post_process_item", { itemId });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to remove multi-model item:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updateModelChain: async (field: string, chain: ModelChain | null) => {
      const { setUpdating, refreshSettings } = get();
      setUpdating(field, true);
      try {
        await invoke("update_model_chain", { field, chain });
        await refreshSettings();
      } finally {
        setUpdating(field, false);
      }
    },

    getPostProcessApiKeys: async (providerId) => {
      const keys: KeyEntry[] = await invoke("get_post_process_api_keys", {
        providerId,
      });
      return keys;
    },

    setPostProcessApiKeys: async (providerId, keys) => {
      await invoke("set_post_process_api_keys", { providerId, keys });
      set((state) => ({
        postProcessModelOptions: {
          ...state.postProcessModelOptions,
          [providerId]: [],
        },
      }));
      await get().refreshSettings();
    },

    setProxySettings: async (url, globalEnabled) => {
      await invoke("set_proxy_settings", { url, globalEnabled });
      await get().refreshSettings();
    },

    setProviderUseProxy: async (providerId, useProxy) => {
      await invoke("set_provider_use_proxy", {
        providerId,
        useProxy,
      });
      await get().refreshSettings();
    },

    addCachedModel: async (model) => {
      const updateKey = "cached_model_add";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("add_cached_model", { model });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to add cached model:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updateCachedModelType: async (modelId, modelType) => {
      const updateKey = `cached_model_update:${modelId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("update_cached_model_capability", {
          id: modelId,
          modelType: modelType,
        });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to update cached model type:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updateCachedModelPromptMessageRole: async (modelId, role) => {
      const updateKey = `cached_model_prompt_role:${modelId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("change_cached_model_prompt_message_role", {
          id: modelId,
          role,
        });
        await refreshSettings();
      } catch (error) {
        console.error(
          "Failed to update cached model prompt message role:",
          error,
        );
      } finally {
        setUpdating(updateKey, false);
      }
    },

    toggleCachedModelThinking: async (modelId, enabled) => {
      const updateKey = `cached_model_thinking:${modelId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("toggle_cached_model_thinking", {
          id: modelId,
          enabled,
        });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to toggle thinking mode:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    removeCachedModel: async (modelId) => {
      const updateKey = `cached_model_remove:${modelId}`;
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("remove_cached_model", { id: modelId });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to remove cached model:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    toggleOnlineAsr: async (enabled) => {
      const updateKey = "toggle_online_asr";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("toggle_online_asr", { enabled });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to toggle online ASR:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    selectAsrModel: async (modelId) => {
      const updateKey = "select_asr_model";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("select_asr_model", { modelId });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to select ASR model:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    selectPostProcessModel: async (modelId) => {
      const updateKey = "select_post_process_model";
      const { setUpdating, refreshSettings } = get();
      setUpdating(updateKey, true);
      try {
        await invoke("select_post_process_model", { modelId });
        await refreshSettings();
      } catch (error) {
        console.error("Failed to select post-process model:", error);
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Initialize everything
    initialize: async () => {
      const {
        refreshSettings,
        refreshAudioDevices,
        refreshOutputDevices,
        checkCustomSounds,
      } = get();
      await Promise.all([
        refreshSettings(),
        refreshAudioDevices(),
        refreshOutputDevices(),
        checkCustomSounds(),
      ]);
    },
  })),
);
