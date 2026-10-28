use ruterm_parser::{Action, Csi, Esc, Osc};

use crate::{
    attributes::Attributes,
    common::Position,
    screen::{Cell, Screen},
};

#[derive(Debug)]
pub struct Terminal {
    pub screen: Screen,
    pub cursor: Cursor,
    pub attrs: Attributes,
}

impl Terminal {
    pub fn process(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.print(ch),
            Action::Control(ctrl) => self.control(ctrl),
            Action::Esc(esc) => self.esc(esc),
            Action::Csi(csi) => self.csi(csi),
            Action::Osc(osc) => self.osc(osc),
        }
    }

    fn print(&mut self, ch: char) {
        if self.cursor.pending_wrap {
            self.wrap_line();
            self.cursor.pending_wrap = false;
        }

        let row = self.cursor.position.row;
        let col = self.cursor.position.col;

        self.screen.set_cell(
            row,
            col,
            Cell {
                ch,
                attrs: self.attrs,
            },
        );

        if col + 1 == self.screen.cols() {
            self.cursor.pending_wrap = true;
        } else {
            self.cursor.position.col += 1;
        }
    }

    fn wrap_line(&mut self) {
        self.cursor.position.col = 0;

        if self.cursor.position.row + 1 < self.screen.rows() {
            self.cursor.position.row += 1;
        } else {
            self.screen.scroll_up(1);
        }
    }

    fn control(&mut self, ctrl: u8) {}

    fn esc(&mut self, esc: Esc) {}

    fn csi(&mut self, csi: Csi) {}

    fn osc(&mut self, osc: Osc) {}
}

#[derive(Debug)]
pub struct Cursor {
    pub position: Position,
    pub pending_wrap: bool,

    pub visible: bool,
}
