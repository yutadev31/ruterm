use crate::attributes::Attributes;

#[derive(Debug)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub attrs: Attributes,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
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
        }
    }

    pub fn set_cell(&mut self, row: usize, col: usize, cell: Cell) {
        self.cells[row * self.width + col] = cell;
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
        self.cells.copy_within(lines * self.width.., 0);
        self.cells[(self.height - lines) * self.width..].fill(Cell::default());
    }

    pub fn scroll_down(&mut self, amount: usize) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let lines = amount.min(self.height);
        self.cells
            .copy_within(..(self.height - lines) * self.width, lines * self.width);
        self.cells[..lines * self.width].fill(Cell::default());
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
    }
}
