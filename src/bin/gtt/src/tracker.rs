use serde::{Deserialize, Serialize};

pub const PATTERN_TABLE_WIDTH: u16 = 104;
pub const PATTERN_BEATS: usize = 64;

pub type Pattern = [[Beat; PATTERN_BEATS]; 8];

pub fn empty_pattern() -> Pattern {
    std::array::from_fn(|_| std::array::from_fn(|_| Beat::default()))
}

pub const FX_ID_NONE: u8 = 0;
pub const FX_ID_INSTRUMENT: u8 = 1;
pub const FX_ID_ARPEGGIO: u8 = 2;
pub const FX_ID_PITCH_UP: u8 = 3;
pub const FX_ID_PITCH_DOWN: u8 = 4;
pub const FX_ID_ENVELOPE: u8 = 5;
pub const MAX_FX_ID: u8 = FX_ID_ENVELOPE;
pub const MAX_INSTRUMENT_INDEX: u8 = crate::file::NUM_INSTRUMENTS as u8 - 1;
pub const WAVEFORM_SAMPLES: usize = 256;
pub const BEAT_THRESHOLD: u16 = 3600;
pub const MAX_TEMPO: u16 = 1800;

pub const NUM_ENVELOPES: usize = 8;
pub const MAX_ENVELOPE_INDEX: u8 = NUM_ENVELOPES as u8 - 1;
pub const ENVELOPE_MAX_STEPS: usize = 32;

/// Resolve an Envelope Fx parameter to a slot. Anything above the last
/// assigned slot leaves the channel without an envelope.
pub fn envelope_slot(param: u8) -> Option<usize> {
    (param <= MAX_ENVELOPE_INDEX).then_some(param as usize)
}

/// Number of distinct volume levels the firmware provides. Levels are 1.3 dB
/// apart at the top, widening to 3.5 dB at the bottom, so each step is a roughly equal-sounding jump rather than an equal
/// numeric one, and level 0 is silent.
pub const VOLUME_LEVELS: usize = 24;

/// Loudest volume level.
pub const MAX_VOLUME: u8 = (VOLUME_LEVELS - 1) as u8;

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
            ChannelCmd::PitchUp(x) => Some((FX_ID_PITCH_UP, *x, None)),
            ChannelCmd::PitchDown(x) => Some((FX_ID_PITCH_DOWN, *x, None)),
            ChannelCmd::Envelope(x) => Some((FX_ID_ENVELOPE, *x, None)),
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
    Tempo(u16),
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
    PitchUp(u8),
    PitchDown(u8),
    Envelope(u8),
    // Retired in schema version 1. Kept so version 0 files still deserialize;
    // `TrackerFile::load` strips them during migration.
    FadeIn(u8),
    FadeOut(u8),
    Tremble(u8),
    Volume(u8),
    Wavetable(u16),
}
