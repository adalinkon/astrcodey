//! Turn pipeline stage state shared by the turn runner.

use std::collections::HashSet;

use astrcode_core::{
    llm::{LlmMessage, LlmRole},
    tool::{ToolDefinition, ToolPromptMetadata, ToolResult},
};

use crate::deferred_tools::{
    ToolSnapshot, activate_deferred_tools, clone_tools_by_index, provider_visible_tool_indexes,
};

/// Mutable state carried across provider/tool iterations in a single turn.
pub(crate) struct TurnState {
    pub(crate) messages: Vec<LlmMessage>,
    pub(crate) final_text: String,
    pub(crate) tool_results: Vec<ToolResult>,
    active_deferred_tools: HashSet<String>,
    all_tools: Vec<ToolSnapshot>,
    visible_tools: Vec<ToolSnapshot>,
}

impl TurnState {
    pub(crate) fn new(
        initial_history: Vec<LlmMessage>,
        system_prompt: &str,
        user_text: &str,
        all_tools: Vec<(ToolDefinition, Option<ToolPromptMetadata>)>,
    ) -> Self {
        let mut messages = Vec::with_capacity(initial_history.len() + 2);
        if !system_prompt.trim().is_empty() {
            messages.push(LlmMessage::system(system_prompt));
        }
        messages.extend(
            initial_history
                .into_iter()
                .filter(|message| message.role != LlmRole::System),
        );
        messages.push(LlmMessage::user(user_text));

        let all_tools = all_tools
            .into_iter()
            .map(|(definition, prompt_metadata)| ToolSnapshot {
                definition,
                prompt_metadata,
            })
            .collect::<Vec<_>>();
        let active_deferred_tools = HashSet::new();
        let tool_indexes = provider_visible_tool_indexes(&all_tools, &active_deferred_tools);
        let visible_tools = clone_tools_by_index(&all_tools, &tool_indexes);

        Self {
            messages,
            final_text: String::new(),
            tool_results: Vec::new(),
            active_deferred_tools,
            all_tools,
            visible_tools,
        }
    }

    pub(crate) fn all_tool_snapshots(&self) -> &[ToolSnapshot] {
        &self.all_tools
    }

    pub(crate) fn visible_tools(&self) -> Vec<ToolDefinition> {
        ToolSnapshot::definitions(&self.visible_tools)
    }

    pub(crate) fn active_deferred_tools(&self) -> &HashSet<String> {
        &self.active_deferred_tools
    }

    pub(crate) fn activate_deferred_tools(&mut self, discovered_tools: Vec<String>) -> bool {
        let changed = activate_deferred_tools(
            &mut self.active_deferred_tools,
            &self.all_tools,
            discovered_tools,
        );
        if changed {
            let tool_indexes =
                provider_visible_tool_indexes(&self.all_tools, &self.active_deferred_tools);
            self.visible_tools = clone_tools_by_index(&self.all_tools, &tool_indexes);
        }
        changed
    }
}

pub(crate) struct PreparedProviderRequest {
    pub(crate) llm: std::sync::Arc<dyn astrcode_core::llm::LlmProvider>,
    pub(crate) messages: Vec<LlmMessage>,
}
