//! 扩展 [`SessionOperations`] trait 的 server 实现。
//!
//! 子 agent 编排（guard / recycle / task 回填）与 [`TurnScheduler`] 协作；
//! durable 读走 [`SessionManager::event_store`]，不写第二套查询层。

use std::sync::Arc;

use astrcode_core::{
    event::EventPayload,
    tool::{
        CreateSessionRequest, SessionApiError, SessionHandle, SessionOperations, SessionStatus,
        SubmitTurnRequest, SubmitTurnResult,
    },
    types::{SessionId, new_message_id},
    user_prompt::UserPromptParts,
};
use astrcode_session::child_turn::{ChildCleanup, ChildOutcome, ChildTurnConfig, ChildTurnGuard};

use crate::{session_manager::SessionManager, turn_scheduler::{TurnScheduleError, TurnScheduler}};

/// 服务端 SessionOperations 实现。
pub struct ServerSessionOperations {
    pub session_manager: Arc<SessionManager>,
    pub scheduler: Arc<TurnScheduler>,
}

#[async_trait::async_trait]
impl SessionOperations for ServerSessionOperations {
    async fn create_session(
        &self,
        parent_session_id: &str,
        request: CreateSessionRequest,
    ) -> Result<SessionHandle, SessionApiError> {
        let parent_sid = SessionId::from(parent_session_id);
        let parent_session = self
            .session_manager
            .open(parent_sid.clone())
            .await
            .map_err(|e| SessionApiError::NotFound(format!("parent: {e}")))?;

        let depth = self.session_depth(&parent_sid).await?;
        let max_depth = self
            .session_manager
            .config()
            .read_effective()
            .agent
            .max_depth;
        if depth >= max_depth {
            return Err(SessionApiError::MaxDepthExceeded {
                current: depth,
                max: max_depth,
            });
        }

        let parent_model = parent_session
            .read_model()
            .await
            .map_err(|e| SessionApiError::Internal(e.to_string()))?;

        let working_dir = request.working_dir.unwrap_or(parent_model.working_dir);
        let model_id = request
            .model_preference
            .filter(|m| m != "inherit" && !m.is_empty())
            .unwrap_or(parent_model.model_id);

        let task = request.task.unwrap_or_default();

        let child = parent_session
            .spawn_child(
                &working_dir,
                &model_id,
                request.name,
                task,
                request.system_prompt,
                request.tool_policy,
                request.source_extension.as_deref(),
                request.tool_call_id.into(),
            )
            .await
            .map_err(|e| SessionApiError::Internal(format!("spawn child: {e}")))?;

        let child_sid = child.id().clone();
        self.session_manager.register_child_session(&child);

        Ok(SessionHandle {
            session_id: child_sid.into_string(),
        })
    }

    async fn inject_message(
        &self,
        caller_session_id: &str,
        target_session_id: &str,
        content: String,
    ) -> Result<(), SessionApiError> {
        let caller_sid = SessionId::from(caller_session_id);
        let target_sid = SessionId::from(target_session_id);

        self.verify_access(&caller_sid, &target_sid).await?;

        if self.scheduler.registry().has_active(&target_sid) {
            match self
                .scheduler
                .inject(
                    &target_sid,
                    UserPromptParts::text_only(content.clone()),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(TurnScheduleError::NoActiveTurn) => {
                    tracing::debug!(
                        session_id = %target_sid,
                        "inject raced with turn completion; persisting as durable message"
                    );
                },
                Err(error) => {
                    return Err(SessionApiError::Internal(error.to_string()));
                },
            }
        }

        self.persist_idle_user_message(&target_sid, content).await
    }

    async fn submit_turn(
        &self,
        caller_session_id: &str,
        request: SubmitTurnRequest,
    ) -> Result<SubmitTurnResult, SessionApiError> {
        let caller_sid = SessionId::from(caller_session_id);
        let target_sid = SessionId::from(request.target_session_id.as_str());

        self.verify_access(&caller_sid, &target_sid).await?;

        let is_child_turn = caller_sid != target_sid;
        let user_prompt = request.user_prompt.clone();
        if is_child_turn {
            self.ensure_child_task_recorded(&caller_sid, &target_sid, &user_prompt)
                .await;
        }

        let session = self
            .session_manager
            .open(target_sid.clone())
            .await
            .map_err(|e| SessionApiError::NotFound(e.to_string()))?;
        if let Err(e) = session.ensure_runtime_ready(false).await {
            return Err(SessionApiError::Internal(format!("runtime init: {e}")));
        }

        let input = UserPromptParts::text_only(user_prompt);

        let result = if is_child_turn {
            let (turn_id, handle) = self
                .scheduler
                .submit_untracked(target_sid.clone(), input)
                .await
                .map_err(|e| SessionApiError::Internal(format!("submit: {e}")))?;
            self.submit_child_turn(
                caller_sid.clone(),
                target_sid.clone(),
                turn_id,
                handle,
                request,
            )
            .await
        } else if request.wait_for_result {
            match self
                .scheduler
                .submit_and_wait(target_sid.clone(), input)
                .await
            {
                Ok(crate::turn_scheduler::TurnSummary::Completed { content, .. }) => {
                    Ok(SubmitTurnResult::Completed { content })
                },
                Ok(crate::turn_scheduler::TurnSummary::Failed { error }) => {
                    Err(SessionApiError::Internal(format!("turn error: {error}")))
                },
                Ok(crate::turn_scheduler::TurnSummary::Aborted) => {
                    Err(SessionApiError::Internal("turn aborted".into()))
                },
                Err(e) => Err(SessionApiError::Internal(format!("submit: {e}"))),
            }
        } else {
            let turn_id = self
                .scheduler
                .submit_tracked(target_sid.clone(), input)
                .await
                .map_err(|e| SessionApiError::Internal(format!("submit: {e}")))?;
            Ok(SubmitTurnResult::Backgrounded {
                task_id: turn_id.into_string(),
                session_id: target_sid.into_string(),
            })
        };

        // 与旧语义一致：每次 submit 返回前 drain 已完成子 agent（幂等，collect-once）。
        self.scheduler.process_child_completions(&caller_sid).await;

        result
    }

    async fn query_session(
        &self,
        caller_session_id: &str,
        target_session_id: &str,
    ) -> Result<SessionStatus, SessionApiError> {
        let caller_sid = SessionId::from(caller_session_id);
        let target_sid = SessionId::from(target_session_id);

        self.verify_access(&caller_sid, &target_sid).await?;

        let model = self
            .session_manager
            .event_store()
            .session_read_model(&target_sid)
            .await
            .map_err(|e| SessionApiError::NotFound(e.to_string()))?;

        Ok(SessionStatus {
            alive: true,
            has_active_turn: self
                .scheduler
                .session_has_active_turn(&target_sid, model.phase),
            last_finish_reason: None,
            message_count: model.messages.len(),
        })
    }

    async fn recycle_session(
        &self,
        caller_session_id: &str,
        target_session_id: &str,
    ) -> Result<(), SessionApiError> {
        let caller_sid = SessionId::from(caller_session_id);
        let target_sid = SessionId::from(target_session_id);

        self.verify_access(&caller_sid, &target_sid).await?;

        self.scheduler.recycle_child(&caller_sid, &target_sid).await;

        Ok(())
    }

    async fn delete_session(
        &self,
        caller_session_id: &str,
        target_session_id: &str,
    ) -> Result<(), SessionApiError> {
        let caller_sid = SessionId::from(caller_session_id);
        let target_sid = SessionId::from(target_session_id);

        self.verify_access(&caller_sid, &target_sid).await?;

        self.session_manager
            .delete_with_turn_teardown(self.scheduler.as_ref(), &target_sid)
            .await
            .map_err(|e| SessionApiError::Internal(e.to_string()))?;

        Ok(())
    }

    async fn restore_session(
        &self,
        caller_session_id: &str,
        target_session_id: &str,
    ) -> Result<(), SessionApiError> {
        let caller_sid = SessionId::from(caller_session_id);
        let target_sid = SessionId::from(target_session_id);

        self.verify_access(&caller_sid, &target_sid).await?;

        self.session_manager
            .restore_session(&target_sid)
            .await
            .map_err(|e| SessionApiError::Internal(e.to_string()))?;

        Ok(())
    }
}

impl ServerSessionOperations {
    async fn persist_idle_user_message(
        &self,
        target_sid: &SessionId,
        content: String,
    ) -> Result<(), SessionApiError> {
        let session = self
            .session_manager
            .open(target_sid.clone())
            .await
            .map_err(|e| SessionApiError::NotFound(e.to_string()))?;

        let message_id = new_message_id();
        session
            .emit_durable(
                None,
                EventPayload::UserMessage {
                    message_id,
                    text: content,
                    images: vec![],
                },
            )
            .await
            .map_err(|e| SessionApiError::Internal(e.to_string()))?;

        Ok(())
    }

    async fn submit_child_turn(
        &self,
        parent_sid: SessionId,
        child_sid: SessionId,
        turn_id: astrcode_core::types::TurnId,
        handle: astrcode_session::turn_handle::TurnHandle,
        request: SubmitTurnRequest,
    ) -> Result<SubmitTurnResult, SessionApiError> {
        let cleanup = if request.recycle_on_complete {
            ChildCleanup::Recycle
        } else {
            ChildCleanup::Keep
        };
        let config = ChildTurnConfig {
            child_session_id: child_sid.clone(),
            parent_session_id: parent_sid.clone(),
            cleanup,
            notify_on_complete: request.notify_parent_on_complete,
        };

        let parent_session = self
            .session_manager
            .open(parent_sid.clone())
            .await
            .map_err(|e| SessionApiError::Internal(format!("open parent: {e}")))?;
        let parent_session = Arc::new(parent_session);
        let guard = Arc::new(ChildTurnGuard::spawn(
            handle,
            config,
            Arc::clone(&parent_session),
            parent_session.runtime().completed_tx(),
            request.wait_for_result,
            self.scheduler.shutdown_token().clone(),
        ));
        parent_session
            .runtime()
            .child_turn_manager()
            .register(Arc::clone(&guard));

        if request.wait_for_result {
            let outcome = guard.outcome().await;
            self.scheduler.sync_durable_events(&child_sid).await;
            self.scheduler
                .release_finished_turn(&child_sid, &turn_id)
                .await;
            self.scheduler
                .continue_queued_turns_if_any(child_sid.clone())
                .await;
            match outcome {
                ChildOutcome::Completed {
                    response: Some(content),
                    ..
                } => Ok(SubmitTurnResult::Completed { content }),
                ChildOutcome::Completed { response: None, .. } => Err(SessionApiError::Internal(
                    "child turn completed without response payload".into(),
                )),
                ChildOutcome::Failed { error } => {
                    Err(SessionApiError::Internal(format!("turn error: {error}")))
                },
                ChildOutcome::Aborted => Err(SessionApiError::Internal("turn aborted".into())),
                ChildOutcome::TimedOut => Err(SessionApiError::Internal("turn timed out".into())),
            }
        } else {
            Ok(SubmitTurnResult::Backgrounded {
                task_id: turn_id.into_string(),
                session_id: child_sid.into_string(),
            })
        }
    }

    async fn ensure_child_task_recorded(
        &self,
        parent_sid: &SessionId,
        child_sid: &SessionId,
        task: &str,
    ) {
        if task.is_empty() {
            return;
        }
        let model = match self
            .session_manager
            .event_store()
            .session_read_model(parent_sid)
            .await
        {
            Ok(model) => model,
            Err(error) => {
                tracing::warn!(
                    parent_session_id = %parent_sid,
                    child_session_id = %child_sid,
                    error = %error,
                    "ensure_child_task_recorded: failed to read parent model"
                );
                return;
            },
        };
        let needs_backfill = model
            .agent_sessions
            .iter()
            .any(|link| link.child_session_id == *child_sid && link.task.is_empty());
        if !needs_backfill {
            return;
        }
        let event = astrcode_core::event::Event::new(
            parent_sid.clone(),
            None,
            astrcode_session::agent_session_task_assigned_payload(
                child_sid.clone(),
                task.to_string(),
            ),
        );
        if let Err(error) = self
            .session_manager
            .append_durable_event(parent_sid, event)
            .await
        {
            tracing::warn!(
                parent_session_id = %parent_sid,
                child_session_id = %child_sid,
                error = %error,
                "failed to append AgentSessionTaskAssigned event"
            );
        }
    }

    async fn verify_access(
        &self,
        caller: &SessionId,
        target: &SessionId,
    ) -> Result<(), SessionApiError> {
        if caller == target {
            return Ok(());
        }
        if self
            .parent_chain(target)
            .await?
            .iter()
            .any(|id| id == caller)
        {
            return Ok(());
        }
        Err(SessionApiError::PermissionDenied(format!(
            "session {target} is not a descendant of {caller}"
        )))
    }

    async fn session_depth(&self, session_id: &SessionId) -> Result<usize, SessionApiError> {
        Ok(self.parent_chain(session_id).await?.len())
    }

    async fn parent_chain(&self, from: &SessionId) -> Result<Vec<SessionId>, SessionApiError> {
        let store = self.session_manager.event_store();
        let max_depth = self
            .session_manager
            .config()
            .read_effective()
            .agent
            .max_depth;
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut current = from.clone();
        loop {
            if !visited.insert(current.clone()) {
                return Err(SessionApiError::Internal(format!(
                    "parent chain cycle detected at session {current}"
                )));
            }
            if chain.len() > max_depth {
                return Err(SessionApiError::Internal(format!(
                    "parent chain exceeds configured max_depth ({max_depth})"
                )));
            }
            let model = store
                .session_read_model(&current)
                .await
                .map_err(|e| SessionApiError::NotFound(e.to_string()))?;
            match model.parent_session_id {
                Some(parent) => {
                    chain.push(parent.clone());
                    current = parent;
                },
                None => return Ok(chain),
            }
        }
    }
}
