use crate::attributes::Attributes;

#[derive(Debug)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Cell>,
}

#[derive(Debug)]
pub struct Cell {
    pub ch: char,
    pub attrs: Attributes,
}

impl Screen {
    pub fn set_cell(&mut self, row: usize, col: usize, cell: Cell) {
        self.cells[row * self.width + col] = cell;
    }

    pub fn rows(&self) -> usize {
        self.width
    }

    pub fn cols(&self) -> usize {
        self.height
    }

    pub fn scroll_up(&mut self, amount: usize) {
        todo!()
    }
}
