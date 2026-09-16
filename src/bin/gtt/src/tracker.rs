use serde::{Deserialize, Serialize};

pub const PATTERN_TABLE_WIDTH: u16 = 117;
pub const PATTERN_BEATS: usize = 64;

pub type Pattern = [[Beat; PATTERN_BEATS]; 9];

pub fn empty_pattern() -> Pattern {
    std::array::from_fn(|_| std::array::from_fn(|_| Beat::default()))
}

pub const FX_ID_NONE: u8 = 0;
pub const FX_ID_INSTRUMENT: u8 = 1;
pub const FX_ID_ARPEGGIO: u8 = 2;
pub const MAX_INSTRUMENT_INDEX: u8 = 10;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Beat {
    pub cmd_list: Vec<ChannelCmd>,
    pub sqc: Option<SequencerCmd>,
}

impl Beat {
    // Returns the active channel Fx command (if any) as (fx_id, x, y)
    pub fn fx(&self) -> Option<(u8, u8, Option<u8>)> {
        self.cmd_list.iter().find_map(|c| match c {
            ChannelCmd::Instrument(x) => Some((FX_ID_INSTRUMENT, *x, None)),
            ChannelCmd::Arpeggio(x, y) => Some((FX_ID_ARPEGGIO, *x, *y)),
            _ => None,
        })
    }
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
    Instrument(u8),
    Arpeggio(u8, Option<u8>),
    // SlidePitch(u8, i16),
    // SlideVol(u8, i16),
    // StopPSlide,
    // StopVSlide,
    // Tremolo(u8, u8),
    // Vibrato(u8, u8),
    Volume(u8),
    Wavetable(u16),
}
