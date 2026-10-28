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
