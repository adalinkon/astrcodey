//! 下一 turn 输入队列（FIFO）。

use astrcode_core::{
    types::{SessionId, TurnId},
    user_prompt::UserPromptParts,
};
use astrcode_session::turn_handle::TurnHandle;

use super::{SubmitOutcome, TurnScheduleError, TurnScheduler};

/// 连发 prompt 的调度结果。
pub enum UserInputOutcome {
    Queued,
    Started { turn_id: TurnId },
}

impl TurnScheduler {
    /// 连发 prompt：有活跃 turn 则 FIFO 入队，否则 `submit_tracked`。
    pub async fn accept_user_input(
        &self,
        session_id: SessionId,
        input: UserPromptParts,
    ) -> Result<UserInputOutcome, TurnScheduleError> {
        if self.registry.has_active(&session_id) {
            self.enqueue_pending_input(session_id, input)?;
            return Ok(UserInputOutcome::Queued);
        }
        let turn_id = self.submit_tracked(session_id, input).await?;
        Ok(UserInputOutcome::Started { turn_id })
    }

    /// 通知需要处理，在**下一 turn** 触发（[`accept_user_input`] 的别名映射）。
    pub async fn notify_turn(
        &self,
        session_id: SessionId,
        input: UserPromptParts,
    ) -> Result<SubmitOutcome, TurnScheduleError> {
        match self.accept_user_input(session_id, input).await? {
            UserInputOutcome::Queued => Ok(SubmitOutcome::Queued),
            UserInputOutcome::Started { turn_id } => Ok(SubmitOutcome::Started { turn_id }),
        }
    }

    pub(super) fn enqueue_pending_input(
        &self,
        session_id: SessionId,
        input: UserPromptParts,
    ) -> Result<(), TurnScheduleError> {
        let mut queues = self.pending_queues.lock();
        let queue = queues.entry(session_id.clone()).or_default();
        queue.push_back(super::PendingMessage { input });

        tracing::info!(
            session_id = %session_id,
            queue_len = queue.len(),
            "message queued for next turn"
        );
        Ok(())
    }

    pub(super) fn dequeue_next_pending(&self, session_id: &SessionId) -> Option<UserPromptParts> {
        let mut queues = self.pending_queues.lock();
        let queue = queues.get_mut(session_id)?;
        while let Some(pending) = queue.pop_front() {
            let input = pending.input;
            if input.is_submittable() {
                if queue.is_empty() {
                    queues.remove(session_id);
                }
                return Some(input);
            }
            tracing::warn!(
                session_id = %session_id,
                text_len = input.text.len(),
                image_count = input.images.len(),
                "skipped non-submittable queued input"
            );
            if queue.is_empty() {
                queues.remove(session_id);
                return None;
            }
        }
        None
    }

    pub(super) fn clear_pending_queue(&self, session_id: &SessionId) {
        if self.pending_queues.lock().remove(session_id).is_some() {
            tracing::info!(session_id = %session_id, "cleaned up pending message queue");
        }
    }

    pub(crate) async fn drain_child_completions_after_turn(&self, session_id: &SessionId) {
        self.process_child_completions(session_id).await;
    }

    /// 弹出一条 pending 输入并 `submit_raw`（由 completion 链注册 watcher）。
    pub(super) async fn dequeue_submit_raw(
        &self,
        session_id: &SessionId,
    ) -> Option<(TurnId, TurnHandle)> {
        if self.registry.has_active(session_id) {
            return None;
        }
        let input = self.dequeue_next_pending(session_id)?;
        tracing::info!(session_id = %session_id, "auto-submitting next queued message for new turn");
        match self.submit_raw(session_id.clone(), input).await {
            Ok(pair) => Some(pair),
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "failed to auto-submit queued message"
                );
                None
            },
        }
    }
}
