use super::wavetable_8ch::{VOICE_COUNT, WAVETABLE, voices};
use crate::console::Console;

const SAMPLE_RATE_REG: u8 = 0xCB;

macro_rules! instrument_table {
    ($n:literal) => {
        *include_bytes!(concat!(
            // gt-tracker's exported `instruments/` folder
            "../../../assets/instruments/instrument_",
            $n,
            ".raw"
        ))
    };
}

// The 11 fixed instrument slots don't fit in FIXED_FLASH
// alongside everything else, so they're placed in bank 125.
const INSTRUMENT_BANK: u8 = 125;

#[unsafe(link_section = ".rodata.bank125")]
static INSTRUMENT_TABLES: [[u8; 256]; 11] = [
    instrument_table!(1),
    instrument_table!(2),
    instrument_table!(3),
    instrument_table!(4),
    instrument_table!(5),
    instrument_table!(6),
    instrument_table!(7),
    instrument_table!(8),
    instrument_table!(9),
    instrument_table!(10),
    instrument_table!(11),
];

// Number of parallel per-beat arrays packed into each channel's slice of a
// pattern's data block:
// - freq_lo
// - freq_hi
// - vol
// - fx_id
// - fx_x
// - fx_y
// - arp_freq_x_lo
// - arp_freq_x_hi
// - arp_freq_y_lo
// - arp_freq_y_hi
const CHANNEL_ARRAYS: usize = 10;

const FX_ID_INSTRUMENT: u8 = 1;
const FX_ID_ARPEGGIO: u8 = 2;
const ARP_NO_THIRD_NOTE: u8 = 0xFF;

/// Drives the 8-channel wavetable synth from a gt-tracker export. Create one
/// sequencer per track, point it at the `<name>_track` descriptor, then call
/// `init_voices` once after loading the firmware and `tick` once per frame.
pub struct TrackSequencer {
    track: *const u8,
    beat: u8,
    frame_acc: u16,
    tick_acc: u16,
    bpm: u16,
    speed: u16,
    pattern_idx: u8,
    pattern_count: u8,
    flow_count: u8,
    stopped: bool,
    arp_active_any: bool,
}

// Channel base note pitch, written by `advance_beat`. This is different than the active
// note used by `advance_tick` which represents the frequency after applied effects.
static mut BASE_FREQ: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// Whether the current channel and beat has an active arpeggio
static mut ARP_ACTIVE: [bool; VOICE_COUNT] = [false; VOICE_COUNT];
static mut ARP_X_FREQ: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
static mut ARP_Y_FREQ: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// Keep track if the channel has a 2-note or 3-note arpeggio
static mut ARP_STEP_COUNT: [u8; VOICE_COUNT] = [3; VOICE_COUNT];
// Each channel's current position within its own arpeggio cycle
static mut ARP_STEP: [u8; VOICE_COUNT] = [0; VOICE_COUNT];

// See the gt-tracker README for the track descriptor layout
const TRACK_BPM_OFFSET: usize = 0;
const TRACK_SPEED_OFFSET: usize = 2;
const TRACK_PATTERN_COUNT_OFFSET: usize = 4;
const TRACK_HEADER_SIZE: usize = 5;

// seq_cmd_type values baked into each pattern's trailing per-beat arrays
const SEQ_CMD_STOP: u8 = 1;
const SEQ_CMD_TEMPO: u8 = 2;
const SEQ_CMD_SPEED: u8 = 3;
const SEQ_CMD_FLOW_COUNT: u8 = 4;
const SEQ_CMD_COUNT_JUMP: u8 = 5;
const SEQ_CMD_JUMP: u8 = 6;

impl TrackSequencer {
    // Create a sequencer for the given track descriptor
    pub fn new(track: *const u8) -> Self {
        let bpm = unsafe { read_u16(track, TRACK_BPM_OFFSET) };
        let speed = unsafe { read_u16(track, TRACK_SPEED_OFFSET) };
        let pattern_count = unsafe { *track.add(TRACK_PATTERN_COUNT_OFFSET) };
        Self {
            track,
            beat: 0,
            frame_acc: 0,
            tick_acc: 0,
            bpm,
            speed,
            pattern_idx: 0,
            pattern_count,
            flow_count: 0,
            stopped: false,
            arp_active_any: false,
        }
    }

    // Load the gt-tracker instrument tables into ACP RAM,
    // point each voice at a wavetable, mute all voices,
    // and set the audio_freq register. TODO: channels
    // should default to the first instrument perhaps?
    pub fn init_voices(&self, console: &mut Console, restore_bank: u8) {
        console.set_rom_bank(INSTRUMENT_BANK);
        console.audio.load_instruments(&INSTRUMENT_TABLES);
        console.set_rom_bank(restore_bank);

        let v = voices();
        for i in 0..VOICE_COUNT {
            v[i].set_wavetable(WAVETABLE[i]);
            v[i].set_volume(0);
        }
        unsafe { core::ptr::write_volatile(0x2006 as *mut u8, SAMPLE_RATE_REG) };
    }

    // Advance the sequencer by one frame every game loop
    pub fn tick(&mut self) {
        if self.stopped {
            return;
        }

        self.frame_acc += self.bpm;

        if self.arp_active_any {
            let mut remaining = self.speed;
            while remaining > 0 {
                // read_volatile prevents LLVM from trying to optimize this into an unsupported `__mulhi3`
                let bpm = unsafe { core::ptr::read_volatile(&self.bpm) };
                self.tick_acc += bpm;
                remaining -= 1;
            }

            while self.tick_acc >= 1800 {
                self.tick_acc -= 1800;
                self.advance_tick();
            }
        }

        if self.frame_acc >= 1800 {
            self.frame_acc -= 1800;
            self.advance_beat();
        }
    }

    // Run sub-tick effect
    fn advance_tick(&mut self) {
        let v = voices();
        unsafe {
            for ch in 0..VOICE_COUNT {
                if !ARP_ACTIVE[ch] {
                    continue;
                }
                let freq = match ARP_STEP[ch] {
                    0 => BASE_FREQ[ch],
                    1 => ARP_X_FREQ[ch],
                    _ => ARP_Y_FREQ[ch],
                };
                v[ch].set_frequency(freq);
                ARP_STEP[ch] = if ARP_STEP[ch] + 1 >= ARP_STEP_COUNT[ch] {
                    0
                } else {
                    ARP_STEP[ch] + 1
                };
            }
        }
    }

    fn pattern_ptr(&self, pattern_idx: u8) -> *const u8 {
        let table = unsafe { self.track.add(TRACK_HEADER_SIZE) as *const u16 };
        let pattern_offset = unsafe { read_ptr(table, pattern_idx as usize) } as usize;
        unsafe { self.track.add(pattern_offset) }
    }

    // Processes sequence and channel commands for the current pattern_idx+beat
    fn advance_beat(&mut self) {
        loop {
            let pat = self.pattern_ptr(self.pattern_idx);
            let pattern_beats = unsafe { *pat } as usize;
            let data = unsafe { pat.add(1) };
            // channel_stride = pattern_beats * CHANNEL_ARRAYS
            let mut channel_stride = 0usize;
            for _ in 0..CHANNEL_ARRAYS {
                channel_stride += pattern_beats;
            }

            let beat = self.beat as usize;

            let seq_cmd_base = {
                let mut b = 0usize;
                for _ in 0..VOICE_COUNT {
                    b += channel_stride;
                }
                b
            };
            let off_seq_cmd_type = 0usize;
            let off_seq_cmd_value = off_seq_cmd_type + pattern_beats;
            let off_seq_cmd_value2 = off_seq_cmd_value + pattern_beats;
            let seq_cmd_type = unsafe { *data.add(seq_cmd_base + off_seq_cmd_type + beat) };
            let seq_cmd_value = unsafe { *data.add(seq_cmd_base + off_seq_cmd_value + beat) };
            let seq_cmd_value2 = unsafe { *data.add(seq_cmd_base + off_seq_cmd_value2 + beat) };

            match seq_cmd_type {
                SEQ_CMD_STOP => {
                    self.stopped = true;
                    return;
                }
                SEQ_CMD_TEMPO => {
                    self.bpm = seq_cmd_value as u16;
                }
                SEQ_CMD_SPEED => {
                    self.speed = seq_cmd_value as u16;
                }
                SEQ_CMD_FLOW_COUNT => {
                    self.flow_count = seq_cmd_value;
                }
                SEQ_CMD_COUNT_JUMP => {
                    if self.flow_count > 0 {
                        self.flow_count -= 1;
                        self.pattern_idx = seq_cmd_value.min(self.pattern_count.saturating_sub(1));
                        self.beat = seq_cmd_value2;
                        continue;
                    }
                }
                SEQ_CMD_JUMP => {
                    self.pattern_idx = seq_cmd_value.min(self.pattern_count.saturating_sub(1));
                    self.beat = seq_cmd_value2;
                    continue;
                }
                _ => {}
            }

            self.trigger_channels(data, channel_stride, pattern_beats, beat);

            self.beat += 1;
            if (self.beat as usize) >= pattern_beats {
                self.beat = 0;
            }
            return;
        }
    }

    fn trigger_channels(
        &mut self,
        data: *const u8,
        channel_stride: usize,
        pattern_beats: usize,
        beat: usize,
    ) {
        let off_freq_lo = 0usize;
        let off_freq_hi = off_freq_lo + pattern_beats;
        let off_vol = off_freq_hi + pattern_beats;
        let off_fx_id = off_vol + pattern_beats;
        let off_fx_x = off_fx_id + pattern_beats;
        let off_fx_y = off_fx_x + pattern_beats;
        let off_arp_x_lo = off_fx_y + pattern_beats;
        let off_arp_x_hi = off_arp_x_lo + pattern_beats;
        let off_arp_y_lo = off_arp_x_hi + pattern_beats;
        let off_arp_y_hi = off_arp_y_lo + pattern_beats;

        let v = voices();
        let mut base = 0usize;
        let mut any_active = false;
        for ch in 0..VOICE_COUNT {
            let lo = unsafe { *data.add(base + off_freq_lo + beat) } as u16;
            let hi = unsafe { *data.add(base + off_freq_hi + beat) } as u16;
            let vol = unsafe { *data.add(base + off_vol + beat) };
            let fx_id = unsafe { *data.add(base + off_fx_id + beat) };
            let fx_x = unsafe { *data.add(base + off_fx_x + beat) };
            let fx_y = unsafe { *data.add(base + off_fx_y + beat) };

            if lo | hi != 0 {
                unsafe {
                    BASE_FREQ[ch] = lo | (hi << 8);
                }
            }
            if vol != 0xFF {
                v[ch].set_volume(vol);
            }

            if fx_id == FX_ID_ARPEGGIO {
                let arp_x_lo = unsafe { *data.add(base + off_arp_x_lo + beat) } as u16;
                let arp_x_hi = unsafe { *data.add(base + off_arp_x_hi + beat) } as u16;
                let arp_y_lo = unsafe { *data.add(base + off_arp_y_lo + beat) } as u16;
                let arp_y_hi = unsafe { *data.add(base + off_arp_y_hi + beat) } as u16;
                unsafe {
                    ARP_ACTIVE[ch] = true;
                    ARP_X_FREQ[ch] = arp_x_lo | (arp_x_hi << 8);
                    ARP_Y_FREQ[ch] = arp_y_lo | (arp_y_hi << 8);
                    ARP_STEP_COUNT[ch] = if fx_y == ARP_NO_THIRD_NOTE { 2 } else { 3 };
                    ARP_STEP[ch] = 0;
                }
                any_active = true;
            } else {
                if fx_id == FX_ID_INSTRUMENT {
                    v[ch].set_wavetable(WAVETABLE[fx_x as usize]);
                }
                unsafe {
                    ARP_ACTIVE[ch] = false;
                    v[ch].set_frequency(BASE_FREQ[ch]);
                }
            }
            base += channel_stride;
        }
        self.arp_active_any = any_active;
    }
}

unsafe fn read_u16(base: *const u8, offset: usize) -> u16 {
    (base.add(offset) as *const u16).read_unaligned()
}

unsafe fn read_ptr(table: *const u16, index: usize) -> u16 {
    table.add(index).read_unaligned()
}
