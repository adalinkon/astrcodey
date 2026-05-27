//! 用户 prompt 输入的统一内部表示（文本 + 图片）。
//!
//! 在 server / session / storage 边界之间传递，避免 `text` 与 `attachments`
//! 在多层函数签名中散落。

use serde::{Deserialize, Serialize};

use crate::{
    event::EventPayload,
    llm::{LlmContent, LlmMessage},
    types::MessageId,
};

/// 持久化与 LLM 可见的用户图片附件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserImagePart {
    pub filename: String,
    pub media_type: String,
    pub base64: String,
}

/// 一次用户提交的完整输入（文本 + 可选图片）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserPromptParts {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<UserImagePart>,
}

impl From<String> for UserPromptParts {
    fn from(text: String) -> Self {
        Self::text_only(text)
    }
}

impl From<&str> for UserPromptParts {
    fn from(text: &str) -> Self {
        Self::text_only(text)
    }
}

impl UserPromptParts {
    pub fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn is_submittable(&self) -> bool {
        !self.text.trim().is_empty() || !self.images.is_empty()
    }

    pub fn to_llm_message(&self) -> LlmMessage {
        let mut content = Vec::new();
        if !self.text.trim().is_empty() {
            content.push(LlmContent::Text {
                text: self.text.clone(),
            });
        }
        for image in &self.images {
            content.push(LlmContent::Image {
                base64: image.base64.clone(),
                media_type: image.media_type.clone(),
            });
        }
        if content.is_empty() {
            return LlmMessage::user("");
        }
        LlmMessage {
            role: crate::llm::LlmRole::User,
            content,
            name: None,
            reasoning_content: None,
        }
    }

    /// 用于侧边栏标题、TUI 展示、生命周期 hook 的有损文本。
    pub fn display_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.text.trim().is_empty() {
            parts.push(self.text.trim().to_string());
        }
        for (index, image) in self.images.iter().enumerate() {
            parts.push(image_label(index + 1, &image.filename));
        }
        parts.join("\n")
    }

    pub fn user_message_event(&self, message_id: MessageId) -> EventPayload {
        EventPayload::UserMessage {
            message_id,
            text: self.text.clone(),
            images: self.images.clone(),
        }
    }
}

pub fn image_label(index: usize, filename: &str) -> String {
    if filename.trim().is_empty() {
        format!("[Image {index}]")
    } else {
        format!("[Image {index}: {filename}]")
    }
}
