//! Pi-style terminal: history goes straight to stdout (terminal owns scrollback + reflow),
//! bottom panel is a fixed-height region redrawn each frame using ANSI escape sequences.
//!
//! Resize is handled entirely by the terminal emulator — we never touch scroll regions
//! (DECSTBM) or insert_history_lines. On resize we just redraw the bottom panel.

use std::io::{self, Stdout, Write};

use crossterm::{
    cursor::{Hide, MoveToColumn, MoveToRow, RestorePosition, SavePosition, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute, queue,
    style::{Print, SetAttribute, SetForegroundColor},
    terminal::{
        self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::tui::{
    render::scrollback_entry_to_lines, store::transcript::ScrollbackEntry, theme::Theme,
};

/// Fixed height of the bottom panel (composer + footer).
const BOTTOM_PANEL_HEIGHT: u16 = 4;

pub struct TerminalSession {
    stdout: Stdout,
    /// Current terminal size (columns, rows).
    size: (u16, u16),
}

impl TerminalSession {
    pub fn enter() -> io::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnableBracketedPaste)?;
        // Reserve space for the bottom panel by scrolling down.
        let size = terminal::size()?;
        // Move cursor to bottom of screen and print newlines to create
        // the reserved area for our bottom panel.
        execute!(stdout, MoveToRow(size.1.saturating_sub(1)))?;
        for _ in 0..BOTTOM_PANEL_HEIGHT {
            execute!(stdout, Print("\n"))?;
        }
        Ok(Self { stdout, size })
    }

    pub fn composer_width(&self) -> usize {
        self.size.0.saturating_sub(4).max(1) as usize
    }

    /// Write scrollback entries directly to stdout (terminal native scrollback).
    /// The terminal emulator owns these lines and handles resize reflow.
    pub fn flush_scrollback(
        &mut self,
        entries: Vec<ScrollbackEntry>,
        theme: &Theme,
    ) -> io::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let width = self.size.0;
        // Save cursor, move to the line above the bottom panel, write history.
        let history_row = self.size.1.saturating_sub(BOTTOM_PANEL_HEIGHT);
        queue!(self.stdout, SavePosition)?;
        queue!(self.stdout, MoveToRow(history_row), MoveToColumn(0))?;

        for entry in entries {
            let lines = scrollback_entry_to_lines(&entry, width, theme);
            for line in lines {
                self.write_styled_line(&line)?;
                queue!(self.stdout, Print("\n"))?;
            }
        }
        queue!(self.stdout, RestorePosition)?;
        self.stdout.flush()?;
        Ok(())
    }

    /// Draw the bottom panel (composer + footer). Clears and redraws completely.
    pub fn draw_bottom_panel(&mut self, lines: Vec<Line<'static>>) -> io::Result<()> {
        self.size = terminal::size()?;
        let panel_top = self.size.1.saturating_sub(BOTTOM_PANEL_HEIGHT);

        queue!(self.stdout, Hide)?;
        queue!(self.stdout, MoveToRow(panel_top), MoveToColumn(0))?;
        queue!(self.stdout, Clear(ClearType::FromCursorDown))?;

        for (i, line) in lines.iter().take(BOTTOM_PANEL_HEIGHT as usize).enumerate() {
            queue!(
                self.stdout,
                MoveToRow(panel_top + i as u16),
                MoveToColumn(0)
            )?;
            self.write_styled_line(line)?;
        }

        // Show cursor at composer position (first line of panel, after "> ")
        let cursor_row = panel_top;
        let cursor_col = 2u16; // After "> "
        queue!(self.stdout, Show)?;
        execute!(
            self.stdout,
            crossterm::cursor::MoveTo(cursor_col, cursor_row)
        )?;
        Ok(())
    }

    /// Draw bottom panel with explicit cursor position.
    pub fn draw_bottom_panel_with_cursor(
        &mut self,
        lines: Vec<Line<'static>>,
        cursor_col: u16,
        cursor_row_offset: u16,
    ) -> io::Result<()> {
        self.size = terminal::size()?;
        let panel_top = self.size.1.saturating_sub(BOTTOM_PANEL_HEIGHT);

        queue!(self.stdout, Hide)?;
        queue!(self.stdout, MoveToRow(panel_top), MoveToColumn(0))?;
        queue!(self.stdout, Clear(ClearType::FromCursorDown))?;

        for (i, line) in lines.iter().take(BOTTOM_PANEL_HEIGHT as usize).enumerate() {
            queue!(
                self.stdout,
                MoveToRow(panel_top + i as u16),
                MoveToColumn(0)
            )?;
            self.write_styled_line(line)?;
        }

        let cursor_y = panel_top + cursor_row_offset;
        queue!(self.stdout, Show)?;
        execute!(self.stdout, crossterm::cursor::MoveTo(cursor_col, cursor_y))?;
        Ok(())
    }

    fn write_styled_line(&mut self, line: &Line<'_>) -> io::Result<()> {
        for span in &line.spans {
            // Apply foreground color if set.
            if let Some(fg) = span.style.fg {
                let ct_color = ratatui_color_to_crossterm(fg);
                queue!(self.stdout, SetForegroundColor(ct_color))?;
            }
            // Apply bold if set.
            if span
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
            {
                queue!(self.stdout, SetAttribute(crossterm::style::Attribute::Bold))?;
            }
            queue!(self.stdout, Print(&*span.content))?;
            // Reset after each span.
            queue!(
                self.stdout,
                SetAttribute(crossterm::style::Attribute::Reset)
            )?;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.stdout, Show);
        let _ = execute!(self.stdout, DisableBracketedPaste);
        let _ = disable_raw_mode();
        // Print a newline so the shell prompt appears below our panel.
        let _ = execute!(self.stdout, Print("\n"));
    }
}

fn ratatui_color_to_crossterm(c: Color) -> crossterm::style::Color {
    match c {
        Color::Reset => crossterm::style::Color::Reset,
        Color::Black => crossterm::style::Color::Black,
        Color::Red => crossterm::style::Color::Red,
        Color::Green => crossterm::style::Color::Green,
        Color::Yellow => crossterm::style::Color::Yellow,
        Color::Blue => crossterm::style::Color::Blue,
        Color::Magenta => crossterm::style::Color::Magenta,
        Color::Cyan => crossterm::style::Color::Cyan,
        Color::Gray => crossterm::style::Color::Grey,
        Color::DarkGray => crossterm::style::Color::DarkGrey,
        Color::LightRed => crossterm::style::Color::DarkRed,
        Color::LightGreen => crossterm::style::Color::DarkGreen,
        Color::LightYellow => crossterm::style::Color::DarkYellow,
        Color::LightBlue => crossterm::style::Color::DarkBlue,
        Color::LightMagenta => crossterm::style::Color::DarkMagenta,
        Color::LightCyan => crossterm::style::Color::DarkCyan,
        Color::White => crossterm::style::Color::White,
        Color::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
        Color::Indexed(i) => crossterm::style::Color::AnsiValue(i),
    }
}
