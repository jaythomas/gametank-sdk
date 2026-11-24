use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, unbounded};
use gte_acp::{ARAM, AcpBus, audio_output::GameTankAudio};
use gte_w65c02s::W65C02S;
use indexmap::IndexMap;

use crate::file::NUM_INSTRUMENTS;
use crate::track_state::{ChannelState, RowCmds, TrackState, note_freq};
use crate::tracker::{Beat, Pattern, SequencerCmd, empty_pattern};
use crate::tracker::{
    DEFAULT_PATTERN_BEATS, MAX_VOLUME, NUM_ENVELOPES, PATTERN_BEATS, VOLUME_LEVELS, envelope_slot,
};
use crate::volume_scale::{SCALE_STRIDE, scale_table};

const FIRMWARE: &[u8; 4096] =
    include_bytes!("../../../../rom-template/gametank/audiofw/wavetable.bin");

pub const AUDIO_CHANNELS: usize = 7;

const WAVETABLE: [u16; NUM_INSTRUMENTS] = [
    0x0700, 0x0800, 0x0900, 0x0A00, 0x0B00, 0x0C00, 0x0D00, 0x0E00,
];

/// ACP RAM addresses of the firmware's gain tables, indexed by volume level.
/// Volume is a pointer into one of these rather than a level, so the firmware
/// can scale a sample with a single indexed load. Four tables are interleaved
/// per 256-byte block, so level L is at $0100 + 256 * (L / 4) + L % 4.
const GAIN_PAGE: [u16; VOLUME_LEVELS] = {
    let mut pages = [0u16; VOLUME_LEVELS];
    let mut level = 0;
    while level < VOLUME_LEVELS {
        pages[level] = 0x0100 + (level as u16 / 4) * 0x100 + level as u16 % 4;
        level += 1;
    }
    pages
};

/// Voice parameters are operands inside each voice's code block in the firmware.
/// Voice 1 starts here; voice 0 is pipelined by the firmware and sits after voice 6.
const VOICE_CODE_BASE: usize = 0x0F0B;
const VOICE_BLOCK_SIZE: usize = 29;
const VOICE_0_CODE: usize = 0x0FB9;

fn voice_code(ch: usize) -> usize {
    if ch == 0 {
        VOICE_0_CODE
    } else {
        VOICE_CODE_BASE + (ch - 1) * VOICE_BLOCK_SIZE
    }
}
const FREQ_LO_OFFSET: usize = 4;
const FREQ_HI_OFFSET: usize = 10;
const WAVE_OFFSET: usize = 15;
const GAIN_OFFSET: usize = 19;

/// ACP clocks between samples at the given `audio_freq` register value.
fn acp_cycles_per_sample(reg: u8) -> i32 {
    crate::sample_rate::sample_rate_divisor(reg) as i32 * 4
}

pub enum PlayerCmd {
    Play(usize, usize, TrackState),
    Preview(usize, Beat, TrackState),
    Pause,
    UpdatePatterns(Vec<Pattern>, Vec<u8>),
    UpdateWaveform(usize, Box<[u8; 256]>),
    UpdateEnvelopes(Vec<Vec<u8>>),
    UpdateTuningNotes(IndexMap<String, f64>),
    SetChannelMuted(usize, bool),
}

struct Preview {
    ch: usize,
    samples_left: f64,
}

struct PlayerInner {
    cmd_rx: Receiver<PlayerCmd>,
    current_row_out: Arc<AtomicUsize>,
    current_pattern_out: Arc<AtomicUsize>,
    is_playing_out: Arc<AtomicBool>,

    acp: W65C02S,
    acp_bus: AcpBus,
    acp_sample_rate: f64,
    acp_sample_rate_reg: u8,

    audio_out: GameTankAudio,
    output_sample_rate: f64,
    current_buffer: Option<[f32; 64]>,
    buffer_position: usize,
    patterns: Vec<Pattern>,
    pattern_beats_list: Vec<u8>,
    current_pattern_idx: usize,
    flow_count: u8,
    tuning_notes: IndexMap<String, f64>,

    playing: bool,
    preview: Option<Preview>,
    current_row: usize,
    fx_speed: u8,
    samples_per_beat: f64,
    samples_until_next_beat: f64,
    samples_per_tick: f64,
    samples_until_next_tick: f64,
    tick_count: u8,
    output_channels: usize,
    remembered_vol: [u8; AUDIO_CHANNELS],
    muted: [bool; AUDIO_CHANNELS],
    channel_muted: [bool; AUDIO_CHANNELS],
    base_freq: [u16; AUDIO_CHANNELS],
    current_instrument: [usize; AUDIO_CHANNELS],
    arp_active: [bool; AUDIO_CHANNELS],
    arp_x_freq: [u16; AUDIO_CHANNELS],
    arp_y_freq: [u16; AUDIO_CHANNELS],
    arp_step_count: [u8; AUDIO_CHANNELS],
    arp_step: [u8; AUDIO_CHANNELS],
    pitch_active: [bool; AUDIO_CHANNELS],
    pitch_target: [u16; AUDIO_CHANNELS],
    pitch_cur: [u16; AUDIO_CHANNELS],
    envelopes: Vec<Vec<u8>>,
    volume_scale: Vec<u8>,
    env_slot: [Option<usize>; AUDIO_CHANNELS],
    env_step: [usize; AUDIO_CHANNELS],
    env_running: [bool; AUDIO_CHANNELS],
    env_suppressed: [bool; AUDIO_CHANNELS],
    samples_per_env_step: [f64; AUDIO_CHANNELS],
    samples_until_env_step: [f64; AUDIO_CHANNELS],
    cur_note_name: [Option<String>; AUDIO_CHANNELS],
}

impl PlayerInner {
    fn new(
        cmd_rx: Receiver<PlayerCmd>,
        current_row_out: Arc<AtomicUsize>,
        current_pattern_out: Arc<AtomicUsize>,
        is_playing_out: Arc<AtomicBool>,
        output_sample_rate: f64,
        output_channels: usize,
        bpm: u16,
    ) -> Self {
        let sample_rate_reg = crate::sample_rate::SAMPLE_RATE_REG;
        unsafe {
            let aram_ptr = std::ptr::addr_of_mut!(ARAM);
            (*aram_ptr).copy_from_slice(FIRMWARE);
        }

        let mut acp = W65C02S::new();
        acp.reset();

        let acp_bus = AcpBus {
            irq_counter: acp_cycles_per_sample(sample_rate_reg),
            ..Default::default()
        };

        let acp_sample_rate = crate::sample_rate::sample_rate_reg_to_hz(sample_rate_reg);
        let audio_out = GameTankAudio::new(acp_sample_rate, output_sample_rate);

        let samples_per_beat = output_sample_rate * 60.0 / (bpm.max(1) as f64);
        let fx_speed: u8 = 6;
        let samples_per_tick = samples_per_beat / fx_speed.max(1) as f64;

        Self {
            cmd_rx,
            current_row_out,
            current_pattern_out,
            is_playing_out,
            acp,
            acp_bus,
            acp_sample_rate,
            acp_sample_rate_reg: sample_rate_reg,
            audio_out,
            output_sample_rate,
            current_buffer: None,
            buffer_position: 0,
            patterns: vec![empty_pattern()],
            pattern_beats_list: vec![DEFAULT_PATTERN_BEATS as u8],
            current_pattern_idx: 0,
            flow_count: 0,
            tuning_notes: IndexMap::new(),
            playing: false,
            preview: None,
            current_row: 0,
            fx_speed,
            samples_per_beat,
            samples_until_next_beat: samples_per_beat,
            samples_per_tick,
            samples_until_next_tick: samples_per_tick,
            tick_count: 0,
            output_channels,
            remembered_vol: [0; AUDIO_CHANNELS],
            muted: [false; AUDIO_CHANNELS],
            channel_muted: [false; AUDIO_CHANNELS],
            base_freq: [0; AUDIO_CHANNELS],
            current_instrument: std::array::from_fn(|ch| ch),
            arp_active: [false; AUDIO_CHANNELS],
            arp_x_freq: [0; AUDIO_CHANNELS],
            arp_y_freq: [0; AUDIO_CHANNELS],
            arp_step_count: [3; AUDIO_CHANNELS],
            arp_step: [0; AUDIO_CHANNELS],
            pitch_active: [false; AUDIO_CHANNELS],
            pitch_target: [0; AUDIO_CHANNELS],
            pitch_cur: [0; AUDIO_CHANNELS],
            envelopes: vec![vec![MAX_VOLUME]; NUM_ENVELOPES],
            volume_scale: scale_table(),
            env_slot: [None; AUDIO_CHANNELS],
            env_step: [0; AUDIO_CHANNELS],
            env_running: [false; AUDIO_CHANNELS],
            env_suppressed: [false; AUDIO_CHANNELS],
            samples_per_env_step: [0.0; AUDIO_CHANNELS],
            samples_until_env_step: [0.0; AUDIO_CHANNELS],
            cur_note_name: std::array::from_fn(|_| None),
        }
    }

    fn recompute_tick_timing(&mut self) {
        self.samples_per_tick = self.samples_per_beat / self.fx_speed.max(1) as f64;
    }

    fn set_timing(&mut self, bpm: u16, fx_speed: u8) {
        self.samples_per_beat = self.output_sample_rate * 60.0 / (bpm.max(1) as f64);
        self.fx_speed = fx_speed;
        self.recompute_tick_timing();
    }

    fn silence_output(&mut self) {
        for ch in 0..AUDIO_CHANNELS {
            self.set_voice_volume(ch, 0);
        }
        self.audio_out = GameTankAudio::new(self.acp_sample_rate, self.output_sample_rate);
        self.current_buffer = None;
        self.buffer_position = 0;
    }

    fn start_preview(&mut self, ch: usize, beat: &Beat, state: &TrackState) {
        self.preview = Some(Preview {
            ch,
            samples_left: 0.0,
        });
        for c in 0..AUDIO_CHANNELS {
            self.set_voice_volume(c, 0);
            self.arp_active[c] = false;
            self.pitch_active[c] = false;
            self.env_running[c] = false;
        }
        self.set_timing(state.bpm, state.fx_speed);
        self.apply_channel_state(ch, &state.channels[ch]);
        self.arp_step[ch] = 0;
        self.trigger_channel(ch, RowCmds::from_beat(beat));
        self.samples_until_next_tick = self.samples_per_tick.max(1.0);
        if let Some(preview) = &mut self.preview {
            preview.samples_left = self.samples_per_beat;
        }
    }

    fn pattern_beats(&self, pattern_idx: usize) -> usize {
        self.pattern_beats_list
            .get(pattern_idx)
            .copied()
            .unwrap_or(DEFAULT_PATTERN_BEATS as u8)
            .clamp(1, PATTERN_BEATS as u8) as usize
    }

    fn process_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                PlayerCmd::Play(pattern_idx, row, state) => {
                    self.preview = None;
                    self.playing = true;
                    self.current_pattern_idx =
                        pattern_idx.min(self.patterns.len().saturating_sub(1));
                    self.current_row = row;
                    self.flow_count = state.flow_count;
                    self.tick_count = 0;
                    self.set_timing(state.bpm, state.fx_speed);
                    for ch in 0..AUDIO_CHANNELS {
                        self.apply_channel_state(ch, &state.channels[ch]);
                    }
                    for ch in 0..AUDIO_CHANNELS {
                        self.set_voice_volume(
                            ch,
                            if self.muted[ch] {
                                0
                            } else {
                                self.remembered_vol[ch]
                            },
                        );
                    }
                    self.trigger_row();
                    self.samples_until_next_beat = self.samples_per_beat;
                    self.samples_until_next_tick = self.samples_per_tick;
                    self.current_row_out
                        .store(self.current_row, Ordering::Relaxed);
                    self.current_pattern_out
                        .store(self.current_pattern_idx, Ordering::Relaxed);
                    self.is_playing_out.store(true, Ordering::Relaxed);
                }
                PlayerCmd::Preview(ch, beat, state) => {
                    if !self.playing && ch < AUDIO_CHANNELS {
                        self.start_preview(ch, &beat, &state);
                    }
                }
                PlayerCmd::Pause => {
                    self.playing = false;
                    self.preview = None;
                    self.is_playing_out.store(false, Ordering::Relaxed);
                    self.silence_output();
                }
                PlayerCmd::UpdatePatterns(patterns, beats_list) => {
                    if !patterns.is_empty() {
                        self.patterns = patterns;
                        self.pattern_beats_list = beats_list;
                        if self.current_pattern_idx >= self.patterns.len() {
                            self.current_pattern_idx = self.patterns.len() - 1;
                        }
                    }
                }
                PlayerCmd::UpdateWaveform(idx, wf) => {
                    if idx < NUM_INSTRUMENTS {
                        self.write_waveform(idx, &wf);
                    }
                }
                PlayerCmd::UpdateEnvelopes(envelopes) => {
                    self.envelopes = envelopes;
                    for ch in 0..AUDIO_CHANNELS {
                        self.env_running[ch] = false;
                    }
                }
                PlayerCmd::UpdateTuningNotes(notes) => {
                    self.tuning_notes = notes;
                }
                PlayerCmd::SetChannelMuted(ch, want_muted) => {
                    if ch < AUDIO_CHANNELS {
                        self.channel_muted[ch] = want_muted;
                        if want_muted {
                            self.set_voice_volume(ch, 0);
                        } else {
                            let vol = self.current_channel_volume(ch);
                            self.set_voice_volume(ch, vol);
                        }
                    }
                }
            }
        }
    }

    fn write_waveform(&mut self, idx: usize, waveform: &[u8; 256]) {
        let base = WAVETABLE[idx] as usize;
        unsafe {
            let aram_ptr = std::ptr::addr_of_mut!(ARAM);
            for (dst, src) in (&mut (*aram_ptr))[base..base + 256]
                .iter_mut()
                .zip(waveform.iter())
            {
                *dst = *src & 0xFC;
            }
        }
    }

    fn set_voice_frequency(&mut self, ch: usize, freq: u16) {
        let base = voice_code(ch);
        unsafe {
            let aram_ptr = std::ptr::addr_of_mut!(ARAM);
            (*aram_ptr)[base + FREQ_LO_OFFSET] = (freq & 0xFF) as u8;
            (*aram_ptr)[base + FREQ_HI_OFFSET] = (freq >> 8) as u8;
        }
    }

    fn set_voice_waveptr(&mut self, ch: usize, waveform_idx: usize) {
        let base = voice_code(ch) + WAVE_OFFSET;
        let ptr = WAVETABLE[waveform_idx];
        unsafe {
            let aram_ptr = std::ptr::addr_of_mut!(ARAM);
            (*aram_ptr)[base] = (ptr & 0xFF) as u8;
            (*aram_ptr)[base + 1] = (ptr >> 8) as u8;
        }
    }

    fn set_voice_volume(&mut self, ch: usize, volume: u8) {
        let base = voice_code(ch) + GAIN_OFFSET;
        let previewing = self.preview.as_ref().is_some_and(|p| p.ch == ch);
        let level = if self.channel_muted[ch] && !previewing {
            0
        } else {
            volume.min(MAX_VOLUME)
        };
        let page = GAIN_PAGE[level as usize];
        unsafe {
            let aram_ptr = std::ptr::addr_of_mut!(ARAM);
            (*aram_ptr)[base] = (page & 0xFF) as u8;
            (*aram_ptr)[base + 1] = (page >> 8) as u8;
        }
    }

    fn envelope_steps(&self, slot: usize) -> &[u8] {
        match self.envelopes.get(slot) {
            Some(steps) if !steps.is_empty() => steps.as_slice(),
            _ => &[MAX_VOLUME],
        }
    }

    fn scaled_volume(&self, ch: usize, step_level: u8) -> u8 {
        let row = (self.remembered_vol[ch].min(MAX_VOLUME) as usize) * SCALE_STRIDE;
        self.volume_scale
            .get(row + step_level.min(MAX_VOLUME) as usize)
            .copied()
            .unwrap_or(self.remembered_vol[ch])
    }

    fn envelope_is_driving(&self, ch: usize) -> bool {
        self.env_slot[ch].is_some() && !self.env_suppressed[ch] && !self.muted[ch]
    }

    fn current_channel_volume(&self, ch: usize) -> u8 {
        if self.muted[ch] {
            0
        } else if let (true, Some(slot)) = (self.envelope_is_driving(ch), self.env_slot[ch]) {
            let steps = self.envelope_steps(slot);
            let step = self.env_step[ch].min(steps.len() - 1);
            self.scaled_volume(ch, steps[step])
        } else {
            self.remembered_vol[ch]
        }
    }

    fn arm_envelope(&mut self, ch: usize) {
        let Some(slot) = self.env_slot[ch] else {
            self.env_running[ch] = false;
            return;
        };
        let count = self.envelope_steps(slot).len();
        let per_step = (self.samples_per_beat / count as f64).max(1.0);
        self.samples_per_env_step[ch] = per_step;
        self.samples_until_env_step[ch] = per_step;
        self.env_step[ch] = 0;
        self.env_running[ch] = count > 1;
    }

    fn advance_envelopes(&mut self) {
        for ch in 0..AUDIO_CHANNELS {
            if !self.env_running[ch] || !self.envelope_is_driving(ch) {
                continue;
            }
            self.samples_until_env_step[ch] -= 1.0;
            while self.samples_until_env_step[ch] <= 0.0 && self.env_running[ch] {
                self.samples_until_env_step[ch] += self.samples_per_env_step[ch];
                let slot = self.env_slot[ch].unwrap();
                let last = self.envelope_steps(slot).len() - 1;
                if self.env_step[ch] < last {
                    self.env_step[ch] += 1;
                }
                if self.env_step[ch] == last {
                    self.env_running[ch] = false;
                }
                let vol = self.current_channel_volume(ch);
                self.set_voice_volume(ch, vol);
            }
        }
    }

    fn apply_channel_state(&mut self, ch: usize, state: &ChannelState) {
        self.remembered_vol[ch] = state.vol;
        self.muted[ch] = state.muted;
        self.cur_note_name[ch] = state.note_name.clone();
        self.base_freq[ch] = state.base_freq;
        self.current_instrument[ch] = state.instrument;
        self.set_voice_waveptr(ch, state.instrument);
        self.arp_active[ch] = false;
        self.pitch_active[ch] = false;
        self.env_slot[ch] = state.env_slot;
        self.env_suppressed[ch] = state.env_suppressed;
        self.env_step[ch] = 0;
        self.env_running[ch] = false;
    }

    // Process sequencer command and channel commands for the current row
    fn trigger_row(&mut self) {
        loop {
            let pattern_idx = self.current_pattern_idx;
            let row = self.current_row;

            if let Some(sqc) = self.patterns[pattern_idx][0][row].sqc.clone() {
                match sqc {
                    SequencerCmd::Stop => {
                        self.playing = false;
                        self.is_playing_out.store(false, Ordering::Relaxed);
                        for ch in 0..AUDIO_CHANNELS {
                            self.set_voice_volume(ch, 0);
                        }
                        return;
                    }
                    SequencerCmd::Tempo(bpm) => {
                        self.set_timing(bpm, self.fx_speed);
                    }
                    SequencerCmd::FxSpeed(fx_speed) => {
                        self.fx_speed = fx_speed;
                        self.recompute_tick_timing();
                    }
                    SequencerCmd::FlowCount(count) => {
                        self.flow_count = count;
                    }
                    SequencerCmd::CountJump(pat, beat) => {
                        if self.flow_count > 0 {
                            self.flow_count -= 1;
                            let target_pattern = (pat as usize).min(self.patterns.len() - 1);
                            let target_beat =
                                (beat as usize).min(self.pattern_beats(target_pattern) - 1);
                            self.current_pattern_idx = target_pattern;
                            self.current_row = target_beat;
                            self.current_pattern_out
                                .store(target_pattern, Ordering::Relaxed);
                            self.current_row_out.store(target_beat, Ordering::Relaxed);
                            continue;
                        }
                    }
                    SequencerCmd::Jump(pat, beat) => {
                        let target_pattern = (pat as usize).min(self.patterns.len() - 1);
                        let target_beat =
                            (beat as usize).min(self.pattern_beats(target_pattern) - 1);
                        self.current_pattern_idx = target_pattern;
                        self.current_row = target_beat;
                        self.current_pattern_out
                            .store(target_pattern, Ordering::Relaxed);
                        self.current_row_out.store(target_beat, Ordering::Relaxed);
                        continue;
                    }
                }
            }

            self.trigger_channels(pattern_idx, row);
            return;
        }
    }

    fn trigger_channels(&mut self, pattern_idx: usize, row: usize) {
        for ch in 0..AUDIO_CHANNELS {
            let cmds = RowCmds::from_beat(&self.patterns[pattern_idx][ch + 1][row]);
            self.trigger_channel(ch, cmds);
        }
    }

    fn trigger_channel(&mut self, ch: usize, cmds: RowCmds) {
        let RowCmds {
            note: maybe_note,
            vol: maybe_vol,
            note_off,
            arp: maybe_arp,
            instrument: maybe_instrument,
            pitch_up: maybe_pitch_up,
            pitch_down: maybe_pitch_down,
            envelope: maybe_envelope,
        } = cmds;

        if let Some(idx) = maybe_instrument {
            self.current_instrument[ch] = idx;
            self.set_voice_waveptr(ch, idx);
        }

        if let Some(slot) = maybe_envelope {
            self.env_slot[ch] = envelope_slot(slot);
        }

        if let Some(v) = maybe_vol {
            self.remembered_vol[ch] = v;
            self.muted[ch] = false;
            self.env_suppressed[ch] = false;
            self.set_voice_volume(ch, v);
        } else if note_off {
            self.muted[ch] = true;
            self.env_suppressed[ch] = true;
            self.set_voice_volume(ch, 0);
        }

        if let Some(note_name) = &maybe_note {
            self.cur_note_name[ch] = Some(note_name.clone());
            if let Some(&freq_hz) = self.tuning_notes.get(note_name.as_str()) {
                self.base_freq[ch] = note_freq(freq_hz, self.acp_sample_rate);
                self.env_suppressed[ch] = false;
                if maybe_vol.is_none() && self.muted[ch] {
                    self.muted[ch] = false;
                    self.set_voice_volume(ch, self.remembered_vol[ch]);
                }
            }
        }

        if self.envelope_is_driving(ch) {
            self.arm_envelope(ch);
        } else {
            self.env_running[ch] = false;
            self.env_step[ch] = 0;
        }
        let vol = self.current_channel_volume(ch);
        self.set_voice_volume(ch, vol);

        if let Some((x, y)) = maybe_arp {
            let note_name = maybe_note
                .clone()
                .or_else(|| self.cur_note_name[ch].clone());
            if let Some(note_name) = &note_name {
                self.arp_x_freq[ch] = self
                    .arp_offset_freq(note_name, x)
                    .unwrap_or(self.base_freq[ch]);
                self.arp_step_count[ch] = match y {
                    Some(y) => {
                        self.arp_y_freq[ch] = self
                            .arp_offset_freq(note_name, y)
                            .unwrap_or(self.base_freq[ch]);
                        3
                    }
                    None => 2,
                };
                self.arp_active[ch] = true;
                self.pitch_active[ch] = false;
            } else {
                self.arp_active[ch] = false;
                self.pitch_active[ch] = false;
                self.set_voice_frequency(ch, self.base_freq[ch]);
            }
        } else if let Some(x) = maybe_pitch_up {
            self.arp_active[ch] = false;
            let note_name = maybe_note
                .clone()
                .or_else(|| self.cur_note_name[ch].clone());
            if let Some(note_name) = &note_name {
                self.pitch_target[ch] = self
                    .arp_offset_freq(note_name, x)
                    .unwrap_or(self.base_freq[ch]);
                self.pitch_cur[ch] = self.base_freq[ch];
                self.pitch_active[ch] = true;
                self.set_voice_frequency(ch, self.pitch_cur[ch]);
            } else {
                self.pitch_active[ch] = false;
                self.set_voice_frequency(ch, self.base_freq[ch]);
            }
        } else if let Some(x) = maybe_pitch_down {
            self.arp_active[ch] = false;
            let note_name = maybe_note
                .clone()
                .or_else(|| self.cur_note_name[ch].clone());
            if let Some(note_name) = &note_name {
                self.pitch_target[ch] = self
                    .pitch_down_offset_freq(note_name, x)
                    .unwrap_or(self.base_freq[ch]);
                self.pitch_cur[ch] = self.base_freq[ch];
                self.pitch_active[ch] = true;
                self.set_voice_frequency(ch, self.pitch_cur[ch]);
            } else {
                self.pitch_active[ch] = false;
                self.set_voice_frequency(ch, self.base_freq[ch]);
            }
        } else {
            self.arp_active[ch] = false;
            self.pitch_active[ch] = false;
            self.set_voice_frequency(ch, self.base_freq[ch]);
        }
    }

    fn arp_offset_freq(&self, note_name: &str, steps: u8) -> Option<u16> {
        let idx = self.tuning_notes.get_index_of(note_name)?;
        let target_idx = (idx + steps as usize).min(self.tuning_notes.len().saturating_sub(1));
        let (_, &hz) = self.tuning_notes.get_index(target_idx)?;
        Some(note_freq(hz, self.acp_sample_rate))
    }

    fn pitch_down_offset_freq(&self, note_name: &str, steps: u8) -> Option<u16> {
        let idx = self.tuning_notes.get_index_of(note_name)?;
        let target_idx = idx.saturating_sub(steps as usize);
        let (_, &hz) = self.tuning_notes.get_index(target_idx)?;
        Some(note_freq(hz, self.acp_sample_rate))
    }

    fn advance_tick(&mut self) {
        for ch in 0..AUDIO_CHANNELS {
            if self.arp_active[ch] {
                let freq = match self.arp_step[ch] {
                    0 => self.base_freq[ch],
                    1 => self.arp_x_freq[ch],
                    _ => self.arp_y_freq[ch],
                };
                self.set_voice_frequency(ch, freq);
                self.arp_step[ch] = if self.arp_step[ch] + 1 >= self.arp_step_count[ch] {
                    0
                } else {
                    self.arp_step[ch] + 1
                };
            } else if self.pitch_active[ch] {
                let speed = (self.fx_speed as u16) * 4;
                let cur = self.pitch_cur[ch];
                let target = self.pitch_target[ch];
                let next = if cur < target {
                    let stepped = cur.saturating_add(speed);
                    if stepped >= target {
                        self.pitch_active[ch] = false;
                        target
                    } else {
                        stepped
                    }
                } else if cur > target {
                    let stepped = cur.saturating_sub(speed);
                    if stepped <= target {
                        self.pitch_active[ch] = false;
                        target
                    } else {
                        stepped
                    }
                } else {
                    self.pitch_active[ch] = false;
                    target
                };
                self.pitch_cur[ch] = next;
                self.set_voice_frequency(ch, next);
            }
        }
    }

    fn run_acp_until_sample(&mut self) -> bool {
        let cycles_per_sample = acp_cycles_per_sample(self.acp_sample_rate_reg);
        let max_cycles = cycles_per_sample * 8;
        let mut cycles_run = 0;

        loop {
            let acp_cycles = self.acp.step(&mut self.acp_bus);
            cycles_run += acp_cycles;
            self.acp_bus.irq_counter -= acp_cycles;

            self.acp.set_irq(false);
            self.acp.set_nmi(false);

            if self.acp_bus.irq_counter <= 0 {
                self.acp_bus.irq_counter += cycles_per_sample;
                self.acp.set_irq(true);

                let sample_u8 = self.acp_bus.sample;
                let _ = self.audio_out.producer.push(sample_u8);
                return true;
            }

            if cycles_run >= max_cycles {
                return false;
            }
        }
    }

    fn fill_output(&mut self, data: &mut [f32]) {
        self.process_commands();

        // Prevent buffer underrun
        if !self.playing && self.preview.is_none() {
            data.fill(0.0);
            return;
        }

        let out_ch = self.output_channels;
        let frame_count = data.len() / out_ch;

        for frame in 0..frame_count {
            if !self.playing {
                let finished = match &mut self.preview {
                    Some(preview) => {
                        preview.samples_left -= 1.0;
                        preview.samples_left <= 0.0
                    }
                    None => true,
                };
                if finished {
                    self.preview = None;
                    self.silence_output();
                    data[frame * out_ch..].fill(0.0);
                    return;
                }
            }

            self.advance_envelopes();

            self.samples_until_next_tick -= 1.0;
            if self.samples_until_next_tick <= 0.0 {
                self.samples_until_next_tick += self.samples_per_tick.max(1.0);
                self.advance_tick();
            }

            self.samples_until_next_beat -= 1.0;
            if self.playing && self.samples_until_next_beat <= 0.0 {
                let beats = self.pattern_beats(self.current_pattern_idx);
                self.current_row = (self.current_row + 1) % beats.max(1);
                self.tick_count = 0;
                self.trigger_row();
                self.samples_until_next_beat += self.samples_per_beat;
                self.samples_until_next_tick = self.samples_per_tick.max(1.0);
                self.current_row_out
                    .store(self.current_row, Ordering::Relaxed);
                self.current_pattern_out
                    .store(self.current_pattern_idx, Ordering::Relaxed);
            }

            if self.current_buffer.is_none() || self.buffer_position >= 64 {
                self.current_buffer = None;
                self.buffer_position = 0;

                while self.audio_out.output_buffer.slots() == 0 {
                    for _ in 0..128 {
                        self.run_acp_until_sample();
                    }
                    self.audio_out.convert_to_output_buffers();
                }

                if let Ok(buffer) = self.audio_out.output_buffer.pop() {
                    let mut arr = [0.0f32; 64];
                    for (i, &val) in buffer.iter().enumerate() {
                        arr[i] = val;
                    }
                    self.current_buffer = Some(arr);
                }
            }

            let sample = match self.current_buffer {
                Some(ref buffer) if self.buffer_position < 64 => {
                    let s = buffer[self.buffer_position];
                    self.buffer_position += 1;
                    s
                }
                _ => 0.0,
            };

            for ch in 0..out_ch {
                data[frame * out_ch + ch] = sample;
            }
        }
    }
}

/// Suppress errors like ALSA lib that go straight to stderr and clobber the UI
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd"
))]
pub fn silence_alsa_errors() {
    use std::os::raw::{c_char, c_int};

    // The real handler type is variadic, which stable Rust cannot define. A
    // handler that ignores its arguments is safe with this fixed signature.
    type Handler = unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char);

    unsafe extern "C" fn ignore(
        _: *const c_char,
        _: c_int,
        _: *const c_char,
        _: c_int,
        _: *const c_char,
    ) {
    }

    unsafe extern "C" {
        fn snd_lib_error_set_handler(handler: Option<Handler>) -> c_int;
    }

    unsafe {
        snd_lib_error_set_handler(Some(ignore));
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd"
)))]
pub fn silence_alsa_errors() {}

pub struct Player {
    cmd_tx: Sender<PlayerCmd>,
    current_row: Arc<AtomicUsize>,
    current_pattern: Arc<AtomicUsize>,
    is_playing: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

impl Player {
    pub fn new(bpm: u16) -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;

        let output_sample_rate = config.sample_rate().0 as f64;
        let output_channels = config.channels() as usize;

        let (cmd_tx, cmd_rx) = unbounded::<PlayerCmd>();
        let current_row = Arc::new(AtomicUsize::new(0));
        let current_pattern = Arc::new(AtomicUsize::new(0));
        let is_playing = Arc::new(AtomicBool::new(false));

        let mut inner = PlayerInner::new(
            cmd_rx,
            current_row.clone(),
            current_pattern.clone(),
            is_playing.clone(),
            output_sample_rate,
            output_channels,
            bpm,
        );

        let stream = device
            .build_output_stream(
                &config.into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    inner.fill_output(data);
                },
                // Printing to stderr would draw over the TUI.
                |_err| {},
                None,
            )
            .ok()?;

        stream.play().ok()?;

        Some(Player {
            cmd_tx,
            current_row,
            current_pattern,
            is_playing,
            _stream: stream,
        })
    }

    pub fn play(&self, pattern_idx: usize, row: usize, state: TrackState) {
        let _ = self.cmd_tx.send(PlayerCmd::Play(pattern_idx, row, state));
    }

    pub fn preview(&self, ch: usize, beat: Beat, state: TrackState) {
        let _ = self.cmd_tx.send(PlayerCmd::Preview(ch, beat, state));
    }

    pub fn pause(&self) {
        let _ = self.cmd_tx.send(PlayerCmd::Pause);
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::Relaxed)
    }

    pub fn current_row(&self) -> usize {
        self.current_row.load(Ordering::Relaxed)
    }

    pub fn current_pattern(&self) -> usize {
        self.current_pattern.load(Ordering::Relaxed)
    }

    pub fn update_patterns(&self, patterns: Vec<Pattern>, beats_list: Vec<u8>) {
        let _ = self
            .cmd_tx
            .send(PlayerCmd::UpdatePatterns(patterns, beats_list));
    }

    pub fn update_waveform(&self, idx: usize, waveform: [u8; 256]) {
        let _ = self
            .cmd_tx
            .send(PlayerCmd::UpdateWaveform(idx, Box::new(waveform)));
    }

    pub fn update_envelopes(&self, envelopes: Vec<Vec<u8>>) {
        let _ = self.cmd_tx.send(PlayerCmd::UpdateEnvelopes(envelopes));
    }

    pub fn update_tuning_notes(&self, notes: IndexMap<String, f64>) {
        let _ = self.cmd_tx.send(PlayerCmd::UpdateTuningNotes(notes));
    }

    pub fn set_channel_muted(&self, ch: usize, muted: bool) {
        let _ = self.cmd_tx.send(PlayerCmd::SetChannelMuted(ch, muted));
    }
}
