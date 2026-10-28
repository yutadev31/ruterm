use super::Terminal;
use crate::{attributes::Attributes, common::Position};

#[derive(Debug)]
pub struct Cursor {
    pub position: Position,
    pub pending_wrap: bool,
    pub visible: bool,
}

impl Terminal {
    pub(super) fn wrap_line(&mut self) {
        self.cursor.position.col = 0;
        self.line_feed();
        self.cursor.pending_wrap = false;
    }

    pub(super) fn backspace(&mut self) {
        self.cursor.position.col = self.cursor.position.col.saturating_sub(1);
    }

    pub(super) fn tab(&mut self) {
        self.move_tabs(1, true);
    }

    pub(super) fn move_tabs(&mut self, count: usize, forward: bool) {
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

    pub(super) fn line_feed(&mut self) {
        if self.cursor.position.row + 1 < self.screen.rows() {
            self.cursor.position.row += 1;
        } else {
            self.screen.scroll_up(1);
        }
    }

    pub(super) fn carriage_return(&mut self) {
        self.cursor.position.col = 0;
    }

    pub(super) fn index(&mut self) {
        self.line_feed();
    }

    pub(super) fn next_line(&mut self) {
        self.line_feed();
        self.carriage_return();
    }

    pub(super) fn reverse_index(&mut self) {
        if self.cursor.position.row == 0 {
            self.screen.scroll_down(1);
        } else {
            self.cursor.position.row -= 1;
        }
    }

    pub(super) fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cursor.position, self.attrs));
    }

    pub(super) fn restore_cursor(&mut self) {
        if let Some((position, attrs)) = self.saved_cursor {
            self.cursor.position = Position {
                row: position.row.min(self.screen.rows().saturating_sub(1)),
                col: position.col.min(self.screen.cols().saturating_sub(1)),
            };
            self.attrs = attrs;
        }
    }

    pub(super) fn set_tab_stop(&mut self) {
        if let Some(stop) = self.tab_stops.get_mut(self.cursor.position.col) {
            *stop = true;
        }
    }

    pub(super) fn clear_tab_stops(&mut self, mode: u16) {
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

    pub(super) fn reset(&mut self) {
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
}
