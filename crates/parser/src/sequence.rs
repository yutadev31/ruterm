use crate::{Csi, Dcs, Esc, Osc};

#[derive(Debug, Default)]
pub(crate) struct EscParser {
    intermediates: Vec<u8>,
}

impl EscParser {
    pub(crate) fn reset(&mut self) {
        self.intermediates.clear();
    }

    pub(crate) fn push(&mut self, byte: u8) {
        self.intermediates.push(byte);
    }

    pub(crate) fn build(&self, final_byte: u8) -> Option<Esc> {
        let intermediate_len = self.intermediates.len();
        if intermediate_len > 2 {
            return None;
        }

        let mut intermediates = [0; 2];
        intermediates[..intermediate_len].copy_from_slice(&self.intermediates);
        Some(Esc {
            intermediates,
            intermediate_len,
            final_byte,
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct CsiParser {
    private: Option<u8>,
    params: Vec<Option<u16>>,
    current_param: Option<u16>,
    intermediates: Vec<u8>,
}

impl CsiParser {
    pub(crate) fn reset(&mut self) {
        self.private = None;
        self.params.clear();
        self.current_param = None;
        self.intermediates.clear();
    }

    pub(crate) fn set_private(&mut self, byte: u8) {
        self.private = Some(byte);
    }

    pub(crate) fn push_digit(&mut self, byte: u8) {
        let digit = u16::from(byte - b'0');
        self.current_param = Some(
            self.current_param
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit),
        );
    }

    pub(crate) fn finish_param(&mut self) {
        self.params.push(self.current_param.take());
    }

    pub(crate) fn push_intermediate(&mut self, byte: u8) {
        self.intermediates.push(byte);
    }

    pub(crate) fn build(&self, final_byte: u8) -> Csi {
        Csi {
            private: self.private,
            params: self.params.clone(),
            intermediates: self.intermediates.clone(),
            final_byte,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct DcsParser {
    pub(crate) header: CsiParser,
    final_byte: u8,
    data: Vec<u8>,
}

impl DcsParser {
    pub(crate) fn reset(&mut self) {
        self.header.reset();
        self.final_byte = 0;
        self.data.clear();
    }

    pub(crate) fn set_final_byte(&mut self, byte: u8, has_param: bool) {
        if has_param {
            self.header.finish_param();
        }
        self.final_byte = byte;
    }

    pub(crate) fn push(&mut self, byte: u8) {
        self.data.push(byte);
    }

    pub(crate) fn build(&self) -> Dcs {
        let header = self.header.build(self.final_byte);
        Dcs {
            private: header.private,
            params: header.params,
            intermediates: header.intermediates,
            final_byte: header.final_byte,
            data: self.data.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct OscParser {
    data: Vec<u8>,
}

impl OscParser {
    pub(crate) fn reset(&mut self) {
        self.data.clear();
    }

    pub(crate) fn push(&mut self, byte: u8) {
        self.data.push(byte);
    }

    pub(crate) fn build(&self) -> Option<Osc> {
        let separator = self.data.iter().position(|&byte| byte == b';')?;
        let (command, rest) = self.data.split_at(separator);
        let data = &rest[1..];
        let command = std::str::from_utf8(command).ok()?.parse().ok()?;
        Some(Osc {
            command,
            data: data.to_vec(),
        })
    }
}
