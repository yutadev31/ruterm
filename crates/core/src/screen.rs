use crate::attributes::Attributes;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub combining: String,
    pub wide_continuation: bool,
    pub attrs: Attributes,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            combining: String::new(),
            wide_continuation: false,
            attrs: Attributes::default(),
        }
    }
}

impl Screen {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            width: cols,
            height: rows,
            cells: vec![Cell::default(); rows.saturating_mul(cols)],
            scrollback: VecDeque::new(),
        }
    }

    pub fn set_cell(&mut self, row: usize, col: usize, cell: Cell) {
        self.cells[row * self.width + col] = cell;
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let mut cells = vec![Cell::default(); rows.saturating_mul(cols)];
        for row in 0..rows.min(self.height) {
            for col in 0..cols.min(self.width) {
                cells[row * cols + col] = self.cells[row * self.width + col].clone();
            }
            if cols > 0 && cells[row * cols + cols - 1].wide_continuation {
                cells[row * cols + cols - 1] = Cell::default();
            }
            if cols > 0
                && cols < self.width
                && self.cells[row * self.width + cols].wide_continuation
            {
                cells[row * cols + cols - 1] = Cell::default();
            }
        }
        self.width = cols;
        self.height = rows;
        self.cells = cells;
        for line in &mut self.scrollback {
            line.resize(cols, Cell::default());
        }
    }

    pub fn rows(&self) -> usize {
        self.height
    }

    pub fn cols(&self) -> usize {
        self.width
    }

    pub fn scroll_up(&mut self, amount: usize) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let lines = amount.min(self.height);
        let mut removed = self.cells.drain(..lines * self.width);
        for _ in 0..lines {
            self.scrollback
                .push_back(removed.by_ref().take(self.width).collect());
        }
        drop(removed);
        while self.scrollback.len() > 1000 {
            self.scrollback.pop_front();
        }
        self.cells
            .extend(std::iter::repeat_with(Cell::default).take(lines * self.width));
    }

    pub fn scroll_down(&mut self, amount: usize) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let lines = amount.min(self.height);
        self.cells.rotate_right(lines * self.width);
        self.cells[..lines * self.width].fill(Cell::default());
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
    }

    pub fn view_cells(&self, offset: usize) -> Vec<Cell> {
        let offset = offset.min(self.scrollback.len());
        let start = self.scrollback.len() - offset;
        (start..start + self.height)
            .flat_map(|row| {
                if row < self.scrollback.len() {
                    self.scrollback[row].clone()
                } else {
                    let row = row - self.scrollback.len();
                    self.cells[row * self.width..(row + 1) * self.width].to_vec()
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_multiple_lines_moves_cells_and_preserves_scrollback() {
        let mut screen = Screen::new(4, 1);
        for (cell, ch) in screen.cells.iter_mut().zip(['a', 'b', 'c', 'd']) {
            cell.ch = ch;
            cell.combining.push('\u{301}');
        }
        screen.scroll_up(2);
        assert_eq!(screen.scrollback[0][0].ch, 'a');
        assert_eq!(screen.scrollback[1][0].ch, 'b');
        assert_eq!(
            screen.cells.iter().map(|cell| cell.ch).collect::<String>(),
            "cd  "
        );
        assert_eq!(screen.cells[0].combining, "\u{301}");
        screen.scroll_down(2);
        assert_eq!(
            screen.cells.iter().map(|cell| cell.ch).collect::<String>(),
            "  cd"
        );
        assert_eq!(screen.cells[2].combining, "\u{301}");
    }
}
