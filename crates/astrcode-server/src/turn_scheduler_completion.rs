//! TurnScheduler 内部：completion watcher 循环与 turn 结果摘要。

use std::sync::Arc;

use astrcode_core::types::{SessionId, TurnId};
use astrcode_session::{
    RunTurnResult,
    turn_handle::{TurnHandle, TurnWaitOutcome},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::TurnScheduler;

/// Turn 完成摘要（handler / ACP / 扩展 API 共用）。
#[derive(Debug, Clone)]
pub enum TurnSummary {
    Completed {
        finish_reason: String,
        content: String,
    },
    Failed {
        error: String,
    },
    Aborted,
}

pub(super) type SessionIdleHook = Arc<dyn Fn(SessionId, TurnId, TurnSummary) + Send + Sync>;

pub(super) fn spawn_watcher(
    scheduler: Arc<TurnScheduler>,
    session_id: SessionId,
    turn_id: TurnId,
    handle: TurnHandle,
    completion_tx: Option<oneshot::Sender<TurnSummary>>,
    on_session_idle: Option<SessionIdleHook>,
) {
    let shutdown = scheduler.shutdown_token().clone();
    tokio::spawn(async move {
        run_chain(
            scheduler,
            session_id,
            turn_id,
            handle,
            completion_tx,
            on_session_idle,
            shutdown,
        )
        .await;
    });
}

pub(super) async fn spawn_queued_chain_if_any(
    scheduler: Arc<TurnScheduler>,
    session_id: SessionId,
) {
    let Some((turn_id, handle)) = scheduler.dequeue_submit_raw(&session_id).await else {
        return;
    };
    spawn_watcher(
        Arc::clone(&scheduler),
        session_id,
        turn_id,
        handle,
        None,
        scheduler.session_idle_hook(),
    );
}

pub(super) async fn summary_from_wait(
    scheduler: &TurnScheduler,
    session_id: &SessionId,
    wait_result: Option<RunTurnResult>,
) -> TurnSummary {
    match wait_result {
        Some(result) => match result.output {
            Ok(output) => {
                scheduler.sync_durable_events(session_id).await;
                TurnSummary::Completed {
                    finish_reason: output.finish_reason,
                    content: output.text,
                }
            },
            Err(error) => {
                scheduler.sync_durable_events(session_id).await;
                TurnSummary::Failed {
                    error: error.to_string(),
                }
            },
        },
        None => TurnSummary::Aborted,
    }
}

async fn run_chain(
    scheduler: Arc<TurnScheduler>,
    session_id: SessionId,
    mut turn_id: TurnId,
    mut handle: TurnHandle,
    mut completion_tx: Option<oneshot::Sender<TurnSummary>>,
    on_session_idle: Option<SessionIdleHook>,
    shutdown: CancellationToken,
) {
    loop {
        let wait_result = match handle.wait_or_shutdown(&shutdown).await {
            TurnWaitOutcome::Shutdown => {
                scheduler
                    .release_finished_turn(&session_id, &turn_id)
                    .await;
                return;
            },
            TurnWaitOutcome::Completed(result) => result,
        };
        let summary = summary_from_wait(&scheduler, &session_id, wait_result).await;

        scheduler.release_finished_turn(&session_id, &turn_id).await;

        if let Some((next_turn_id, next_handle)) = scheduler.dequeue_submit_raw(&session_id).await {
            if let Some(tx) = completion_tx.take() {
                let _ = tx.send(summary.clone());
            }
            turn_id = next_turn_id;
            handle = next_handle;
            continue;
        }

        if let Some(tx) = completion_tx.take() {
            let _ = tx.send(summary.clone());
        }
        if let Some(hook) = on_session_idle.as_ref() {
            hook(session_id, turn_id, summary);
        }
        break;
    }
}
