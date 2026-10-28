use ruterm_parser::{Action, Csi, Dcs, Esc, Osc};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    Press,
    Drag,
    Motion,
}

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
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    pub origin_mode: bool,
    pub auto_wrap: bool,
    pub insert_mode: bool,
    pub bracketed_paste: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub mouse_mode: MouseMode,
    pub sgr_mouse: bool,
    pub cursor_shape: CursorShape,
    pub title: String,
    pub(crate) responses: Vec<Vec<u8>>,
    charsets: [bool; 2],
    active_charset: usize,
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
            scroll_top: 0,
            scroll_bottom: rows,
            origin_mode: false,
            auto_wrap: true,
            insert_mode: false,
            bracketed_paste: false,
            application_cursor: false,
            application_keypad: false,
            mouse_mode: MouseMode::Off,
            sgr_mouse: false,
            cursor_shape: CursorShape::Block,
            title: String::new(),
            responses: Vec::new(),
            charsets: [false; 2],
            active_charset: 0,
        }
    }

    pub fn process(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.print(ch),
            Action::Control(ctrl) => self.control(ctrl),
            Action::Esc(esc) => self.esc(esc),
            Action::Csi(csi) => self.csi(csi),
            Action::Osc(osc) => self.osc(osc),
            Action::Dcs(dcs) => self.dcs(dcs),
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let old_cols = self.tab_stops.len();
        self.screen.resize(rows, cols);
        self.scroll_top = 0;
        self.scroll_bottom = rows;
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

    pub fn take_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.responses)
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
        let ch = if self.charsets[self.active_charset] {
            dec_graphics(ch)
        } else {
            ch
        };
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
        if self.cursor.pending_wrap && self.auto_wrap {
            self.wrap_line();
        }
        if width == 2 && self.cursor.position.col + 1 >= self.screen.cols() {
            if self.auto_wrap {
                self.wrap_line();
            } else {
                return;
            }
        }
        let row = self.cursor.position.row.min(self.screen.rows() - 1);
        let col = self.cursor.position.col.min(self.screen.cols() - 1);
        self.cursor.position = Position { row, col };
        if self.insert_mode {
            self.insert_chars(width);
        }
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
            self.cursor.pending_wrap = self.auto_wrap;
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
            b'\x0e' => self.active_charset = 1,
            b'\x0f' => self.active_charset = 0,
            _ => return,
        }
        self.cursor.pending_wrap = false;
    }

    fn esc(&mut self, esc: Esc) {
        if esc.intermediate_len == 1 && esc.intermediates[0] == b'(' {
            self.charsets[0] = esc.final_byte == b'0';
            return;
        }
        if esc.intermediate_len == 1 && esc.intermediates[0] == b')' {
            self.charsets[1] = esc.final_byte == b'0';
            return;
        }
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
            b'=' => self.application_keypad = true,
            b'>' => self.application_keypad = false,
            _ => return,
        }
        self.cursor.pending_wrap = false;
    }

    fn csi(&mut self, csi: Csi) {
        if !csi.intermediates.is_empty() {
            if csi.intermediates == b" " && csi.final_byte == b'q' {
                self.cursor_shape = match param(&csi, 0) {
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => CursorShape::Block,
                };
            }
            return;
        }
        if csi.private == Some(b'?') {
            if matches!(csi.final_byte, b'h' | b'l') {
                let enabled = csi.final_byte == b'h';
                for mode in csi
                    .params
                    .iter()
                    .filter_map(|p| p.subparams.first().copied().flatten())
                {
                    match mode {
                        25 => self.cursor.visible = enabled,
                        1 => self.application_cursor = enabled,
                        6 => {
                            self.origin_mode = enabled;
                            self.cursor.position = Position {
                                row: if enabled { self.scroll_top } else { 0 },
                                col: 0,
                            };
                        }
                        7 => self.auto_wrap = enabled,
                        2004 => self.bracketed_paste = enabled,
                        1000 => {
                            self.mouse_mode = if enabled {
                                MouseMode::Press
                            } else {
                                MouseMode::Off
                            }
                        }
                        1002 => {
                            self.mouse_mode = if enabled {
                                MouseMode::Drag
                            } else {
                                MouseMode::Off
                            }
                        }
                        1003 => {
                            self.mouse_mode = if enabled {
                                MouseMode::Motion
                            } else {
                                MouseMode::Off
                            }
                        }
                        1006 => self.sgr_mouse = enabled,
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
            if csi.private == Some(b'>') && csi.final_byte == b'c' {
                self.responses.push(b"\x1b[>0;0;0c".to_vec());
            }
            return;
        }
        let n = param_count(&csi, 0);
        let rows = self.screen.rows();
        let cols = self.screen.cols();
        let (top, bottom) = if self.origin_mode {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, rows)
        };
        let lower = bottom.saturating_sub(1);
        match csi.final_byte {
            b'A' => self.cursor.position.row = self.cursor.position.row.saturating_sub(n).max(top),
            b'B' | b'e' => {
                self.cursor.position.row = self.cursor.position.row.saturating_add(n).min(lower)
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
                self.cursor.position.row = self.cursor.position.row.saturating_add(n).min(lower);
                self.cursor.position.col = 0;
            }
            b'F' => {
                self.cursor.position.row = self.cursor.position.row.saturating_sub(n).max(top);
                self.cursor.position.col = 0;
            }
            b'G' | b'`' => {
                self.cursor.position.col = n.saturating_sub(1).min(cols.saturating_sub(1))
            }
            b'd' => self.cursor.position.row = top.saturating_add(n.saturating_sub(1)).min(lower),
            b'H' | b'f' => {
                self.cursor.position.row = top
                    .saturating_add(param_count(&csi, 0).saturating_sub(1))
                    .min(lower);
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
            b'S' => self
                .screen
                .scroll_up_region(self.scroll_top, self.scroll_bottom, n),
            b'T' => self
                .screen
                .scroll_down_region(self.scroll_top, self.scroll_bottom, n),
            b'I' => self.move_tabs(n, true),
            b'Z' => self.move_tabs(n, false),
            b'g' => self.clear_tab_stops(param(&csi, 0)),
            b'm' => self.sgr(&csi.params),
            b'h' | b'l' => {
                if csi
                    .params
                    .iter()
                    .any(|p| p.subparams.first() == Some(&Some(4)))
                {
                    self.insert_mode = csi.final_byte == b'h';
                }
            }
            b'r' => {
                let new_top = param_count(&csi, 0).saturating_sub(1);
                let new_bottom = if param(&csi, 1) == 0 {
                    rows
                } else {
                    usize::from(param(&csi, 1)).min(rows)
                };
                if new_top < new_bottom && new_bottom <= rows {
                    self.scroll_top = new_top;
                    self.scroll_bottom = new_bottom;
                    self.cursor.position = Position {
                        row: if self.origin_mode { new_top } else { 0 },
                        col: 0,
                    };
                }
            }
            b'n' => match param(&csi, 0) {
                5 => self.responses.push(b"\x1b[0n".to_vec()),
                6 => self.responses.push(
                    format!(
                        "\x1b[{};{}R",
                        self.cursor.position.row + 1
                            - if self.origin_mode { self.scroll_top } else { 0 },
                        self.cursor.position.col + 1
                    )
                    .into_bytes(),
                ),
                _ => {}
            },
            b'c' => self.responses.push(b"\x1b[?1;2c".to_vec()),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            _ => return,
        }
        if !matches!(csi.final_byte, b'm' | b'g') {
            self.cursor.pending_wrap = false;
        }
    }

    fn osc(&mut self, osc: Osc) {
        if matches!(osc.command, 0 | 2) {
            self.title = String::from_utf8_lossy(&osc.data).into_owned();
        }
    }

    fn dcs(&mut self, _dcs: Dcs) {}
}

fn dec_graphics(ch: char) -> char {
    match ch {
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'q' => '─',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'o' => '⎺',
        'p' => '⎻',
        'r' => '⎼',
        's' => '⎽',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => ch,
    }
}

fn param(csi: &Csi, index: usize) -> u16 {
    csi.params
        .get(index)
        .and_then(|p| p.subparams.first().copied().flatten())
        .unwrap_or(0)
}

fn param_count(csi: &Csi, index: usize) -> usize {
    usize::from(param(csi, index).max(1))
}
