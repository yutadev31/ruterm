use ruterm_parser::{Action, Csi, Esc, Osc};
use unicode_width::UnicodeWidthChar;

use crate::{
    attributes::Attributes,
    common::Position,
    screen::{Cell, Screen},
};

mod cursor;
mod editing;
mod rendition;
#[cfg(test)]
mod tests;

pub use cursor::Cursor;

#[derive(Debug)]
pub struct Terminal {
    pub screen: Screen,
    pub cursor: Cursor,
    pub attrs: Attributes,
    saved_cursor: Option<(Position, Attributes)>,
    normal_saved_cursor: Option<(Position, Attributes)>,
    normal_screen: Option<Screen>,
    alternate_saved_cursor: Option<(Cursor, Attributes)>,
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
            normal_saved_cursor: None,
            normal_screen: None,
            alternate_saved_cursor: None,
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
        if let Some(normal_screen) = &mut self.normal_screen {
            normal_screen.resize(rows, cols);
        }
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
        if let Some((position, _)) = &mut self.normal_saved_cursor {
            position.row = position.row.min(rows.saturating_sub(1));
            position.col = position.col.min(cols.saturating_sub(1));
        }
        if let Some((cursor, _)) = &mut self.alternate_saved_cursor {
            cursor.position.row = cursor.position.row.min(rows.saturating_sub(1));
            cursor.position.col = cursor.position.col.min(cols.saturating_sub(1));
        }
    }

    pub fn is_alternate_screen(&self) -> bool {
        self.normal_screen.is_some()
    }

    fn set_alternate_screen(&mut self, enabled: bool, save_cursor: bool) {
        if enabled {
            if self.normal_screen.is_some() {
                return;
            }
            if save_cursor {
                self.alternate_saved_cursor = Some((self.cursor, self.attrs));
                self.cursor.position = Position::default();
                self.cursor.pending_wrap = false;
            }
            self.normal_saved_cursor = self.saved_cursor.take();
            let alternate = Screen::alternate(self.screen.rows(), self.screen.cols());
            self.normal_screen = Some(std::mem::replace(&mut self.screen, alternate));
        } else if let Some(normal_screen) = self.normal_screen.take() {
            self.screen = normal_screen;
            self.saved_cursor = self.normal_saved_cursor.take();
            if let Some((cursor, attrs)) = self.alternate_saved_cursor.take() {
                self.cursor = cursor;
                self.attrs = attrs;
            }
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
            if matches!(csi.final_byte, b'h' | b'l') {
                let enabled = csi.final_byte == b'h';
                for mode in csi.params.iter().flatten() {
                    match mode {
                        25 => self.cursor.visible = enabled,
                        47 | 1047 => self.set_alternate_screen(enabled, false),
                        1048 => {
                            if enabled {
                                self.save_cursor();
                            } else {
                                self.restore_cursor();
                            }
                        }
                        1049 => self.set_alternate_screen(enabled, true),
                        _ => {}
                    }
                }
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
}

fn param(csi: &Csi, index: usize) -> u16 {
    csi.params.get(index).copied().flatten().unwrap_or(0)
}

fn param_count(csi: &Csi, index: usize) -> usize {
    usize::from(param(csi, index).max(1))
}
