#[derive(Debug, Default)]
pub struct Utf8Decoder {
    buf: [u8; 4],
    len: usize,
    expected: usize,
}

impl Utf8Decoder {
    pub fn decode(&mut self, byte: u8) -> Option<char> {
        if self.len == 0 {
            let Some(expected) = utf8_len(byte) else {
                return Some('\u{fffd}');
            };

            self.expected = expected;
        }

        self.buf[self.len] = byte;
        self.len += 1;

        if self.len < self.expected {
            return None;
        }

        let bytes = &self.buf[..self.expected];

        let ch = match std::str::from_utf8(bytes) {
            Ok(s) => s.chars().next().unwrap(),
            Err(_) => '\u{fffd}',
        };

        self.reset();

        Some(ch)
    }

    fn reset(&mut self) {
        self.len = 0;
        self.expected = 0;
    }
}

fn utf8_len(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}
