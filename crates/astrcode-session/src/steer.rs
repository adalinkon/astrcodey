//! 中途用户输入（steer）在 step 边界的显式 flush。

use astrcode_context::compaction::is_synthetic_context_message;
use astrcode_core::{llm::LlmRole, storage::SessionReadModel};

/// 统计读模型中 provider 可见的非合成 user 消息条数。
pub(crate) fn count_visible_user_messages(model: &SessionReadModel) -> usize {
    model
        .messages
        .iter()
        .filter(|entry| {
            entry.message.role == LlmRole::User && !is_synthetic_context_message(&entry.message)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use astrcode_core::{llm::LlmMessage, storage::SequencedLlmMessage, types::SessionId};

    use super::*;

    fn model_with_messages(messages: Vec<LlmMessage>) -> SessionReadModel {
        let mut model = SessionReadModel::empty(SessionId::new("s-test"));
        model.messages = messages
            .into_iter()
            .enumerate()
            .map(|(updated_seq, message)| SequencedLlmMessage {
                message,
                updated_seq: updated_seq as u64,
                source: None,
            })
            .collect();
        model
    }

    #[test]
    fn count_visible_user_messages_excludes_compact_summary_marker() {
        let model = model_with_messages(vec![
            LlmMessage::user("real"),
            LlmMessage::user("<compact_summary>summary</compact_summary>"),
            LlmMessage::user("also real"),
        ]);
        assert_eq!(count_visible_user_messages(&model), 2);
    }
}
