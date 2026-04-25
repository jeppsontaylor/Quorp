use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
#[cfg(test)]
use ratatui::text::Line;
use vt100::{Parser, Screen};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCell {
    pub symbol: String,
    pub fg: Color,
    pub bg: Color,
    pub modifier: Modifier,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            symbol: " ".to_string(),
            fg: Color::Reset,
            bg: Color::Reset,
            modifier: Modifier::empty(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct TerminalSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<TerminalCell>,
    pub cursor: (u16, u16),
    pub hide_cursor: bool,
    pub scrollback: usize,
    pub alternate_screen: bool,
    pub bracketed_paste: bool,
}

impl TerminalSnapshot {
    pub fn blank(rows: u16, cols: u16) -> Self {
        let len = rows as usize * cols as usize;
        Self {
            rows,
            cols,
            cells: vec![TerminalCell::default(); len],
            cursor: (0, 0),
            hide_cursor: false,
            scrollback: 0,
            alternate_screen: false,
            bracketed_paste: false,
        }
    }

    pub fn from_screen(screen: &Screen) -> Self {
        let (rows, cols) = screen.size();
        let mut snapshot = Self::blank(rows, cols);
        snapshot.cursor = screen.cursor_position();
        snapshot.hide_cursor = screen.hide_cursor();
        snapshot.scrollback = screen.scrollback();
        snapshot.alternate_screen = screen.alternate_screen();
        snapshot.bracketed_paste = screen.bracketed_paste();

        for row in 0..rows {
            for col in 0..cols {
                let cell = screen.cell(row, col);
                let Some(cell) = cell else {
                    continue;
                };
                let index = row as usize * cols as usize + col as usize;
                snapshot.cells[index] = terminal_cell_from_vt100(cell);
            }
        }

        snapshot
    }

    #[cfg(test)]
    pub fn from_lines(lines: &[Line<'_>]) -> Self {
        let rows = lines.len() as u16;
        let cols = lines
            .iter()
            .map(|line| line.width() as u16)
            .max()
            .unwrap_or(0)
            .max(1);
        let mut snapshot = Self::blank(rows.max(1), cols);
        snapshot.hide_cursor = true;
        for (row_index, line) in lines.iter().enumerate() {
            let mut col_index = 0usize;
            for span in &line.spans {
                let style = span.style;
                for character in span.content.chars() {
                    if col_index >= usize::from(cols) {
                        break;
                    }
                    let index = row_index * usize::from(cols) + col_index;
                    if let Some(cell) = snapshot.cells.get_mut(index) {
                        *cell = TerminalCell {
                            symbol: character.to_string(),
                            fg: style.fg.unwrap_or(Color::Reset),
                            bg: style.bg.unwrap_or(Color::Reset),
                            modifier: style.add_modifier,
                        };
                    }
                    col_index += 1;
                }
            }
        }
        snapshot
    }

    pub fn row_strings(&self, max_rows: usize) -> Vec<String> {
        let row_count = usize::from(self.rows).min(max_rows);
        (0..row_count)
            .map(|row| {
                let mut line = String::new();
                for col in 0..usize::from(self.cols) {
                    let index = row * usize::from(self.cols) + col;
                    if let Some(cell) = self.cells.get(index) {
                        line.push_str(cell.symbol.as_str());
                    }
                }
                line.trim_end_matches(' ').to_string()
            })
            .collect()
    }

    pub fn render(&self, buf: &mut Buffer, rect: Rect, background: Color, cursor_visible: bool) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        let visible_rows = rect.height.min(self.rows);
        let visible_cols = rect.width.min(self.cols);

        for row in 0..rect.height {
            for col in 0..rect.width {
                if let Some(cell) = buf.cell_mut((rect.x + col, rect.y + row)) {
                    cell.set_symbol(" ").set_bg(background).set_fg(Color::Reset);
                }
            }
        }

        for row in 0..visible_rows {
            for col in 0..visible_cols {
                let index = row as usize * self.cols as usize + col as usize;
                let Some(snapshot_cell) = self.cells.get(index) else {
                    continue;
                };
                let style = Style::default()
                    .fg(snapshot_cell.fg)
                    .bg(resolve_bg(snapshot_cell.bg, background))
                    .add_modifier(snapshot_cell.modifier);
                if let Some(cell) = buf.cell_mut((rect.x + col, rect.y + row)) {
                    cell.set_symbol(snapshot_cell.symbol.as_str())
                        .set_style(style);
                }
            }
        }

        if cursor_visible && !self.hide_cursor {
            let (cursor_row, cursor_col) = self.cursor;
            if cursor_row < visible_rows && cursor_col < visible_cols {
                let index = cursor_row as usize * self.cols as usize + cursor_col as usize;
                let mut cursor_style = self
                    .cells
                    .get(index)
                    .map(|cell| {
                        Style::default()
                            .fg(resolve_bg(cell.bg, background))
                            .bg(resolve_fg(cell.fg))
                            .add_modifier(cell.modifier)
                    })
                    .unwrap_or_else(|| Style::default().fg(background).bg(Color::White));
                cursor_style = cursor_style.add_modifier(Modifier::REVERSED);
                if let Some(cell) = buf.cell_mut((rect.x + cursor_col, rect.y + cursor_row)) {
                    let symbol = cell.symbol().to_string();
                    let symbol = if symbol.is_empty() {
                        " "
                    } else {
                        symbol.as_str()
                    };
                    cell.set_symbol(symbol).set_style(cursor_style);
                }
            }
        }
    }
}

pub fn new_parser(rows: u16, cols: u16, scrollback_len: usize) -> Parser {
    Parser::new(rows, cols, scrollback_len)
}

pub fn terminal_window_title(parser: &Parser) -> Option<String> {
    let title = parser.screen().title().trim().to_string();
    (!title.is_empty()).then_some(title)
}

fn terminal_cell_from_vt100(cell: &vt100::Cell) -> TerminalCell {
    let mut fg = vt100_color_to_ratatui(cell.fgcolor());
    let mut bg = vt100_color_to_ratatui(cell.bgcolor());
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut modifier = Modifier::empty();
    if cell.bold() {
        modifier |= Modifier::BOLD;
    }
    if cell.italic() {
        modifier |= Modifier::ITALIC;
    }
    if cell.underline() {
        modifier |= Modifier::UNDERLINED;
    }

    let symbol = if cell.is_wide_continuation() {
        " ".to_string()
    } else if cell.has_contents() {
        cell.contents().to_string()
    } else {
        " ".to_string()
    };

    TerminalCell {
        symbol,
        fg,
        bg,
        modifier,
    }
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

fn resolve_bg(color: Color, fallback: Color) -> Color {
    if color == Color::Reset {
        fallback
    } else {
        color
    }
}

fn resolve_fg(color: Color) -> Color {
    if color == Color::Reset {
        Color::Black
    } else {
        color
    }
}

#[cfg(test)]
mod terminal_certification_tests {
    use super::*;

    fn snapshot_from_chunks(rows: u16, cols: u16, chunks: &[&[u8]]) -> TerminalSnapshot {
        let mut parser = new_parser(rows, cols, 256);
        for chunk in chunks {
            parser.process(chunk);
        }
        TerminalSnapshot::from_screen(parser.screen())
    }

    #[test]
    fn terminal_certification_carriage_return_redraw_matches_latest_prompt() {
        let snapshot = snapshot_from_chunks(4, 20, &[b"hello world", b"\rquorp> ready"]);
        assert_eq!(snapshot.row_strings(1), vec!["quorp> ready".to_string()]);
    }

    #[test]
    fn terminal_certification_tracks_ansi_color_and_inverse_cells() {
        let snapshot = snapshot_from_chunks(2, 8, &[b"\x1b[31;47mR\x1b[0m\x1b[31;47;7mI\x1b[0m"]);
        assert_eq!(snapshot.cells[0].fg, Color::Indexed(1));
        assert_eq!(snapshot.cells[0].bg, Color::Indexed(7));
        assert_eq!(snapshot.cells[1].fg, Color::Indexed(7));
        assert_eq!(snapshot.cells[1].bg, Color::Indexed(1));
    }

    #[test]
    fn terminal_certification_chunking_invariance_holds_for_split_utf8() {
        let bytes = "alpha界🙂beta".as_bytes();
        let whole = snapshot_from_chunks(4, 20, &[bytes]);
        let chunked = snapshot_from_chunks(
            4,
            20,
            &[
                &bytes[..2],
                &bytes[2..5],
                &bytes[5..8],
                &bytes[8..12],
                &bytes[12..],
            ],
        );
        assert_eq!(chunked, whole);
    }

    #[test]
    fn terminal_certification_alt_screen_and_bracketed_paste_flags_toggle() {
        let enabled = snapshot_from_chunks(4, 20, &[b"\x1b[?1049h\x1b[?2004hALT"]);
        assert!(enabled.alternate_screen);
        assert!(enabled.bracketed_paste);

        let disabled = snapshot_from_chunks(
            4,
            20,
            &[b"\x1b[?1049h\x1b[?2004hALT", b"\x1b[?2004l\x1b[?1049l"],
        );
        assert!(!disabled.alternate_screen);
        assert!(!disabled.bracketed_paste);
    }

    #[test]
    fn terminal_certification_wide_character_continuations_do_not_duplicate_glyphs() {
        let snapshot = snapshot_from_chunks(2, 6, &["界x".as_bytes()]);
        assert_eq!(snapshot.cells[0].symbol, "界");
        assert_eq!(snapshot.cells[1].symbol, " ");
        assert_eq!(snapshot.cells[2].symbol, "x");
    }

    #[test]
    fn terminal_certification_render_keeps_cursor_within_terminal_bounds() {
        let snapshot = snapshot_from_chunks(2, 4, &[b"abc"]);
        let rect = Rect::new(0, 0, 4, 2);
        let mut buffer = Buffer::empty(rect);
        snapshot.render(&mut buffer, rect, Color::Black, true);

        let rendered = (0..4u16)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect::<String>();
        assert!(rendered.starts_with("abc"));
    }
}
