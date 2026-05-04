//! Pipeline：纯函数式 system prompt 组装。
//!
//! `build_system_prompt()` 接收结构化输入，组装固定顺序的 section 后直接
//! 返回完整字符串。扩展通过 `PromptBuild` 事件追加内容到固定 section。

use std::{
    fs,
    path::{Path, PathBuf},
};

use astrcode_core::{
    prompt::{ExtensionPromptBlock, ExtensionSection, SystemPromptInput},
    tool::{ToolDefinition, ToolOrigin},
};
use astrcode_support::hostpaths::astrcode_dir;

// ─── 内置常量 ──────────────────────────────────────────────────────────

pub const DEFAULT_IDENTITY: &str =
    "You are AstrCode, a genius-level engineer and team leader. Code is your expression — \
     correct, maintainable. Thoroughly understand before precisely executing; pursue perfect and \
     elegant best practices, root-causing problems rather than patching symptoms. In complex \
     tasks, orchestrate agent-tool collaboration to coordinate resources and drive projects to \
     success.";

const MAX_IDENTITY_SIZE: usize = 8192;

const RESPONSE_STYLE: &str =
    "Write for the user, not for a console log. Lead with the answer, action, or next step when \
     it is clear.\n\nWhen the task needs tools, multiple steps, or noticeable wait time:\n- \
     Before the first tool call, briefly state what you are going to do.\n- Give short progress \
     updates when you confirm something important, change direction, or make meaningful progress \
     after a stretch of silence.\n- Use complete sentences and enough context that the user can \
     resume cold.\n\nDo not present a guess, lead, or partial result as if it were confirmed. \
     Distinguish a suspicion from a supported finding, and distinguish both from the final \
     conclusion.\n\nPrefer clear prose over running debug-log narration. Use light structure only \
     when it improves readability.\n\nWhen closing out implementation work, briefly cover:\n- \
     what changed,\n- why this shape is correct,\n- what you verified,\n- any remaining risk or \
     next step if verification was partial.";

// ─── Identity 加载 ─────────────────────────────────────────────────────

pub fn user_identity_md_path() -> PathBuf {
    astrcode_dir().join("IDENTITY.md")
}

pub fn user_agents_md_path() -> PathBuf {
    astrcode_dir().join("AGENTS.md")
}

pub fn load_identity_md(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    let identity = if trimmed.len() > MAX_IDENTITY_SIZE {
        truncate_to_char_boundary(trimmed, MAX_IDENTITY_SIZE)
    } else {
        trimmed
    };
    Some(identity.to_string())
}

pub fn load_user_rules(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let content = content.trim();
    if content.is_empty() {
        return None;
    }

    Some(format!(
        "User-wide instructions from {}:\n{}",
        path.display(),
        content
    ))
}

fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }

    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ─── 核心构建函数 ──────────────────────────────────────────────────────

/// 根据结构化输入构建完整的 system prompt 字符串。
///
/// 纯函数，无副作用。section 顺序固定，不可配置。
pub fn build_system_prompt(input: &SystemPromptInput) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Stable identity and behavioral policy come first for prompt-cache reuse.
    let identity = input.identity.as_deref().unwrap_or(DEFAULT_IDENTITY);
    sections.push(render_section("Identity", identity.trim()));

    sections.push(render_section(
        "Environment",
        &format!(
            "Working directory: {}\nOS: {}\nShell: {}\nDate: {}",
            input.working_dir, input.os, input.shell, input.date
        ),
    ));

    sections.push(render_section("Response Style", RESPONSE_STYLE));

    if let Some(rules) = &input.user_rules {
        sections.push(render_section("User Rules", rules.trim()));
    }

    if let Some(project_rules) = &input.project_rules {
        sections.push(render_section("Project Rules", project_rules.trim()));
    }

    if let Some(tool_summary) = tool_summary_section(&input.tools) {
        sections.push(render_section("Tool Summary", &tool_summary));
    }

    if let Some(example_workflow) = example_workflow_section(&input.tools) {
        sections.push(render_section("Example Workflow", &example_workflow));
    }

    // Extension blocks remain grouped so their order is deterministic.
    push_extension_section(
        &mut sections,
        "SystemPromptInstruction",
        &input.extension_blocks,
        ExtensionSection::PlatformInstructions,
    );
    push_extension_section(
        &mut sections,
        "Skills",
        &input.extension_blocks,
        ExtensionSection::Skills,
    );
    push_extension_section(
        &mut sections,
        "Agents",
        &input.extension_blocks,
        ExtensionSection::Agents,
    );

    // Extra instructions (子会话等)
    if let Some(extra) = &input.extra_instructions {
        sections.push(render_section("Additional Instructions", extra.trim()));
    }

    sections.join("\n\n")
}

/// 从扩展块中过滤指定 section 的内容，非空时追加到 sections 列表。
fn push_extension_section(
    sections: &mut Vec<String>,
    title: &str,
    blocks: &[ExtensionPromptBlock],
    kind: ExtensionSection,
) {
    let body = blocks
        .iter()
        .filter(|b| b.section == kind)
        .map(|b| b.content.trim())
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !body.is_empty() {
        sections.push(render_section(title, &body));
    }
}

fn render_section(title: &str, body: &str) -> String {
    let body = indent_body(body.trim());
    format!("[{title}]\n{body}")
}

fn indent_body(body: &str) -> String {
    body.lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("  {}", line.trim_end())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_summary_section(tools: &[ToolDefinition]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let mut lines = vec![
        "Use the narrowest tool that can answer the request. Prefer read-only inspection before \
         mutation."
            .to_string(),
        "All file paths passed to builtin file tools must stay inside the working directory \
         unless the tool explicitly accepts a persisted result reference."
            .to_string(),
        "When a tool returns a persisted-result reference for large output, keep the reference in \
         context and inspect it with `read` chunks instead of asking the tool to inline the whole \
         result again."
            .to_string(),
        String::new(),
    ];

    push_tool_group(&mut lines, "Builtin Tools", tools, |tool| {
        tool.origin == ToolOrigin::Builtin
            || matches!(tool.name.as_str(), "Skill" | "agent" | "tool_search_tool")
    });

    let agent_tools = tools
        .iter()
        .filter(|tool| tool.name == "agent")
        .collect::<Vec<_>>();
    if !agent_tools.is_empty() {
        lines.push(String::new());
        lines.push("Agent Collaboration Tools".into());
        lines.push(
            "- Use these tools to spawn and inspect child agents. Keep the original agent \
             identifier byte-for-byte across related calls."
                .into(),
        );
        for tool in agent_tools {
            lines.push(format!(
                "- `{}`: {}",
                tool.name,
                one_line(&tool.description)
            ));
        }
    }

    let external_tools = tools
        .iter()
        .filter(|tool| is_external_tool(tool))
        .collect::<Vec<_>>();
    if !external_tools.is_empty() {
        lines.push(String::new());
        lines.push("External MCP / Plugin Tools".into());
        for tool in external_tools {
            lines.push(format!(
                "- `{}`: {}",
                tool.name,
                one_line(&tool.description)
            ));
        }
    }

    if has_tool(tools, "tool_search_tool") && tools.iter().any(is_external_tool) {
        lines.push(String::new());
        lines.push("When To Use `tool_search_tool`".into());
        lines.push("- Builtin tools do not need discovery through `tool_search_tool`.".into());
        lines.push(
            "- Use `tool_search_tool` when builtin tools are not enough and you need the schema \
             of an external MCP/plugin tool from its rough summary."
                .into(),
        );
        lines.push(
            "- After `tool_search_tool` returns candidate tools and schemas, call the matching \
             concrete tool directly."
                .into(),
        );
    }

    Some(lines.join("\n").trim().to_string())
}

fn example_workflow_section(tools: &[ToolDefinition]) -> Option<String> {
    if !(has_tool(tools, "tool_search_tool") && tools.iter().any(is_external_tool)) {
        return None;
    }

    Some(
        "1. Check whether builtin tools already solve the task.\n2. If an external tool is needed \
         or a visible `mcp__...` tool has unclear parameters, call `tool_search_tool` first with \
         part of the tool name or the task purpose, for example `{ \"query\": \"webReader\" }` or \
         `{ \"query\": \"github repo structure\" }`.\n3. Read the returned input schema from \
         `tool_search_tool` before making the external tool call.\n4. Pick the matching concrete \
         tool from the search results, such as `mcp__...`, and call it directly. Do not guess \
         argument names when schema is available."
            .to_string(),
    )
}

fn push_tool_group(
    lines: &mut Vec<String>,
    title: &str,
    tools: &[ToolDefinition],
    include: impl Fn(&ToolDefinition) -> bool,
) {
    let mut selected = tools
        .iter()
        .filter(|tool| include(tool))
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| left.name.cmp(&right.name));
    if selected.is_empty() {
        return;
    }

    lines.push(title.into());
    for tool in selected {
        lines.push(format!(
            "- `{}`: {}",
            tool.name,
            one_line(&tool.description)
        ));
    }
}

fn is_external_tool(tool: &ToolDefinition) -> bool {
    tool.origin == ToolOrigin::Extension
        || tool.name.starts_with("mcp__")
        || (tool.origin == ToolOrigin::Bundled
            && !matches!(tool.name.as_str(), "Skill" | "agent" | "tool_search_tool"))
}

fn has_tool(tools: &[ToolDefinition], name: &str) -> bool {
    tools.iter().any(|tool| tool.name == name)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ─── AGENTS.md 加载（供 bootstrap 使用） ────────────────────────────────

/// 从 working_dir 向上遍历查找所有 AGENTS.md，由浅到深排序。
///
/// 目录越深的 AGENTS.md 规则越具体，因此加载时先返回浅层，再返回深层，
/// 让最终 prompt 里的冲突规则顺序和覆盖语义一致。
pub fn find_agents_files(working_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = Some(working_dir);
    while let Some(dir) = current {
        dirs.push(dir.to_path_buf());
        current = dir.parent();
    }

    dirs.reverse();
    dirs.into_iter()
        .map(|dir| dir.join("AGENTS.md"))
        .filter(|path| path.is_file())
        .collect()
}

/// 读取并合并 AGENTS.md 文件为一段 project rules 文本。
pub fn load_project_rules(working_dir: &Path) -> Option<String> {
    let files = find_agents_files(working_dir);
    if files.is_empty() {
        return None;
    }

    let mut content = String::from(
        "以下内容来自 AGENTS.md。必须遵守；如果规则冲突，目录更深的 AGENTS.md 优先。\n",
    );
    for path in files {
        if let Ok(text) = fs::read_to_string(&path) {
            content.push_str("\n--- ");
            content.push_str(&path.display().to_string());
            content.push_str(" ---\n");
            content.push_str(&text);
            if !text.ends_with('\n') {
                content.push('\n');
            }
        }
    }

    non_empty_string(content)
}

fn non_empty_string(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use astrcode_core::tool::ExecutionMode;

    use super::*;

    fn tool(name: &str, description: &str, origin: ToolOrigin) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            parameters: Default::default(),
            origin,
            execution_mode: ExecutionMode::Sequential,
        }
    }

    #[test]
    fn build_renders_all_sections_in_order() {
        let input = SystemPromptInput {
            working_dir: "/test".into(),
            os: "linux".into(),
            shell: "bash".into(),
            date: "2026-04-29".into(),
            identity: Some("custom identity".into()),
            user_rules: Some("test rules".into()),
            project_rules: Some("project rules content".into()),
            tools: vec![
                tool("read", "Read files.", ToolOrigin::Builtin),
                tool(
                    "tool_search_tool",
                    "Search external tools.",
                    ToolOrigin::Bundled,
                ),
                tool(
                    "mcp__demo__search",
                    "Search demo server.",
                    ToolOrigin::Bundled,
                ),
            ],
            extension_blocks: vec![
                ExtensionPromptBlock {
                    section: ExtensionSection::Skills,
                    content: "skill a".into(),
                },
                ExtensionPromptBlock {
                    section: ExtensionSection::Agents,
                    content: "agent x".into(),
                },
                ExtensionPromptBlock {
                    section: ExtensionSection::PlatformInstructions,
                    content: "extra hint".into(),
                },
            ],
            extra_instructions: Some("extra body".into()),
        };

        let prompt = build_system_prompt(&input);

        // All sections present
        assert!(prompt.contains("[Identity]\n  custom identity"));
        assert!(prompt.contains("[Environment]\n  Working directory: /test"));
        assert!(prompt.contains("[User Rules]\n  test rules"));
        assert!(prompt.contains("[Project Rules]\n  project rules content"));
        assert!(prompt.contains("[Tool Summary]"));
        assert!(prompt.contains("- `read`: Read files."));
        assert!(prompt.contains("When To Use `tool_search_tool`"));
        assert!(prompt.contains("[Example Workflow]"));
        assert!(prompt.contains("[SystemPromptInstruction]\n  extra hint"));
        assert!(prompt.contains("[Skills]\n  skill a"));
        assert!(prompt.contains("[Agents]\n  agent x"));
        assert!(prompt.contains("[Response Style]"));
        assert!(prompt.contains("[Additional Instructions]\n  extra body"));

        // Ordering keeps stable policy text before volatile environment data.
        let identity = prompt.find("[Identity]").unwrap();
        let env = prompt.find("[Environment]").unwrap();
        let style = prompt.find("[Response Style]").unwrap();
        let user_rules = prompt.find("[User Rules]").unwrap();
        let project_rules = prompt.find("[Project Rules]").unwrap();
        let tools = prompt.find("[Tool Summary]").unwrap();
        let workflow = prompt.find("[Example Workflow]").unwrap();
        let platform = prompt.find("[SystemPromptInstruction]").unwrap();
        let skills = prompt.find("[Skills]").unwrap();
        let agents = prompt.find("[Agents]").unwrap();

        assert!(identity < env);
        assert!(env < style);
        assert!(style < user_rules);
        assert!(user_rules < project_rules);
        assert!(project_rules < tools);
        assert!(tools < workflow);
        assert!(workflow < platform);
        assert!(platform < skills);
        assert!(skills < agents);
    }

    #[test]
    fn empty_optionals_are_skipped() {
        let input = SystemPromptInput {
            working_dir: "/test".into(),
            os: "linux".into(),
            shell: "bash".into(),
            date: "2026-04-29".into(),
            identity: None,
            user_rules: None,
            project_rules: None,
            tools: vec![],
            extension_blocks: vec![],
            extra_instructions: None,
        };

        let prompt = build_system_prompt(&input);

        // Should have Identity (fallback to default), Environment, Response Style
        assert!(prompt.contains("[Identity]\n"));
        assert!(prompt.contains("[Environment]"));
        assert!(prompt.contains("[Response Style]"));
        // Should NOT have empty sections
        assert!(!prompt.contains("[User Rules]"));
        assert!(!prompt.contains("[Project Rules]"));
        assert!(!prompt.contains("[Tool Summary]"));
        assert!(!prompt.contains("[Example Workflow]"));
        assert!(!prompt.contains("[SystemPromptInstruction]"));
        assert!(!prompt.contains("[Skills]"));
        assert!(!prompt.contains("[Agents]"));
    }

    #[test]
    fn environment_changes_keep_identity_prefix_stable() {
        let base = SystemPromptInput {
            working_dir: "/one".into(),
            os: "linux".into(),
            shell: "bash".into(),
            date: "2026-04-29".into(),
            identity: Some("stable identity".into()),
            user_rules: Some("stable user rules".into()),
            project_rules: Some("stable project rules".into()),
            tools: vec![tool("read", "Read files.", ToolOrigin::Builtin)],
            extension_blocks: vec![
                ExtensionPromptBlock {
                    section: ExtensionSection::PlatformInstructions,
                    content: "stable platform".into(),
                },
                ExtensionPromptBlock {
                    section: ExtensionSection::Skills,
                    content: "stable skills".into(),
                },
                ExtensionPromptBlock {
                    section: ExtensionSection::Agents,
                    content: "stable agents".into(),
                },
            ],
            extra_instructions: None,
        };
        let mut changed = base.clone();
        changed.working_dir = "/two".into();
        changed.shell = "zsh".into();

        let first = build_system_prompt(&base);
        let second = build_system_prompt(&changed);
        let env = first.find("[Environment]").unwrap();

        assert_eq!(&first[..env], &second[..env]);
    }
}
