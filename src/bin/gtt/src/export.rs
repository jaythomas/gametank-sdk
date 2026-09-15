use std::fmt::Write as FmtWrite;
use std::io;
use std::path::Path;

use crate::file::{NUM_INSTRUMENTS, TrackerFile};
use crate::tracker::{ChannelCmd, SequencerCmd};

const CPU_FREQ: f64 = 3_579_545.0;

const CHANNELS: usize = 8;

const VOL_NO_CHANGE: u8 = 0xFF;

// Fx cell encoding
const FX_NONE: u8 = 0;
const FX_ARPEGGIO: u8 = 1;
const ARP_NO_THIRD_NOTE: u8 = 0xFF;

fn note_to_freq_inc(hz: f64, sample_rate_hz: f64) -> u16 {
    ((hz / sample_rate_hz) * 65536.0).round().min(65535.0) as u16
}

// Resolve the phase increment of the note n number of `steps` above the base `note_name`
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

// SequencerCmd encoding
const SEQ_CMD_NONE: u8 = 0;
const SEQ_CMD_STOP: u8 = 1;
const SEQ_CMD_TEMPO: u8 = 2;
const SEQ_CMD_FX_SPEED: u8 = 3;
const SEQ_CMD_FLOW_COUNT: u8 = 4;
const SEQ_CMD_COUNT_JUMP: u8 = 5;
const SEQ_CMD_JUMP: u8 = 6;

fn bake_pattern(
    file: &TrackerFile,
    pattern_idx: usize,
    beats: usize,
    sample_rate_hz: f64,
) -> Vec<u8> {
    let pattern = file.current_pattern(pattern_idx as u8);
    let channel_stride = beats * 10;
    let seq_cmd_base = 1 + CHANNELS * channel_stride;
    let mut out = vec![0u8; seq_cmd_base + beats * 3];
    out[0] = beats as u8;

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
            let mut fx_id = FX_NONE;
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
                        explicit_vol = Some((*v).min(63));
                    }
                    ChannelCmd::NoteOff => {
                        has_note_off = true;
                    }
                    ChannelCmd::Arpeggio(x, y) => {
                        fx_id = FX_ARPEGGIO;
                        fx_x = *x;
                        fx_y = y.unwrap_or(ARP_NO_THIRD_NOTE);
                    }
                    _ => {}
                }
            }

            let vol_out = if let Some(v) = explicit_vol {
                remembered_vol = v;
                muted = false;
                v
            } else if has_note_off {
                muted = true;
                0
            } else if has_note && muted {
                muted = false;
                remembered_vol
            } else {
                VOL_NO_CHANGE
            };

            let (mut arp_x_freq, mut arp_y_freq) = (0u16, 0u16);
            if fx_id == FX_ARPEGGIO
                && let Some(name) = &cur_note_name
            {
                arp_x_freq = arp_offset_freq_inc(file, name, fx_x, sample_rate_hz).unwrap_or(0);
                if fx_y != ARP_NO_THIRD_NOTE {
                    arp_y_freq = arp_offset_freq_inc(file, name, fx_y, sample_rate_hz).unwrap_or(0);
                }
            }

            let base = 1 + ch * channel_stride;
            out[base + beat] = (cur_freq & 0xFF) as u8;
            out[base + beats + beat] = (cur_freq >> 8) as u8;
            out[base + beats * 2 + beat] = vol_out;
            out[base + beats * 3 + beat] = fx_id;
            out[base + beats * 4 + beat] = fx_x;
            out[base + beats * 5 + beat] = fx_y;
            out[base + beats * 6 + beat] = (arp_x_freq & 0xFF) as u8;
            out[base + beats * 7 + beat] = (arp_x_freq >> 8) as u8;
            out[base + beats * 8 + beat] = (arp_y_freq & 0xFF) as u8;
            out[base + beats * 9 + beat] = (arp_y_freq >> 8) as u8;
        }
    }

    // The SEQ lane carries one pattern-wide SequencerCmd per beat
    for beat in 0..beats {
        let seq_row = &pattern[0][beat];
        let (seq_cmd_type, seq_cmd_value, seq_cmd_value2) = match &seq_row.sqc {
            Some(SequencerCmd::Stop) => (SEQ_CMD_STOP, 0u8, 0u8),
            Some(SequencerCmd::Tempo(bpm)) => (SEQ_CMD_TEMPO, *bpm, 0u8),
            Some(SequencerCmd::FxSpeed(fx_speed)) => (SEQ_CMD_FX_SPEED, *fx_speed, 0u8),
            Some(SequencerCmd::FlowCount(count)) => (SEQ_CMD_FLOW_COUNT, *count, 0u8),
            Some(SequencerCmd::CountJump(pat, beat_target)) => {
                (SEQ_CMD_COUNT_JUMP, *pat, *beat_target)
            }
            Some(SequencerCmd::Jump(pat, beat_target)) => {
                (SEQ_CMD_JUMP, *pat, *beat_target)
            }
            None => (SEQ_CMD_NONE, 0u8, 0u8),
        };
        out[seq_cmd_base + beat] = seq_cmd_type;
        out[seq_cmd_base + beats + beat] = seq_cmd_value;
        out[seq_cmd_base + beats * 2 + beat] = seq_cmd_value2;
    }

    out
}

fn write_wave_asm(file: &TrackerFile, dir: &Path) -> io::Result<()> {
    let mut s = String::new();
    writeln!(s, ".section .const.wavetables, \"a\"").unwrap();
    for i in 0..NUM_INSTRUMENTS {
        let name = &file.instruments[i].name;
        writeln!(s).unwrap();
        writeln!(s, ".align 256").unwrap();
        writeln!(s, ".global instrument{}_table", i + 1).unwrap();
        writeln!(s, "instrument{}_table:", i + 1).unwrap();
        writeln!(s, "    .incbin \"./src/asm/instruments/{}.raw\"", name).unwrap();
    }
    std::fs::write(dir.join("wave.asm"), s)
}

fn sanitize_ident(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn write_track_asm(
    bpm: u16,
    fx_speed: u8,
    stem: &str,
    baked: &[Vec<u8>],
    dir: &Path,
    sample_rate_reg: u8,
) -> io::Result<()> {
    let ident = sanitize_ident(stem);
    let pattern_len = baked.len();
    let sample_rate_hz = (CPU_FREQ / sample_rate_reg as f64).round() as u32;
    let mut s = String::new();

    writeln!(s, "; Auto-generated by gt-tracker.").unwrap();
    writeln!(
        s,
        "; Track: {} | BPM: {} | FxSpeed: {} | Patterns: {} | Sample rate: 0x{:02X} ({} Hz)",
        stem, bpm, fx_speed, pattern_len, sample_rate_reg, sample_rate_hz
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(s, ".section .rodata, \"a\"").unwrap();
    writeln!(s).unwrap();

    writeln!(s, ".global {}_track", ident).unwrap();
    writeln!(s, "{}_track:", ident).unwrap();
    writeln!(s, "    .word {}", bpm).unwrap();
    writeln!(s, "    .word {}", fx_speed).unwrap();
    writeln!(s, "    .byte {}", pattern_len).unwrap();
    writeln!(s, "    .word {}_patterns", ident).unwrap();
    writeln!(s).unwrap();

    // Pattern pointer table
    writeln!(s, "{}_patterns:", ident).unwrap();
    for i in 0..pattern_len {
        writeln!(s, "    .word {}_pattern{}", ident, i + 1).unwrap();
    }

    for (i, data) in baked.iter().enumerate() {
        writeln!(s).unwrap();
        writeln!(s, ".align 2").unwrap();
        writeln!(s, ".global {}_pattern{}", ident, i + 1).unwrap();
        writeln!(s, "{}_pattern{}:", ident, i + 1).unwrap();
        for chunk in data.chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("0x{:02X}", b)).collect();
            writeln!(s, "    .byte {}", hex.join(", ")).unwrap();
        }
    }

    std::fs::write(dir.join(format!("{}.asm", stem)), s)
}

pub fn export_all(
    file: &TrackerFile,
    bpm: u16,
    fx_speed: u8,
    stem: &str,
    export_dir: &Path,
) -> io::Result<()> {
    std::fs::create_dir_all(export_dir)?;

    let instruments_dir = export_dir.join("instruments");
    std::fs::create_dir_all(&instruments_dir)?;

    for i in 0..NUM_INSTRUMENTS {
        let name = &file.instruments[i].name;
        let path = instruments_dir.join(format!("{}.raw", name));
        std::fs::write(path, &file.instruments[i].waveform)?;
    }

    write_wave_asm(file, export_dir)?;

    let baked: Vec<Vec<u8>> = (0..file.patterns.len())
        .map(|i| {
            bake_pattern(
                file,
                i,
                file.beats_for(i as u8) as usize,
                CPU_FREQ / file.sample_rate as f64,
            )
        })
        .collect();

    write_track_asm(bpm, fx_speed, stem, &baked, export_dir, file.sample_rate)?;

    Ok(())
}

