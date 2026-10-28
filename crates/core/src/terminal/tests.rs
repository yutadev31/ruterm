use super::*;
use crate::attributes::{Color, Flags, UnderlineStyle};
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
fn margins_origin_and_line_edits_stay_inside_region() {
    let mut t = Terminal::new(4, 3);
    feed(&mut t, b"aaa\r\nbbb\r\nccc\r\nddd");
    feed(&mut t, b"\x1b[2;3r\x1b[?6h\x1b[2;1H\n");
    assert_eq!(row(&t, 0), "aaa");
    assert_eq!(row(&t, 1), "ccc");
    assert_eq!(row(&t, 2), "   ");
    assert_eq!(row(&t, 3), "ddd");
    feed(&mut t, b"\x1b[1;1H\x1b[L");
    assert_eq!(row(&t, 1), "   ");
    assert_eq!(row(&t, 2), "ccc");
}

#[test]
fn reports_position_status_and_identification() {
    let mut t = Terminal::new(4, 5);
    feed(&mut t, b"\x1b[2;3H\x1b[6n\x1b[5n\x1b[c\x1b[>c");
    assert_eq!(
        t.take_responses(),
        vec![
            b"\x1b[2;3R".to_vec(),
            b"\x1b[0n".to_vec(),
            b"\x1b[?1;2c".to_vec(),
            b"\x1b[>0;0;0c".to_vec()
        ]
    );
    assert!(t.take_responses().is_empty());
}

#[test]
fn modes_graphics_title_and_extended_sgr() {
    let mut t = Terminal::new(2, 5);
    feed(&mut t, b"\x1b[?1;7;2004h\x1b[4h\x1b=\x1b[?1002;1006h\x1b[5 q\x1b]2;hello\x07\x1b(0lqk\x1b(B\x1b[4:3;38:2::1:2:3;58;5;42mX");
    assert!(t.application_cursor && t.application_keypad && t.bracketed_paste && t.insert_mode);
    assert_eq!(t.mouse_mode, MouseMode::Drag);
    assert!(t.sgr_mouse);
    assert_eq!(t.cursor_shape, CursorShape::Bar);
    assert_eq!(t.title, "hello");
    assert_eq!(row(&t, 0), "┌─┐X ");
    assert_eq!(t.attrs.underline, UnderlineStyle::Curly);
    assert_eq!(t.attrs.fg, Color::Rgb(1, 2, 3));
    assert_eq!(t.attrs.underline_color, Color::Indexed(42));
    feed(&mut t, b"\x1b[59m\x1b[3J");
    assert_eq!(t.attrs.underline_color, Color::Default);
    assert!(t.screen.scrollback.is_empty());
}

#[test]
fn no_wrap_insert_and_scrollback_clear() {
    let mut t = Terminal::new(2, 3);
    feed(&mut t, b"abc\r\ndef\n");
    assert!(!t.screen.scrollback.is_empty());
    feed(&mut t, b"\x1b[3J\x1b[?7l\x1b[1;1HxyzQ");
    assert!(t.screen.scrollback.is_empty());
    assert_eq!(row(&t, 0), "xyQ");
    assert_eq!(t.cursor.position.row, 0);
    feed(&mut t, b"\r\x1b[4hZ");
    assert_eq!(row(&t, 0), "Zxy");
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

#[test]
fn alternate_screen_restores_normal_content_history_and_cursor() {
    let mut terminal = Terminal::new(2, 3);
    feed(&mut terminal, b"abc\r\ndef\r\ng\x1b[31m");
    let normal_cursor = terminal.cursor.position;
    let normal_history = terminal.screen.scrollback.clone();

    feed(&mut terminal, b"\x1b[?1049h");
    assert!(terminal.is_alternate_screen());
    assert_eq!(row(&terminal, 0), "   ");
    assert_eq!(terminal.cursor.position, Position::default());
    feed(&mut terminal, b"one\r\ntwo\r\nend\x1b[32m");
    assert!(terminal.screen.scrollback.is_empty());

    feed(&mut terminal, b"\x1b[?1049l");
    assert!(!terminal.is_alternate_screen());
    assert_eq!(row(&terminal, 0), "def");
    assert_eq!(row(&terminal, 1), "g  ");
    assert_eq!(terminal.screen.scrollback, normal_history);
    assert_eq!(terminal.cursor.position, normal_cursor);
    assert_eq!(terminal.attrs.fg, Color::Indexed(1));
}

#[test]
fn alternate_screen_modes_are_idempotent_and_resize_both_buffers() {
    let mut terminal = Terminal::new(1, 3);
    feed(&mut terminal, b"abc\x1b[?47hX\x1b[?47h");
    assert_eq!(row(&terminal, 0), "X  ");
    terminal.resize(2, 4);
    feed(&mut terminal, b"\x1b[?1047l");
    assert_eq!(row(&terminal, 0), "abc ");
    assert_eq!(terminal.screen.rows(), 2);
    feed(&mut terminal, b"\x1b[?47l");
    assert_eq!(row(&terminal, 0), "abc ");
}

#[test]
fn private_cursor_save_and_reset_leave_no_alternate_screen() {
    let mut terminal = Terminal::new(2, 3);
    feed(&mut terminal, b"\x1b[2;2H\x1b[?1048h\x1b[1;1H\x1b[?1048l");
    assert_eq!(terminal.cursor.position, Position { row: 1, col: 1 });
    feed(&mut terminal, b"abc\x1b[?1049hxyz\x1bc");
    assert!(!terminal.is_alternate_screen());
    assert_eq!(row(&terminal, 0), "   ");
    assert!(terminal.screen.scrollback.is_empty());
}

#[test]
fn alternate_cursor_saves_do_not_replace_normal_cursor_save() {
    let mut terminal = Terminal::new(2, 4);
    feed(
        &mut terminal,
        b"\x1b[1;2H\x1b7\x1b[?1049h\x1b[2;3H\x1b7\x1b[?1049l\x1b[2;4H\x1b8",
    );
    assert_eq!(terminal.cursor.position, Position { row: 0, col: 1 });
}

#[test]
fn dcs_payload_does_not_change_screen_or_cursor() {
    let mut terminal = Terminal::new(1, 8);
    feed(&mut terminal, b"A\x1bP1;2qhidden\x07\x1b\\B");
    assert_eq!(row(&terminal, 0), "AB      ");
    assert_eq!(terminal.cursor.position, Position { row: 0, col: 2 });
}
