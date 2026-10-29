mod action;
mod sequence;
mod utf8;

pub use action::{Action, Csi, CsiParam, Dcs, Esc, Osc};
use sequence::{CsiParser, DcsParser, EscParser, OscParser};
use utf8::Utf8Decoder;

const MAX_SEQUENCE_BYTES: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIgnore,

    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,

    OscString,
    OscEscape,
    OscIgnore,
    OscIgnoreEscape,

    StringIgnore,
    StringIgnoreEscape,

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
    sequence_bytes: usize,
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
        if !matches!(self.state, State::Ground) {
            self.sequence_bytes = self.sequence_bytes.saturating_add(1);
            if self.sequence_bytes > MAX_SEQUENCE_BYTES {
                self.ignore_oversized_sequence();
            }
        }

        if matches!(byte, 0x18 | 0x1a) && !matches!(self.state, State::Ground) {
            self.state = State::Ground;
            return None;
        }

        if byte == 0x1b
            && matches!(
                self.state,
                State::Escape
                    | State::EscapeIgnore
                    | State::CsiEntry
                    | State::CsiParam
                    | State::CsiIntermediate
                    | State::CsiIgnore
            )
        {
            self.esc.reset();
            self.sequence_bytes = 0;
            self.state = State::Escape;
            return None;
        }

        match self.state {
            State::Ground => self.advance_ground(byte),
            State::Escape => self.advance_escape(byte),
            State::EscapeIgnore => self.advance_escape_ignore(byte),
            State::CsiEntry | State::CsiParam => self.advance_csi(byte),
            State::CsiIntermediate => self.advance_csi_intermediate(byte),
            State::CsiIgnore => self.advance_csi_ignore(byte),
            State::OscString => self.advance_osc_string(byte),
            State::OscEscape => self.advance_osc_escape(byte),
            State::OscIgnore => self.advance_osc_ignore(byte),
            State::OscIgnoreEscape => self.advance_osc_ignore_escape(byte),
            State::StringIgnore => self.advance_string_ignore(byte),
            State::StringIgnoreEscape => self.advance_string_ignore_escape(byte),
            State::DcsEntry | State::DcsParam => self.advance_dcs_header(byte),
            State::DcsIntermediate => self.advance_dcs_intermediate(byte),
            State::DcsIgnore => self.advance_dcs_ignore(byte),
            State::DcsString => self.advance_dcs_string(byte),
            State::DcsEscape => self.advance_dcs_escape(byte),
            State::DcsIgnoreEscape => self.advance_dcs_ignore_escape(byte),
        }
    }

    fn ignore_oversized_sequence(&mut self) {
        self.state = match self.state {
            State::Escape => {
                self.esc.reset();
                State::EscapeIgnore
            }
            State::CsiEntry | State::CsiParam | State::CsiIntermediate => {
                self.csi.reset();
                State::CsiIgnore
            }
            State::OscString => {
                self.osc.reset();
                State::OscIgnore
            }
            State::OscEscape => {
                self.osc.reset();
                State::OscIgnoreEscape
            }
            State::DcsEntry | State::DcsParam | State::DcsIntermediate | State::DcsString => {
                self.dcs.reset();
                State::DcsIgnore
            }
            State::DcsEscape => {
                self.dcs.reset();
                State::DcsIgnoreEscape
            }
            state => state,
        };
    }

    fn advance_ground(&mut self, byte: u8) -> Option<Action> {
        match byte {
            // ESC
            0x1b => {
                self.utf8.reset();
                self.esc.reset();
                self.sequence_bytes = 0;
                self.state = State::Escape;
            }

            // C0 Control
            0x00..=0x1f | 0x7f => {
                self.utf8.reset();
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
            b'[' if self.esc.is_empty() => {
                self.csi.reset();
                self.state = State::CsiEntry;
            }

            // OSC
            b']' if self.esc.is_empty() => {
                self.osc.reset();
                self.state = State::OscString;
            }

            // DCS
            b'P' if self.esc.is_empty() => {
                self.dcs.reset();
                self.state = State::DcsEntry;
            }

            // String Ignore
            b'X' | b'^' | b'_' if self.esc.is_empty() => {
                self.state = State::StringIgnore;
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

    fn advance_escape_ignore(&mut self, byte: u8) -> Option<Action> {
        if (0x30..=0x7e).contains(&byte) {
            self.state = State::Ground;
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
                self.csi.separator(byte);
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
                if matches!(self.state, State::CsiParam) {
                    self.csi.finish_param();
                }
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

    fn advance_osc_ignore(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x07 => self.state = State::Ground,
            0x1b => self.state = State::OscIgnoreEscape,
            _ => {}
        }
        None
    }

    fn advance_osc_ignore_escape(&mut self, byte: u8) -> Option<Action> {
        self.state = match byte {
            b'\\' | 0x07 => State::Ground,
            0x1b => State::OscIgnoreEscape,
            _ => State::OscIgnore,
        };
        None
    }

    fn advance_string_ignore(&mut self, byte: u8) -> Option<Action> {
        if byte == 0x1b {
            self.state = State::StringIgnoreEscape;
        }
        None
    }

    fn advance_string_ignore_escape(&mut self, byte: u8) -> Option<Action> {
        self.state = match byte {
            b'\\' => State::Ground,
            0x1b => State::StringIgnoreEscape,
            _ => State::StringIgnore,
        };
        None
    }

    fn advance_dcs_header(&mut self, byte: u8) -> Option<Action> {
        match byte {
            0x18 | 0x1a => self.state = State::Ground,
            0x1b => {
                self.esc.reset();
                self.sequence_bytes = 0;
                self.state = State::Escape;
            }
            b'0'..=b'9' => {
                self.dcs.header.push_digit(byte);
                self.state = State::DcsParam;
            }
            b';' | b':' => {
                self.dcs.header.separator(byte);
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
                self.sequence_bytes = 0;
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
                params: vec![CsiParam {
                    subparams: vec![Some(31)]
                }],
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
                params: vec![
                    CsiParam {
                        subparams: vec![Some(1)]
                    },
                    CsiParam {
                        subparams: vec![Some(31)]
                    },
                    CsiParam {
                        subparams: vec![Some(42)]
                    }
                ],
                intermediates: vec![],
                final_byte: b'm',
            }),],
        );
    }

    #[test]
    fn groups_colon_subparameters_without_merging_semicolon_parameters() {
        assert_eq!(
            parse(b"\x1b[4:3;38:2::1:2:3m"),
            vec![Action::Csi(Csi {
                private: None,
                params: vec![
                    CsiParam {
                        subparams: vec![Some(4), Some(3)]
                    },
                    CsiParam {
                        subparams: vec![Some(38), Some(2), None, Some(1), Some(2), Some(3)]
                    },
                ],
                intermediates: vec![],
                final_byte: b'm',
            })]
        );
    }

    #[test]
    fn parses_empty_csi_params() {
        assert_eq!(
            parse(b"\x1b[;31m"),
            vec![Action::Csi(Csi {
                private: None,
                params: vec![
                    CsiParam {
                        subparams: vec![None]
                    },
                    CsiParam {
                        subparams: vec![Some(31)]
                    }
                ],
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
                params: vec![CsiParam {
                    subparams: vec![Some(25)]
                }],
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
                    params: vec![CsiParam {
                        subparams: vec![Some(25)]
                    }],
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

    #[test]
    fn restarts_or_cancels_csi() {
        assert_eq!(
            parse(b"\x1b[1\x1b[31mA"),
            vec![
                Action::Csi(Csi {
                    private: None,
                    params: vec![CsiParam {
                        subparams: vec![Some(31)]
                    }],
                    intermediates: vec![],
                    final_byte: b'm',
                }),
                Action::Print('A'),
            ]
        );
        for cancel in [0x18, 0x1a] {
            assert_eq!(
                parse(&[b'\x1b', b'[', b'1', cancel, b'A']),
                vec![Action::Print('A')]
            );
        }
    }

    #[test]
    fn cancels_osc() {
        for cancel in [0x18, 0x1a] {
            assert_eq!(
                parse(&[b'\x1b', b']', b'0', b';', b'x', cancel, b'A']),
                vec![Action::Print('A')]
            );
        }
    }

    #[test]
    fn resets_incomplete_utf8_on_control_and_resynchronizes() {
        assert_eq!(
            parse(b"\xe2\x1b[31mABC"),
            vec![
                Action::Csi(Csi {
                    private: None,
                    params: vec![CsiParam {
                        subparams: vec![Some(31)]
                    }],
                    intermediates: vec![],
                    final_byte: b'm',
                }),
                Action::Print('A'),
                Action::Print('B'),
                Action::Print('C'),
            ]
        );
        assert_eq!(
            parse(b"\xe2ABC"),
            vec![Action::Print('A'), Action::Print('B'), Action::Print('C')]
        );
    }

    #[test]
    fn new_escape_discards_old_intermediates() {
        assert_eq!(
            parse(b"\x1b(\x1b7"),
            vec![Action::Esc(Esc {
                intermediates: [0, 0],
                intermediate_len: 0,
                final_byte: b'7',
            })]
        );
    }

    #[test]
    fn escape_introducers_only_apply_without_intermediates() {
        assert_eq!(
            parse(b"\x1b([A"),
            vec![
                Action::Esc(Esc {
                    intermediates: [b'(', 0],
                    intermediate_len: 1,
                    final_byte: b'[',
                }),
                Action::Print('A'),
            ]
        );
    }

    #[test]
    fn ignores_sos_pm_and_apc_strings() {
        for introducer in *b"X^_" {
            assert_eq!(
                parse(&[
                    b'\x1b', introducer, b'a', b'\x1b', b'Q', b'b', b'\x1b', b'\\', b'Z'
                ]),
                vec![Action::Print('Z')]
            );
        }
    }

    #[test]
    fn discards_oversized_sequences_and_recovers() {
        for introducer in *b"]P[(" {
            let mut input = vec![b'\x1b', introducer];
            input.extend(std::iter::repeat_n(
                if introducer == b'(' { b'(' } else { b'1' },
                MAX_SEQUENCE_BYTES,
            ));
            match introducer {
                b']' => input.extend_from_slice(b"\x07Z"),
                b'P' => input.extend_from_slice(b"\x1b\\Z"),
                b'[' => input.extend_from_slice(b"mZ"),
                _ => input.extend_from_slice(b"AZ"),
            }
            assert_eq!(parse(&input), vec![Action::Print('Z')]);
        }
    }

    #[test]
    fn oversized_string_can_end_at_st_boundary() {
        for introducer in *b"]P" {
            let mut input = vec![b'\x1b', introducer];
            if introducer == b'P' {
                input.push(b'q');
            }
            let header_len = if introducer == b'P' { 2 } else { 1 };
            input.extend(std::iter::repeat_n(
                b'a',
                MAX_SEQUENCE_BYTES - header_len - 1,
            ));
            input.extend_from_slice(b"\x1b\\Z");
            assert_eq!(parse(&input), vec![Action::Print('Z')]);
        }
    }
}
