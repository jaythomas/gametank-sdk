use serde::{Deserialize, Serialize};

pub const PATTERN_TABLE_WIDTH: u16 = 117;
pub const PATTERN_BEATS: usize = 64;

pub type Pattern = [[Beat; PATTERN_BEATS]; 9];

pub fn empty_pattern() -> Pattern {
    std::array::from_fn(|_| std::array::from_fn(|_| Beat::default()))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Beat {
    pub cmd_list: Vec<ChannelCmd>,
    pub sqc: Option<SequencerCmd>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SequencerCmd {
    // Advance,
    // Beat(u8),
    // Load(u8, u16),
    // Pattern(u8),
    Stop,
    Tempo(u8),
    FxSpeed(u8),
    FlowCount(u8),
    CountJump(u8, u8),
    Jump(u8, u8),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelCmd {
    Note(String),
    NoteOff,
    Phase(u16),
    Arpeggio(u8, u8),
    // SlidePitch(u8, i16),
    // SlideVol(u8, i16),
    // StopPSlide,
    // StopVSlide,
    // Tremolo(u8, u8),
    // Vibrato(u8, u8),
    Volume(u8),
    Wavetable(u16),
}
