#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    pub fg: Color,
    pub bg: Color,

    pub flags: Flags,
    pub underline: UnderlineStyle,
}

impl Default for Attributes {
    fn default() -> Self {
        Self {
            fg: Color::Default,
            bg: Color::Default,
            flags: Flags::empty(),
            underline: UnderlineStyle::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Flags: u16 {
        const BOLD          = 1 << 0;
        const FAINT         = 1 << 1;
        const ITALIC        = 1 << 2;
        const BLINK         = 1 << 3;
        const INVERSE       = 1 << 4;
        const HIDDEN        = 1 << 5;
        const STRIKETHROUGH = 1 << 6;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}
