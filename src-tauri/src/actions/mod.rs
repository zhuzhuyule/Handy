use crate::utils;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::AppHandle;

pub mod post_process;
pub mod task_cluster_generator;
mod transcribe;

pub(crate) use transcribe::compute_change_percent;

// Shortcut Action Trait
// Shortcut Action Mode
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[allow(dead_code)]
pub enum ActionMode {
    Stateless, // Fire on Release, no start/stop distinction (e.g. Paste, Settings)
    Stateful,  // Has active state, uses user settings for Toggle vs PTT (e.g. Transcribe)
}

// Shortcut Action Trait
pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    #[allow(dead_code)]
    fn mode(&self) -> ActionMode {
        ActionMode::Stateless
    }
}

// Cancel Action
struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// Test Action
struct TestAction;

impl ShortcutAction for TestAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Started - {} (App: {})",
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Stopped - {} (App: {})",
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }
}

// Open Settings Action
struct OpenSettingsAction;

impl ShortcutAction for OpenSettingsAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Open settings shortcut triggered: ID='{}', Shortcut='{}'",
            binding_id,
            shortcut_str
        );

        if let Err(e) = utils::show_or_create_main_window(app, Some("dashboard")) {
            log::error!("Failed to show/create main window: {}", e);
        }
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for open settings
    }
}

// Paste First Entry Action
struct PasteFirstEntryAction;

impl ShortcutAction for PasteFirstEntryAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Paste first entry shortcut triggered: ID='{}', Shortcut='{}'",
            binding_id,
            shortcut_str
        );

        // Get the first history entry and paste it
        match crate::commands::get_first_history_entry(app.clone()) {
            Ok(Some(entry)) => {
                // Use post_processed_text if available, otherwise use transcription_text
                let text_to_paste = entry
                    .post_processed_text
                    .or(Some(entry.transcription_text))
                    .unwrap_or_default();

                if text_to_paste.is_empty() {
                    log::warn!("First entry has no text to paste");
                    return;
                }

                // Paste the text to the active window
                if let Err(e) =
                    crate::commands::paste_text_to_active_window(app.clone(), text_to_paste)
                {
                    log::error!("Failed to paste text to active window: {}", e);
                } else {
                    log::info!("Successfully pasted first entry to active window");
                }
            }
            Ok(None) => {
                log::warn!("No history entries found to paste");
            }
            Err(e) => {
                log::error!("Failed to get first history entry: {}", e);
            }
        }
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for paste first entry
    }
}

// Static Action Map
pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "transcribe".to_string(),
        Arc::new(transcribe::TranscribeAction::new(false)) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "invoke_skill".to_string(),
        Arc::new(transcribe::TranscribeAction::new(true)) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "test".to_string(),
        Arc::new(TestAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "open_settings".to_string(),
        Arc::new(OpenSettingsAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "paste_first_entry".to_string(),
        Arc::new(PasteFirstEntryAction) as Arc<dyn ShortcutAction>,
    );
    map
});
