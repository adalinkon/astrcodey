//! Deferred tool visibility for provider requests.

use std::collections::HashSet;

use astrcode_core::{
    llm::LlmMessage,
    tool::{DEFERRED_TOOLS_METADATA_KEY, ToolDefinition, ToolPromptMetadata, ToolResult},
};

#[derive(Clone)]
pub(crate) struct ToolSnapshot {
    pub definition: ToolDefinition,
    pub prompt_metadata: Option<ToolPromptMetadata>,
}

impl ToolSnapshot {
    pub(crate) fn definitions(tools: &[Self]) -> Vec<ToolDefinition> {
        tools.iter().map(|tool| tool.definition.clone()).collect()
    }
}

pub fn provider_visible_tool_indexes(
    tools: &[ToolSnapshot],
    active_deferred_tools: &HashSet<String>,
) -> Vec<usize> {
    tools
        .iter()
        .enumerate()
        .filter(|(_, tool)| {
            !is_deferred_tool(tool)
                || active_deferred_tools.contains(&tool.definition.name)
                || is_deferred_gate(tool)
        })
        .map(|(index, _)| index)
        .collect()
}

pub fn clone_tools_by_index(tools: &[ToolSnapshot], indexes: &[usize]) -> Vec<ToolSnapshot> {
    indexes
        .iter()
        .filter_map(|index| tools.get(*index))
        .cloned()
        .collect()
}

pub fn append_deferred_tools_reminder(
    messages: &mut Vec<LlmMessage>,
    tools: &[ToolSnapshot],
    active_deferred_tools: &HashSet<String>,
) {
    let deferred = tools
        .iter()
        .filter(|tool| is_deferred_tool(tool))
        .filter(|tool| !active_deferred_tools.contains(&tool.definition.name))
        .map(|tool| tool.definition.name.as_str())
        .collect::<Vec<_>>();
    if deferred.is_empty() || !tools.iter().any(is_deferred_gate) {
        return;
    }

    let mut text = String::from(
        "<available-deferred-tools>\nDeferred tools are listed by name only. Use the matching \
         discovery tool to fetch full schemas before calling one of these tools.\n",
    );
    for name in deferred {
        text.push_str(name);
        text.push('\n');
    }
    text.push_str("</available-deferred-tools>");
    messages.push(LlmMessage::system(text));
}

pub fn activate_deferred_tools(
    active_deferred_tools: &mut HashSet<String>,
    tools: &[ToolSnapshot],
    discovered: Vec<String>,
) -> bool {
    let available = tools
        .iter()
        .filter(|tool| is_deferred_tool(tool))
        .map(|tool| tool.definition.name.as_str())
        .collect::<HashSet<_>>();
    let mut changed = false;
    for name in discovered {
        if available.contains(name.as_str()) {
            changed |= active_deferred_tools.insert(name);
        }
    }
    changed
}

pub fn discovered_deferred_tool_names(result: &ToolResult) -> Vec<String> {
    result
        .metadata
        .get(DEFERRED_TOOLS_METADATA_KEY)
        .and_then(|value| value.get("matches"))
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|match_value| match_value.as_str())
        .map(str::to_string)
        .collect()
}

pub fn tool_is_visible(tools: &[ToolDefinition], name: &str) -> bool {
    tools.iter().any(|tool| tool.name == name)
}

fn is_deferred_tool(tool: &ToolSnapshot) -> bool {
    tool.prompt_metadata
        .as_ref()
        .and_then(|metadata| metadata.deferred_discovery_group.as_ref())
        .is_some()
}

fn is_deferred_gate(tool: &ToolSnapshot) -> bool {
    tool.prompt_metadata
        .as_ref()
        .and_then(|metadata| metadata.deferred_discovery_gate.as_ref())
        .is_some()
}
