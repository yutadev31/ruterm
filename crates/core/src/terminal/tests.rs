use super::*;
use crate::attributes::{Color, Flags};
use ruterm_parser::Parser;

fn feed(terminal: &mut Terminal, input: &[u8]) {
    Parser::default().advance(input, |action| terminal.process(action));
}

fn row(terminal: &Terminal, index: usize) -> String {
    let start = index * terminal.screen.cols();
    terminal.screen.cells[start..start + terminal.screen.cols()]
        .iter()
        .map(|cell| cell.ch)
        .collect()
}

#[test]
fn prints_wraps_and_scrolls() {
    let mut terminal = Terminal::new(2, 3);
    feed(&mut terminal, b"abcdefg");
    assert_eq!(row(&terminal, 0), "def");
    assert_eq!(row(&terminal, 1), "g  ");
    assert_eq!(terminal.cursor.position, Position { row: 1, col: 1 });
}

#[test]
fn handles_cursor_movement_and_erasure() {
    let mut terminal = Terminal::new(2, 5);
    feed(&mut terminal, b"abcde\x1b[1;3H\x1b[0K");
    assert_eq!(row(&terminal, 0), "ab   ");
    feed(&mut terminal, b"Z\x1b[2;1Hq\x1b[2J");
    assert_eq!(row(&terminal, 0), "     ");
    assert_eq!(row(&terminal, 1), "     ");
}

#[test]
fn handles_sgr_colors_and_reset() {
    let mut terminal = Terminal::new(1, 4);
    feed(&mut terminal, b"\x1b[1;38;2;12;34;56;48;5;123mA\x1b[0mB");
    let first = terminal.screen.cells[0].clone();
    assert_eq!(first.attrs.fg, Color::Rgb(12, 34, 56));
    assert_eq!(first.attrs.bg, Color::Indexed(123));
    assert!(first.attrs.flags.contains(Flags::BOLD));
    assert_eq!(terminal.screen.cells[1].attrs, Attributes::default());
}

#[test]
fn saves_cursor_and_manages_tabs() {
    let mut terminal = Terminal::new(2, 20);
    feed(&mut terminal, b"\tX\x1b7\x1b[2;1H\x1b[31mY\x1b8Z");
    assert_eq!(terminal.screen.cells[8].ch, 'X');
    assert_eq!(terminal.screen.cells[9].ch, 'Z');
    assert_eq!(terminal.screen.cells[20].attrs.fg, Color::Indexed(1));
    assert_eq!(terminal.screen.cells[9].attrs.fg, Color::Default);
    feed(&mut terminal, b"\x1b[3g\r\t");
    assert_eq!(terminal.cursor.position.col, 19);
}

#[test]
fn supports_insert_and_delete() {
    let mut terminal = Terminal::new(1, 5);
    feed(&mut terminal, b"abcde\r\x1b[2C\x1b[@");
    assert_eq!(row(&terminal, 0), "ab cd");
    feed(&mut terminal, b"\x1b[P");
    assert_eq!(row(&terminal, 0), "abcd ");
}

#[test]
fn reverse_index_scrolls_down_at_top() {
    let mut terminal = Terminal::new(2, 3);
    feed(&mut terminal, b"abc\r\nxyz\x1b[1;1H\x1bM");
    assert_eq!(row(&terminal, 0), "   ");
    assert_eq!(row(&terminal, 1), "abc");
}

#[test]
fn empty_screen_is_safe() {
    let mut terminal = Terminal::new(0, 0);
    feed(&mut terminal, b"abc\n\x1b[2J\x1bc");
    assert!(terminal.screen.cells.is_empty());
}

#[test]
fn resize_preserves_visible_content_and_cursor() {
    let mut terminal = Terminal::new(2, 4);
    feed(&mut terminal, b"abc\x1b[2;4H!");
    terminal.resize(3, 5);
    assert_eq!(row(&terminal, 0), "abc  ");
    assert_eq!(terminal.screen.cells[8].ch, '!');
    assert_eq!(terminal.cursor.position, Position { row: 1, col: 3 });
    terminal.resize(1, 2);
    assert_eq!(row(&terminal, 0), "ab");
    assert_eq!(terminal.cursor.position, Position { row: 0, col: 1 });
}

#[test]
fn wide_and_combining_characters_use_expected_cells() {
    let mut terminal = Terminal::new(2, 4);
    feed(&mut terminal, "日e\u{301}".as_bytes());
    assert_eq!(terminal.screen.cells[0].ch, '日');
    assert!(terminal.screen.cells[1].wide_continuation);
    assert_eq!(terminal.screen.cells[2].ch, 'e');
    assert_eq!(terminal.screen.cells[2].combining, "\u{301}");
    assert_eq!(terminal.cursor.position.col, 3);
    let mut terminal = Terminal::new(2, 1);
    feed(&mut terminal, "a\u{301}".as_bytes());
    assert_eq!(terminal.screen.cells[0].combining, "\u{301}");
    assert_eq!(terminal.cursor.position.row, 0);
}

#[test]
fn scrollback_keeps_scrolled_lines() {
    let mut terminal = Terminal::new(2, 2);
    feed(&mut terminal, b"aa\r\nbb\r\ncc");
    assert_eq!(terminal.screen.scrollback.len(), 1);
    let view = terminal.screen.view_cells(1);
    assert_eq!(view[0].ch, 'a');
    assert_eq!(view[2].ch, 'b');
}
