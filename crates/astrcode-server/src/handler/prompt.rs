//! Prompt 提交、注入与斜杠命令拦截。

use astrcode_core::{types::SessionId, user_prompt::UserPromptParts};
use astrcode_protocol::commands::Attachment;
use astrcode_support::prompt_attachments::{self, PromptAttachmentError};

use super::{CommandHandler, HandlerError, PromptSubmission, slash};

impl CommandHandler {
    pub(super) async fn submit_prompt(
        &mut self,
        text: String,
        attachments: Vec<Attachment>,
    ) -> Result<(), HandlerError> {
        let input = user_prompt_from_wire(text, attachments)?;
        let sid = self.ensure_session().await?;
        match self
            .submit_input_for_session(sid.clone(), input.clone())
            .await
        {
            Ok(_) => Ok(()),
            Err(HandlerError::TurnAlreadyRunning) => {
                self.inject_mid_turn_message_for_session(&sid, input).await
            },
            Err(error) => {
                self.send_error(slash::command_error_code(&error), &error.to_string());
                Err(error)
            },
        }
    }

    pub(super) async fn inject_mid_turn_message(
        &mut self,
        text: String,
    ) -> Result<(), HandlerError> {
        let sid = self.ensure_session().await?;
        self.inject_mid_turn_message_for_session(&sid, UserPromptParts::text_only(text))
            .await
    }

    pub(super) async fn inject_mid_turn_message_for_session(
        &self,
        sid: &SessionId,
        input: UserPromptParts,
    ) -> Result<(), HandlerError> {
        self.scheduler
            .inject(sid, input)
            .await
            .map_err(HandlerError::from)?;
        Ok(())
    }

    pub async fn submit_input_for_session(
        &mut self,
        sid: SessionId,
        input: impl Into<UserPromptParts>,
    ) -> Result<PromptSubmission, HandlerError> {
        let input = input.into();
        let visible_text = if input.text.trim().starts_with('/') {
            input.text.clone()
        } else {
            input.display_text()
        };
        if let Some(command) = slash::parse_slash_command(&visible_text) {
            match self
                .execute_slash_command_for_session(sid.clone(), command, visible_text.clone())
                .await
            {
                Err(HandlerError::UnknownCommand(_)) => {},
                other => return other,
            }
        }

        self.start_turn_for_session(sid, input, None)
            .await
            .map(|turn_id| PromptSubmission::Accepted { turn_id })
    }

    pub async fn command_infos_for_session(
        &self,
        sid: &SessionId,
    ) -> Result<Vec<astrcode_protocol::events::ExtensionCommandInfo>, HandlerError> {
        let state = self
            .runtime
            .session_manager()
            .read_model(sid)
            .await
            .map_err(HandlerError::SessionManager)?;
        Ok(self.command_infos_for_working_dir(&state.working_dir).await)
    }
}

pub(crate) fn user_prompt_from_wire(
    text: String,
    attachments: Vec<Attachment>,
) -> Result<UserPromptParts, HandlerError> {
    prompt_attachments::build_user_prompt(text, &attachments).map_err(|error| match error {
        PromptAttachmentError::Empty => {
            HandlerError::InvalidRequest("prompt must include text or at least one image".into())
        },
        PromptAttachmentError::UnsupportedAttachment {
            filename,
            media_type,
        } => HandlerError::InvalidRequest(format!(
            "unsupported attachment `{filename}` ({media_type})"
        )),
        PromptAttachmentError::Image(image_error) => {
            HandlerError::InvalidRequest(format!("invalid image attachment: {image_error}"))
        },
    })
}

pub(crate) fn user_prompt_from_http(
    text: String,
    attachments: Vec<astrcode_protocol::http::PromptAttachmentDto>,
) -> Result<UserPromptParts, HandlerError> {
    let wire: Vec<Attachment> = attachments.into_iter().map(Into::into).collect();
    user_prompt_from_wire(text, wire)
}
