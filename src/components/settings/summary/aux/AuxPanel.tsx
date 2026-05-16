import { Box, Flex, IconButton, Text } from "@radix-ui/themes";
import { IconX } from "@tabler/icons-react";
import { useSummaryStore } from "../stores/summaryStore";
import type { AuxSection } from "../summaryTypes";

const SECTIONS: { key: AuxSection; label: string }[] = [
  { key: "stats", label: "Stats" },
  { key: "recap", label: "Recap" },
  { key: "profile", label: "Profile" },
  { key: "hotword", label: "Hotword" },
  { key: "export", label: "Export" },
  { key: "feedback", label: "Feedback History" },
];

interface AuxPanelProps {
  sectionsContent: Record<AuxSection, React.ReactNode>;
}

export function AuxPanel({ sectionsContent }: AuxPanelProps) {
  const {
    auxPanelOpen,
    auxActiveSection,
    openAuxPanel,
    closeAuxPanel,
    setAuxSection,
  } = useSummaryStore();

  return (
    <>
      <Box className="border-y border-gray-200 dark:border-gray-800 py-2 my-3">
        <Flex align="center" gap="2" wrap="wrap">
          <Text size="1" color="gray" className="opacity-70">
            ▸ AUX
          </Text>
          {SECTIONS.map((s) => (
            <button
              key={s.key}
              type="button"
              onClick={() => openAuxPanel(s.key)}
              className={
                "text-xs px-2 py-0.5 rounded-full border " +
                (auxPanelOpen && auxActiveSection === s.key
                  ? "border-blue-500 text-blue-600"
                  : "border-gray-300 dark:border-gray-700 text-gray-600 hover:border-gray-500")
              }
            >
              {s.label}
            </button>
          ))}
        </Flex>
      </Box>

      {auxPanelOpen && (
        <Box className="fixed top-0 right-0 h-full w-96 bg-[var(--color-panel-solid)] shadow-xl border-l border-gray-200 dark:border-gray-800 z-50 overflow-y-auto">
          <Flex
            justify="between"
            align="center"
            className="p-3 border-b border-gray-200 dark:border-gray-800"
          >
            <Flex gap="2" align="center">
              <Text size="2" weight="bold">
                {SECTIONS.find((s) => s.key === auxActiveSection)?.label}
              </Text>
            </Flex>
            <IconButton size="1" variant="ghost" onClick={closeAuxPanel}>
              <IconX size={16} />
            </IconButton>
          </Flex>
          <Flex direction="column" className="p-3">
            <Flex gap="1" mb="3" wrap="wrap">
              {SECTIONS.map((s) => (
                <button
                  key={s.key}
                  type="button"
                  onClick={() => setAuxSection(s.key)}
                  className={
                    "text-xs px-2 py-0.5 rounded " +
                    (s.key === auxActiveSection
                      ? "bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-200"
                      : "text-gray-600 hover:bg-gray-100 dark:hover:bg-gray-900")
                  }
                >
                  {s.label}
                </button>
              ))}
            </Flex>
            <Box>{sectionsContent[auxActiveSection]}</Box>
          </Flex>
        </Box>
      )}
    </>
  );
}
