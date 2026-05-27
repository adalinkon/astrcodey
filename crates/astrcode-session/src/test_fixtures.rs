//! 单元测试共享 fixture（mock LLM / SessionRuntimeServices）。

use std::sync::Arc;

use astrcode_context::ContextSettings;
use astrcode_core::{
    config::{EffectiveConfig, ExtensionSettings, LlmSettings, OpenAiApiMode},
    llm::{LlmError, LlmEvent, LlmMessage, LlmProvider, ModelLimits},
    tool::ToolDefinition,
};
use astrcode_extensions::runner::ExtensionRunner;
use tokio::sync::mpsc;

use crate::SessionRuntimeServices;

struct PendingMockLlm;

#[async_trait::async_trait]
impl LlmProvider for PendingMockLlm {
    async fn generate(
        &self,
        _messages: Vec<LlmMessage>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<mpsc::UnboundedReceiver<LlmEvent>, LlmError> {
        std::future::pending().await
    }

    fn model_limits(&self) -> ModelLimits {
        ModelLimits {
            max_input_tokens: 1024,
            max_output_tokens: 1024,
        }
    }
}

fn mock_effective_config() -> EffectiveConfig {
    EffectiveConfig {
        llm: LlmSettings {
            provider_kind: "mock".into(),
            base_url: String::new(),
            api_key: String::new(),
            api_mode: OpenAiApiMode::ChatCompletions,
            model_id: "mock".into(),
            max_tokens: 1024,
            context_limit: 1024,
            connect_timeout_secs: 1,
            read_timeout_secs: 1,
            max_retries: 0,
            retry_base_delay_ms: 0,
            supports_prompt_cache_key: false,
            prompt_cache_retention: None,
            reasoning: false,
            thinking_level: None,
        },
        small_llm: LlmSettings {
            provider_kind: "mock".into(),
            base_url: String::new(),
            api_key: String::new(),
            api_mode: OpenAiApiMode::ChatCompletions,
            model_id: "mock".into(),
            max_tokens: 1024,
            context_limit: 1024,
            connect_timeout_secs: 1,
            read_timeout_secs: 1,
            max_retries: 0,
            retry_base_delay_ms: 0,
            supports_prompt_cache_key: false,
            prompt_cache_retention: None,
            reasoning: false,
            thinking_level: None,
        },
        context: ContextSettings::default(),
        agent: astrcode_core::config::AgentSettings::default(),
        extensions: ExtensionSettings::default(),
    }
}

/// 构造带指定 LLM 的 mock [`SessionRuntimeServices`]。
pub fn mock_runtime_services(llm: Arc<dyn LlmProvider>) -> Arc<SessionRuntimeServices> {
    let extension_runner = Arc::new(ExtensionRunner::new(std::time::Duration::from_secs(1)));
    let context_assembler = Arc::new(
        astrcode_context::context_assembler::LlmContextAssembler::new(ContextSettings::default()),
    );
    Arc::new(SessionRuntimeServices::new(
        Arc::clone(&llm),
        llm,
        extension_runner,
        context_assembler,
        mock_effective_config(),
    ))
}

/// 默认 pending mock LLM（不返回 completion）。
pub fn default_mock_runtime_services() -> Arc<SessionRuntimeServices> {
    mock_runtime_services(Arc::new(PendingMockLlm))
}
