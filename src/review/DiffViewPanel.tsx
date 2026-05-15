// Single model diff view with source + output panels

import { ScrollArea } from "@radix-ui/themes";
import { Editor, EditorContent } from "@tiptap/react";
import { IconTextPlus } from "@tabler/icons-react";
import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ChangeStats } from "./diff-utils";
import { NeonBorder } from "./NeonBorder";

interface DiffViewPanelProps {
  sourceHtml: string;
  editor: Editor | null;
  isRerunning: boolean;
  currentModelName?: string;
  changeStats?: ChangeStats | null;
  onInsertOriginal?: () => void;
  onInsertPolished?: () => void;
  showInsertPolished?: boolean;
  insertShortcut?: string;
  isSubmitting?: boolean;
  /** The app-profile has signalled that English insertion is the default
   *  intent for the active window — dims the Insert button to secondary
   *  styling since the English counterpart (in the preview header) is the
   *  expected primary action. */
  translationIntended?: boolean;
  /** Which shortcut modifier the user is currently holding, so we can
   *  render a neon marquee border on the Insert button + card. */
  pressedModifier?: "meta" | "ctrl" | null;
}

export const DiffViewPanel: React.FC<DiffViewPanelProps> = ({
  sourceHtml,
  editor,
  isRerunning,
  changeStats,
  onInsertOriginal,
  onInsertPolished,
  showInsertPolished = true,
  insertShortcut,
  isSubmitting = false,
  translationIntended = false,
  pressedModifier,
}) => {
  const { t } = useTranslation();
  const [isSourceHovered, setIsSourceHovered] = useState(false);

  const focusEditor = () => {
    editor?.commands.focus();
  };

  const changePercent = changeStats?.changePercent ?? 0;
  // Insert button lives fixed at the bottom-right of the polished output
  // card. Its shortcut flips based on whether translation is required:
  // - translationIntended=true → ⌘⏎ is reserved for English, so polish
  //   uses Ctrl+Enter and armed-mode responds to Ctrl.
  // - translationIntended=false → no English block exists, so ⌘⏎ IS the
  //   polish shortcut and armed-mode responds to Cmd/Meta.
  const rightIsPrimary = !translationIntended;
  const insertModifier: "meta" | "ctrl" = translationIntended ? "ctrl" : "meta";
  const insertArmed = pressedModifier === insertModifier;
  const showActionRow =
    showInsertPolished && !!onInsertPolished && !isRerunning;

  return (
    <div className="review-content-area review-panels-layout">
      {/* Source text — simple inline frame with hover insert button */}
      <div
        className="review-source-inline"
        onMouseEnter={() => setIsSourceHovered(true)}
        onMouseLeave={() => setIsSourceHovered(false)}
      >
        <div dangerouslySetInnerHTML={{ __html: sourceHtml || "—" }} />
        {isSourceHovered && onInsertOriginal && (
          <button
            className="review-source-insert-btn"
            onClick={onInsertOriginal}
            title={t("transcription.review.insertOriginal")}
          >
            <IconTextPlus size={16} />
          </button>
        )}
      </div>
      {/* Final output panel — body (scrollable editor) and a persistent bottom
          action row. The row lives *below* the scrollable body so text never
          overlaps the buttons; it carries the change% badge + insert actions.
          When Ctrl is held (Insert's modifier), the whole card flashes a
          neon border to preview what will be inserted. */}
      <div
        className="review-panel review-panel-output review-polish-surface review-panel-output-compact"
        data-mod-armed={insertArmed && translationIntended ? "true" : undefined}
      >
        {insertArmed && translationIntended && (
          <NeonBorder
            radius={10}
            gradientId="review-neon-gradient-panel"
            strokeWidth={2.8}
            durationSec={3.2}
          />
        )}
        <ScrollArea
          type="hover"
          scrollbars="vertical"
          className="review-output-scroll"
        >
          <div
            className="review-panel-body review-output-content"
            onMouseDown={(event) => {
              if (!(event.target instanceof HTMLElement)) {
                return;
              }

              if (event.target.closest("button")) {
                return;
              }

              if (event.target.closest(".ProseMirror")) {
                return;
              }
              event.preventDefault();
              focusEditor();
            }}
          >
            {isRerunning ? (
              <div className="candidate-loading-shimmer" />
            ) : (
              <EditorContent editor={editor} />
            )}
          </div>
        </ScrollArea>
        {showActionRow && (
          <div className="review-inline-action-row">
            {changePercent !== 0 && (
              <span
                className={`review-change-stats ${Math.abs(changePercent) < 20 ? "low" : Math.abs(changePercent) < 40 ? "mid" : "high"}`}
                title={`+${changeStats?.addedChars ?? 0} −${changeStats?.removedChars ?? 0}`}
              >
                {changePercent > 0 ? "+" : ""}
                {changePercent}%
              </span>
            )}
            <div className="review-inline-action-row-spacer" />
            <button
              type="button"
              className={`${rightIsPrimary ? "review-btn-primary" : "review-btn-secondary"} review-inline-insert-btn`}
              onClick={onInsertPolished}
              disabled={isSubmitting || !onInsertPolished}
              title={t("transcription.review.insert", "Insert")}
              aria-label={t("transcription.review.insert", "Insert")}
              data-mod-armed={insertArmed ? "true" : undefined}
            >
              {insertArmed && (
                <NeonBorder
                  radius={8}
                  gradientId="review-neon-gradient-insert"
                  strokeWidth={rightIsPrimary ? 2.6 : 1.6}
                />
              )}
              {t("transcription.review.insert", "Insert")}
              {insertShortcut && (
                <span
                  className="review-shortcut-hint"
                  data-mod-armed={insertArmed ? "true" : undefined}
                >
                  {insertShortcut}
                </span>
              )}
            </button>
          </div>
        )}
      </div>
    </div>
  );
};
