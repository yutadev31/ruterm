use super::Terminal;
use crate::attributes::{Attributes, Color, Flags, UnderlineStyle};
use ruterm_parser::CsiParam;

impl Terminal {
    pub(super) fn sgr(&mut self, params: &[CsiParam]) {
        let mut values = Vec::new();
        let mut separators = Vec::new();

        for (index, param) in params.iter().enumerate() {
            if index > 0 {
                separators.push(b';');
            }

            for (subindex, value) in param.subparams.iter().copied().enumerate() {
                if subindex > 0 {
                    separators.push(b':');
                }
                values.push(value);
            }
        }

        self.sgr_flat(&values, &separators);
    }

    fn sgr_flat(&mut self, params: &[Option<u16>], separators: &[u8]) {
        let mut i = 0;

        while i < params.len().max(1) {
            let code = params.get(i).copied().flatten().unwrap_or(0);
            let colon_end = (i + 1..params.len())
                .find(|&j| separators.get(j - 1) != Some(&b':'))
                .unwrap_or(params.len());
            if colon_end > i + 1 {
                let values = &params[i + 1..colon_end];
                match code {
                    4 => {
                        self.attrs.underline = match values.first().copied().flatten().unwrap_or(1)
                        {
                            0 => UnderlineStyle::None,
                            2 => UnderlineStyle::Double,
                            3 => UnderlineStyle::Curly,
                            4 => UnderlineStyle::Dotted,
                            5 => UnderlineStyle::Dashed,
                            _ => UnderlineStyle::Single,
                        }
                    }
                    38 | 48 | 58 => {
                        let color = match values {
                            [Some(5), Some(index), ..] if *index <= 255 => {
                                Some(Color::Indexed(*index as u8))
                            }
                            [Some(2), rest @ ..] => {
                                let rgb = if rest.len() >= 4 && matches!(rest[0], None | Some(0)) {
                                    rest.get(1..4)
                                } else {
                                    rest.get(0..3)
                                };
                                rgb.and_then(|rgb| match rgb {
                                    [Some(r), Some(g), Some(b)]
                                        if *r <= 255 && *g <= 255 && *b <= 255 =>
                                    {
                                        Some(Color::Rgb(*r as u8, *g as u8, *b as u8))
                                    }
                                    _ => None,
                                })
                            }
                            _ => None,
                        };

                        if let Some(color) = color {
                            match code {
                                38 => self.attrs.fg = color,
                                48 => self.attrs.bg = color,
                                _ => self.attrs.underline_color = color,
                            }
                        }
                    }
                    _ => {}
                }

                i = colon_end;
                continue;
            }

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
                59 => self.attrs.underline_color = Color::Default,
                38 | 48 | 58 => {
                    if let Some((color, consumed)) =
                        parse_extended_color(params.get(i + 1..).unwrap_or(&[]))
                    {
                        if code == 38 {
                            self.attrs.fg = color;
                        } else if code == 48 {
                            self.attrs.bg = color;
                        } else {
                            self.attrs.underline_color = color;
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
