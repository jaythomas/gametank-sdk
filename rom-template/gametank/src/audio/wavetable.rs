//! # 7-Voice Wavetable Synthesizer
//!
//! This firmware provides 7 independent voices, each with:
//! - **Note/Frequency** - MIDI notes or raw frequency values
//! - **Volume** - 0 (silent) to 7 (max), in 3 dB steps
//! - **Wavetable** - One of 8 waveform slots
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use rom::sdk::audio::{voices, MidiNote, WAVETABLE};
//!
//! let v = voices();
//!
//! // Play middle C at full volume with the first wavetable
//! v[0].set_note(MidiNote::C4);
//! v[0].set_volume(MAX_VOLUME);
//! v[0].set_wavetable(WAVETABLE[0]);
//!
//! // Play a chord
//! v[1].set_note(MidiNote::E4);  v[1].set_volume(5);
//! v[2].set_note(MidiNote::G4);  v[2].set_volume(5);
//!
//! // Stop a voice
//! v[0].mute();
//! ```
//!
//! ## Volume
//!
//! Levels run 0 (silent) to [`MAX_VOLUME`] (23) and are spaced 1.3 dB apart
//! at the loud end, widening to 3.5 dB at the quiet end so that level 1 sits near
//! -42 dB. Each step is a roughly equal-sounding jump rather than an equal
//! numeric one.
//! Gains are normalized so that four voices at [`MAX_VOLUME`] reach full scale;
//! pushing more than that into the mix saturates rather than wrapping, so it
//! distorts gracefully instead of inverting the waveform.
//!
//! ## Wavetables
//!
//! The firmware has 8 wavetable slots. Use [`WAVETABLE`] to get slot addresses:
//!
//! ```rust,ignore
//! v[0].set_wavetable(WAVETABLE[0]);  // First waveform
//! v[1].set_wavetable(WAVETABLE[1]);  // Second waveform
//! ```
//!
//!
//! You can load custom waveforms (256 bytes each) into audio RAM:
//!
//! ```rust,ignore
//! // Wavetable 0 is at $3700, wavetable 1 at $3800, etc.
//! let my_wave: [u8; 256] = generate_sine();
//! console.audio[0x700..0x800].copy_from_slice(&my_wave);
//! ```

use crate::audio::pitch_table::{MidiNote, midi_inc};

/// Number of voices
pub const VOICE_COUNT: usize = 7;

/// CPU-side address of ACP RAM.
const ACP_RAM: usize = 0x3000;
/// ACP address of the first voice's phase accumulator (zero page, 2 bytes per voice).
const PHASE_BASE: usize = 0x0041;
/// ACP address of voice 1's code block. Voice parameters are not data but the
/// operands of instructions in each voice's code block, which is how the
/// firmware reads them without any indexing or pointer loads.
const VOICE_CODE_BASE: usize = 0x0F0B;
/// Distance between consecutive voice code blocks.
const VOICE_BLOCK_SIZE: usize = 29;
/// ACP address of voice 0's parameter block. The firmware pipelines voice 0, so
/// its block sits after voice 6 rather than before voice 1.
const VOICE_0_CODE: usize = 0x0FB9;
/// ACP address of each voice's code block. Held as a table because the 6502
/// has no multiply, and a stride of 29 does not reduce to shifts and adds.
const VOICE_CODE: [usize; VOICE_COUNT] = {
    let mut table = [VOICE_0_CODE; VOICE_COUNT];
    let mut i = 1;
    while i < VOICE_COUNT {
        table[i] = VOICE_CODE_BASE + (i - 1) * VOICE_BLOCK_SIZE;
        i += 1;
    }
    table
};
const FREQ_LO_OFFSET: usize = 4;
const FREQ_HI_OFFSET: usize = 10;
const WAVE_OFFSET: usize = 15;
const GAIN_OFFSET: usize = 19;

/// Base address for wavetables in ACP RAM (CPU-side)
pub const WAVETABLE_BASE: usize = 0x3700;
/// Size of each wavetable in bytes
pub const WAVETABLE_SIZE: usize = 256;
/// Number of entries in [`WAVETABLE`].
pub const WAVETABLE_COUNT: usize = 8;

/// Wavetable slot addresses (ACP-side).
pub const WAVETABLE: [u16; WAVETABLE_COUNT] = [
    0x0700,
    0x0800,
    0x0900,
    0x0A00,
    0x0B00,
    0x0C00,
    0x0D00,
    0x0E00,
];

/// Number of distinct volume levels.
pub const VOLUME_LEVELS: usize = 24;

/// Loudest volume level.
pub const MAX_VOLUME: u8 = (VOLUME_LEVELS - 1) as u8;

/// ACP address of the first gain table.
const GAIN_BASE: u16 = 0x0100;
/// Gain tables are interleaved four to a block of this many bytes; a level's
/// pointer is its block's address plus its position (0-3) within the block.
const GAIN_BLOCK_SIZE: u16 = 256;
const GAIN_TABLES_PER_BLOCK: u16 = 4;

/// Handle to a single synthesizer voice.
///
/// A voice is only an index; its parameters live in audio RAM as operands of
/// the firmware's instructions. Handles are cheap to copy and all methods take
/// `&self`.
#[derive(Clone, Copy)]
pub struct Voice {
    index: u8,
}

#[inline]
fn poke(acp_addr: usize, value: u8) {
    unsafe { core::ptr::write_volatile((ACP_RAM + acp_addr) as *mut u8, value) }
}

#[inline]
fn peek(acp_addr: usize) -> u8 {
    unsafe { core::ptr::read_volatile((ACP_RAM + acp_addr) as *const u8) }
}

#[inline]
fn poke16(acp_addr: usize, value: u16) {
    poke(acp_addr, value as u8);
    poke(acp_addr + 1, (value >> 8) as u8);
}

impl Voice {
    #[inline]
    fn code(&self, offset: usize) -> usize {
        VOICE_CODE[self.index as usize] + offset
    }

    /// Set the voice frequency from a MIDI note number.
    #[inline]
    pub fn set_note(&self, note: MidiNote) {
        self.set_frequency(midi_inc(note));
    }

    /// Set the voice frequency directly as a 16-bit increment value.
    ///
    /// Use `pitch_table::midi_inc()` to convert from MIDI notes,
    /// or calculate directly: `inc = (freq_hz * 65536) / SAMPLE_RATE`
    ///
    /// The two bytes are written separately, so a sample landing between them
    /// plays one sample at a slightly wrong pitch.
    #[inline]
    pub fn set_frequency(&self, freq_inc: u16) {
        poke(self.code(FREQ_LO_OFFSET), freq_inc as u8);
        poke(self.code(FREQ_HI_OFFSET), (freq_inc >> 8) as u8);
    }

    /// Set the volume level (0 = silence, [`MAX_VOLUME`] = loudest).
    ///
    /// Levels are 1.3 dB apart at the top, widening to 3.5 dB near the bottom. Values above [`MAX_VOLUME`] are clamped.
    ///
    /// The firmware tolerates this landing mid-sample: gain table pointers
    /// differ only in a low byte of 0-3 and a high byte of 2-5, so a partially
    /// written pointer still addresses a real table and the worst case is one
    /// sample at a different volume.
    #[inline]
    pub fn set_volume(&self, volume: u8) {
        let level = if volume > MAX_VOLUME { MAX_VOLUME } else { volume };
        let level = level as u16;
        let gain = GAIN_BASE
            + (level / GAIN_TABLES_PER_BLOCK) * GAIN_BLOCK_SIZE
            + level % GAIN_TABLES_PER_BLOCK;
        poke16(self.code(GAIN_OFFSET), gain);
    }

    /// Set which wavetable this voice uses.
    ///
    /// Pass the ACP-side address (e.g., `WAVETABLE[0]` = 0x0700). Wavetables
    /// must be 256-byte aligned, which every [`WAVETABLE`] slot is.
    #[inline]
    pub fn set_wavetable(&self, wavetable_addr: u16) {
        poke16(self.code(WAVE_OFFSET), wavetable_addr);
    }

    /// Silence this voice immediately.
    #[inline]
    pub fn mute(&self) {
        self.set_volume(0);
    }

    /// Reset the phase accumulator to zero (useful for hard sync effects).
    #[inline]
    pub fn reset_phase(&self) {
        let phase = PHASE_BASE + self.index as usize * 2;
        poke(phase, 0);
        poke(phase + 1, 0);
    }

    /// Get the wavetable address this voice is currently using.
    #[inline]
    pub fn get_wavetable(&self) -> u16 {
        let addr = self.code(WAVE_OFFSET);
        peek(addr) as u16 | (peek(addr + 1) as u16) << 8
    }

    /// Get the current volume level.
    #[inline]
    pub fn get_volume(&self) -> u8 {
        let addr = self.code(GAIN_OFFSET);
        let gain = peek(addr) as u16 | (peek(addr + 1) as u16) << 8;
        let offset = gain - GAIN_BASE;
        ((offset / GAIN_BLOCK_SIZE) * GAIN_TABLES_PER_BLOCK + offset % GAIN_BLOCK_SIZE) as u8
    }
}

/// Get handles to all 7 voices.
#[inline]
pub fn voices() -> [Voice; VOICE_COUNT] {
    [0, 1, 2, 3, 4, 5, 6].map(|index| Voice { index })
}

/// Get a handle to a single voice by index.
///
/// # Panics
/// Panics if `index >= VOICE_COUNT`.
#[inline]
pub fn voice(index: usize) -> Voice {
    assert!(index < VOICE_COUNT, "voice index out of range");
    Voice { index: index as u8 }
}

/// Silence all voices.
#[inline]
pub fn mute_all() {
    for voice in voices() {
        voice.mute();
    }
}
