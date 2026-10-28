mod action;
mod sequence;
mod utf8;

pub use action::{Action, Csi, Dcs, Esc, Osc};
use sequence::{CsiParser, DcsParser, EscParser, OscParser};
use utf8::Utf8Decoder;

#[derive(Debug, Default, Clone, Copy)]
enum State {
    #[default]
    Ground,
    Escape,

    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,

    OscString,
    OscEscape,

    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsIgnore,
    DcsString,
    DcsEscape,
    DcsIgnoreEscape,
}

#[derive(Debug, Default)]
pub struct Parser {
    state: State,
    utf8: Utf8Decoder,
    esc: EscParser,
    csi: CsiParser,
    osc: OscParser,
    dcs: DcsParser,
}

impl Parser {
    pub fn advance<F>(&mut self, bytes: &[u8], mut callback: F)
    where
        F: FnMut(Action),
    {
        for &byte in bytes {
            if let Some(action) = self.advance_byte(byte) {
                callback(action)
            }
        }
    }

    fn advance_byte(&mut self, byte: u8) -> Option<Action> {
        match self.state {
            State::Ground => self.advance_ground(byte),
            State::Escape => self.advance_escape(byte),
            State::CsiEntry | State::CsiParam => self.advance_csi(byte),
            State::CsiIntermediate => self.advance_csi_intermediate(byte),
            State::CsiIgnore => self.advance_csi_ignore(byte),
            State::OscString => self.advance_osc_string(byte),
            State::OscEscape => self.advance_osc_escape(byte),
            State::DcsEntry | State::DcsParam => self.advance_dcs_header(byte),
            State::DcsIntermediate => self.advance_dcs_intermediate(byte),
            State::DcsIgnore => self.advance_dcs_ignore(byte),
            State::DcsString => self.advance_dcs_string(byte),
            State::DcsEscape => self.advance_dcs_escape(byte),
            State::DcsIgnoreEscape => self.advance_dcs_ignore_escape(byte),
        }
    }

    fn advance_ground(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x1b => {
                self.esc.reset();
                self.state = State::Escape;
            }

            0x00..=0x1f | 0x7f => {
                return Some(Action::Control(byte));
            }

            _ => {
                if let Some(ch) = self.utf8.decode(byte) {
                    return Some(Action::Print(ch));
                }
            }
        }

        None
    }

    fn advance_escape(&mut self, byte: u8) -> Option<Action> {
        match byte {
            // CSI
            b'[' => {
                self.csi.reset();
                self.state = State::CsiEntry;
            }

            // OSC
            b']' => {
                self.osc.reset();
                self.state = State::OscString;
            }

            // DCS
            b'P' => {
                self.dcs.reset();
                self.state = State::DcsEntry;
            }

            // ESC
            0x20..=0x2f => {
                self.esc.push(byte);
            }
            0x30..=0x7e => {
                self.state = State::Ground;
                return self.esc.build(byte).map(Action::Esc);
            }

            _ => {}
        }

        None
    }

    fn advance_csi(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x30..=0x39 => {
                self.csi.push_digit(byte);
                self.state = State::CsiParam;
            }
            b';' | b':' => {
                self.csi.finish_param();
                self.state = State::CsiParam;
            }
            b'<' | b'=' | b'>' | b'?' => {
                if matches!(self.state, State::CsiEntry) {
                    self.csi.set_private(byte);
                    self.state = State::CsiParam;
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x20..=0x2f => {
                self.csi.push_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => {
                return Some(self.finish_csi(byte));
            }
            _ => {}
        }

        None
    }

    fn advance_csi_intermediate(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x30..=0x3f => {
                self.state = State::CsiIgnore;
            }
            0x20..=0x2f => {
                self.csi.push_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => {
                return Some(self.finish_csi(byte));
            }
            _ => {}
        }

        None
    }

    fn finish_csi(&mut self, byte: u8) -> Action {
        if let State::CsiParam = self.state {
            self.csi.finish_param();
        }

        self.state = State::Ground;

        let csi = self.csi.build(byte);
        Action::Csi(csi)
    }

    fn advance_csi_ignore(&mut self, byte: u8) -> Option<Action> {
        if let 0x40..=0x7e = byte {
            self.csi.reset();
            self.state = State::Ground;
        }

        None
    }

    fn advance_osc_string(&mut self, byte: u8) -> Option<Action> {
        match byte {
            // BEL
            0x07 => return self.finish_osc(),
            // ESC
            0x1b => {
                self.state = State::OscEscape;
            }

            byte => {
                self.osc.push(byte);
            }
        }

        None
    }

    fn advance_osc_escape(&mut self, byte: u8) -> Option<Action> {
        match byte {
            b'\\' => return self.finish_osc(),
            byte => {
                self.osc.push(0x1b);
                self.osc.push(byte);
                self.state = State::OscString;
            }
        }

        None
    }

    fn finish_osc(&mut self) -> Option<Action> {
        self.state = State::Ground;

        self.osc.build().map(Action::Osc)
    }

    fn advance_dcs_header(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x18 | 0x1a => self.state = State::Ground,
            0x1b => {
                self.esc.reset();
                self.state = State::Escape;
            }
            b'0'..=b'9' => {
                self.dcs.header.push_digit(byte);
                self.state = State::DcsParam;
            }
            b';' | b':' => {
                self.dcs.header.finish_param();
                self.state = State::DcsParam;
            }
            b'<' | b'=' | b'>' | b'?' => {
                if matches!(self.state, State::DcsEntry) {
                    self.dcs.header.set_private(byte);
                    self.state = State::DcsParam;
                } else {
                    self.state = State::DcsIgnore;
                }
            }
            0x20..=0x2f => {
                if matches!(self.state, State::DcsParam) {
                    self.dcs.header.finish_param();
                }
                self.dcs.header.push_intermediate(byte);
                self.state = State::DcsIntermediate;
            }
            0x40..=0x7e => {
                self.dcs
                    .set_final_byte(byte, matches!(self.state, State::DcsParam));
                self.state = State::DcsString;
            }
            _ => {}
        }
        None
    }

    fn advance_dcs_intermediate(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x18 | 0x1a => self.state = State::Ground,
            0x1b => {
                self.esc.reset();
                self.state = State::Escape;
            }
            0x30..=0x3f => self.state = State::DcsIgnore,
            0x20..=0x2f => self.dcs.header.push_intermediate(byte),
            0x40..=0x7e => {
                self.dcs.set_final_byte(byte, false);
                self.state = State::DcsString;
            }
            _ => {}
        }
        None
    }

    fn advance_dcs_ignore(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x18 | 0x1a => self.state = State::Ground,
            0x1b => self.state = State::DcsIgnoreEscape,
            _ => {}
        }
        None
    }

    fn advance_dcs_ignore_escape(&mut self, byte: u8) -> Option<Action> {
        self.state = if byte == b'\\' || matches!(byte, 0x18 | 0x1a) {
            State::Ground
        } else if byte == 0x1b {
            State::DcsIgnoreEscape
        } else {
            State::DcsIgnore
        };
        None
    }

    fn advance_dcs_string(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x18 | 0x1a => self.state = State::Ground,
            0x1b => self.state = State::DcsEscape,
            _ => self.dcs.push(byte),
        }
        None
    }

    fn advance_dcs_escape(&mut self, byte: u8) -> Option<Action> {
        match byte {
            b'\\' => {
                self.state = State::Ground;
                return Some(Action::Dcs(self.dcs.build()));
            }
            0x18 | 0x1a => self.state = State::Ground,
            _ => {
                self.dcs.push(0x1b);
                self.dcs.push(byte);
                self.state = State::DcsString;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &[u8]) -> Vec<Action> {
        let mut parser = Parser::default();
        let mut actions = Vec::new();
        parser.advance(input, |action| actions.push(action));
        actions
    }

    #[test]
    fn prints_ascii() {
        assert_eq!(
            parse(b"abc"),
            vec![Action::Print('a'), Action::Print('b'), Action::Print('c'),],
        );
    }

    #[test]
    fn prints_utf8() {
        assert_eq!(
            parse("日本語".as_bytes()),
            vec![
                Action::Print('日'),
                Action::Print('本'),
                Action::Print('語'),
            ],
        );
    }

    #[test]
    fn parses_control() {
        assert_eq!(
            parse(b"\r\n"),
            vec![Action::Control(b'\r'), Action::Control(b'\n'),],
        );
    }

    #[test]
    fn parses_esc() {
        assert_eq!(
            parse(b"\x1b7"),
            vec![Action::Esc(Esc {
                intermediates: Default::default(),
                intermediate_len: 0,
                final_byte: b'7',
            }),],
        );
    }

    #[test]
    fn returns_to_ground_after_esc() {
        assert_eq!(
            parse(b"\x1b7A"),
            vec![
                Action::Esc(Esc {
                    intermediates: [0, 0],
                    intermediate_len: 0,
                    final_byte: b'7',
                }),
                Action::Print('A'),
            ],
        );
    }

    #[test]
    fn parses_esc_with_intermediate() {
        let actions = parse(b"\x1b(B");

        assert_eq!(actions.len(), 1);

        assert!(matches!(
            &actions[0],
            Action::Esc(Esc {
                final_byte: b'B',
                ..
            })
        ));
    }

    #[test]
    fn parses_csi_sgr() {
        assert_eq!(
            parse(b"\x1b[31m"),
            vec![Action::Csi(Csi {
                private: None,
                params: vec![Some(31)],
                intermediates: vec![],
                final_byte: b'm',
            }),],
        );
    }

    #[test]
    fn parses_csi_multiple_params() {
        assert_eq!(
            parse(b"\x1b[1;31;42m"),
            vec![Action::Csi(Csi {
                private: None,
                params: vec![Some(1), Some(31), Some(42),],
                intermediates: vec![],
                final_byte: b'm',
            }),],
        );
    }

    #[test]
    fn parses_empty_csi_params() {
        assert_eq!(
            parse(b"\x1b[;31m"),
            vec![Action::Csi(Csi {
                private: None,
                params: vec![None, Some(31),],
                intermediates: vec![],
                final_byte: b'm',
            }),],
        );
    }

    #[test]
    fn parses_private_csi() {
        assert_eq!(
            parse(b"\x1b[?25h"),
            vec![Action::Csi(Csi {
                private: Some(b'?'),
                params: vec![Some(25)],
                intermediates: vec![],
                final_byte: b'h',
            }),],
        );
    }

    #[test]
    fn parses_osc_with_bel_and_st() {
        let osc = Action::Osc(Osc {
            command: 0,
            data: b"title".to_vec(),
        });
        assert_eq!(
            parse(b"\x1b]0;title\x07A"),
            vec![osc.clone(), Action::Print('A')]
        );
        assert_eq!(parse(b"\x1b]0;title\x1b\\A"), vec![osc, Action::Print('A')]);
    }

    #[test]
    fn keeps_osc_escape_in_data_when_not_terminator() {
        assert_eq!(
            parse(b"\x1b]0;a\x1bb\x07"),
            vec![Action::Osc(Osc {
                command: 0,
                data: b"a\x1bb".to_vec(),
            })],
        );
    }

    #[test]
    fn parses_sequences_across_calls() {
        let mut parser = Parser::default();
        let mut actions = Vec::new();
        for chunk in [b"\x1b[?".as_slice(), b"25h\x1b]0;ti", b"tle\x1b\\"] {
            parser.advance(chunk, |action| actions.push(action));
        }
        assert_eq!(
            actions,
            vec![
                Action::Csi(Csi {
                    private: Some(b'?'),
                    params: vec![Some(25)],
                    intermediates: vec![],
                    final_byte: b'h',
                }),
                Action::Osc(Osc {
                    command: 0,
                    data: b"title".to_vec(),
                }),
            ],
        );
    }

    #[test]
    fn ignores_invalid_csi_and_resumes_printing() {
        assert_eq!(parse(b"\x1b[1?25hA"), vec![Action::Print('A')]);
    }

    #[test]
    fn parses_dcs_header_and_binary_payload() {
        assert_eq!(
            parse(b"\x1bP?1;23 $qabc\x07\xff\x1b\\Z"),
            vec![
                Action::Dcs(Dcs {
                    private: Some(b'?'),
                    params: vec![Some(1), Some(23)],
                    intermediates: b" $".to_vec(),
                    final_byte: b'q',
                    data: b"abc\x07\xff".to_vec(),
                }),
                Action::Print('Z'),
            ]
        );
    }

    #[test]
    fn parses_dcs_across_calls_and_keeps_non_terminating_escape() {
        let mut parser = Parser::default();
        let mut actions = Vec::new();
        for chunk in [b"\x1bP1;".as_slice(), b"2qhello\x1bX", b"!\x1b", b"\\A"] {
            parser.advance(chunk, |action| actions.push(action));
        }
        assert_eq!(
            actions,
            vec![
                Action::Dcs(Dcs {
                    private: None,
                    params: vec![Some(1), Some(2)],
                    intermediates: vec![],
                    final_byte: b'q',
                    data: b"hello\x1bX!".to_vec(),
                }),
                Action::Print('A'),
            ]
        );
    }

    #[test]
    fn ignores_invalid_dcs_until_st_and_recovers_after_cancel() {
        assert_eq!(parse(b"\x1bP1?qa\x1b\\B"), vec![Action::Print('B')]);
        assert_eq!(parse(b"\x1bPqabc\x18C"), vec![Action::Print('C')]);
    }
}
