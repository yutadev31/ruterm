use super::Terminal;
use crate::attributes::{Attributes, Color, Flags, UnderlineStyle};

impl Terminal {
    pub(super) fn sgr(&mut self, params: &[Option<u16>]) {
        let mut i = 0;
        while i < params.len().max(1) {
            let code = params.get(i).copied().flatten().unwrap_or(0);
            match code {
                0 => self.attrs = Attributes::default(),
                1 => self.attrs.flags.insert(Flags::BOLD),
                2 => self.attrs.flags.insert(Flags::FAINT),
                3 => self.attrs.flags.insert(Flags::ITALIC),
                4 => self.attrs.underline = UnderlineStyle::Single,
                5 | 6 => self.attrs.flags.insert(Flags::BLINK),
                7 => self.attrs.flags.insert(Flags::INVERSE),
                8 => self.attrs.flags.insert(Flags::HIDDEN),
                9 => self.attrs.flags.insert(Flags::STRIKETHROUGH),
                21 => self.attrs.underline = UnderlineStyle::Double,
                22 => self.attrs.flags.remove(Flags::BOLD | Flags::FAINT),
                23 => self.attrs.flags.remove(Flags::ITALIC),
                24 => self.attrs.underline = UnderlineStyle::None,
                25 => self.attrs.flags.remove(Flags::BLINK),
                27 => self.attrs.flags.remove(Flags::INVERSE),
                28 => self.attrs.flags.remove(Flags::HIDDEN),
                29 => self.attrs.flags.remove(Flags::STRIKETHROUGH),
                30..=37 => self.attrs.fg = Color::Indexed((code - 30) as u8),
                40..=47 => self.attrs.bg = Color::Indexed((code - 40) as u8),
                90..=97 => self.attrs.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.attrs.bg = Color::Indexed((code - 100 + 8) as u8),
                39 => self.attrs.fg = Color::Default,
                49 => self.attrs.bg = Color::Default,
                38 | 48 => {
                    if let Some((color, consumed)) =
                        parse_extended_color(params.get(i + 1..).unwrap_or(&[]))
                    {
                        if code == 38 {
                            self.attrs.fg = color;
                        } else {
                            self.attrs.bg = color;
                        }
                        i += consumed;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}
fn parse_extended_color(values: &[Option<u16>]) -> Option<(Color, usize)> {
    match values {
        [Some(5), index, ..] if index.unwrap_or(0) <= 255 => {
            Some((Color::Indexed(index.unwrap_or(0) as u8), 2))
        }
        [Some(2), r, g, b, ..]
            if r.unwrap_or(0) <= 255 && g.unwrap_or(0) <= 255 && b.unwrap_or(0) <= 255 =>
        {
            Some((
                Color::Rgb(
                    r.unwrap_or(0) as u8,
                    g.unwrap_or(0) as u8,
                    b.unwrap_or(0) as u8,
                ),
                4,
            ))
        }
        _ => None,
    }
}
