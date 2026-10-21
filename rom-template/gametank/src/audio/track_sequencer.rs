use super::wavetable::{VOICE_COUNT, WAVETABLE, WAVETABLE_COUNT, voices};
use crate::console::Console;

const SAMPLE_RATE_REG: u8 = 0xA8;

macro_rules! instrument_table {
    ($n:literal) => {
        *include_bytes!(concat!(
            // gt-tracker's exported `instruments/` folder
            "../../../assets/instruments/instrument_",
            $n,
            ".bin"
        ))
    };
}

// The 8 fixed instrument slots, the envelope step rows and the volume scale
// don't fit in FIXED_FLASH alongside everything else, so they share bank 125.
const AUDIO_ASSET_BANK: u8 = 125;

#[unsafe(link_section = ".rodata.bank125")]
static INSTRUMENT_TABLES: [[u8; 256]; WAVETABLE_COUNT] = [
    instrument_table!(0),
    instrument_table!(1),
    instrument_table!(2),
    instrument_table!(3),
    instrument_table!(4),
    instrument_table!(5),
    instrument_table!(6),
    instrument_table!(7),
];

macro_rules! envelope_bin {
    ($n:literal) => {
        *include_bytes!(concat!(
            // gt-tracker's exported `envelopes/` folder
            "../../../assets/envelopes/envelope_",
            $n,
            ".bin"
        ))
    };
}

const NUM_ENVELOPES: usize = 8;
const ENVELOPE_MAX_STEPS: usize = 32;
const ENVELOPE_BIN_SIZE: usize = 3 + ENVELOPE_MAX_STEPS;

const ENVELOPE_BINS: [[u8; ENVELOPE_BIN_SIZE]; NUM_ENVELOPES] = [
    envelope_bin!(0),
    envelope_bin!(1),
    envelope_bin!(2),
    envelope_bin!(3),
    envelope_bin!(4),
    envelope_bin!(5),
    envelope_bin!(6),
    envelope_bin!(7),
];

// The bins are unpacked at compile time into three parallel tables so that
// nothing on the hot path needs a multiply the 6502 cannot do. The step table
// keeps a stride of ENVELOPE_MAX_STEPS, which makes a slot's row a shift.
// Only the counts and thresholds are small enough to keep in FIXED_FLASH;
// the step rows live in the asset bank alongside the volume scale.
const fn envelope_step_counts() -> [u8; NUM_ENVELOPES] {
    let mut counts = [0u8; NUM_ENVELOPES];
    let mut slot = 0;
    while slot < NUM_ENVELOPES {
        counts[slot] = ENVELOPE_BINS[slot][0];
        slot += 1;
    }
    counts
}

const fn envelope_thresholds() -> [u16; NUM_ENVELOPES] {
    let mut thresholds = [0u16; NUM_ENVELOPES];
    let mut slot = 0;
    while slot < NUM_ENVELOPES {
        thresholds[slot] =
            ENVELOPE_BINS[slot][1] as u16 | ((ENVELOPE_BINS[slot][2] as u16) << 8);
        slot += 1;
    }
    thresholds
}

const fn envelope_steps() -> [u8; NUM_ENVELOPES * ENVELOPE_MAX_STEPS] {
    let mut steps = [0u8; NUM_ENVELOPES * ENVELOPE_MAX_STEPS];
    let mut slot = 0;
    while slot < NUM_ENVELOPES {
        let mut step = 0;
        while step < ENVELOPE_MAX_STEPS {
            steps[slot * ENVELOPE_MAX_STEPS + step] = ENVELOPE_BINS[slot][3 + step];
            step += 1;
        }
        slot += 1;
    }
    steps
}

static ENVELOPE_STEP_COUNTS: [u8; NUM_ENVELOPES] = envelope_step_counts();
static ENVELOPE_THRESHOLDS: [u16; NUM_ENVELOPES] = envelope_thresholds();

#[unsafe(link_section = ".rodata.bank125")]
static ENVELOPE_STEPS: [u8; NUM_ENVELOPES * ENVELOPE_MAX_STEPS] = envelope_steps();

// Resolves an envelope step against the channel's set volume. Both axes are
// the same 24-level ladder and the ladder is logarithmic, so the product of
// the two gains is a sum of attenuations, which gt-tracker folds into this
// table at export. A row is the set volume, a column the envelope step.
#[unsafe(link_section = ".rodata.bank125")]
static VOLUME_SCALE: [u8; VOLUME_SCALE_SIZE] =
    *include_bytes!("../../../assets/envelopes/volume_scale.bin");
const VOLUME_SCALE_STRIDE: u16 = 32;
const VOLUME_SCALE_SIZE: usize = 24 * VOLUME_SCALE_STRIDE as usize;

const FX_ID_INSTRUMENT: u8 = 1;
const FX_ID_ARPEGGIO: u8 = 2;
const FX_ID_PITCH_UP: u8 = 3;
const FX_ID_PITCH_DOWN: u8 = 4;
const FX_ID_ENVELOPE: u8 = 5;

/// Drives the 7-channel wavetable synth from a gt-tracker export. Create one
/// sequencer per track, point it at the `<name>_track` descriptor, then call
/// `init_voices` once after loading the firmware and `tick` once per frame.
pub struct TrackSequencer {
    track: *const u8,
    // ROM bank the track descriptor/patterns live in. Every tick before
    // reading `track`, this bank gets selected, then switches to back.
    track_bank: u8,
    beat: u8,
    frame_acc: u16,
    tick_acc: u16,
    bpm: u16,
    speed: u16,
    pattern_idx: u8,
    pattern_count: u8,
    flow_count: u8,
    stopped: bool,
    // Set once the first beat has been triggered, so `tick` can prime beat 0
    // immediately instead of waiting a full beat-length at the track's
    // starting tempo before anything plays.
    primed: bool,
    // One bit per channel with an effect still running. Sub-beat ticks are
    // skipped entirely while it is zero, and each effect clears its own bit as
    // soon as it finishes rather than waiting for the next beat.
    fx_active_mask: u8,
    // Phase increments are stored once per track and referenced by a one-byte
    // index from the patterns.
    freq_table: *const u16,
}

// Channel base note pitch, written by `advance_beat`. This is different than the active
// note used by `advance_tick` which represents the frequency after applied effects.
static mut BASE_FREQ: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// Which glide effect is currently active
const ACTIVE_FX_NONE: u8 = 0;
const ACTIVE_FX_ARP: u8 = 1;
const ACTIVE_FX_PITCH: u8 = 2;
static mut ACTIVE_FX: [u8; VOICE_COUNT] = [ACTIVE_FX_NONE; VOICE_COUNT];
static mut ARP_X_FREQ: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
static mut ARP_Y_FREQ: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// Keep track if the channel has a 2-note or 3-note arpeggio
static mut ARP_STEP_COUNT: [u8; VOICE_COUNT] = [3; VOICE_COUNT];
// Each channel's current position within its own arpeggio cycle
static mut ARP_STEP: [u8; VOICE_COUNT] = [0; VOICE_COUNT];
// The baked frequency the glide is sliding toward
static mut PITCH_TARGET: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// The glide's current frequency, moved toward PITCH_TARGET by `speed` each tick
static mut PITCH_CUR: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// Track the volume column, so the channel can be restored
// without needing to read hardware state back
static mut CUR_VOL: [u8; VOICE_COUNT] = [0; VOICE_COUNT];
// Envelope slot assigned to the channel, persisting across beats like the
// instrument does. Any other value means no envelope.
static mut ENV_SLOT: [u8; VOICE_COUNT] = [NUM_ENVELOPES as u8; VOICE_COUNT];
// Offset of the channel's envelope row in ENVELOPE_STEPS
static mut ENV_STEP_BASE: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// Offset of the channel's set-volume row in VOLUME_SCALE
static mut ENV_SCALE_BASE: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
static mut ENV_STEP: [u8; VOICE_COUNT] = [0; VOICE_COUNT];
static mut ENV_LAST_STEP: [u8; VOICE_COUNT] = [0; VOICE_COUNT];
// Frame accumulator driving the envelope, advanced by bpm and drained at
// ENV_THRESHOLD. A zero threshold means the envelope has reached its final
// step and is holding, so the channel is skipped until the next beat.
static mut ENV_ACC: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
static mut ENV_THRESHOLD: [u16; VOICE_COUNT] = [0; VOICE_COUNT];
// An explicit note OFF holds the envelope silent until a new note or volume
static mut ENV_SUPPRESSED: [bool; VOICE_COUNT] = [false; VOICE_COUNT];

// See the gt-tracker README for the track descriptor layout
const TRACK_BPM_OFFSET: usize = 0;
const TRACK_SPEED_OFFSET: usize = 2;
const TRACK_PATTERN_COUNT_OFFSET: usize = 4;
const TRACK_HEADER_SIZE: usize = 6;

// A pattern block opens with its beat count and two channel bitmasks, so the
// sequencer can skip channels the pattern never writes to and skip the effect
// arrays for channels that have no effects.
const PATTERN_BEATS_OFFSET: usize = 0;
const PATTERN_CHANNEL_MASK_OFFSET: usize = 1;
const PATTERN_FX_MASK_OFFSET: usize = 2;
const PATTERN_HEADER_SIZE: usize = 3;
// Set in the channel mask when the pattern carries a sequence-command list.
const PATTERN_HAS_SEQ: u8 = 0x80;
// Bytes per entry in that list: beat, command, value, value2.
const SEQ_ENTRY_SIZE: usize = 4;

// Each beat packs a channel's volume and effect ID into one byte: volume in
// the low five bits, effect ID in the top three. The volume needs 24 values,
// which leaves 31 spare to mean "leave it alone" - a sentinel rather than a
// flag bit, because there is no eighth bit to spare.
const VOL_FX_VOLUME_MASK: u8 = 0x1F;
const VOL_FX_ID_SHIFT: u8 = 5;
const VOL_FX_ID_MASK: u8 = 0x07;
const VOL_FX_VOLUME_HOLD: u8 = 0x1F;
// An explicit note OFF bakes to this code: silence that also suspends the
// channel's envelope until a new note or an explicit volume arrives.
const VOL_FX_VOLUME_MUTE: u8 = 0x18;

const BEAT_THRESHOLD: u16 = 3600;

// seq_cmd_type values baked into each pattern's trailing per-beat arrays
const SEQ_CMD_STOP: u8 = 1;
const SEQ_CMD_TEMPO: u8 = 2;
const SEQ_CMD_SPEED: u8 = 3;
const SEQ_CMD_FLOW_COUNT: u8 = 4;
const SEQ_CMD_COUNT_JUMP: u8 = 5;
const SEQ_CMD_JUMP: u8 = 6;

impl TrackSequencer {
    // Create a sequencer for the given track descriptor
    pub fn new(console: &mut Console, track: *const u8, track_bank: u8, restore_bank: u8) -> Self {
        console.set_rom_bank(track_bank);
        let bpm = unsafe { read_u16(track, TRACK_BPM_OFFSET) };
        let speed = unsafe { read_u16(track, TRACK_SPEED_OFFSET) };
        let pattern_count = unsafe { *track.add(TRACK_PATTERN_COUNT_OFFSET) };
        let freq_table =
            unsafe { track.add(TRACK_HEADER_SIZE + pattern_count as usize * 2) as *const u16 };
        console.set_rom_bank(restore_bank);
        Self {
            track,
            track_bank,
            beat: 0,
            frame_acc: 0,
            tick_acc: 0,
            bpm,
            speed,
            pattern_idx: 0,
            pattern_count,
            flow_count: 0,
            stopped: false,
            primed: false,
            fx_active_mask: 0,
            freq_table,
        }
    }

    // Load the gt-tracker instrument tables into ACP RAM,
    // point each voice at a wavetable, mute all voices,
    // and set the audio_freq register. TODO: channels
    // should default to the first instrument perhaps?
    pub fn init_voices(&self, console: &mut Console, restore_bank: u8) {
        console.set_rom_bank(AUDIO_ASSET_BANK);
        console.audio.load_instruments(&INSTRUMENT_TABLES);
        console.set_rom_bank(restore_bank);

        let v = voices();
        for i in 0..VOICE_COUNT {
            v[i].set_wavetable(WAVETABLE[i]);
            v[i].set_volume(0);
        }
        unsafe { core::ptr::write_volatile(0x2006 as *mut u8, SAMPLE_RATE_REG) };
    }

    // Advance the sequencer by one frame every game loop. `restore_bank` us
    // gets back to where we started after the sequencer uses its `track_bank`
    pub fn tick(&mut self, console: &mut Console, restore_bank: u8) {
        if self.stopped {
            return;
        }

        if !self.primed {
            self.primed = true;
            console.set_rom_bank(self.track_bank);
            self.advance_beat(console);
            console.set_rom_bank(restore_bank);
            if self.stopped {
                return;
            }
        }

        self.frame_acc += self.bpm;

        self.advance_envelopes(console, restore_bank);

        if self.fx_active_mask != 0 {
            let mut remaining = self.speed;
            while remaining > 0 {
                // read_volatile prevents LLVM from trying to optimize this into an unsupported `__mulhi3`
                let bpm = unsafe { core::ptr::read_volatile(&self.bpm) };
                self.tick_acc += bpm;
                while self.tick_acc >= BEAT_THRESHOLD {
                    self.tick_acc -= BEAT_THRESHOLD;
                    self.advance_tick();
                }
                remaining -= 1;
            }
        }

        if self.frame_acc >= BEAT_THRESHOLD {
            self.frame_acc -= BEAT_THRESHOLD;
            console.set_rom_bank(self.track_bank);
            self.advance_beat(console);
            console.set_rom_bank(restore_bank);
        }
    }

    // Walk each channel's volume envelope on the frame clock. Envelopes
    // span an entire beat, so its threshold is the beat threshold
    // divided by the step count, which gt-tracker bakes at export.
    fn advance_envelopes(&mut self, console: &mut Console, restore_bank: u8) {
        let v = voices();
        console.set_rom_bank(AUDIO_ASSET_BANK);
        for ch in 0..VOICE_COUNT {
            unsafe {
                if ENV_THRESHOLD[ch] == 0 {
                    continue;
                }
                let bpm = core::ptr::read_volatile(&self.bpm);
                ENV_ACC[ch] += bpm;
                while ENV_ACC[ch] >= ENV_THRESHOLD[ch] {
                    ENV_ACC[ch] -= ENV_THRESHOLD[ch];
                    ENV_STEP[ch] += 1;
                    v[ch].set_volume(envelope_volume(ch));
                    if ENV_STEP[ch] >= ENV_LAST_STEP[ch] {
                        ENV_THRESHOLD[ch] = 0;
                        break;
                    }
                }
            }
        }
        console.set_rom_bank(restore_bank);
    }

    fn advance_tick(&mut self) {
        let v = voices();
        let speed = self.speed << 2;
        let mut mask = self.fx_active_mask;
        let mut bit = 1u8;
        unsafe {
            for ch in 0..VOICE_COUNT {
                if mask & bit == 0 {
                    bit <<= 1;
                    continue;
                }
                if ACTIVE_FX[ch] == ACTIVE_FX_ARP {
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
                } else if ACTIVE_FX[ch] == ACTIVE_FX_PITCH {
                    let cur = PITCH_CUR[ch];
                    let target = PITCH_TARGET[ch];
                    let next = if cur < target {
                        let stepped = cur.saturating_add(speed);
                        if stepped >= target {
                            ACTIVE_FX[ch] = ACTIVE_FX_NONE;
                            mask &= !bit;
                            target
                        } else {
                            stepped
                        }
                    } else if cur > target {
                        let stepped = cur.saturating_sub(speed);
                        if stepped <= target {
                            ACTIVE_FX[ch] = ACTIVE_FX_NONE;
                            mask &= !bit;
                            target
                        } else {
                            stepped
                        }
                    } else {
                        ACTIVE_FX[ch] = ACTIVE_FX_NONE;
                        mask &= !bit;
                        target
                    };
                    PITCH_CUR[ch] = next;
                    v[ch].set_frequency(next);
                }
                bit <<= 1;
            }
        }
        self.fx_active_mask = mask;
    }

    fn pattern_ptr(&self, pattern_idx: u8) -> *const u8 {
        let table = unsafe { self.track.add(TRACK_HEADER_SIZE) as *const u16 };
        let pattern_offset = unsafe { read_ptr(table, pattern_idx as usize) } as usize;
        unsafe { self.track.add(pattern_offset) }
    }

    // Resolve a one-byte note index against the track's frequency table. Slot 0
    // is reserved, so it always reads back as "no note here".
    fn freq(&self, index: u8) -> u16 {
        unsafe { read_ptr(self.freq_table, index as usize) }
    }

    // Processes sequence and channel commands for the current pattern_idx+beat
    fn advance_beat(&mut self, console: &mut Console) {
        loop {
            let pat = self.pattern_ptr(self.pattern_idx);
            let pattern_beats = unsafe { *pat.add(PATTERN_BEATS_OFFSET) } as usize;
            let channel_mask = unsafe { *pat.add(PATTERN_CHANNEL_MASK_OFFSET) };
            let fx_mask = unsafe { *pat.add(PATTERN_FX_MASK_OFFSET) };
            let mut data = unsafe { pat.add(PATTERN_HEADER_SIZE) };
            let beat = self.beat;

            let mut jumped = false;
            if channel_mask & PATTERN_HAS_SEQ != 0 {
                let seq_count = unsafe { *data } as usize;
                let entries = unsafe { data.add(1) };
                data = unsafe { entries.add(seq_count * SEQ_ENTRY_SIZE) };

                if let Some(entry) = unsafe { find_seq_entry(entries, seq_count, beat) } {
                    let seq_cmd_value = unsafe { *entry.add(2) };
                    let seq_cmd_value2 = unsafe { *entry.add(3) };
                    match unsafe { *entry.add(1) } {
                        SEQ_CMD_STOP => {
                            self.stopped = true;
                            return;
                        }
                        SEQ_CMD_TEMPO => {
                            self.bpm = seq_cmd_value as u16 | ((seq_cmd_value2 as u16) << 8);
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
                                self.pattern_idx =
                                    seq_cmd_value.min(self.pattern_count.saturating_sub(1));
                                self.beat = seq_cmd_value2;
                                jumped = true;
                            }
                        }
                        SEQ_CMD_JUMP => {
                            self.pattern_idx =
                                seq_cmd_value.min(self.pattern_count.saturating_sub(1));
                            self.beat = seq_cmd_value2;
                            jumped = true;
                        }
                        _ => {}
                    }
                }
            }
            if jumped {
                continue;
            }

            self.trigger_channels(data, pattern_beats, beat as usize, channel_mask, fx_mask);
            self.arm_envelopes(console);

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
        pattern_beats: usize,
        beat: usize,
        channel_mask: u8,
        fx_mask: u8,
    ) {
        // Channels sit back to back, so each one's arrays begin where the
        // previous channel's ended - two arrays wide normally, five when the
        // channel uses effects. Walking them costs adds rather than a multiply.
        let to_vol_fx = pattern_beats;
        let to_fx_x = to_vol_fx + pattern_beats;
        let to_fx_arg_x = to_fx_x + pattern_beats;
        let to_fx_arg_y = to_fx_arg_x + pattern_beats;
        let plain_channel = to_fx_x;
        let fx_channel = to_fx_arg_y + pattern_beats;

        let v = voices();
        let mut cur = unsafe { data.add(beat) };
        let mut bit = 1u8;
        let mut active = 0u8;

        for ch in 0..VOICE_COUNT {
            // A channel this pattern never writes to reads as an empty beat,
            // which still means something: it ends any effect left running.
            let mut note_idx = 0u8;
            let mut vol_fx = 0u8;
            let mut fx_x = 0u8;
            let mut fx_arg_x_idx = 0u8;
            let mut fx_arg_y_idx = 0u8;
            if channel_mask & bit != 0 {
                note_idx = unsafe { *cur };
                vol_fx = unsafe { *cur.add(to_vol_fx) };
                if fx_mask & bit != 0 {
                    fx_x = unsafe { *cur.add(to_fx_x) };
                    fx_arg_x_idx = unsafe { *cur.add(to_fx_arg_x) };
                    fx_arg_y_idx = unsafe { *cur.add(to_fx_arg_y) };
                    cur = unsafe { cur.add(fx_channel) };
                } else {
                    cur = unsafe { cur.add(plain_channel) };
                }
            }

            // Whether a glide effect was in progress before this beat, so
            // switching to a non-glide fx only resets hardware that a glide
            // actually left in a non-resting state.
            let prev_fx = unsafe { ACTIVE_FX[ch] };
            let had_freq_fx = prev_fx == ACTIVE_FX_ARP || prev_fx == ACTIVE_FX_PITCH;

            let new_note = note_idx != 0;
            if new_note {
                let freq = self.freq(note_idx);
                unsafe {
                    BASE_FREQ[ch] = freq;
                    ENV_SUPPRESSED[ch] = false;
                }
            }
            let vol = vol_fx & VOL_FX_VOLUME_MASK;
            if vol == VOL_FX_VOLUME_MUTE {
                unsafe {
                    CUR_VOL[ch] = 0;
                    ENV_SCALE_BASE[ch] = 0;
                    ENV_SUPPRESSED[ch] = true;
                }
                v[ch].set_volume(0);
            } else if vol != VOL_FX_VOLUME_HOLD {
                unsafe {
                    CUR_VOL[ch] = vol;
                    ENV_SCALE_BASE[ch] = (vol as u16) << 5;
                    ENV_SUPPRESSED[ch] = false;
                }
                v[ch].set_volume(vol);
            }

            let fx_id = (vol_fx >> VOL_FX_ID_SHIFT) & VOL_FX_ID_MASK;
            if fx_id == FX_ID_ARPEGGIO {
                let arp_x = self.freq(fx_arg_x_idx);
                let arp_y = self.freq(fx_arg_y_idx);
                unsafe {
                    ACTIVE_FX[ch] = ACTIVE_FX_ARP;
                    ARP_X_FREQ[ch] = arp_x;
                    ARP_Y_FREQ[ch] = arp_y;
                    // No baked third note means a two-note arpeggio.
                    ARP_STEP_COUNT[ch] = if fx_arg_y_idx == 0 { 2 } else { 3 };
                    ARP_STEP[ch] = 0;
                }
                active |= bit;
            } else if fx_id == FX_ID_PITCH_UP || fx_id == FX_ID_PITCH_DOWN {
                let target = self.freq(fx_arg_x_idx);
                unsafe {
                    ACTIVE_FX[ch] = ACTIVE_FX_PITCH;
                    PITCH_TARGET[ch] = target;
                    PITCH_CUR[ch] = BASE_FREQ[ch];
                    v[ch].set_frequency(PITCH_CUR[ch]);
                }
                active |= bit;
            } else if fx_id == FX_ID_ENVELOPE {
                unsafe {
                    ENV_SLOT[ch] = fx_x;
                    if (fx_x as usize) < NUM_ENVELOPES {
                        ENV_STEP_BASE[ch] = (fx_x as u16) << 5;
                    }
                    ACTIVE_FX[ch] = ACTIVE_FX_NONE;
                    if new_note || had_freq_fx {
                        v[ch].set_frequency(BASE_FREQ[ch]);
                    }
                }
            } else if fx_id == FX_ID_INSTRUMENT {
                v[ch].set_wavetable(WAVETABLE[fx_x as usize]);
                unsafe {
                    ACTIVE_FX[ch] = ACTIVE_FX_NONE;
                    if new_note || had_freq_fx {
                        v[ch].set_frequency(BASE_FREQ[ch]);
                    }
                }
            } else {
                unsafe {
                    ACTIVE_FX[ch] = ACTIVE_FX_NONE;
                    if new_note || had_freq_fx {
                        v[ch].set_frequency(BASE_FREQ[ch]);
                    }
                }
            }

            bit <<= 1;
        }
        self.fx_active_mask = active;
    }

    fn arm_envelopes(&mut self, console: &mut Console) {
        let v = voices();
        console.set_rom_bank(AUDIO_ASSET_BANK);
        for ch in 0..VOICE_COUNT {
            unsafe {
                let slot = ENV_SLOT[ch];
                if ENV_SUPPRESSED[ch] {
                    ENV_THRESHOLD[ch] = 0;
                } else if slot as usize >= NUM_ENVELOPES {
                    ENV_THRESHOLD[ch] = 0;
                    v[ch].set_volume(CUR_VOL[ch]);
                } else {
                    let last_step = ENVELOPE_STEP_COUNTS[slot as usize] - 1;
                    ENV_LAST_STEP[ch] = last_step;
                    ENV_STEP[ch] = 0;
                    ENV_ACC[ch] = 0;
                    ENV_THRESHOLD[ch] = if last_step == 0 {
                        0
                    } else {
                        ENVELOPE_THRESHOLDS[slot as usize]
                    };
                    v[ch].set_volume(envelope_volume(ch));
                }
            }
        }
        console.set_rom_bank(self.track_bank);
    }
}

unsafe fn envelope_volume(ch: usize) -> u8 {
    unsafe {
        let step = ENVELOPE_STEPS[(ENV_STEP_BASE[ch] + ENV_STEP[ch] as u16) as usize];
        VOLUME_SCALE[(ENV_SCALE_BASE[ch] + step as u16) as usize]
    }
}

// Entries are ordered by beat, so a miss is known as soon as a later beat turns up.
unsafe fn find_seq_entry(entries: *const u8, count: usize, beat: u8) -> Option<*const u8> {
    let mut entry = entries;
    for _ in 0..count {
        let entry_beat = unsafe { *entry };
        if entry_beat == beat {
            return Some(entry);
        }
        if entry_beat > beat {
            return None;
        }
        entry = unsafe { entry.add(SEQ_ENTRY_SIZE) };
    }
    None
}

unsafe fn read_u16(base: *const u8, offset: usize) -> u16 {
    unsafe { (base.add(offset) as *const u16).read_unaligned() }
}

unsafe fn read_ptr(table: *const u16, index: usize) -> u16 {
    unsafe { table.add(index).read_unaligned() }
}
