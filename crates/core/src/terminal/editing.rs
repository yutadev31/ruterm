use super::Terminal;
use crate::screen::Cell;

impl Terminal {
    pub(super) fn erase_display(&mut self, mode: u16) {
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

    pub(super) fn erase_line(&mut self, mode: u16) {
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

    pub(super) fn erase_chars(&mut self, count: usize) {
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

    pub(super) fn erase_range(&mut self, start: usize, end: usize) {
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

    pub(super) fn insert_chars(&mut self, count: usize) {
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

    pub(super) fn delete_chars(&mut self, count: usize) {
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

    pub(super) fn insert_lines(&mut self, count: usize) {
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

    pub(super) fn delete_lines(&mut self, count: usize) {
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
}
