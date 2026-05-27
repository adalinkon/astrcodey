//! Turn 管理 — Agent turn 任务启停。

use astrcode_core::{types::*, user_prompt::UserPromptParts};

use super::{CommandHandler, HandlerError, errors::turn_schedule_error_for_client};
use crate::turn_scheduler::{TurnScheduleError, TurnSummary};

impl CommandHandler {
    /// 启动新 Turn（completion 由 scheduler 内部跟踪）。
    pub(in crate::handler) async fn start_turn_for_session(
        &self,
        sid: SessionId,
        input: UserPromptParts,
        completion_tx: Option<tokio::sync::oneshot::Sender<TurnSummary>>,
    ) -> Result<TurnId, HandlerError> {
        tracing::info!(
            session_id = %sid,
            text_len = input.text.len(),
            image_count = input.images.len(),
            "start_turn"
        );
        let result = if let Some(tx) = completion_tx {
            self.scheduler
                .submit_tracked_with_notify(sid.clone(), input, tx)
                .await
        } else {
            self.scheduler.submit_tracked(sid.clone(), input).await
        };
        result.map_err(|e| {
            let (code, err) = turn_schedule_error_for_client(e);
            if code == 40900 {
                self.send_error(code, "A turn is already running");
            }
            err
        })
    }

    pub(in crate::handler) async fn abort_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), HandlerError> {
        match self.scheduler.abort(session_id).await {
            Ok(()) => Ok(()),
            Err(TurnScheduleError::NoActiveTurn) => {
                self.send_error(40400, "No active turn");
                Err(HandlerError::NoActiveTurn)
            },
            Err(e) => Err(HandlerError::from(e)),
        }
    }

    pub(in crate::handler) async fn abort_active_turn(&self) -> Result<(), HandlerError> {
        let Some(sid) = self.active_session_id.as_ref() else {
            self.send_error(40400, "No active turn");
            return Ok(());
        };
        self.abort_session(sid).await
    }

    pub(in crate::handler) async fn repair_stale_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), HandlerError> {
        self.scheduler
            .repair_stale(session_id)
            .await
            .map_err(HandlerError::from)
    }

    pub(in crate::handler) async fn submit_input_with_completion(
        &self,
        sid: SessionId,
        input: UserPromptParts,
    ) -> Result<(TurnId, tokio::sync::oneshot::Receiver<TurnSummary>), HandlerError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let turn_id = self.start_turn_for_session(sid, input, Some(tx)).await?;
        Ok((turn_id, rx))
    }
}
