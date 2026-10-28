use crate::attributes::Attributes;
use std::collections::VecDeque;
use std::ops::Range;

pub(crate) fn clone_within(cells: &mut [Cell], source: Range<usize>, destination: usize) {
    if destination > source.start {
        for offset in (0..source.len()).rev() {
            cells[destination + offset] = cells[source.start + offset].clone();
        }
    } else {
        for offset in 0..source.len() {
            cells[destination + offset] = cells[source.start + offset].clone();
        }
    }
}

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
        for row in 0..lines {
            self.scrollback
                .push_back(self.cells[row * self.width..(row + 1) * self.width].to_vec());
        }
        while self.scrollback.len() > 1000 {
            self.scrollback.pop_front();
        }
        let len = self.cells.len();
        clone_within(&mut self.cells, lines * self.width..len, 0);
        self.cells[(self.height - lines) * self.width..].fill(Cell::default());
    }

    pub fn scroll_down(&mut self, amount: usize) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let lines = amount.min(self.height);
        clone_within(
            &mut self.cells,
            0..(self.height - lines) * self.width,
            lines * self.width,
        );
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
