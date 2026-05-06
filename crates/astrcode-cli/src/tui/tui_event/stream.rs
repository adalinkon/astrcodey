//! TUI 事件流实现。
//!
//! 将 crossterm 事件映射为 TuiEvent，支持暂停/恢复。

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use crossterm::event::{Event, KeyEvent, KeyEventKind};
use tokio_stream::{
    Stream,
    wrappers::{BroadcastStream, WatchStream},
};

use super::{EventBroker, TerminalFocus};

/// TUI 事件类型。
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// 键盘按键事件（已过滤 Release 事件）
    Key(KeyEvent),
    /// Bracketed paste 文本
    Paste(String),
    /// 计划的重绘（包括 resize 触发的重绘）
    Draw,
}

/// TUI 事件流。
///
/// 从 crossterm EventStream 和绘制通道读取事件。
pub struct EventStream {
    /// 共享的事件代理
    broker: Option<EventBroker>,
    /// 绘制事件流（broadcast）
    draw_stream: BroadcastStream<()>,
    /// 恢复通知流（watch）
    resume_stream: WatchStream<()>,
    /// 终端焦点状态
    focus: TerminalFocus,
    /// 轮询优先级标志（round-robin）
    poll_draw_first: bool,
    /// 延迟定时器（用于避免忙等待）
    _pin: std::marker::PhantomPinned,
}

// Unpin 实现：EventStream 可以安全地移动
impl Unpin for EventStream {}

impl EventStream {
    /// 创建新的事件流。
    pub fn new(
        broker: EventBroker,
        draw_rx: tokio::sync::broadcast::Receiver<()>,
        focus: TerminalFocus,
    ) -> Self {
        let resume_stream = WatchStream::from_changes(broker.resume_rx());

        Self {
            broker: Some(broker),
            draw_stream: BroadcastStream::new(draw_rx),
            resume_stream,
            focus,
            poll_draw_first: false,
            _pin: std::marker::PhantomPinned,
        }
    }

    /// 轮询 crossterm 事件。
    ///
    /// 过滤掉的事件（key release、FocusLost 等）不会结束流，
    /// 而是继续轮询直到获得有效事件或 `Pending`。
    fn poll_crossterm_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
        loop {
            let broker = match &self.broker {
                Some(b) => b,
                None => return Poll::Ready(None),
            };

            match broker.poll_crossterm_event(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if let Some(tui_event) = self.map_crossterm_event(event) {
                        return Poll::Ready(Some(tui_event));
                    }
                    // 事件被过滤（key release / FocusLost 等），继续轮询
                },
                Poll::Ready(Some(Err(e))) => {
                    tracing::error!("crossterm event stream error, treating as end-of-stream: {e}");
                    return Poll::Ready(None);
                },
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    /// 轮询绘制事件。
    fn poll_draw_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
        match Pin::new(&mut self.draw_stream).poll_next(cx) {
            Poll::Ready(Some(Ok(()))) => Poll::Ready(Some(TuiEvent::Draw)),
            Poll::Ready(Some(Err(_))) => Poll::Ready(Some(TuiEvent::Draw)),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    /// 轮询恢复事件。
    fn poll_resume_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
        match Pin::new(&mut self.resume_stream).poll_next(cx) {
            Poll::Ready(Some(_)) => {
                // 恢复后触发重绘
                Poll::Ready(Some(TuiEvent::Draw))
            },
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    /// 映射 crossterm 事件到 TuiEvent。
    fn map_crossterm_event(&mut self, event: Event) -> Option<TuiEvent> {
        match event {
            Event::Key(key_event) => {
                // 过滤 Release 事件，只处理 Press 和 Repeat
                if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    return None;
                }
                Some(TuiEvent::Key(key_event))
            },
            // Resize 映射为 Draw，与 Codex 一致。
            // resize 的实际处理在 draw_frame 的 pending_viewport_area + update_inline_viewport 中。
            Event::Resize(_, _) => Some(TuiEvent::Draw),
            Event::Paste(text) => Some(TuiEvent::Paste(text)),
            Event::FocusGained => {
                self.focus.set_focused(true);
                Some(TuiEvent::Draw)
            },
            Event::FocusLost => {
                self.focus.set_focused(false);
                None
            },
            _ => None,
        }
    }
}

impl Stream for EventStream {
    type Item = TuiEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Round-robin 轮询以避免饥饿
        let draw_first = self.poll_draw_first;
        self.poll_draw_first = !self.poll_draw_first;

        // 首先检查恢复事件
        if let Poll::Ready(event) = self.as_mut().poll_resume_event(cx) {
            return Poll::Ready(event);
        }

        if draw_first {
            if let Poll::Ready(event) = self.as_mut().poll_draw_event(cx) {
                return Poll::Ready(event);
            }
            if let Poll::Ready(event) = self.as_mut().poll_crossterm_event(cx) {
                return Poll::Ready(event);
            }
        } else {
            if let Poll::Ready(event) = self.as_mut().poll_crossterm_event(cx) {
                return Poll::Ready(event);
            }
            if let Poll::Ready(event) = self.as_mut().poll_draw_event(cx) {
                return Poll::Ready(event);
            }
        }

        Poll::Pending
    }
}
