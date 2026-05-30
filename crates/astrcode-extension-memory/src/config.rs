//! Memory extension configuration from `extensions.astrcode.memory`.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct MemoryConfig {
    /// Maximum index records to retain per scope (user + project each trimmed separately).
    pub max_contexts: usize,
    /// Whether SessionStart auto-extraction runs.
    pub auto_extract: bool,
    /// Whether `memory_save` triggers a background sync of changed session rollouts.
    pub auto_extract_after_save: bool,
    /// Max changed sessions to process per pipeline run.
    pub max_changed_sessions: usize,
    /// Skip sessions whose extracted conversation is shorter than this (characters).
    pub min_conversation_chars: usize,
    /// Delete `contexts/` files older than this many days.
    pub max_context_age_days: u64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_contexts: 10,
            auto_extract: true,
            auto_extract_after_save: true,
            max_changed_sessions: 5,
            min_conversation_chars: 200,
            max_context_age_days: 90,
        }
    }
}

impl MemoryConfig {
    pub(crate) fn from_extension_config(
        config: &astrcode_extension_sdk::extension::ExtensionConfig,
    ) -> Self {
        config.deserialize().unwrap_or_default()
    }
}
