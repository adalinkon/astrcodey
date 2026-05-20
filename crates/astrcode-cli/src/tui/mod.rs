//! TUI — interactive terminal mode.
//!
//! Architecture:
//! - Component trait (codex Renderable signature: render(Rect, &mut Buffer) + desired_height)
//! - Container + OverlayStack (pi-mono design)
//! - FrameRequester actor (codex design, 120 FPS cap)
//! - AdaptiveChunkingPolicy for streaming (codex design)
//! - ToolRenderer / MessageRenderer registries (pi-mono design)

// The Component/Container/OverlayStack infrastructure is intentionally built
// ahead of the main loop wiring. Suppress dead_code for these public APIs.
#![allow(dead_code, unused_imports)]

pub(crate) mod app;
pub(crate) mod command;
pub(crate) mod component;
pub(crate) mod custom_terminal;
pub(crate) mod ext;
pub(crate) mod frame;
pub(crate) mod insert_history;
pub(crate) mod render;
pub(crate) mod store;
pub(crate) mod streaming;
pub(crate) mod terminal;
pub(crate) mod terminal_probe;
pub(crate) mod theme;

use std::{io, sync::Arc};

use astrcode_client::client::AstrcodeClient;
use astrcode_protocol::commands::ClientCommand;
use crossterm::event::{KeyCode, KeyModifiers};
use tokio_stream::StreamExt;

use self::{
    app::App,
    command::slash::{self, SlashCommand},
    frame::{
        FrameRequester,
        event_stream::{EventBroker, EventStream, TerminalFocus, TuiEvent},
    },
    streaming::{chunking::AdaptiveChunkingPolicy, commit_tick::run_commit_tick},
    terminal::TerminalSession,
    theme::Theme,
};
use crate::transport::InProcessTransport;

type Client = AstrcodeClient<InProcessTransport>;

/// TUI entry point — called from main.rs.
pub async fn run() -> io::Result<()> {
    let client = Arc::new(AstrcodeClient::new(InProcessTransport::start()));
    let mut server_stream = client.subscribe_events().await.map_err(io_error)?;

    let mut terminal = TerminalSession::enter()?;
    let theme = Theme::detect();
    let mut app = App::new();

    // Frame scheduling — draw_tx drives the event_stream's draw channel
    let (draw_tx, draw_rx) = tokio::sync::broadcast::channel::<()>(16);
    let _frame_requester = FrameRequester::new(draw_tx.clone());

    // Input event stream
    let broker = EventBroker::new();
    let focus = TerminalFocus::new();
    let mut event_stream = EventStream::new(broker, draw_rx, focus);

    // Streaming chunking policy
    let mut chunking_policy = AdaptiveChunkingPolicy::new();

    // Initial draw
    draw_frame(&mut terminal, &app, &theme)?;

    // Query extension commands
    client
        .send_command(&ClientCommand::ListExtensionCommands)
        .await
        .map_err(io_error)?;

    let mut exit_reason = None::<String>;

    loop {
        let dirty;
        tokio::select! {
            // Input events (keyboard, paste, resize/draw)
            event = event_stream.next() => {
                let Some(event) = event else {
                    exit_reason = Some("event stream ended".into());
                    break;
                };
                match event {
                    TuiEvent::Key(key) => {
                        handle_key(key, &mut app, &client, &mut terminal).await?;
                    },
                    TuiEvent::Paste(text) => {
                        let text = normalize_paste(&text);
                        app.composer.insert_paste(&text);
                    },
                    TuiEvent::Draw => {},
                }
                dirty = true;
            },
            // Server notifications
            notification = server_stream.recv() => {
                app.apply(&notification.map_err(io_error)?);
                for pending in server_stream.drain_pending() {
                    app.apply(&pending);
                }
                // Commit streaming lines
                let now = std::time::Instant::now();
                for ctrl in app.stream_states.values_mut() {
                    let output = run_commit_tick(&mut chunking_policy, Some(ctrl), now);
                    for line in output.lines {
                        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                        app.scrollback_queue.push(store::transcript::ScrollbackEntry::StreamText {
                            role: store::transcript::MessageRole::Assistant,
                            text,
                        });
                    }
                }
                dirty = true;
            },
        }

        if app.should_quit {
            break;
        }
        if dirty {
            // Flush scrollback entries to terminal native scrollback
            let entries = std::mem::take(&mut app.scrollback_queue);
            terminal.flush_scrollback(entries, &theme)?;
            draw_frame(&mut terminal, &app, &theme)?;
        }
    }

    drop(terminal);

    if let Some(reason) = exit_reason {
        eprintln!("[TUI] exited abnormally: {reason}");
    }

    Ok(())
}

// ─── Key handling ─────────────────────────────────────────────────────────────

async fn handle_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    client: &Arc<Client>,
    terminal: &mut TerminalSession,
) -> io::Result<()> {
    match key.code {
        KeyCode::Esc => {
            if app.show_slash_palette {
                app.close_slash();
            } else if app.is_streaming {
                client
                    .send_command(&ClientCommand::Abort)
                    .await
                    .map_err(io_error)?;
                app.status_text = "Stopping turn".into();
            }
        },
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::ALT)
            {
                app.composer.insert_char('\n');
            } else if app.show_slash_palette {
                accept_slash_selection(app, client).await?;
            } else {
                submit_current_input(app, client).await?;
            }
        },
        KeyCode::Tab if app.show_slash_palette => {
            complete_slash_selection(app);
        },
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
            app.composer.delete_previous_word();
        },
        KeyCode::Backspace => {
            app.composer.backspace();
            app.sync_slash_filter_pub();
        },
        KeyCode::Delete => {
            app.composer.delete();
        },
        KeyCode::Left => {
            app.composer.move_left();
        },
        KeyCode::Right => {
            app.composer.move_right();
        },
        KeyCode::Home => {
            app.composer.move_home();
        },
        KeyCode::End => {
            app.composer.move_end();
        },
        KeyCode::Up => {
            if app.show_slash_palette {
                app.slash_move_up();
            } else if !app.composer.move_visual_up(terminal.composer_width()) {
                app.history_previous();
            }
        },
        KeyCode::Down => {
            if app.show_slash_palette {
                app.slash_move_down();
            } else if !app.composer.move_visual_down(terminal.composer_width()) {
                app.history_next();
            }
        },
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match ch.to_ascii_lowercase() {
                'a' => {
                    app.composer.move_home();
                },
                'e' => {
                    app.composer.move_end();
                },
                'u' => {
                    app.composer.delete_before_cursor();
                },
                'k' => {
                    app.composer.delete_after_cursor();
                },
                'w' => {
                    app.composer.delete_previous_word();
                },
                'c' => {
                    app.should_quit = true;
                },
                _ => {},
            }
        },
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::ALT) {
                return Ok(());
            }
            app.composer.insert_char(ch);
            app.sync_slash_filter_pub();
        },
        _ => {},
    }
    Ok(())
}

async fn accept_slash_selection(app: &mut App, client: &Arc<Client>) -> io::Result<()> {
    let commands = slash::filtered(&app.slash_filter, &app.extension_commands);
    let Some(spec) = commands
        .get(app.slash_selected.min(commands.len().saturating_sub(1)))
        .cloned()
    else {
        app.close_slash();
        return Ok(());
    };

    let cmd_name = spec.usage.split_whitespace().next().unwrap_or(&spec.usage);
    let argument = app
        .input_text()
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");

    if spec.needs_argument && argument.is_empty() {
        app.set_input(format!("{cmd_name} "));
        return Ok(());
    }

    let full_input = if argument.is_empty() {
        cmd_name.to_string()
    } else {
        format!("{cmd_name} {argument}")
    };
    app.set_input(full_input);
    submit_current_input(app, client).await
}

fn complete_slash_selection(app: &mut App) {
    let commands = slash::filtered(&app.slash_filter, &app.extension_commands);
    let Some(spec) = commands
        .get(app.slash_selected.min(commands.len().saturating_sub(1)))
        .cloned()
    else {
        return;
    };
    app.set_input(slash::command_line_for(&spec));
}

async fn submit_current_input(app: &mut App, client: &Arc<Client>) -> io::Result<()> {
    let input = app.input_text().trim_end().to_string();
    if input.trim().is_empty() {
        return Ok(());
    }
    let is_slash_input = input.trim_start().starts_with('/');

    if let Some(command) = slash::parse(
        &input,
        &app.extension_commands
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
    ) {
        let input = app.take_input();
        app.remember_input(&input);
        execute_slash_command(command, app, client).await?;
        return Ok(());
    }

    if app.is_streaming {
        app.status_text = "Turn running · Esc stop".into();
        return Ok(());
    }

    let input = app.take_input();
    app.remember_input(&input);
    if !is_slash_input {
        app.push_user(&input);
    }

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
    command: SlashCommand,
    app: &mut App,
    client: &Arc<Client>,
) -> io::Result<()> {
    match command {
        SlashCommand::New => {
            let working_dir = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".into());
            client
                .send_command(&ClientCommand::CreateSession { working_dir })
                .await
                .map_err(io_error)?;
            app.status_text = "Creating session".into();
        },
        SlashCommand::Resume(session_id) => {
            if session_id.trim().is_empty() {
                app.push_message(
                    store::transcript::MessageRole::System,
                    "Usage".into(),
                    "/resume <session-id>".into(),
                    false,
                    None,
                );
            } else {
                let sid = app.resolve_session_id(&session_id);
                client
                    .send_command(&ClientCommand::ResumeSession { session_id: sid })
                    .await
                    .map_err(io_error)?;
                app.status_text = "Resuming session".into();
            }
        },
        SlashCommand::Sessions => {
            client
                .send_command(&ClientCommand::ListSessions)
                .await
                .map_err(io_error)?;
            app.status_text = "Listing sessions".into();
        },
        SlashCommand::Compact => {
            client
                .send_command(&ClientCommand::Compact)
                .await
                .map_err(io_error)?;
            app.status_text = "Compacting session".into();
        },
        SlashCommand::Quit => {
            app.should_quit = true;
        },
        SlashCommand::Help => {
            let mut lines = vec![
                "/new                 create a fresh session".into(),
                "/sessions            list known sessions".into(),
                "/resume <id>         resume a session".into(),
                "/help                show this help".into(),
                "/quit                exit astrcode".into(),
            ];
            for cmd in &app.extension_commands {
                let padding = if cmd.needs_argument { " <args>" } else { "" };
                lines.push(format!("/{}{}", cmd.name, padding));
            }
            app.push_message(
                store::transcript::MessageRole::System,
                "Help".into(),
                lines.join("\n"),
                false,
                None,
            );
        },
        SlashCommand::Extension { name, arguments } => {
            client
                .send_command(&ClientCommand::ExecuteExtensionCommand {
                    command_name: name,
                    arguments,
                })
                .await
                .map_err(io_error)?;
            app.status_text = "Executing command".into();
        },
    }
    Ok(())
}

// ─── Rendering ────────────────────────────────────────────────────────────────

fn draw_frame(terminal: &mut TerminalSession, app: &App, theme: &Theme) -> io::Result<()> {
    let params = RenderParams {
        show_slash: app.show_slash_palette,
        input_text: app.composer.text().to_string(),
        input_cursor: app.composer.cursor(),
        model_name: app.model_name.clone(),
        working_dir: app.working_dir.clone(),
        active_session_id: app.active_session_id.clone(),
        is_streaming: app.is_streaming,
        slash_filter: app.slash_filter.clone(),
        slash_selected: app.slash_selected,
        extension_commands: app.extension_commands.clone(),
        theme: theme.clone(),
    };
    terminal.draw_frame(move |frame| render_bottom_panel(frame, &params))
}

struct RenderParams {
    show_slash: bool,
    input_text: String,
    input_cursor: usize,
    model_name: String,
    working_dir: String,
    active_session_id: Option<String>,
    is_streaming: bool,
    slash_filter: String,
    slash_selected: usize,
    extension_commands: Vec<command::slash::SlashCommandSpec>,
    theme: Theme,
}

fn render_bottom_panel(frame: &mut crate::tui::custom_terminal::Frame<'_>, p: &RenderParams) {
    use ratatui::{
        layout::{Constraint, Direction, Layout, Margin, Rect},
        text::{Line, Span, Text},
        widgets::{Block, Borders, Clear, Paragraph},
    };
    use render::layout_visual_text;

    let area = frame.area();
    let footer_height = 1u16;
    let buffer_height = area.height.saturating_sub(footer_height + 1).min(1);
    let composer_height = area.height.saturating_sub(footer_height + buffer_height);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(buffer_height),
            Constraint::Length(composer_height),
            Constraint::Length(footer_height),
        ])
        .split(area);

    // Composer
    let content_width = layout[1].width.max(1);
    let vl = layout_visual_text(
        &p.input_text,
        content_width.saturating_sub(2) as usize,
        Some(p.input_cursor),
    );
    let cursor = (
        2 + vl.cursor_column.unwrap_or(0) as u16,
        vl.cursor_row.unwrap_or(0) as u16,
    );
    let styled_lines: Vec<Line> = if p.input_text.is_empty() {
        vec![Line::from(vec![
            Span::styled("> ", p.theme.assistant_label),
            Span::styled(
                "Ask astrcode to inspect, edit, or explain...",
                p.theme.composer_placeholder,
            ),
        ])]
    } else {
        vl.lines
            .into_iter()
            .enumerate()
            .map(|(idx, line)| {
                let prefix = if idx == 0 { "> " } else { "  " };
                Line::from(vec![
                    Span::styled(prefix, p.theme.assistant_label),
                    Span::styled(line, p.theme.composer),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(Text::from(styled_lines)), layout[1]);
    let cx = layout[1].x + cursor.0.min(layout[1].width.saturating_sub(1));
    let cy = layout[1].y + cursor.1.min(layout[1].height.saturating_sub(1));
    frame.set_cursor_position((cx, cy));

    // Footer
    let session = p
        .active_session_id
        .as_deref()
        .map(|id| id.get(..8).unwrap_or(id))
        .unwrap_or("none");
    let model = if p.model_name.is_empty() {
        "model: pending".to_string()
    } else {
        p.model_name.clone()
    };
    let cwd = if p.working_dir.is_empty() {
        "cwd pending".into()
    } else {
        compact_path(&p.working_dir)
    };
    let hints = if p.is_streaming {
        "Esc stop"
    } else {
        "Enter send · Shift+Enter newline · /help"
    };
    let footer_text = format!("  {model} · {cwd} · session {session}   {hints}");
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(footer_text, p.theme.footer))),
        layout[2],
    );

    // Slash palette overlay
    if p.show_slash {
        let commands = command::slash::filtered(&p.slash_filter, &p.extension_commands);
        if !commands.is_empty() {
            let max_height = area.height.saturating_sub(1).max(1);
            let visible = commands
                .len()
                .min(max_height.saturating_sub(2).max(1) as usize);
            let selected = p.slash_selected.min(commands.len().saturating_sub(1));
            let start = selected.saturating_add(1).saturating_sub(visible);
            let height = (visible as u16 + 2).min(max_height);
            let popup_width = ((area.width as u32 * 70 / 100) as u16)
                .max(24)
                .min(area.width);
            let popup_height = height.min(area.height);
            let bottom_gap = 3u16.min(area.height.saturating_sub(popup_height));
            let popup = Rect {
                x: area.x + (area.width.saturating_sub(popup_width)) / 2,
                y: area.y + area.height.saturating_sub(popup_height + bottom_gap),
                width: popup_width,
                height: popup_height,
            };
            let inner = popup.inner(Margin {
                vertical: 1,
                horizontal: 1,
            });
            let lines: Vec<Line> = commands
                .iter()
                .skip(start)
                .take(visible)
                .enumerate()
                .map(|(idx, cmd)| {
                    let is_sel = start + idx == selected;
                    let label_style = if is_sel {
                        p.theme.popup_selected
                    } else {
                        p.theme.assistant_label
                    };
                    let desc_style = if is_sel { p.theme.body } else { p.theme.dim };
                    Line::from(vec![
                        Span::styled(format!("{:<16}", cmd.usage), label_style),
                        Span::styled(cmd.description.clone(), desc_style),
                    ])
                })
                .collect();
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(p.theme.popup_border)
                    .title(" Slash Commands "),
                popup,
            );
            frame.render_widget(Paragraph::new(Text::from(lines)), inner);
        }
    }
}

fn compact_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<_> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 3 {
        return normalized;
    }
    let root = if normalized.contains(":/") {
        parts.first().copied().unwrap_or_default()
    } else if normalized.starts_with('/') {
        ""
    } else {
        parts.first().copied().unwrap_or_default()
    };
    let tail = &parts[parts.len().saturating_sub(2)..];
    if root.is_empty() {
        format!("/.../{}", tail.join("/"))
    } else {
        format!("{root}/.../{}", tail.join("/"))
    }
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
