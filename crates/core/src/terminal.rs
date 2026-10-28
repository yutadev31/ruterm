use ruterm_parser::{Action, Csi, Esc, Osc};
use unicode_width::UnicodeWidthChar;

use crate::{
    attributes::{Attributes, Color, Flags, UnderlineStyle},
    common::Position,
    screen::{Cell, Screen},
};

#[derive(Debug)]
pub struct Terminal {
    pub screen: Screen,
    pub cursor: Cursor,
    pub attrs: Attributes,
    saved_cursor: Option<(Position, Attributes)>,
    tab_stops: Vec<bool>,
}

impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            screen: Screen::new(rows, cols),
            cursor: Cursor {
                position: Position::default(),
                pending_wrap: false,
                visible: true,
            },
            attrs: Attributes::default(),
            saved_cursor: None,
            tab_stops: (0..cols).map(|col| col != 0 && col % 8 == 0).collect(),
        }
    }

    pub fn process(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.print(ch),
            Action::Control(ctrl) => self.control(ctrl),
            Action::Esc(esc) => self.esc(esc),
            Action::Csi(csi) => self.csi(csi),
            Action::Osc(osc) => self.osc(osc),
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let old_cols = self.tab_stops.len();
        self.screen.resize(rows, cols);
        self.cursor.position.row = self.cursor.position.row.min(rows.saturating_sub(1));
        self.cursor.position.col = self.cursor.position.col.min(cols.saturating_sub(1));
        self.cursor.pending_wrap = false;
        self.tab_stops.resize(cols, false);
        for col in old_cols..cols {
            if col != 0 && col % 8 == 0 {
                self.tab_stops[col] = true;
            }
        }
        if let Some((position, _)) = &mut self.saved_cursor {
            position.row = position.row.min(rows.saturating_sub(1));
            position.col = position.col.min(cols.saturating_sub(1));
        }
    }

    fn print(&mut self, ch: char) {
        if self.screen.rows() == 0 || self.screen.cols() == 0 {
            return;
        }
        let width = UnicodeWidthChar::width(ch)
            .unwrap_or(0)
            .min(self.screen.cols());
        if width == 0 {
            let mut col = if self.cursor.pending_wrap {
                self.cursor.position.col
            } else {
                self.cursor.position.col.saturating_sub(1)
            };
            if col > 0
                && self.screen.cells[self.cursor.position.row * self.screen.cols() + col]
                    .wide_continuation
            {
                col -= 1;
            }
            let index = self.cursor.position.row * self.screen.cols() + col;
            if let Some(cell) = self.screen.cells.get_mut(index) {
                cell.combining.push(ch);
            }
            return;
        }
        if self.cursor.pending_wrap {
            self.wrap_line();
        }
        if width == 2 && self.cursor.position.col + 1 >= self.screen.cols() {
            self.wrap_line();
        }
        let row = self.cursor.position.row.min(self.screen.rows() - 1);
        let col = self.cursor.position.col.min(self.screen.cols() - 1);
        self.cursor.position = Position { row, col };
        self.screen.set_cell(
            row,
            col,
            Cell {
                ch,
                combining: String::new(),
                wide_continuation: false,
                attrs: self.attrs,
            },
        );
        if width == 2 {
            self.screen.set_cell(
                row,
                col + 1,
                Cell {
                    ch: ' ',
                    combining: String::new(),
                    wide_continuation: true,
                    attrs: self.attrs,
                },
            );
        }
        if col + width >= self.screen.cols() {
            self.cursor.pending_wrap = true;
        } else {
            self.cursor.position.col += width;
        }
    }

    fn control(&mut self, ctrl: u8) {
        match ctrl {
            b'\x08' => self.backspace(),
            b'\x09' => self.tab(),
            b'\x0a' | b'\x0b' | b'\x0c' => self.line_feed(),
            b'\x0d' => self.carriage_return(),
            _ => return,
        }
        self.cursor.pending_wrap = false;
    }

    fn esc(&mut self, esc: Esc) {
        if esc.intermediate_len != 0 {
            return;
        }
        match esc.final_byte {
            b'D' => self.index(),
            b'E' => self.next_line(),
            b'M' => self.reverse_index(),
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'H' => self.set_tab_stop(),
            b'c' => self.reset(),
            _ => return,
        }
        self.cursor.pending_wrap = false;
    }

    fn csi(&mut self, csi: Csi) {
        if !csi.intermediates.is_empty() {
            return;
        }
        if csi.private == Some(b'?') {
            if matches!(csi.final_byte, b'h' | b'l') && csi.params.contains(&Some(25)) {
                self.cursor.visible = csi.final_byte == b'h';
            }
            return;
        }
        if csi.private.is_some() {
            return;
        }
        let n = param_count(&csi, 0);
        let rows = self.screen.rows();
        let cols = self.screen.cols();
        match csi.final_byte {
            b'A' => self.cursor.position.row = self.cursor.position.row.saturating_sub(n),
            b'B' | b'e' => {
                self.cursor.position.row = self
                    .cursor
                    .position
                    .row
                    .saturating_add(n)
                    .min(rows.saturating_sub(1))
            }
            b'C' | b'a' => {
                self.cursor.position.col = self
                    .cursor
                    .position
                    .col
                    .saturating_add(n)
                    .min(cols.saturating_sub(1))
            }
            b'D' => self.cursor.position.col = self.cursor.position.col.saturating_sub(n),
            b'E' => {
                self.cursor.position.row = self
                    .cursor
                    .position
                    .row
                    .saturating_add(n)
                    .min(rows.saturating_sub(1));
                self.cursor.position.col = 0;
            }
            b'F' => {
                self.cursor.position.row = self.cursor.position.row.saturating_sub(n);
                self.cursor.position.col = 0;
            }
            b'G' | b'`' => {
                self.cursor.position.col = n.saturating_sub(1).min(cols.saturating_sub(1))
            }
            b'd' => self.cursor.position.row = n.saturating_sub(1).min(rows.saturating_sub(1)),
            b'H' | b'f' => {
                self.cursor.position.row = param_count(&csi, 0)
                    .saturating_sub(1)
                    .min(rows.saturating_sub(1));
                self.cursor.position.col = param_count(&csi, 1)
                    .saturating_sub(1)
                    .min(cols.saturating_sub(1));
            }
            b'J' => self.erase_display(param(&csi, 0)),
            b'K' => self.erase_line(param(&csi, 0)),
            b'X' => self.erase_chars(n),
            b'@' => self.insert_chars(n),
            b'P' => self.delete_chars(n),
            b'L' => self.insert_lines(n),
            b'M' => self.delete_lines(n),
            b'S' => self.screen.scroll_up(n),
            b'T' => self.screen.scroll_down(n),
            b'I' => self.move_tabs(n, true),
            b'Z' => self.move_tabs(n, false),
            b'g' => self.clear_tab_stops(param(&csi, 0)),
            b'm' => self.sgr(&csi.params),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            _ => return,
        }
        if !matches!(csi.final_byte, b'm' | b'g') {
            self.cursor.pending_wrap = false;
        }
    }

    fn osc(&mut self, _osc: Osc) {}

    fn wrap_line(&mut self) {
        self.cursor.position.col = 0;
        self.line_feed();
        self.cursor.pending_wrap = false;
    }

    fn backspace(&mut self) {
        self.cursor.position.col = self.cursor.position.col.saturating_sub(1);
    }

    fn tab(&mut self) {
        self.move_tabs(1, true);
    }

    fn move_tabs(&mut self, count: usize, forward: bool) {
        for _ in 0..count {
            let col = self.cursor.position.col;
            let stop = if forward {
                (col.saturating_add(1)..self.tab_stops.len())
                    .find(|&i| self.tab_stops[i])
                    .unwrap_or(self.screen.cols().saturating_sub(1))
            } else {
                (0..col.min(self.tab_stops.len()))
                    .rev()
                    .find(|&i| self.tab_stops[i])
                    .unwrap_or(0)
            };
            self.cursor.position.col = stop;
        }
    }

    fn line_feed(&mut self) {
        if self.cursor.position.row + 1 < self.screen.rows() {
            self.cursor.position.row += 1;
        } else {
            self.screen.scroll_up(1);
        }
    }

    fn carriage_return(&mut self) {
        self.cursor.position.col = 0;
    }

    fn index(&mut self) {
        self.line_feed();
    }

    fn next_line(&mut self) {
        self.line_feed();
        self.carriage_return();
    }

    fn reverse_index(&mut self) {
        if self.cursor.position.row == 0 {
            self.screen.scroll_down(1);
        } else {
            self.cursor.position.row -= 1;
        }
    }

    fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cursor.position, self.attrs));
    }

    fn restore_cursor(&mut self) {
        if let Some((position, attrs)) = self.saved_cursor {
            self.cursor.position = Position {
                row: position.row.min(self.screen.rows().saturating_sub(1)),
                col: position.col.min(self.screen.cols().saturating_sub(1)),
            };
            self.attrs = attrs;
        }
    }

    fn set_tab_stop(&mut self) {
        if let Some(stop) = self.tab_stops.get_mut(self.cursor.position.col) {
            *stop = true;
        }
    }

    fn clear_tab_stops(&mut self, mode: u16) {
        match mode {
            0 => {
                if let Some(stop) = self.tab_stops.get_mut(self.cursor.position.col) {
                    *stop = false;
                }
            }
            3 => self.tab_stops.fill(false),
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.screen.clear();
        self.cursor = Cursor {
            position: Position::default(),
            pending_wrap: false,
            visible: true,
        };
        self.attrs = Attributes::default();
        self.saved_cursor = None;
        self.tab_stops = (0..self.screen.cols())
            .map(|col| col != 0 && col % 8 == 0)
            .collect();
    }

    fn erase_display(&mut self, mode: u16) {
        let cols = self.screen.cols();
        let row = self.cursor.position.row;
        let col = self.cursor.position.col;
        let len = self.screen.cells.len();
        let index = row.saturating_mul(cols).saturating_add(col).min(len);
        match mode {
            0 => self.erase_range(index, len),
            1 => self.erase_range(0, index.saturating_add(1).min(len)),
            2 => self.erase_range(0, len),
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let cols = self.screen.cols();
        let start = self.cursor.position.row.saturating_mul(cols);
        let end = start.saturating_add(cols).min(self.screen.cells.len());
        let index = start.saturating_add(self.cursor.position.col).min(end);
        match mode {
            0 => self.erase_range(index, end),
            1 => self.erase_range(start, index.saturating_add(1).min(end)),
            2 => self.erase_range(start, end),
            _ => {}
        }
    }

    fn erase_chars(&mut self, count: usize) {
        let start = self
            .cursor
            .position
            .row
            .saturating_mul(self.screen.cols())
            .saturating_add(self.cursor.position.col);
        let end = (start.saturating_add(count))
            .min((self.cursor.position.row + 1).saturating_mul(self.screen.cols()));
        self.erase_range(start, end);
    }

    fn erase_range(&mut self, start: usize, end: usize) {
        let end = end.min(self.screen.cells.len());
        if start < end {
            self.screen.cells[start..end].fill(Cell {
                ch: ' ',
                combining: String::new(),
                wide_continuation: false,
                attrs: self.attrs,
            });
        }
    }

    fn insert_chars(&mut self, count: usize) {
        let cols = self.screen.cols();
        let start = self
            .cursor
            .position
            .row
            .saturating_mul(cols)
            .saturating_add(self.cursor.position.col);
        let end = (self.cursor.position.row + 1).saturating_mul(cols);
        if start >= end || end > self.screen.cells.len() {
            return;
        }
        let n = count.min(end - start);
        self.screen.cells[start..end].rotate_right(n);
        self.erase_range(start, start + n);
    }

    fn delete_chars(&mut self, count: usize) {
        let cols = self.screen.cols();
        let start = self
            .cursor
            .position
            .row
            .saturating_mul(cols)
            .saturating_add(self.cursor.position.col);
        let end = (self.cursor.position.row + 1).saturating_mul(cols);
        if start >= end || end > self.screen.cells.len() {
            return;
        }
        let n = count.min(end - start);
        self.screen.cells[start..end].rotate_left(n);
        self.erase_range(end - n, end);
    }

    fn insert_lines(&mut self, count: usize) {
        let cols = self.screen.cols();
        let start = self.cursor.position.row.saturating_mul(cols);
        let end = self.screen.cells.len();
        if cols == 0 || start >= end {
            return;
        }
        let n = count.min(self.screen.rows() - self.cursor.position.row) * cols;
        self.screen.cells[start..end].rotate_right(n);
        self.erase_range(start, start + n);
    }

    fn delete_lines(&mut self, count: usize) {
        let cols = self.screen.cols();
        let start = self.cursor.position.row.saturating_mul(cols);
        let end = self.screen.cells.len();
        if cols == 0 || start >= end {
            return;
        }
        let n = count.min(self.screen.rows() - self.cursor.position.row) * cols;
        self.screen.cells[start..end].rotate_left(n);
        self.erase_range(end - n, end);
    }

    fn sgr(&mut self, params: &[Option<u16>]) {
        let mut i = 0;
        while i < params.len().max(1) {
            let code = params.get(i).copied().flatten().unwrap_or(0);
            match code {
                0 => self.attrs = Attributes::default(),
                1 => self.attrs.flags.insert(Flags::BOLD),
                2 => self.attrs.flags.insert(Flags::FAINT),
                3 => self.attrs.flags.insert(Flags::ITALIC),
                4 => self.attrs.underline = UnderlineStyle::Single,
                5 | 6 => self.attrs.flags.insert(Flags::BLINK),
                7 => self.attrs.flags.insert(Flags::INVERSE),
                8 => self.attrs.flags.insert(Flags::HIDDEN),
                9 => self.attrs.flags.insert(Flags::STRIKETHROUGH),
                21 => self.attrs.underline = UnderlineStyle::Double,
                22 => self.attrs.flags.remove(Flags::BOLD | Flags::FAINT),
                23 => self.attrs.flags.remove(Flags::ITALIC),
                24 => self.attrs.underline = UnderlineStyle::None,
                25 => self.attrs.flags.remove(Flags::BLINK),
                27 => self.attrs.flags.remove(Flags::INVERSE),
                28 => self.attrs.flags.remove(Flags::HIDDEN),
                29 => self.attrs.flags.remove(Flags::STRIKETHROUGH),
                30..=37 => self.attrs.fg = Color::Indexed((code - 30) as u8),
                40..=47 => self.attrs.bg = Color::Indexed((code - 40) as u8),
                90..=97 => self.attrs.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.attrs.bg = Color::Indexed((code - 100 + 8) as u8),
                39 => self.attrs.fg = Color::Default,
                49 => self.attrs.bg = Color::Default,
                38 | 48 => {
                    if let Some((color, consumed)) =
                        parse_extended_color(params.get(i + 1..).unwrap_or(&[]))
                    {
                        if code == 38 {
                            self.attrs.fg = color;
                        } else {
                            self.attrs.bg = color;
                        }
                        i += consumed;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn parse_extended_color(values: &[Option<u16>]) -> Option<(Color, usize)> {
    match values {
        [Some(5), index, ..] if index.unwrap_or(0) <= 255 => {
            Some((Color::Indexed(index.unwrap_or(0) as u8), 2))
        }
        [Some(2), r, g, b, ..]
            if r.unwrap_or(0) <= 255 && g.unwrap_or(0) <= 255 && b.unwrap_or(0) <= 255 =>
        {
            Some((
                Color::Rgb(
                    r.unwrap_or(0) as u8,
                    g.unwrap_or(0) as u8,
                    b.unwrap_or(0) as u8,
                ),
                4,
            ))
        }
        _ => None,
    }
}

fn param(csi: &Csi, index: usize) -> u16 {
    csi.params.get(index).copied().flatten().unwrap_or(0)
}

fn param_count(csi: &Csi, index: usize) -> usize {
    usize::from(param(csi, index).max(1))
}

#[derive(Debug)]
pub struct Cursor {
    pub position: Position,
    pub pending_wrap: bool,
    pub visible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruterm_parser::Parser;

    fn feed(terminal: &mut Terminal, input: &[u8]) {
        Parser::default().advance(input, |action| terminal.process(action));
    }

    fn row(terminal: &Terminal, index: usize) -> String {
        let start = index * terminal.screen.cols();
        terminal.screen.cells[start..start + terminal.screen.cols()]
            .iter()
            .map(|cell| cell.ch)
            .collect()
    }

    #[test]
    fn prints_wraps_and_scrolls() {
        let mut terminal = Terminal::new(2, 3);
        feed(&mut terminal, b"abcdefg");
        assert_eq!(row(&terminal, 0), "def");
        assert_eq!(row(&terminal, 1), "g  ");
        assert_eq!(terminal.cursor.position, Position { row: 1, col: 1 });
    }

    #[test]
    fn handles_cursor_movement_and_erasure() {
        let mut terminal = Terminal::new(2, 5);
        feed(&mut terminal, b"abcde\x1b[1;3H\x1b[0K");
        assert_eq!(row(&terminal, 0), "ab   ");
        feed(&mut terminal, b"Z\x1b[2;1Hq\x1b[2J");
        assert_eq!(row(&terminal, 0), "     ");
        assert_eq!(row(&terminal, 1), "     ");
    }

    #[test]
    fn handles_sgr_colors_and_reset() {
        let mut terminal = Terminal::new(1, 4);
        feed(&mut terminal, b"\x1b[1;38;2;12;34;56;48;5;123mA\x1b[0mB");
        let first = terminal.screen.cells[0].clone();
        assert_eq!(first.attrs.fg, Color::Rgb(12, 34, 56));
        assert_eq!(first.attrs.bg, Color::Indexed(123));
        assert!(first.attrs.flags.contains(Flags::BOLD));
        assert_eq!(terminal.screen.cells[1].attrs, Attributes::default());
    }

    #[test]
    fn saves_cursor_and_manages_tabs() {
        let mut terminal = Terminal::new(2, 20);
        feed(&mut terminal, b"\tX\x1b7\x1b[2;1H\x1b[31mY\x1b8Z");
        assert_eq!(terminal.screen.cells[8].ch, 'X');
        assert_eq!(terminal.screen.cells[9].ch, 'Z');
        assert_eq!(terminal.screen.cells[20].attrs.fg, Color::Indexed(1));
        assert_eq!(terminal.screen.cells[9].attrs.fg, Color::Default);
        feed(&mut terminal, b"\x1b[3g\r\t");
        assert_eq!(terminal.cursor.position.col, 19);
    }

    #[test]
    fn supports_insert_and_delete() {
        let mut terminal = Terminal::new(1, 5);
        feed(&mut terminal, b"abcde\r\x1b[2C\x1b[@");
        assert_eq!(row(&terminal, 0), "ab cd");
        feed(&mut terminal, b"\x1b[P");
        assert_eq!(row(&terminal, 0), "abcd ");
    }

    #[test]
    fn reverse_index_scrolls_down_at_top() {
        let mut terminal = Terminal::new(2, 3);
        feed(&mut terminal, b"abc\r\nxyz\x1b[1;1H\x1bM");
        assert_eq!(row(&terminal, 0), "   ");
        assert_eq!(row(&terminal, 1), "abc");
    }

    #[test]
    fn empty_screen_is_safe() {
        let mut terminal = Terminal::new(0, 0);
        feed(&mut terminal, b"abc\n\x1b[2J\x1bc");
        assert!(terminal.screen.cells.is_empty());
    }

    #[test]
    fn resize_preserves_visible_content_and_cursor() {
        let mut terminal = Terminal::new(2, 4);
        feed(&mut terminal, b"abc\x1b[2;4H!");
        terminal.resize(3, 5);
        assert_eq!(row(&terminal, 0), "abc  ");
        assert_eq!(terminal.screen.cells[8].ch, '!');
        assert_eq!(terminal.cursor.position, Position { row: 1, col: 3 });
        terminal.resize(1, 2);
        assert_eq!(row(&terminal, 0), "ab");
        assert_eq!(terminal.cursor.position, Position { row: 0, col: 1 });
    }

    #[test]
    fn wide_and_combining_characters_use_expected_cells() {
        let mut terminal = Terminal::new(2, 4);
        feed(&mut terminal, "日e\u{301}".as_bytes());
        assert_eq!(terminal.screen.cells[0].ch, '日');
        assert!(terminal.screen.cells[1].wide_continuation);
        assert_eq!(terminal.screen.cells[2].ch, 'e');
        assert_eq!(terminal.screen.cells[2].combining, "\u{301}");
        assert_eq!(terminal.cursor.position.col, 3);
        let mut terminal = Terminal::new(2, 1);
        feed(&mut terminal, "a\u{301}".as_bytes());
        assert_eq!(terminal.screen.cells[0].combining, "\u{301}");
        assert_eq!(terminal.cursor.position.row, 0);
    }

    #[test]
    fn scrollback_keeps_scrolled_lines() {
        let mut terminal = Terminal::new(2, 2);
        feed(&mut terminal, b"aa\r\nbb\r\ncc");
        assert_eq!(terminal.screen.scrollback.len(), 1);
        let view = terminal.screen.view_cells(1);
        assert_eq!(view[0].ch, 'a');
        assert_eq!(view[2].ch, 'b');
    }
}
