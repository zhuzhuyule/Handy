import {
  Button,
  Callout,
  Dialog,
  Flex,
  SegmentedControl,
  Text,
  TextField,
} from "@radix-ui/themes";
import { IconInfoCircle } from "@tabler/icons-react";
import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { useSettings } from "../../../../hooks/useSettings";
import type { ModelType } from "../../../../lib/types";

interface CustomAddModelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerId: string;
}

export const CustomAddModelDialog: React.FC<CustomAddModelDialogProps> = ({
  open,
  onOpenChange,
  providerId,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();

  const [modelId, setModelId] = useState("");
  const [modelType, setModelType] = useState<ModelType>("text");
  const [label, setLabel] = useState("");

  const cachedModels = settings?.cached_models ?? [];
  const isDuplicate = useMemo(() => {
    const trimmed = modelId.trim();
    if (trimmed.length === 0) return false;
    return cachedModels.some(
      (m) => m.provider_id === providerId && m.model_id === trimmed,
    );
  }, [cachedModels, providerId, modelId]);

  const handleClose = () => {
    setModelId("");
    setModelType("text");
    setLabel("");
    onOpenChange(false);
  };

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && handleClose()}>
      <Dialog.Content maxWidth="540px">
        <Dialog.Title>
          {t(
            "settings.postProcessing.models.customAdd.title",
            "Add Custom Model",
          )}
        </Dialog.Title>

        <Flex direction="column" gap="4" mt="3">
          {isDuplicate && (
            <Callout.Root color="amber" size="1">
              <Callout.Icon>
                <IconInfoCircle size={14} />
              </Callout.Icon>
              <Callout.Text>
                {t(
                  "settings.postProcessing.models.customAdd.duplicateWarning",
                  "A model with this ID already exists for this provider. Adding will create a copy.",
                )}
              </Callout.Text>
            </Callout.Root>
          )}

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.customAdd.modelId",
                "Model ID",
              )}
            </Text>
            <TextField.Root
              value={modelId}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="e.g. gpt-4o, my-custom-llama"
              autoFocus
            />
          </Flex>

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.selectModel.usageTypeTitle",
                "Usage Type",
              )}
            </Text>
            <SegmentedControl.Root
              value={modelType}
              onValueChange={(v) => setModelType(v as ModelType)}
              size="1"
            >
              <SegmentedControl.Item value="text">
                {t(
                  "settings.postProcessing.models.modelTypes.text.label",
                  "Text",
                )}
              </SegmentedControl.Item>
              <SegmentedControl.Item value="asr">
                {t(
                  "settings.postProcessing.models.modelTypes.asr.label",
                  "ASR",
                )}
              </SegmentedControl.Item>
              <SegmentedControl.Item value="other">
                {t(
                  "settings.postProcessing.models.modelTypes.other.label",
                  "Other",
                )}
              </SegmentedControl.Item>
            </SegmentedControl.Root>
          </Flex>

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.selectModel.customLabel",
                "Display Name",
              )}
            </Text>
            <TextField.Root
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={modelId || t("common.optional", "Optional")}
            />
          </Flex>

          <Flex justify="end" gap="3" mt="2">
            <Button variant="soft" color="gray" onClick={handleClose}>
              {t("common.cancel")}
            </Button>
          </Flex>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
};
