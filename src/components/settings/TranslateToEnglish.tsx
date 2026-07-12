import React, { useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useModels } from "../../hooks/useModels";

interface TranslateToEnglishProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const unsupportedTranslationModels = [
  "parakeet-tdt-0.6b-v2",
  "parakeet-tdt-0.6b-v3",
  "turbo",
];

export const TranslateToEnglish: React.FC<TranslateToEnglishProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const { currentModel, loadCurrentModel, models } = useModels();

    const translateToEnglish = getSetting("translate_to_english") || false;
    const currentModelInfo = models.find((model) => model.id === currentModel);
    const isUnsupportedModelId =
      currentModel && unsupportedTranslationModels.includes(currentModel);
    // Translation to English is a Whisper-only capability. All models now share
    // the transcribe.cpp engine, so detect Whisper via the model id.
    const isNonWhisperEngine =
      !!currentModelInfo &&
      !currentModelInfo.id.toLowerCase().includes("whisper");
    const isDisabledTranslation =
      !currentModel ||
      !currentModelInfo ||
      isUnsupportedModelId ||
      isNonWhisperEngine;

    const description = useMemo(() => {
      if (!currentModel) {
        return t("settings.advanced.translateToEnglish.descriptionNoModel");
      }

      if (!currentModelInfo) {
        return t(
          "settings.advanced.translateToEnglish.descriptionUnknownModel",
        );
      }

      if (isNonWhisperEngine) {
        return t("settings.advanced.translateToEnglish.descriptionNonWhisper", {
          model: currentModelInfo.name,
        });
      }

      if (isUnsupportedModelId) {
        return t(
          "settings.advanced.translateToEnglish.descriptionUnsupported",
          {
            model: currentModelInfo.name,
          },
        );
      }

      return t("settings.advanced.translateToEnglish.description");
    }, [
      t,
      currentModel,
      currentModelInfo,
      isUnsupportedModelId,
      isNonWhisperEngine,
    ]);

    // Listen for model state changes to update UI reactively
    useEffect(() => {
      const modelStateUnlisten = listen("model-state-changed", () => {
        loadCurrentModel();
      });

      return () => {
        modelStateUnlisten.then((fn) => fn());
      };
    }, [loadCurrentModel]);

    return (
      <ToggleSwitch
        checked={translateToEnglish}
        onChange={(enabled) => updateSetting("translate_to_english", enabled)}
        isUpdating={isUpdating("translate_to_english")}
        disabled={isDisabledTranslation}
        label={t("settings.advanced.translateToEnglish.label")}
        description={description}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  },
);
