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

    fn control(&mut self, ctrl: u8) {
        match ctrl {
            b'\x08' => self.backspace(),
            b'\x09' => self.tab(),
            b'\x0a' | b'\x0b' | b'\x0c' => self.line_feed(),
            b'\x0d' => self.carriage_return(),
            _ => {}
        }

        self.cursor.pending_wrap = false;
    }

    fn esc(&mut self, esc: Esc) {
        match (&esc.intermediates, esc.final_byte) {
            (&[0, 0], b'D') => self.index(),
            (&[0, 0], b'E') => self.next_line(),
            (&[0, 0], b'M') => self.reverse_index(),

            (&[0, 0], b'7') => self.save_cursor(),
            (&[0, 0], b'8') => self.restore_cursor(),

            (&[0, 0], b'H') => self.set_tab_stop(),
            (&[0, 0], b'c') => self.reset(),

            _ => {}
        }
    }

    fn csi(&mut self, csi: Csi) {}

    fn osc(&mut self, osc: Osc) {}

    fn wrap_line(&mut self) {
        self.cursor.position.col = 0;
        self.line_feed();
    }

    fn backspace(&mut self) {
        self.cursor.position.col = self.cursor.position.col.saturating_sub(1);
    }

    fn tab(&mut self) {
        todo!()
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
        todo!()
    }

    fn next_line(&mut self) {
        todo!()
    }

    fn reverse_index(&mut self) {
        todo!()
    }

    fn save_cursor(&mut self) {
        todo!()
    }

    fn restore_cursor(&mut self) {
        todo!()
    }

    fn set_tab_stop(&mut self) {
        todo!()
    }

    fn reset(&mut self) {
        todo!()
    }
}

#[derive(Debug)]
pub struct Cursor {
    pub position: Position,
    pub pending_wrap: bool,

    pub visible: bool,
}
