use crate::utf8::Utf8Decoder;

mod utf8;

#[derive(Debug)]
pub enum Action {
    Print(char),
    Control(u8),
    Esc(Esc),
    Csi(Csi),
    Osc(Osc),
}

#[derive(Debug)]
pub struct Esc {
    pub intermediates: [u8; 2],
    pub intermediate_len: usize,
    pub final_byte: u8,
}

#[derive(Debug)]
pub struct Csi {
    pub private: Option<u8>,
    pub params: Vec<Option<u16>>,
    pub intermediates: Vec<u8>,
    pub final_byte: u8,
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct Parser {
    state: State,
    utf8: Utf8Decoder,
    esc: EscParser,
    csi: CsiParser,
    osc: OscParser,
    callback: fn(Action),
}

impl Parser {
    pub fn new(callback: fn(Action)) -> Self {
        Self {
            state: State::default(),
            utf8: Utf8Decoder::default(),
            esc: EscParser::default(),
            csi: CsiParser::default(),
            osc: OscParser::default(),
            callback,
        }
    }

    fn emit(&mut self, action: Action) {
        (self.callback)(action);
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.advance_byte(byte);
        }
    }

    fn advance_byte(&mut self, byte: u8) {
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

    fn advance_ground(&mut self, byte: u8) {
        match byte {
            0x1b => {
                self.esc.reset();
                self.state = State::Escape;
            }

            0x00..=0x1f | 0x7f => {
                self.emit(Action::Control(byte));
            }

            _ => {
                if let Some(ch) = self.utf8.decode(byte) {
                    self.emit(Action::Print(ch));
                }
            }
        }
    }

    fn advance_escape(&mut self, byte: u8) {
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
                if let Some(esc) = self.esc.build(byte) {
                    self.emit(Action::Esc(esc));
                }
                self.state = State::Ground;
            }

            _ => {}
        }
    }

    fn advance_csi_entry(&mut self, byte: u8) {
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
                self.finish_csi(byte);
            }
            _ => {}
        }
    }

    fn advance_csi_param(&mut self, byte: u8) {
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
                self.finish_csi(byte);
            }
            _ => {}
        }
    }

    fn advance_csi_intermediate(&mut self, byte: u8) {
        match byte {
            0x30..=0x3f => {
                self.state = State::CsiIgnore;
            }
            0x20..=0x2f => {
                self.csi.push_intermediate(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => {
                self.finish_csi(byte);
            }
            _ => {}
        }
    }

    fn finish_csi(&mut self, byte: u8) {
        let csi = self.csi.build(byte);
        self.emit(Action::Csi(csi));

        self.state = State::Ground;
    }

    fn advance_csi_ignore(&mut self, byte: u8) {
        if let 0x40..=0x7e = byte {
            self.csi.reset();
            self.state = State::Ground;
        }
    }

    fn advance_osc_string(&mut self, byte: u8) {
        match byte {
            // BEL
            0x07 => {
                self.finish_osc();
            }
            // ESC
            0x1b => {
                self.state = State::OscEscape;
            }

            byte => {
                self.osc.push(byte);
            }
        }
    }

    fn advance_osc_escape(&mut self, byte: u8) {
        match byte {
            b'\\' => {
                self.finish_osc();
            }
            byte => {
                self.osc.push(byte);
            }
        }
    }

    fn finish_osc(&mut self) {
        if let Some(osc) = self.osc.build() {
            self.emit(Action::Osc(osc));
        }

        self.state = State::Ground;
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

        let esc = Esc {
            intermediates: self.intermediates.clone().try_into().unwrap(),
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
