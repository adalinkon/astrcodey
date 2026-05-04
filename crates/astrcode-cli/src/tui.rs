//! 交互式终端模式（原生 scrollback + 底部固定面板）。
//!
//! TUI 运行在主屏幕上，底部只保留很小的交互面板。
//! 消息记录通过 scroll region 写入终端原生 scrollback，
//! 用户可用终端原生滚轮/键盘翻页查看历史消息。

mod composer;
mod input;
mod render;
mod slash;
mod state;
mod theme;
mod tool_display;

use std::{
    io::{self, Stdout},
    sync::Arc,
    time::Duration,
};

use astrcode_client::{client::AstrcodeClient, stream::StreamItem};
use astrcode_protocol::commands::ClientCommand;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use input::Action;
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::{Backend, ClearType, CrosstermBackend},
    buffer::Buffer,
    layout::{Position, Rect, Size},
    prelude::Widget,
    text::Text,
    widgets::Paragraph,
};
use render::scrollback_entry_to_lines;
use state::TuiState;
use tokio::sync::mpsc;

use crate::transport::InProcessTransport;

type Client = AstrcodeClient<InProcessTransport>;

const INLINE_VIEWPORT_HEIGHT: u16 = 5;

/// TUI 主入口：初始化终端、启动事件循环。
pub async fn run() -> io::Result<()> {
    let client = Arc::new(AstrcodeClient::new(InProcessTransport::start()));
    let mut stream = client.subscribe_events().await.map_err(io_error)?;
    let mut terminal = TerminalSession::enter()?;
    let theme = theme::Theme::detect();
    let mut state = TuiState::new();
    state.status = "Ready · type / for commands".into();

    let (action_tx, mut action_rx) = mpsc::unbounded_channel();
    spawn_keyboard_reader(action_tx.clone());

    // 首帧绘制
    terminal.draw_frame(&mut state, &theme)?;
    state.dirty = false;

    loop {
        tokio::select! {
            action = action_rx.recv() => {
                let Some(action) = action else { break };
                handle_action(action, &mut state, &client, &mut terminal).await?;
            },
            item = stream.recv() => {
                match item.map_err(io_error)? {
                    StreamItem::Event(notification) => {
                        state.apply(&notification);
                    },
                    StreamItem::Lagged(n) => {
                        state.status = format!("Skipped {n} event(s) · rehydrating");
                        state.mark_dirty();
                        client
                            .send_command(&ClientCommand::GetState)
                            .await
                            .map_err(io_error)?;
                    },
                }
            },
        }

        if state.should_quit {
            break;
        }
        if state.dirty {
            terminal.draw_frame(&mut state, &theme)?;
            state.dirty = false;
        }
    }

    Ok(())
}

// ─── Action 处理 ──────────────────────────────────────────────────────

async fn handle_action(
    action: Action,
    state: &mut TuiState,
    client: &Arc<Client>,
    terminal: &mut TerminalSession,
) -> io::Result<()> {
    match action {
        Action::Quit => state.should_quit = true,
        Action::Resize => terminal.sync_resize()?,
        Action::Key(event) => handle_key(event, state, client, terminal).await?,
        Action::Paste(text) => {
            let text = normalize_paste(&text);
            state.insert_paste(&text);
        },
    }
    state.mark_dirty();
    Ok(())
}

async fn handle_key(
    event: KeyEvent,
    state: &mut TuiState,
    client: &Arc<Client>,
    terminal: &mut TerminalSession,
) -> io::Result<()> {
    match event.code {
        KeyCode::Esc => {
            if state.show_slash_palette {
                state.close_slash();
            } else if state.is_streaming {
                client
                    .send_command(&ClientCommand::Abort)
                    .await
                    .map_err(io_error)?;
                state.status = "Stopping turn".into();
            }
        },
        KeyCode::Enter => {
            if event.modifiers.contains(KeyModifiers::SHIFT)
                || event.modifiers.contains(KeyModifiers::ALT)
            {
                state.insert_newline();
            } else if state.show_slash_palette {
                accept_slash_selection(state, client).await?;
            } else {
                submit_current_input(state, client, Some(terminal)).await?;
            }
        },
        KeyCode::Tab if state.show_slash_palette => {
            accept_slash_selection(state, client).await?;
        },
        KeyCode::Backspace if event.modifiers.contains(KeyModifiers::ALT) => {
            state.delete_previous_word();
        },
        KeyCode::Backspace => state.backspace(),
        KeyCode::Delete => state.delete(),
        KeyCode::Left => state.move_left(),
        KeyCode::Right => state.move_right(),
        KeyCode::Home => state.move_home(),
        KeyCode::End => state.move_end(),
        KeyCode::Up => {
            if state.show_slash_palette {
                state.slash_move_up(slash::filtered(&state.slash_filter).len());
            } else if !state.move_visual_up(terminal.composer_width()) {
                state.history_previous();
            }
        },
        KeyCode::Down => {
            if state.show_slash_palette {
                state.slash_move_down(slash::filtered(&state.slash_filter).len());
            } else if !state.move_visual_down(terminal.composer_width()) {
                state.history_next();
            }
        },
        KeyCode::Char(ch) if event.modifiers.contains(KeyModifiers::CONTROL) => {
            match ch.to_ascii_lowercase() {
                'a' => state.move_home(),
                'e' => state.move_end(),
                'u' => state.delete_before_cursor(),
                'k' => state.delete_after_cursor(),
                'w' => state.delete_previous_word(),
                _ => {},
            }
        },
        KeyCode::Char(ch) => {
            if event.modifiers.contains(KeyModifiers::ALT) {
                return Ok(());
            }
            state.insert_char(ch);
        },
        _ => {},
    }
    Ok(())
}

async fn accept_slash_selection(state: &mut TuiState, client: &Arc<Client>) -> io::Result<()> {
    let commands = slash::filtered(&state.slash_filter);
    let Some(spec) = commands
        .get(state.slash_selected.min(commands.len().saturating_sub(1)))
        .copied()
    else {
        state.close_slash();
        return Ok(());
    };
    let current_has_argument = state
        .input_text()
        .split_once(char::is_whitespace)
        .is_some_and(|(_, rest)| !rest.trim().is_empty());
    if spec.needs_argument && !current_has_argument {
        state.set_input(slash::command_line_for(spec));
        return Ok(());
    }
    submit_current_input(state, client, None).await
}

async fn submit_current_input(
    state: &mut TuiState,
    client: &Arc<Client>,
    _terminal: Option<&mut TerminalSession>,
) -> io::Result<()> {
    let input = state.input_text().trim_end().to_string();
    if input.trim().is_empty() {
        return Ok(());
    }

    if let Some(command) = slash::parse(&input) {
        let input = state.take_input();
        state.remember_input(&input);
        execute_slash_command(command, state, client).await?;
        return Ok(());
    }

    if state.is_streaming {
        state.status = "Turn running · Esc stop".into();
        return Ok(());
    }

    let input = state.take_input();
    state.remember_input(&input);
    state.push_user(&input);
    state.mark_dirty();

    client
        .send_command(&ClientCommand::SubmitPrompt {
            text: input,
            attachments: vec![],
        })
        .await
        .map_err(io_error)?;

    Ok(())
}

async fn execute_slash_command(
    command: slash::SlashCommand,
    state: &mut TuiState,
    client: &Arc<Client>,
) -> io::Result<()> {
    match command {
        slash::SlashCommand::New => {
            let working_dir = std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".into());
            client
                .send_command(&ClientCommand::CreateSession { working_dir })
                .await
                .map_err(io_error)?;
            state.status = "Creating session".into();
        },
        slash::SlashCommand::Resume(session_id) => {
            if session_id.trim().is_empty() {
                state.push_message(
                    state::MessageRole::System,
                    "Usage".into(),
                    "/resume <session-id>".into(),
                    false,
                    None,
                );
            } else {
                let session_id = resolve_session_id(state, &session_id);
                client
                    .send_command(&ClientCommand::ResumeSession { session_id })
                    .await
                    .map_err(io_error)?;
                state.status = "Resuming session".into();
            }
        },
        slash::SlashCommand::Sessions => {
            client
                .send_command(&ClientCommand::ListSessions)
                .await
                .map_err(io_error)?;
            state.status = "Listing sessions".into();
        },
        slash::SlashCommand::Quit => {
            state.should_quit = true;
        },
        slash::SlashCommand::Help => {
            state.push_message(
                state::MessageRole::System,
                "Help".into(),
                slash_help_text(),
                false,
                None,
            );
        },
    }
    state.mark_dirty();
    Ok(())
}

// ─── 键盘读取线程 ─────────────────────────────────────────────────────

fn spawn_keyboard_reader(action_tx: mpsc::UnboundedSender<Action>) {
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event::Event::Key(key)) => {
                        if let Some(action) = input::map_key(key) {
                            if action_tx.send(action).is_err() {
                                break;
                            }
                        }
                    },
                    Ok(event::Event::Paste(text)) => {
                        if action_tx.send(Action::Paste(text)).is_err() {
                            break;
                        }
                    },
                    Ok(event::Event::Resize(_, _)) => {
                        if action_tx.send(Action::Resize).is_err() {
                            break;
                        }
                    },
                    // 不处理鼠标事件 — 原生选择/滚轮由终端管理
                    Ok(_) => {},
                    Err(_) => {
                        let _ = action_tx.send(Action::Quit);
                        break;
                    },
                },
                Ok(false) => {},
                Err(_) => {
                    let _ = action_tx.send(Action::Quit);
                    break;
                },
            }
        }
    });
}

// ─── 终端会话 ─────────────────────────────────────────────────────────

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnableBracketedPaste)?;
        // 不进入 alternate screen；滚轮/翻页继续走终端原生 scrollback。
        let (columns, rows) = crossterm::terminal::size()?;
        let options = TerminalOptions {
            viewport: Viewport::Fixed(viewport_rect(Size::new(columns, rows))),
        };
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::with_options(backend, options)?;
        Ok(Self { terminal })
    }

    /// 将待提交历史写入原生 scrollback，并绘制底部面板。
    fn draw_frame(&mut self, state: &mut TuiState, theme: &theme::Theme) -> io::Result<()> {
        sync_viewport_resize_if_needed(&mut self.terminal)?;
        flush_scrollback(state, self, theme)?;
        self.terminal
            .draw(|frame| render::render(state, frame, theme))
            .map(|_| ())
    }

    fn sync_resize(&mut self) -> io::Result<()> {
        sync_viewport_resize(&mut self.terminal)
    }

    fn composer_width(&self) -> usize {
        self.terminal
            .size()
            .map(|area| area.width.saturating_sub(2).max(1) as usize)
            .unwrap_or(80)
    }

    /// 将条目插入终端 scrollback（在 viewport 上方）。
    fn insert_scrollback_entry(
        &mut self,
        entry: &state::ScrollbackEntry,
        theme: &theme::Theme,
    ) -> io::Result<()> {
        insert_scrollback_entry(&mut self.terminal, entry, theme)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = disable_raw_mode();
    }
}

fn viewport_rect(screen: Size) -> Rect {
    let height = INLINE_VIEWPORT_HEIGHT.min(screen.height);
    Rect {
        x: 0,
        y: screen.height.saturating_sub(height),
        width: screen.width,
        height,
    }
}

fn sync_viewport_resize<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()> {
    let old_area = terminal.get_frame().area();
    let new_area = viewport_rect(terminal.size()?);
    clear_terminal_rows_from(terminal.backend_mut(), old_area.top().min(new_area.top()))?;
    terminal.resize(new_area)
}

fn sync_viewport_resize_if_needed<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()> {
    let area = terminal.get_frame().area();
    let expected = viewport_rect(terminal.size()?);
    if area != expected {
        sync_viewport_resize(terminal)?;
    }
    Ok(())
}

fn clear_terminal_rows_from<B: Backend>(backend: &mut B, top: u16) -> io::Result<()> {
    let cursor = backend.get_cursor_position()?;
    let screen = backend.size()?;
    for y in top.min(screen.height)..screen.height {
        backend.set_cursor_position(Position { x: 0, y })?;
        backend.clear_region(ClearType::CurrentLine)?;
    }
    backend.set_cursor_position(cursor)?;
    backend.flush()
}

fn insert_scrollback_entry<B: Backend>(
    terminal: &mut Terminal<B>,
    entry: &state::ScrollbackEntry,
    theme: &theme::Theme,
) -> io::Result<()> {
    sync_viewport_resize_if_needed(terminal)?;
    let width = terminal.size()?.width;
    let lines = scrollback_entry_to_lines(entry, width, theme);
    insert_history_lines_above_viewport(terminal, lines)
}

fn insert_history_lines_above_viewport<B: Backend>(
    terminal: &mut Terminal<B>,
    lines: Vec<ratatui::text::Line<'static>>,
) -> io::Result<()> {
    let viewport = terminal.get_frame().area();
    let viewport_top = viewport.top();
    if viewport_top == 0 || lines.is_empty() {
        return Ok(());
    }

    let saved_cursor = terminal.backend_mut().get_cursor_position()?;
    let width = viewport.width.max(1);
    let mut start = 0usize;
    while start < lines.len() {
        let remaining = lines.len() - start;
        let chunk_height = remaining.min(viewport_top as usize) as u16;
        let end = start + chunk_height as usize;
        let chunk = lines[start..end].to_vec();

        terminal
            .backend_mut()
            .scroll_region_up(0..viewport_top, chunk_height)?;
        draw_history_chunk(
            terminal.backend_mut(),
            chunk,
            width,
            viewport_top.saturating_sub(chunk_height),
        )?;
        start = end;
    }
    terminal.backend_mut().set_cursor_position(saved_cursor)?;
    terminal.backend_mut().flush()
}

fn draw_history_chunk<B: Backend>(
    backend: &mut B,
    lines: Vec<ratatui::text::Line<'static>>,
    width: u16,
    y: u16,
) -> io::Result<()> {
    let height = lines.len() as u16;
    if height == 0 {
        return Ok(());
    }
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut buffer = Buffer::empty(area);
    let paragraph = Paragraph::new(Text::from(lines));
    Widget::render(paragraph, area, &mut buffer);
    let width = width as usize;
    let content = buffer.content;
    backend.draw(content.iter().enumerate().map(|(index, cell)| {
        let x = (index % width) as u16;
        let dy = (index / width) as u16;
        (x, y + dy, cell)
    }))
}

/// 将 scrollback_queue 中的消息全部写入终端原生 scrollback。
fn flush_scrollback(
    state: &mut TuiState,
    terminal: &mut TerminalSession,
    theme: &theme::Theme,
) -> io::Result<()> {
    let entries: Vec<_> = state.scrollback_queue.drain(..).collect();
    for entry in entries {
        terminal.insert_scrollback_entry(&entry, theme)?;
    }
    Ok(())
}

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn slash_help_text() -> String {
    [
        "/new                 create a fresh session",
        "/sessions            list known sessions",
        "/resume <id>         resume a session",
        "/help                show this help",
        "/quit                exit astrcode",
    ]
    .join("\n")
}

fn resolve_session_id(state: &TuiState, input: &str) -> String {
    let needle = input.trim();
    state
        .available_sessions
        .iter()
        .find(|session_id| session_id.starts_with(needle))
        .cloned()
        .unwrap_or_else(|| needle.to_string())
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use state::MessageRole;

    use super::*;

    #[test]
    fn scrollback_insert_resizes_fixed_viewport_before_writing() {
        let theme = theme::Theme::detect();
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(viewport_rect(Size::new(20, 6))),
            },
        )
        .unwrap();
        let mut state = TuiState::new();
        state.push_message(
            MessageRole::Assistant,
            "Astrcode".into(),
            "alpha beta gamma delta".into(),
            false,
            None,
        );
        let entry = state.scrollback_queue.pop().unwrap();

        terminal.backend_mut().resize(8, 6);
        insert_scrollback_entry(&mut terminal, &entry, &theme).unwrap();

        {
            let frame = terminal.get_frame();
            assert_eq!(frame.area().width, 8);
            assert_eq!(frame.area().top(), 1);
        }

        let state = TuiState::new();
        terminal
            .draw(|frame| render::render(&state, frame, &theme))
            .unwrap();
    }

    #[test]
    fn resize_clears_stale_fixed_viewport_rows() {
        let theme = theme::Theme::detect();
        let mut backend = TestBackend::new(40, 8);
        backend
            .set_cursor_position(Position { x: 0, y: 7 })
            .unwrap();
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(viewport_rect(Size::new(40, 8))),
            },
        )
        .unwrap();
        let mut state = TuiState::new();
        state.is_streaming = true;
        state.task_activity = Some(state::TaskActivity {
            title: "Agent running".into(),
            detail: None,
        });
        state.status = "Agent running".into();

        terminal
            .draw(|frame| render::render(&state, frame, &theme))
            .unwrap();
        assert!(terminal.backend().to_string().contains("Agent running"));

        terminal.backend_mut().resize(40, 12);
        sync_viewport_resize(&mut terminal).unwrap();

        assert!(!terminal.backend().to_string().contains("Agent running"));
    }

    #[test]
    fn repeated_resize_keeps_single_idle_status() {
        let theme = theme::Theme::detect();
        let mut backend = TestBackend::new(48, 10);
        backend
            .set_cursor_position(Position { x: 0, y: 9 })
            .unwrap();
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(viewport_rect(Size::new(48, 10))),
            },
        )
        .unwrap();
        let mut state = TuiState::new();
        state.status = "Ready · type / for commands".into();

        terminal
            .draw(|frame| render::render(&state, frame, &theme))
            .unwrap();
        terminal.backend_mut().resize(48, 16);
        sync_viewport_resize(&mut terminal).unwrap();
        terminal
            .draw(|frame| render::render(&state, frame, &theme))
            .unwrap();
        terminal.backend_mut().resize(48, 12);
        sync_viewport_resize(&mut terminal).unwrap();

        terminal
            .draw(|frame| render::render(&state, frame, &theme))
            .unwrap();

        let screen = terminal.backend().to_string();
        assert_eq!(screen.matches("Ready · type / for commands").count(), 1);
        assert!(screen.contains("Ask astrcode to inspect"));
    }

    #[test]
    fn fixed_viewport_resize_does_not_append_lines() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(viewport_rect(Size::new(40, 8))),
            },
        )
        .unwrap();
        let initial_scrollback = terminal.backend().scrollback().content.len();

        terminal.backend_mut().resize(40, 12);
        sync_viewport_resize(&mut terminal).unwrap();

        assert_eq!(
            terminal.backend().scrollback().content.len(),
            initial_scrollback
        );
    }
}
