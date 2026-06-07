//! Inline-viewport TUI (`--tui`) — an optional polished front-end.
//!
//! Built on ratatui 0.30's inline viewport (`Viewport::Inline`), mirroring the
//! Codex CLI approach: finalized transcript cells are committed into the terminal's
//! NATIVE scrollback via `Terminal::insert_before`, while a small live area at the
//! bottom shows the in-flight assistant tokens and the input box. This coexists with
//! normal scrollback (no alternate screen), so the user keeps their history and can
//! scroll back as usual.
//!
//! The proven line mode (`LineUi`) remains the DEFAULT; this is opt-in. Both drive
//! the identical agent loop through the `Ui` trait, so behaviour is shared.
//!
//! NOTE: interactive rendering can't be verified headlessly — the pure layout/wrap
//! logic is unit-tested below; the live drawing needs a real terminal.

use crate::ui::Ui;
use crate::Result;
use anyhow::anyhow;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Widget, Wrap};
use ratatui::{DefaultTerminal, Frame, TerminalOptions, Viewport};
use std::time::Duration;

/// Inline viewport height: 1 status/stream row + a 3-row bordered input box.
const VIEWPORT_HEIGHT: u16 = 4;

pub struct TuiUi {
    terminal: DefaultTerminal,
    /// What the user is currently typing (during `read_line`).
    input: String,
    /// Accumulated tokens of the in-flight assistant message.
    streaming: String,
    is_streaming: bool,
    /// True only while actively reading a user line.
    reading: bool,
    /// A transient status line (e.g. "thinking…", an approval prompt).
    status: String,
    /// Last known terminal width (updated every draw); used to measure wrap height.
    width: u16,
}

impl TuiUi {
    pub fn new() -> Result<Self> {
        // Inline viewport: raw mode on, panic hook installed, NO alternate screen.
        let terminal = ratatui::try_init_with_options(TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_HEIGHT),
        })
        .map_err(|e| anyhow!("failed to start TUI: {e}"))?;
        let mut me = TuiUi {
            terminal,
            input: String::new(),
            streaming: String::new(),
            is_streaming: false,
            reading: false,
            status: String::new(),
            width: 80,
        };
        me.draw(); // initial paint + learn the real width
        Ok(me)
    }

    /// Repaint the live bottom viewport.
    fn draw(&mut self) {
        let is_streaming = self.is_streaming;
        // Only the tail is visible in the 1-row live area; cloning a bounded slice
        // keeps streaming O(1) per token instead of O(n).
        let tail = tail_chars(&self.streaming, self.width as usize * 2 + 16);
        let input = self.input.clone();
        let status = self.status.clone();
        let reading = self.reading;
        let mut width = self.width;
        let _ = self.terminal.draw(|f| {
            width = f.area().width.max(1);
            render(f, is_streaming, &tail, &input, &status, reading);
        });
        self.width = width;
    }

    /// Commit a finalized cell of text into the native scrollback above the viewport.
    fn push_block(&mut self, text: &str, color: Option<Color>, bold: bool) {
        let text = text.trim_end_matches('\n');
        if text.is_empty() {
            return;
        }
        let height = wrapped_rows(text, self.width);
        let owned = text.to_string();
        let mut st = Style::default();
        if let Some(c) = color {
            st = st.fg(c);
        }
        if bold {
            st = st.add_modifier(Modifier::BOLD);
        }
        let _ = self.terminal.insert_before(height, move |buf: &mut Buffer| {
            Paragraph::new(owned)
                .style(st)
                .wrap(Wrap { trim: false })
                .render(buf.area, buf);
        });
    }

    /// Read a key (Press only), handling resize. Returns None on read error.
    fn next_key(&mut self) -> Option<event::KeyEvent> {
        loop {
            match event::poll(Duration::from_millis(120)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => return Some(k),
                    Ok(Event::Resize(_, _)) => {
                        let _ = self.terminal.autoresize();
                        self.draw();
                    }
                    Ok(_) => {}
                    Err(_) => return None,
                },
                Ok(false) => return None, // timeout: let caller redraw
                Err(_) => return None,
            }
        }
    }
}

impl Drop for TuiUi {
    fn drop(&mut self) {
        // Restore cooked mode / cursor on clean exit (the panic hook covers panics).
        ratatui::restore();
    }
}

impl Ui for TuiUi {
    fn assistant_message(&mut self, text: &str) {
        self.push_block(&format!("⏺ {text}"), Some(Color::Green), false);
    }

    fn notice(&mut self, text: &str) {
        self.push_block(&format!("ℹ {text}"), Some(Color::Yellow), false);
    }

    fn tool_start(&mut self, _name: &str, summary: &str) {
        self.push_block(&format!("› {summary}"), Some(Color::Cyan), false);
    }

    fn tool_result(&mut self, _name: &str, output: &str, is_error: bool) {
        let mut shown: Vec<&str> = output.lines().take(20).collect();
        if output.lines().count() > 20 {
            shown.push("… (output truncated)");
        }
        let color = if is_error { Color::Red } else { Color::DarkGray };
        self.push_block(&shown.join("\n"), Some(color), false);
    }

    fn confirm(&mut self, prompt: &str, preview: Option<&str>) -> bool {
        if let Some(p) = preview {
            self.push_block(p, None, false);
        }
        self.status = format!("{prompt}  [Y/n]");
        let answer = loop {
            self.draw();
            let Some(k) = self.next_key() else { continue };
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => break true,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => break false,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break false,
                _ => {}
            }
        };
        self.status.clear();
        self.draw();
        answer
    }

    fn stream_start(&mut self) {
        self.streaming.clear();
        self.is_streaming = true;
        self.status = "thinking…".to_string();
        self.draw();
    }

    fn stream_delta(&mut self, piece: &str) {
        self.streaming.push_str(piece);
        self.status.clear();
        self.draw();
    }

    fn stream_end(&mut self) {
        let text = std::mem::take(&mut self.streaming);
        self.is_streaming = false;
        self.status.clear();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            self.push_block(&format!("⏺ {trimmed}"), Some(Color::Green), false);
        }
        self.draw();
    }

    fn history_block(&mut self, text: &str) {
        self.push_block(text, None, false);
    }

    fn read_line(&mut self, _prompt: &str) -> Option<String> {
        self.input.clear();
        self.reading = true;
        self.status.clear();
        loop {
            self.draw();
            let Some(k) = self.next_key() else { continue };
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            match k.code {
                KeyCode::Enter => {
                    let line = std::mem::take(&mut self.input);
                    self.reading = false;
                    if !line.trim().is_empty() {
                        self.push_block(&format!("you› {}", line.trim()), Some(Color::Blue), true);
                    }
                    return Some(line);
                }
                KeyCode::Char('c') if ctrl => {
                    self.reading = false;
                    return None;
                }
                KeyCode::Char('d') if ctrl && self.input.is_empty() => {
                    self.reading = false;
                    return None;
                }
                KeyCode::Char(c) => self.input.push(c),
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Esc => self.input.clear(),
                _ => {}
            }
        }
    }
}

/// Render the live bottom viewport (free fn: no `&self` borrow inside `draw`).
fn render(f: &mut Frame, is_streaming: bool, stream_tail: &str, input: &str, status: &str, reading: bool) {
    let area = f.area();
    let [top, bottom] = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);

    if is_streaming && !stream_tail.is_empty() {
        let p = Paragraph::new(format!("⏺ {stream_tail}")).style(Style::default().fg(Color::Cyan));
        f.render_widget(p, top);
    } else if !status.is_empty() {
        f.render_widget(Paragraph::new(status.to_string()).style(Style::default().fg(Color::Yellow)), top);
    } else {
        let hint = Paragraph::new("localcode · everything stays local · /help")
            .style(Style::default().add_modifier(Modifier::DIM));
        f.render_widget(hint, top);
    }

    let title = if reading { " you " } else { " working… " };
    let content = if reading { format!("{input}▏") } else { String::new() };
    let block = Block::bordered().title(Line::from(title));
    f.render_widget(Paragraph::new(content).block(block), bottom);
}

/// Number of terminal rows `text` occupies when wrapped at `width` columns.
fn wrapped_rows(text: &str, width: u16) -> u16 {
    let w = (width.max(1)) as usize;
    text.split('\n')
        .map(|l| {
            let cols = l.chars().count();
            if cols == 0 {
                1
            } else {
                cols.div_ceil(w) as u16
            }
        })
        .sum::<u16>()
        .max(1)
}

/// Last `max` characters of `s` (on a char boundary).
fn tail_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

#[cfg(test)]
mod tests {
    use super::{tail_chars, wrapped_rows};

    #[test]
    fn wrap_counts_rows() {
        assert_eq!(wrapped_rows("hello", 80), 1);
        assert_eq!(wrapped_rows("", 80), 1);
        // 10 chars at width 4 → ceil(10/4) = 3 rows.
        assert_eq!(wrapped_rows("0123456789", 4), 3);
        // Two lines, each wrapping.
        assert_eq!(wrapped_rows("aaaaa\nbbbbb", 4), 4); // 2 + 2
        // Blank line still occupies a row.
        assert_eq!(wrapped_rows("a\n\nb", 80), 3);
    }

    #[test]
    fn wrap_width_zero_is_safe() {
        // width.max(1) guards the division.
        assert!(wrapped_rows("abc", 0) >= 1);
    }

    #[test]
    fn tail_keeps_last_chars_on_boundary() {
        assert_eq!(tail_chars("hello world", 5), "world");
        assert_eq!(tail_chars("short", 50), "short");
        // Multibyte-safe (no panic, valid utf-8).
        let s = "héllo wörld";
        let t = tail_chars(s, 5);
        assert_eq!(t.chars().count(), 5);
    }
}
