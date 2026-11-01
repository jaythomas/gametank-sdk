use serde::{Deserialize, Serialize};

pub const PATTERN_TABLE_WIDTH: u16 = 104;
pub const DEFAULT_PATTERN_BEATS: usize = 64;
pub const PATTERN_BEATS: usize = 255;

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
pub const MAX_FX_SPEED: u8 = 31;
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
    pub fn is_empty(&self) -> bool {
        self.cmd_list.is_empty() && self.sqc.is_none()
    }
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

/// One row of a pattern across all lanes (sequencer lane plus 7 channels).
pub type BeatRow = [Beat; 8];

fn row_at(pattern: &Pattern, row: usize) -> BeatRow {
    std::array::from_fn(|lane| pattern[lane][row].clone())
}

/// Copies `count` rows starting at `start`. Returns `None` if the range is
/// empty or extends past the pattern's `beats` length.
pub fn yank_rows(pattern: &Pattern, beats: u8, start: u8, count: usize) -> Option<Vec<BeatRow>> {
    let (start, beats) = (start as usize, beats as usize);
    if count == 0 || start + count > beats {
        return None;
    }
    Some((start..start + count).map(|r| row_at(pattern, r)).collect())
}

/// Removes `count` rows starting at `start`, shifting later rows up. Returns
/// the removed rows and the new pattern length, which never drops below 1.
pub fn delete_rows(
    pattern: &mut Pattern,
    beats: u8,
    start: u8,
    count: usize,
) -> Option<(Vec<BeatRow>, u8)> {
    let removed = yank_rows(pattern, beats, start, count)?;
    let (start, beats_usize) = (start as usize, beats as usize);
    for lane in pattern.iter_mut() {
        let active = &mut lane[start..beats_usize];
        active.rotate_left(count);
        for beat in &mut active[beats_usize - start - count..] {
            *beat = Beat::default();
        }
    }
    let new_beats = (beats_usize - count).max(1) as u8;
    Some((removed, new_beats))
}

/// Inserts as many rows of `clip` as fit before the pattern reaches
/// `PATTERN_BEATS`, at row `at`, shifting existing rows down. Returns the new
/// pattern length, or `None` if nothing can be inserted.
pub fn paste_rows(pattern: &mut Pattern, beats: u8, at: u8, clip: &[BeatRow]) -> Option<u8> {
    let (at, beats) = (at as usize, beats as usize);
    let n = clip.len().min(PATTERN_BEATS.saturating_sub(beats));
    if n == 0 || at > beats {
        return None;
    }
    for (lane_idx, lane) in pattern.iter_mut().enumerate() {
        for beat in &mut lane[beats..beats + n] {
            *beat = Beat::default();
        }
        lane[at..beats + n].rotate_right(n);
        for (i, row) in clip[..n].iter().enumerate() {
            lane[at + i] = row[lane_idx].clone();
        }
    }
    Some((beats + n) as u8)
}

pub fn row_after_doubling(beats: u8, row: u8) -> u8 {
    let spaced = beats.min(PATTERN_BEATS as u8 - beats);
    if row < spaced { row * 2 } else { row + spaced }
}

pub fn double_rows(pattern: &mut Pattern, beats: u8) -> Option<u8> {
    let new_beats = beats + beats.min(PATTERN_BEATS as u8 - beats);
    if new_beats == beats {
        return None;
    }
    for lane in pattern.iter_mut() {
        for beat in &mut lane[beats as usize..new_beats as usize] {
            *beat = Beat::default();
        }
        for row in (1..beats).rev() {
            lane[row_after_doubling(beats, row) as usize] =
                std::mem::take(&mut lane[row as usize]);
        }
    }
    Some(new_beats)
}

pub fn remap_jump_targets(patterns: &mut [Pattern], target_pattern: u8, remap: impl Fn(u8) -> u8) {
    for pattern in patterns {
        for beat in &mut pattern[0] {
            if let Some(SequencerCmd::Jump(pat, target) | SequencerCmd::CountJump(pat, target)) =
                &mut beat.sqc
                && *pat == target_pattern
            {
                *target = remap(*target);
            }
        }
    }
}
