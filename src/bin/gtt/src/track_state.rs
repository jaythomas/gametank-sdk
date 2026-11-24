use std::collections::HashSet;

use indexmap::IndexMap;

use crate::file::TrackerFile;
use crate::player::AUDIO_CHANNELS;
use crate::sample_rate::{SAMPLE_RATE_REG, sample_rate_reg_to_hz};
use crate::tracker::{Beat, ChannelCmd, PATTERN_BEATS, SequencerCmd, envelope_slot};

const MAX_STEPS: usize = 200_000;

pub fn note_freq(hz: f64, acp_sample_rate: f64) -> u16 {
    (((hz / acp_sample_rate) * 65536.0).round() as u32).min(0xFFFF) as u16
}

pub struct RowCmds {
    pub note: Option<String>,
    pub vol: Option<u8>,
    pub note_off: bool,
    pub arp: Option<(u8, Option<u8>)>,
    pub instrument: Option<usize>,
    pub pitch_up: Option<u8>,
    pub pitch_down: Option<u8>,
    pub envelope: Option<u8>,
}

impl RowCmds {
    pub fn from_beat(beat: &Beat) -> Self {
        let cmds = &beat.cmd_list;
        Self {
            note: cmds.iter().find_map(|c| match c {
                ChannelCmd::Note(s) => Some(s.clone()),
                _ => None,
            }),
            vol: cmds.iter().find_map(|c| match c {
                ChannelCmd::Volume(v) => Some(*v),
                _ => None,
            }),
            note_off: cmds.iter().any(|c| matches!(c, ChannelCmd::NoteOff)),
            arp: cmds.iter().find_map(|c| match c {
                ChannelCmd::Arpeggio(x, y) => Some((*x, *y)),
                _ => None,
            }),
            instrument: cmds.iter().find_map(|c| match c {
                ChannelCmd::Instrument(idx) => Some(*idx as usize),
                _ => None,
            }),
            pitch_up: cmds.iter().find_map(|c| match c {
                ChannelCmd::PitchUp(x) => Some(*x),
                _ => None,
            }),
            pitch_down: cmds.iter().find_map(|c| match c {
                ChannelCmd::PitchDown(x) => Some(*x),
                _ => None,
            }),
            envelope: cmds.iter().find_map(|c| match c {
                ChannelCmd::Envelope(x) => Some(*x),
                _ => None,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelState {
    pub vol: u8,
    pub muted: bool,
    pub note_name: Option<String>,
    pub base_freq: u16,
    pub instrument: usize,
    pub env_slot: Option<usize>,
    pub env_suppressed: bool,
}

impl ChannelState {
    fn initial(ch: usize) -> Self {
        Self {
            vol: 0,
            muted: true,
            note_name: None,
            base_freq: 0,
            instrument: ch,
            env_slot: None,
            env_suppressed: false,
        }
    }

    fn apply(&mut self, beat: &Beat, tuning: &IndexMap<String, f64>, acp_sample_rate: f64) {
        let cmds = RowCmds::from_beat(beat);
        if let Some(instrument) = cmds.instrument {
            self.instrument = instrument;
        }
        if let Some(slot) = cmds.envelope {
            self.env_slot = envelope_slot(slot);
        }
        if let Some(vol) = cmds.vol {
            self.vol = vol;
            self.muted = false;
            self.env_suppressed = false;
        } else if cmds.note_off {
            self.muted = true;
            self.env_suppressed = true;
        }
        if let Some(name) = cmds.note {
            if let Some(&hz) = tuning.get(name.as_str()) {
                self.base_freq = note_freq(hz, acp_sample_rate);
                self.env_suppressed = false;
                if cmds.vol.is_none() {
                    self.muted = false;
                }
            }
            self.note_name = Some(name);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackState {
    pub channels: [ChannelState; AUDIO_CHANNELS],
    pub bpm: u16,
    pub fx_speed: u8,
    pub flow_count: u8,
}

impl TrackState {
    fn initial(file: &TrackerFile) -> Self {
        Self {
            channels: std::array::from_fn(ChannelState::initial),
            bpm: file.bpm,
            fx_speed: file.fx_speed,
            flow_count: 0,
        }
    }

    fn apply_channels(&mut self, file: &TrackerFile, pattern: usize, row: usize, hz: f64) {
        for (ch, state) in self.channels.iter_mut().enumerate() {
            state.apply(&file.patterns[pattern][ch + 1][row], &file.tuning.notes, hz);
        }
    }
}

fn pattern_beats(file: &TrackerFile, pattern: usize) -> usize {
    file.beats_for(pattern as u8).clamp(1, PATTERN_BEATS as u8) as usize
}

fn jump_target(file: &TrackerFile, pattern: u8, beat: u8) -> (usize, usize) {
    let pattern = (pattern as usize).min(file.patterns.len() - 1);
    (
        pattern,
        (beat as usize).min(pattern_beats(file, pattern) - 1),
    )
}

pub fn track_state_at(file: &TrackerFile, pattern: usize, row: usize) -> TrackState {
    let pattern = pattern.min(file.patterns.len() - 1);
    let target = (pattern, row.min(pattern_beats(file, pattern) - 1));
    let hz = sample_rate_reg_to_hz(SAMPLE_RATE_REG);
    walk_to(file, target, hz).unwrap_or_else(|| look_back(file, target, hz))
}

fn walk_to(file: &TrackerFile, target: (usize, usize), hz: f64) -> Option<TrackState> {
    let mut state = TrackState::initial(file);
    let mut pos = (0, 0);
    let mut visited = HashSet::new();
    loop {
        if pos == target {
            return Some(state);
        }
        if !visited.insert((pos, state.flow_count)) || visited.len() > MAX_STEPS {
            return None;
        }
        let (pattern, row) = pos;
        match &file.patterns[pattern][0][row].sqc {
            Some(SequencerCmd::Stop) => return None,
            Some(SequencerCmd::Tempo(bpm)) => state.bpm = *bpm,
            Some(SequencerCmd::FxSpeed(fx_speed)) => state.fx_speed = *fx_speed,
            Some(SequencerCmd::FlowCount(count)) => state.flow_count = *count,
            Some(SequencerCmd::CountJump(pat, beat)) if state.flow_count > 0 => {
                state.flow_count -= 1;
                pos = jump_target(file, *pat, *beat);
                continue;
            }
            Some(SequencerCmd::Jump(pat, beat)) => {
                pos = jump_target(file, *pat, *beat);
                continue;
            }
            Some(SequencerCmd::CountJump(_, _)) | None => {}
        }
        state.apply_channels(file, pattern, row, hz);
        pos = (pattern, (row + 1) % pattern_beats(file, pattern));
    }
}

fn look_back(file: &TrackerFile, (pattern, row): (usize, usize), hz: f64) -> TrackState {
    let mut state = TrackState::initial(file);
    for r in 0..row {
        match &file.patterns[pattern][0][r].sqc {
            Some(SequencerCmd::Tempo(bpm)) => state.bpm = *bpm,
            Some(SequencerCmd::FxSpeed(fx_speed)) => state.fx_speed = *fx_speed,
            _ => {}
        }
        state.apply_channels(file, pattern, r, hz);
    }
    state
}
