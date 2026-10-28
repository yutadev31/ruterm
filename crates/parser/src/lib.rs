use crate::utf8::Utf8Decoder;

mod utf8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Print(char),
    Control(u8),
    Esc(Esc),
    Csi(Csi),
    Osc(Osc),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Esc {
    pub intermediates: [u8; 2],
    pub intermediate_len: usize,
    pub final_byte: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Csi {
    pub private: Option<u8>,
    pub params: Vec<Option<u16>>,
    pub intermediates: Vec<u8>,
    pub final_byte: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc {
    pub command: u16,
    pub data: Vec<u8>,
}

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
}

#[derive(Debug, Default)]
pub struct Parser {
    state: State,
    utf8: Utf8Decoder,
    esc: EscParser,
    csi: CsiParser,
    osc: OscParser,
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
            State::CsiEntry => self.advance_csi_entry(byte),
            State::CsiParam => self.advance_csi_param(byte),
            State::CsiIntermediate => self.advance_csi_intermediate(byte),
            State::CsiIgnore => self.advance_csi_ignore(byte),
            State::OscString => self.advance_osc_string(byte),
            State::OscEscape => self.advance_osc_escape(byte),
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

    fn advance_csi_entry(&mut self, byte: u8) -> Option<Action> {
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
                self.csi.set_private(byte);
                self.state = State::CsiParam;
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

    fn advance_csi_param(&mut self, byte: u8) -> Option<Action> {
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
            0x07 => {
                self.finish_osc()?;
            }
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
            b'\\' => {
                self.finish_osc()?;
            }
            byte => {
                self.osc.push(byte);
            }
        }

        None
    }

    fn finish_osc(&mut self) -> Option<Action> {
        self.state = State::Ground;

        self.osc.build().map(Action::Osc)
    }
}

#[derive(Debug, Default)]
struct EscParser {
    intermediates: Vec<u8>,
}

impl EscParser {
    fn reset(&mut self) {
        self.intermediates.clear();
    }

    fn push(&mut self, byte: u8) {
        self.intermediates.push(byte);
    }

    fn build(&mut self, final_byte: u8) -> Option<Esc> {
        let intermediate_len = self.intermediates.len();

        if intermediate_len > 2 {
            self.intermediates.clear();
            return None;
        }

        let mut intermediates = [0; 2];
        intermediates[..intermediate_len].copy_from_slice(&self.intermediates);

        let esc = Esc {
            intermediates,
            intermediate_len,
            final_byte,
        };

        Some(esc)
    }
}

#[derive(Debug, Default)]
struct CsiParser {
    private: Option<u8>,
    params: Vec<Option<u16>>,
    current_param: Option<u16>,
    intermediates: Vec<u8>,
}

impl CsiParser {
    fn reset(&mut self) {
        self.private = None;
        self.params.clear();
        self.current_param = None;
        self.intermediates.clear();
    }

    fn set_private(&mut self, byte: u8) {
        self.private = Some(byte);
    }

    fn push_digit(&mut self, byte: u8) {
        let digit = u16::from(byte - b'0');

        self.current_param = Some(
            self.current_param
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit),
        );
    }

    fn finish_param(&mut self) {
        self.params.push(self.current_param.take());
    }

    fn push_intermediate(&mut self, byte: u8) {
        self.intermediates.push(byte);
    }

    fn build(&mut self, final_byte: u8) -> Csi {
        Csi {
            private: self.private,
            params: self.params.clone(),
            intermediates: self.intermediates.clone(),
            final_byte,
        }
    }
}

#[derive(Debug, Default)]
struct OscParser {
    data: Vec<u8>,
}

impl OscParser {
    fn reset(&mut self) {
        self.data.clear();
    }

    fn push(&mut self, byte: u8) {
        self.data.push(byte);
    }

    fn build(&mut self) -> Option<Osc> {
        let mut parts = self.data.splitn(2, |&b| b == b';');

        let command = parts.next().unwrap();
        let data = parts.next()?;

        let command = std::str::from_utf8(command)
            .ok()
            .and_then(|s| s.parse::<u16>().ok())?;

        Some(Osc {
            command,
            data: data.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static ACTIONS: RefCell<Vec<Action>> = const {
            RefCell::new(Vec::new())
        };
    }

    fn callback(action: Action) {
        ACTIONS.with(|actions| {
            actions.borrow_mut().push(action);
        });
    }

    fn parse(input: &[u8]) -> Vec<Action> {
        ACTIONS.with(|actions| actions.borrow_mut().clear());

        let mut parser = Parser::default();
        parser.advance(input, callback);

        ACTIONS.with(|actions| actions.borrow().clone())
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
}
