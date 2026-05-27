//! 运行中 turn 的句柄。
//!
//! `Session::submit` 返回 `TurnHandle`，调用方可以等待完成或中止。Session 内部不再
//! 维护 `active_turn` HashMap：handle 的所有权由调用方持有，析构即放弃。

use astrcode_core::types::TurnId;
use tokio::{
    sync::oneshot,
    task::{AbortHandle, JoinHandle},
};
use tokio_util::sync::CancellationToken;

use crate::turn_runner::RunTurnResult;

/// [`TurnHandle::wait_or_shutdown`] 的结果。
pub enum TurnWaitOutcome {
    Completed(Option<RunTurnResult>),
    Shutdown,
}

/// 一次 turn 的运行时句柄。
pub struct TurnHandle {
    turn_id: TurnId,
    join: JoinHandle<()>,
    completion_rx: oneshot::Receiver<RunTurnResult>,
}

impl TurnHandle {
    pub(crate) fn new(
        turn_id: TurnId,
        join: JoinHandle<()>,
        completion_rx: oneshot::Receiver<RunTurnResult>,
    ) -> Self {
        Self {
            turn_id,
            join,
            completion_rx,
        }
    }

    pub fn turn_id(&self) -> &TurnId {
        &self.turn_id
    }

    pub fn is_running(&self) -> bool {
        !self.join.is_finished()
    }

    pub fn abort_handle(&self) -> AbortHandle {
        self.join.abort_handle()
    }

    /// 中止后台 task。已完成的 handle 上调用是 no-op。
    pub fn abort(&self) {
        if !self.join.is_finished() {
            self.join.abort();
        }
    }

    /// 等待 turn 结束并返回结果。
    ///
    /// 通道被关闭（例如 task panicked）时返回 `None`。
    pub async fn wait(self) -> Option<RunTurnResult> {
        self.completion_rx.await.ok()
    }

    /// 等待 turn 结束，或在 `shutdown` 触发时中止 turn 并返回 [`TurnWaitOutcome::Shutdown`]。
    pub async fn wait_or_shutdown(self, shutdown: &CancellationToken) -> TurnWaitOutcome {
        let abort = self.abort_handle();
        tokio::select! {
            result = self.wait() => TurnWaitOutcome::Completed(result),
            () = shutdown.cancelled() => {
                abort.abort();
                TurnWaitOutcome::Shutdown
            }
        }
    }
}
