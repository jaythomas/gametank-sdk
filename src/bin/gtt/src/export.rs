use std::collections::HashMap;
use std::io;
use std::path::Path;

use crate::file::{NUM_INSTRUMENTS, TrackerFile};
use crate::sample_rate::{SAMPLE_RATE_REG, sample_rate_reg_to_hz};
use crate::tracker::{
    BEAT_THRESHOLD, ChannelCmd, ENVELOPE_MAX_STEPS, FX_ID_ARPEGGIO, FX_ID_ENVELOPE,
    FX_ID_INSTRUMENT, FX_ID_NONE, FX_ID_PITCH_DOWN, FX_ID_PITCH_UP, MAX_VOLUME, NUM_ENVELOPES,
    SequencerCmd,
};
use crate::volume_scale::scale_table;

const CHANNELS: usize = 7;

const TRACK_HEADER_SIZE: usize = 6;

// Frequency table slot 0 is reserved: a note index of 0 means "no note here".
const FREQ_INDEX_NONE: u8 = 0;
const MAX_FREQ_ENTRIES: usize = 256;

// Set in a pattern's channel mask when the pattern carries a sequence-command list.
const PATTERN_HAS_SEQ: u8 = 0x80;

// vol_fx byte: volume in the low five bits, effect ID in the top three. The
// volume needs 24 values, so the eight spare codes carry meaning the byte has
// no room to flag. 31 means "leave the volume alone"; 24 means silence that
// also suppresses the envelope, which is what separates a note OFF from a
// volume the user deliberately set to zero.
const VOL_FX_ID_SHIFT: u8 = 5;
const VOL_FX_VOLUME_HOLD: u8 = 0x1F;
const VOL_FX_VOLUME_MUTE: u8 = 0x18;

const ARP_NO_THIRD_NOTE: u8 = 0xFF;

fn note_to_freq_inc(hz: f64, sample_rate_hz: f64) -> u16 {
    ((hz / sample_rate_hz) * 65536.0).round().min(65535.0) as u16
}

// Resolve the phase increment of the note u8 `steps` above `note_name`
// Returns `None` if note_name isn't found in the tuning table
fn arp_offset_freq_inc(
    file: &TrackerFile,
    note_name: &str,
    steps: u8,
    sample_rate_hz: f64,
) -> Option<u16> {
    let idx = file.tuning.notes.get_index_of(note_name)?;
    let target_idx = (idx + steps as usize).min(file.tuning.notes.len().saturating_sub(1));
    let (_, &hz) = file.tuning.notes.get_index(target_idx)?;
    Some(note_to_freq_inc(hz, sample_rate_hz))
}

// Resolve the phase increment of the note u8 `steps` below `note_name`
// Returns `None` if note_name isn't found in the tuning table
fn pitch_down_offset_freq_inc(
    file: &TrackerFile,
    note_name: &str,
    steps: u8,
    sample_rate_hz: f64,
) -> Option<u16> {
    let idx = file.tuning.notes.get_index_of(note_name)?;
    let target_idx = idx.saturating_sub(steps as usize);
    let (_, &hz) = file.tuning.notes.get_index(target_idx)?;
    Some(note_to_freq_inc(hz, sample_rate_hz))
}

// SequencerCmd encoding
const SEQ_CMD_STOP: u8 = 1;
const SEQ_CMD_TEMPO: u8 = 2;
const SEQ_CMD_FX_SPEED: u8 = 3;
const SEQ_CMD_FLOW_COUNT: u8 = 4;
const SEQ_CMD_COUNT_JUMP: u8 = 5;
const SEQ_CMD_JUMP: u8 = 6;

/// One channel's worth of a single beat, before frequencies are swapped for
/// frequency-table indices.
#[derive(Clone, Copy, Default)]
struct Cell {
    freq: u16,
    vol: Option<u8>,
    fx_id: u8,
    fx_x: u8,
    fx_arg_x: u16,
    fx_arg_y: u16,
}

impl Cell {
    fn is_empty(&self) -> bool {
        self.freq == 0 && self.vol.is_none() && self.fx_id == FX_ID_NONE
    }
}

struct BakedPattern {
    beats: usize,
    // [channel][beat]
    cells: Vec<Vec<Cell>>,
    // (beat, cmd, value, value2), ascending by beat
    seq: Vec<(u8, u8, u8, u8)>,
}

fn bake_pattern(
    file: &TrackerFile,
    pattern_idx: usize,
    beats: usize,
    sample_rate_hz: f64,
) -> BakedPattern {
    let pattern = file.current_pattern(pattern_idx as u8);
    let mut cells = vec![vec![Cell::default(); beats]; CHANNELS];

    for ch in 0..CHANNELS {
        let mut cur_freq: u16 = 0;
        let mut cur_note_name: Option<String> = None;
        let mut remembered_vol: u8 = 0;
        let mut muted = false;

        for beat in 0..beats {
            let row = &pattern[ch + 1][beat];

            let mut explicit_vol: Option<u8> = None;
            let mut has_note = false;
            let mut has_note_off = false;
            let mut fx_id = FX_ID_NONE;
            let mut fx_x = 0u8;
            let mut fx_y = 0u8;

            // Process ChannelCmds
            for cmd in &row.cmd_list {
                match cmd {
                    ChannelCmd::Note(name) => {
                        has_note = true;
                        if let Some(&hz) = file.tuning.notes.get(name.as_str()) {
                            cur_freq = note_to_freq_inc(hz, sample_rate_hz);
                            cur_note_name = Some(name.clone());
                        }
                    }
                    ChannelCmd::Phase(inc) => {
                        cur_freq = *inc;
                        cur_note_name = None;
                    }
                    ChannelCmd::Volume(v) => {
                        explicit_vol = Some((*v).min(MAX_VOLUME));
                    }
                    ChannelCmd::NoteOff => {
                        has_note_off = true;
                    }
                    ChannelCmd::Instrument(idx) => {
                        fx_id = FX_ID_INSTRUMENT;
                        fx_x = *idx;
                        fx_y = 0;
                    }
                    ChannelCmd::Arpeggio(x, y) => {
                        fx_id = FX_ID_ARPEGGIO;
                        fx_x = *x;
                        fx_y = y.unwrap_or(ARP_NO_THIRD_NOTE);
                    }
                    ChannelCmd::PitchUp(x) => {
                        fx_id = FX_ID_PITCH_UP;
                        fx_x = *x;
                    }
                    ChannelCmd::PitchDown(x) => {
                        fx_id = FX_ID_PITCH_DOWN;
                        fx_x = *x;
                    }
                    ChannelCmd::Envelope(x) => {
                        fx_id = FX_ID_ENVELOPE;
                        fx_x = *x;
                    }
                    _ => {}
                }
            }

            let vol_out = if let Some(v) = explicit_vol {
                remembered_vol = v;
                muted = false;
                Some(v)
            } else if has_note_off {
                muted = true;
                Some(VOL_FX_VOLUME_MUTE)
            } else if has_note && muted {
                muted = false;
                Some(remembered_vol)
            } else {
                None
            };

            let (mut fx_arg_x, mut fx_arg_y) = (0u16, 0u16);
            if fx_id == FX_ID_ARPEGGIO
                && let Some(name) = &cur_note_name
            {
                fx_arg_x = arp_offset_freq_inc(file, name, fx_x, sample_rate_hz).unwrap_or(0);
                if fx_y != ARP_NO_THIRD_NOTE {
                    fx_arg_y = arp_offset_freq_inc(file, name, fx_y, sample_rate_hz).unwrap_or(0);
                }
            } else if fx_id == FX_ID_PITCH_UP
                && let Some(name) = &cur_note_name
            {
                fx_arg_x =
                    arp_offset_freq_inc(file, name, fx_x, sample_rate_hz).unwrap_or(cur_freq);
            } else if fx_id == FX_ID_PITCH_DOWN
                && let Some(name) = &cur_note_name
            {
                fx_arg_x = pitch_down_offset_freq_inc(file, name, fx_x, sample_rate_hz)
                    .unwrap_or(cur_freq);
            }

            cells[ch][beat] = Cell {
                freq: cur_freq,
                vol: vol_out,
                fx_id,
                fx_x,
                fx_arg_x,
                fx_arg_y,
            };
        }
    }

    // The SEQ lane carries one pattern-wide SequencerCmd per beat. Only the
    // beats that actually have one are written out, so the sequencer scans a
    // handful of entries instead of reading three arrays every beat.
    let mut seq = Vec::new();
    for beat in 0..beats {
        let encoded = match &pattern[0][beat].sqc {
            Some(SequencerCmd::Stop) => (SEQ_CMD_STOP, 0u8, 0u8),
            Some(SequencerCmd::Tempo(bpm)) => {
                (SEQ_CMD_TEMPO, (*bpm & 0xFF) as u8, (*bpm >> 8) as u8)
            }
            Some(SequencerCmd::FxSpeed(fx_speed)) => (SEQ_CMD_FX_SPEED, *fx_speed, 0u8),
            Some(SequencerCmd::FlowCount(count)) => (SEQ_CMD_FLOW_COUNT, *count, 0u8),
            Some(SequencerCmd::CountJump(pat, beat_target)) => {
                (SEQ_CMD_COUNT_JUMP, *pat, *beat_target)
            }
            Some(SequencerCmd::Jump(pat, beat_target)) => (SEQ_CMD_JUMP, *pat, *beat_target),
            None => continue,
        };
        seq.push((beat as u8, encoded.0, encoded.1, encoded.2));
    }

    BakedPattern { beats, cells, seq }
}

/// Every distinct phase increment the track uses, so patterns can refer to one
/// by a single-byte index instead of repeating a 16-bit value every beat.
struct FreqTable {
    entries: Vec<u16>,
    index_of: HashMap<u16, u8>,
}

impl FreqTable {
    fn new() -> Self {
        Self {
            entries: vec![0],
            index_of: HashMap::new(),
        }
    }

    fn intern(&mut self, freq: u16) -> io::Result<u8> {
        if freq == 0 {
            return Ok(FREQ_INDEX_NONE);
        }
        if let Some(&idx) = self.index_of.get(&freq) {
            return Ok(idx);
        }
        if self.entries.len() >= MAX_FREQ_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "track uses more than {} distinct frequencies, which do not fit the \
                     one-byte note indices the exported format uses",
                    MAX_FREQ_ENTRIES - 1
                ),
            ));
        }
        let idx = self.entries.len() as u8;
        self.entries.push(freq);
        self.index_of.insert(freq, idx);
        Ok(idx)
    }
}

fn serialize_pattern(pattern: &BakedPattern, freqs: &mut FreqTable) -> io::Result<Vec<u8>> {
    let mut channel_mask = 0u8;
    let mut fx_mask = 0u8;
    for ch in 0..CHANNELS {
        let cells = &pattern.cells[ch];
        if cells.iter().any(|c| !c.is_empty()) {
            channel_mask |= 1 << ch;
        }
        if cells.iter().any(|c| c.fx_id != FX_ID_NONE) {
            fx_mask |= 1 << ch;
        }
    }
    if !pattern.seq.is_empty() {
        channel_mask |= PATTERN_HAS_SEQ;
    }

    let mut out = vec![pattern.beats as u8, channel_mask, fx_mask];

    if !pattern.seq.is_empty() {
        out.push(pattern.seq.len() as u8);
        for &(beat, cmd, value, value2) in &pattern.seq {
            out.extend_from_slice(&[beat, cmd, value, value2]);
        }
    }

    for ch in 0..CHANNELS {
        if channel_mask & (1 << ch) == 0 {
            continue;
        }
        let cells = &pattern.cells[ch];

        for cell in cells {
            out.push(freqs.intern(cell.freq)?);
        }
        for cell in cells {
            let mut vol_fx = cell.fx_id << VOL_FX_ID_SHIFT;
            vol_fx |= cell.vol.unwrap_or(VOL_FX_VOLUME_HOLD);
            out.push(vol_fx);
        }

        if fx_mask & (1 << ch) == 0 {
            continue;
        }
        for cell in cells {
            out.push(cell.fx_x);
        }
        for cell in cells {
            out.push(freqs.intern(cell.fx_arg_x)?);
        }
        for cell in cells {
            out.push(freqs.intern(cell.fx_arg_y)?);
        }
    }

    Ok(out)
}

fn build_track_bin(bpm: u16, fx_speed: u8, baked: &[BakedPattern]) -> io::Result<Vec<u8>> {
    let pattern_count = baked.len();

    let mut freqs = FreqTable::new();
    let mut blocks = Vec::with_capacity(pattern_count);
    for pattern in baked {
        blocks.push(serialize_pattern(pattern, &mut freqs)?);
    }

    let mut running = TRACK_HEADER_SIZE + pattern_count * 2 + freqs.entries.len() * 2;
    let mut pattern_offsets = Vec::with_capacity(pattern_count);
    for block in &blocks {
        pattern_offsets.push(running as u16);
        running += block.len();
    }

    let mut out = Vec::with_capacity(running);
    out.extend_from_slice(&bpm.to_le_bytes());
    out.extend_from_slice(&(fx_speed as u16).to_le_bytes());
    out.push(pattern_count as u8);
    out.push(freqs.entries.len() as u8);
    for off in &pattern_offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    for freq in &freqs.entries {
        out.extend_from_slice(&freq.to_le_bytes());
    }
    for block in &blocks {
        out.extend_from_slice(block);
    }
    Ok(out)
}

/// Serialize one envelope: step count, the accumulator threshold that spreads
/// those steps across a beat, then the steps themselves padded to a fixed
/// width so the firmware can index them without parsing.
fn build_envelope_bin(steps: &[u8]) -> Vec<u8> {
    let count = steps.len().clamp(1, ENVELOPE_MAX_STEPS);
    let threshold = BEAT_THRESHOLD / count as u16;
    let last = steps[count - 1];

    let mut out = Vec::with_capacity(3 + ENVELOPE_MAX_STEPS);
    out.push(count as u8);
    out.extend_from_slice(&threshold.to_le_bytes());
    out.extend_from_slice(&steps[..count]);
    out.resize(3 + ENVELOPE_MAX_STEPS, last);
    out
}

pub fn export_all(
    file: &TrackerFile,
    bpm: u16,
    fx_speed: u8,
    stem: &str,
    export_dir: &Path,
) -> io::Result<()> {
    let baked: Vec<BakedPattern> = (0..file.patterns.len())
        .map(|i| {
            bake_pattern(
                file,
                i,
                file.beats_for(i as u8) as usize,
                sample_rate_reg_to_hz(SAMPLE_RATE_REG),
            )
        })
        .collect();

    // Built before anything is written so a track that overflows the frequency
    // table doesn't leave a half-finished export behind.
    let track_bin = build_track_bin(bpm, fx_speed, &baked)?;

    std::fs::create_dir_all(export_dir)?;
    let instruments_dir = export_dir.join("instruments");
    std::fs::create_dir_all(&instruments_dir)?;

    for i in 0..NUM_INSTRUMENTS {
        let path = instruments_dir.join(format!("instrument_{}.bin", i));
        std::fs::write(path, &file.instruments[i].waveform)?;
    }

    let envelopes_dir = export_dir.join("envelopes");
    std::fs::create_dir_all(&envelopes_dir)?;

    for i in 0..NUM_ENVELOPES {
        let path = envelopes_dir.join(format!("envelope_{}.bin", i));
        std::fs::write(path, build_envelope_bin(&file.envelope_steps(i)))?;
    }
    std::fs::write(envelopes_dir.join("volume_scale.bin"), scale_table())?;

    std::fs::write(export_dir.join(format!("{}.bin", stem)), track_bin)?;

    Ok(())
}
