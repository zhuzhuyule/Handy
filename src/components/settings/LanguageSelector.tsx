import React, { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { SettingContainer } from "../ui/SettingContainer";
import { Dropdown } from "../ui/Dropdown";
import { DropdownOption } from "../ui/Dropdown";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../hooks/useSettings";
import { useModels } from "../../hooks/useModels";
import { LANGUAGES } from "../../lib/constants/languages";
import { ActionWrapper } from "../ui";
import type { ModelInfo } from "../../lib/types";

interface LanguageSelectorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const unsupportedModels = ["parakeet-tdt-0.6b-v2", "parakeet-tdt-0.6b-v3"];

export const LanguageSelector: React.FC<LanguageSelectorProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, resetSetting, isUpdating } = useSettings();
  const { currentModel, loadCurrentModel, models } = useModels();

  const selectedLanguage = getSetting("selected_language") || "auto";
  const currentModelInfo = models.find((m: ModelInfo) => m.id === currentModel);

  const supportStatus = (() => {
    if (!currentModel) return "noModel";
    if (!currentModelInfo) return "unknownModel";
    if (unsupportedModels.includes(currentModel)) return "unsupportedModel";
    // All models now share the transcribe.cpp engine, so infer the family from
    // the model id to preserve per-family language handling.
    const modelId = currentModelInfo.id.toLowerCase();
    if (modelId.includes("whisper")) return "whisper";
    if (modelId.includes("parakeet")) return "parakeet";
    if (modelId.includes("sensevoice") || modelId.includes("sense-voice"))
      return "sensevoice";
    return "unsupportedEngine";
  })() as
    | "noModel"
    | "unknownModel"
    | "unsupportedModel"
    | "whisper"
    | "parakeet"
    | "sensevoice"
    | "unsupportedEngine";

  const isDisabled =
    supportStatus === "noModel" ||
    supportStatus === "unknownModel" ||
    supportStatus === "unsupportedModel" ||
    supportStatus === "unsupportedEngine";

  useEffect(() => {
    const modelStateUnlisten = listen("model-state-changed", () => {
      loadCurrentModel();
    });

    return () => {
      modelStateUnlisten.then((fn) => fn());
    };
  }, [loadCurrentModel]);

  const handleLanguageChange = async (value: string) => {
    await updateSetting("selected_language", value);
  };

  const handleReset = async () => {
    await resetSetting("selected_language");
  };

  // Convert LANGUAGES to DropdownOption format
  const languageOptions: DropdownOption[] = LANGUAGES.map((lang) => ({
    value: lang.value,
    label: lang.label,
  }));

  const descriptionMap: Record<typeof supportStatus, string> = {
    noModel: t("settings.general.language.descriptionNoModel"),
    unknownModel: t("settings.general.language.descriptionUnknownModel"),
    unsupportedModel: t("settings.general.language.descriptionUnsupported"),
    unsupportedEngine: t(
      "settings.general.language.descriptionUnsupportedEngine",
    ),
    whisper: t("settings.general.language.description"),
    parakeet: t("settings.general.language.description"),
    sensevoice: t("settings.general.language.descriptionSenseVoice"),
  };

  return (
    <SettingContainer
      title={t("settings.general.language.title")}
      description={descriptionMap[supportStatus]}
      descriptionMode={descriptionMode}
      grouped={grouped}
      disabled={isDisabled}
    >
      <ActionWrapper
        onReset={handleReset}
        resetProps={{
          disabled: isUpdating("selected_language") || isDisabled,
        }}
      >
        <Dropdown
          selectedValue={selectedLanguage}
          onSelect={handleLanguageChange}
          options={languageOptions}
          placeholder={t("settings.general.language.searchPlaceholder")}
          disabled={isDisabled || isUpdating("selected_language")}
        />
      </ActionWrapper>
    </SettingContainer>
  );
};
