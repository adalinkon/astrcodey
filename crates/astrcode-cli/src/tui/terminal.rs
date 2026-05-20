//! Pi-style terminal: history goes straight to stdout (terminal owns scrollback + reflow),
//! bottom panel is a fixed-height region redrawn each frame using ANSI escape sequences.
//!
//! Key insight from pi-mono: history is written by printing lines at the cursor position
//! above the bottom panel. The cursor naturally advances and the terminal scrolls. We
//! never use scroll regions (DECSTBM). On resize the terminal reflows history natively.
//!
//! The bottom panel is at the very bottom of the screen. Before writing history we scroll
//! the panel down (via \n at screen bottom), write history into the freed space, then
//! redraw the panel.

use std::io::{self, Stdout, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute, queue,
    style::{Print, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, ScrollUp, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    style::{Color, Modifier},
    text::Line,
};

use crate::tui::{
    render::scrollback_entry_to_lines, store::transcript::ScrollbackEntry, theme::Theme,
};

/// Fixed height of the bottom panel (composer + status + footer).
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
        let size = terminal::size()?;
        // Reserve space at the bottom for our panel by scrolling the screen up.
        // This creates empty space at the bottom for the panel without erasing history.
        execute!(stdout, ScrollUp(BOTTOM_PANEL_HEIGHT))?;
        execute!(
            stdout,
            MoveTo(0, size.1.saturating_sub(BOTTOM_PANEL_HEIGHT))
        )?;
        Ok(Self { stdout, size })
    }

    pub fn composer_width(&self) -> usize {
        self.size.0.saturating_sub(4).max(1) as usize
    }

    /// Write scrollback entries: scroll the bottom panel down to make room, then write
    /// history lines into the freed space. The terminal naturally owns these lines.
    pub fn flush_scrollback(
        &mut self,
        entries: Vec<ScrollbackEntry>,
        theme: &Theme,
    ) -> io::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.size = terminal::size()?;
        let width = self.size.0;
        let panel_top = self.size.1.saturating_sub(BOTTOM_PANEL_HEIGHT);

        // Collect all lines to write.
        let mut all_lines: Vec<Line<'static>> = Vec::new();
        for entry in entries {
            all_lines.extend(scrollback_entry_to_lines(&entry, width, theme));
        }
        if all_lines.is_empty() {
            return Ok(());
        }

        let line_count = all_lines.len() as u16;

        // Scroll the screen up by line_count rows to make room above the panel.
        execute!(self.stdout, ScrollUp(line_count))?;

        // Write history starting at (panel_top - line_count).
        let write_start = panel_top.saturating_sub(line_count);
        for (i, line) in all_lines.iter().enumerate() {
            queue!(self.stdout, MoveTo(0, write_start + i as u16))?;
            self.write_styled_line(line)?;
        }
        self.stdout.flush()?;
        Ok(())
    }

    /// Draw the bottom panel with explicit cursor position.
    pub fn draw_bottom_panel_with_cursor(
        &mut self,
        lines: Vec<Line<'static>>,
        cursor_col: u16,
        cursor_row_offset: u16,
    ) -> io::Result<()> {
        self.size = terminal::size()?;
        let panel_top = self.size.1.saturating_sub(BOTTOM_PANEL_HEIGHT);

        queue!(self.stdout, Hide)?;

        // Clear and redraw each panel line.
        for i in 0..BOTTOM_PANEL_HEIGHT {
            queue!(self.stdout, MoveTo(0, panel_top + i))?;
            queue!(self.stdout, Clear(ClearType::CurrentLine))?;
            if let Some(line) = lines.get(i as usize) {
                self.write_styled_line(line)?;
            }
        }

        // Position cursor in the composer area.
        let cursor_y = panel_top + cursor_row_offset;
        queue!(self.stdout, Show)?;
        execute!(self.stdout, MoveTo(cursor_col, cursor_y))?;
        Ok(())
    }

    fn write_styled_line(&mut self, line: &Line<'_>) -> io::Result<()> {
        for span in &line.spans {
            if let Some(fg) = span.style.fg {
                queue!(self.stdout, SetForegroundColor(ratatui_to_crossterm(fg)))?;
            }
            if span.style.add_modifier.contains(Modifier::BOLD) {
                queue!(self.stdout, SetAttribute(crossterm::style::Attribute::Bold))?;
            }
            if span.style.add_modifier.contains(Modifier::DIM) {
                queue!(self.stdout, SetAttribute(crossterm::style::Attribute::Dim))?;
            }
            queue!(self.stdout, Print(&*span.content))?;
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
        // Move below panel so shell prompt appears cleanly.
        if let Ok(size) = terminal::size() {
            let _ = execute!(self.stdout, MoveTo(0, size.1));
        }
        let _ = execute!(self.stdout, Print("\n"));
    }
}

fn ratatui_to_crossterm(c: Color) -> crossterm::style::Color {
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
