//! # 7-Channel Wavetable Synthesizer (Linear Volume)
//!
//! This firmware provides 7 independent voices with linear volume scaling.
//! Each voice has:
//! - **Note/Frequency** - MIDI notes or raw frequency values
//! - **Volume** - 17 linear volume levels (0-16)
//! - **Wavetable** - One of 6 waveform slots
//!
//! ## Volume Levels
//!
//! This firmware uses 4 volume tables × 4 shift levels = 16 volume steps + silence:
//! - Level 0: Silence (shift=4)
//! - Level 1-4: 62.5% table with shifts 3,2,1,0
//! - Level 5-8: 75% table with shifts 3,2,1,0
//! - Level 9-12: 87.5% table with shifts 3,2,1,0
//! - Level 13-16: 100% table with shifts 3,2,1,0
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use rom::sdk::audio::{voices, MidiNote, WAVETABLE};
//!
//! let v = voices();
//!
//! // Play middle C at maximum volume with the first wavetable
//! v[0].set_note(MidiNote::C4);
//! v[0].set_volume(16);
//! v[0].set_wavetable(WAVETABLE[0]);
//!
//! // Play a chord with varying volumes (7 voices available: v[0]..v[6])
//! v[1].set_note(MidiNote::E4);  v[1].set_volume(12);
//! v[2].set_note(MidiNote::G4);  v[2].set_volume(10);
//!
//! // Stop a voice
//! v[0].mute();
//! ```
//!
//! ## Wavetables
//!
//! The firmware has 6 wavetable slots (256 bytes each). Use [`WAVETABLE`] to get slot addresses:
//!
//! ```rust,ignore
//! v[0].set_wavetable(WAVETABLE[0]);  // First waveform
//! v[1].set_wavetable(WAVETABLE[1]);  // Second waveform
//! ```
//!
//! You can load custom waveforms into audio RAM:
//!
//! ```rust,ignore
//! // Wavetable 0 is at $3600, wavetable 1 at $3700, etc.
//! let my_wave: [u8; 256] = generate_sine();
//! console.audio[0x600..0x700].copy_from_slice(&my_wave);
//! ```

use crate::audio::pitch_table::{MidiNote, midi_inc};

/// Base address for voice registers (CPU-side address, ACP RAM at 0x3000)
pub const VOICE_BASE: usize = 0x3041;
/// Number of bytes per voice
pub const VOICE_SIZE: usize = 9;
/// Number of voices
pub const VOICE_COUNT: usize = 7;

/// Base address for wavetables in ACP RAM (CPU-side)
pub const WAVETABLE_BASE: usize = 0x3600;
/// Size of each wavetable in bytes
pub const WAVETABLE_SIZE: usize = 256;
/// Number of wavetables available
pub const WAVETABLE_COUNT: usize = 6;

/// Wavetable slot addresses (ACP-side, for setting voice wavetable pointer)
pub const WAVETABLE: [u16; WAVETABLE_COUNT] = [0x0600, 0x0700, 0x0800, 0x0900, 0x0A00, 0x0B00];

/// Volume level mapping to table pointer + shift
/// Each entry: (volume_table_ptr, shift_count)
/// 16 linear levels sorted by shift (most impact) then table
const VOLUME_MAP: [(u16, u8); 17] = [
    (0x0500, 4), // 0: silence (shift >= 4 gives silence)
    // Shift 3 (divide by 8) - quietest audible levels
    (0x0500, 3), // 1: table 3 (62.5%), shift 3
    (0x0400, 3), // 2: table 2 (75%), shift 3
    (0x0300, 3), // 3: table 1 (87.5%), shift 3
    (0x0200, 3), // 4: table 0 (100%), shift 3
    // Shift 2 (divide by 4)
    (0x0500, 2), // 5: table 3 (62.5%), shift 2
    (0x0400, 2), // 6: table 2 (75%), shift 2
    (0x0300, 2), // 7: table 1 (87.5%), shift 2
    (0x0200, 2), // 8: table 0 (100%), shift 2
    // Shift 1 (divide by 2)
    (0x0500, 1), // 9: table 3 (62.5%), shift 1
    (0x0400, 1), // 10: table 2 (75%), shift 1
    (0x0300, 1), // 11: table 1 (87.5%), shift 1
    (0x0200, 1), // 12: table 0 (100%), shift 1
    // Shift 0 (no division) - loudest levels
    (0x0500, 0), // 13: table 3 (62.5%), shift 0
    (0x0400, 0), // 14: table 2 (75%), shift 0
    (0x0300, 0), // 15: table 1 (87.5%), shift 0
    (0x0200, 0), // 16: table 0 (100%), shift 0
];

/// A single synthesizer voice.
///
/// This struct is laid out to match the ACP firmware's memory layout exactly.
/// All fields are little-endian as expected by the 6502.
#[repr(C, packed)]
pub struct Voice {
    /// Phase accumulator (16-bit, high byte indexes wavetable)
    phase: u16,
    /// Frequency increment added to phase each sample
    frequency: u16,
    /// Pointer to 256-byte wavetable in ACP RAM
    wavetable: u16,
    /// Pointer to 256-byte volume table in ACP RAM
    volptr: u16,
    /// Shift count (0-3, or >= 4 for silence)
    shift: u8,
}

impl Voice {
    /// Set the voice frequency from a MIDI note number.
    #[inline]
    pub fn set_note(&mut self, note: MidiNote) {
        self.frequency = midi_inc(note);
    }

    /// Set the voice frequency directly as a 16-bit increment value.
    ///
    /// Use `pitch_table::midi_inc()` to convert from MIDI notes,
    /// or calculate directly: `inc = (freq_hz * 65536) / SAMPLE_RATE`
    #[inline]
    pub fn set_frequency(&mut self, freq_inc: u16) {
        self.frequency = freq_inc;
    }

    /// Set the volume level (0 = silence, 16 = maximum).
    ///
    /// This firmware provides 16 linear volume steps using 4 volume tables
    /// combined with 4 shift levels.
    #[inline]
    pub fn set_volume(&mut self, level: u8) {
        let level = level.min(16);
        let (volptr, shift) = VOLUME_MAP[level as usize];
        self.volptr = volptr;
        self.shift = shift;
    }

    /// Set which wavetable this voice uses.
    ///
    /// Pass the ACP-side address (e.g., `WAVETABLE[0]` = 0x0600).
    #[inline]
    pub fn set_wavetable(&mut self, wavetable_addr: u16) {
        self.wavetable = wavetable_addr;
    }

    /// Silence this voice immediately.
    #[inline]
    pub fn mute(&mut self) {
        self.shift = 4; // Shift >= 4 gives silence
    }

    /// Reset the phase accumulator to zero (useful for hard sync effects).
    #[inline]
    pub fn reset_phase(&mut self) {
        self.phase = 0;
    }

    /// Get the current volume level (0-16).
    #[inline]
    pub fn get_volume(&self) -> u8 {
        // Reverse lookup in VOLUME_MAP
        VOLUME_MAP
            .iter()
            .position(|(ptr, shift)| *ptr == self.volptr && *shift == self.shift)
            .unwrap_or(0) as u8
    }
}

/// Get a mutable reference to all 7 voices.
///
/// # Safety
/// This function creates a mutable reference to memory-mapped hardware.
/// The caller must ensure exclusive access to the voice registers.
#[inline]
pub fn voices() -> &'static mut [Voice; VOICE_COUNT] {
    unsafe { &mut *(VOICE_BASE as *mut [Voice; VOICE_COUNT]) }
}

/// Get a mutable reference to a single voice by index (0-7).
///
/// # Panics
/// Panics if `index >= 8`.
#[inline]
pub fn voice(index: usize) -> &'static mut Voice {
    assert!(index < VOICE_COUNT, "voice index out of range");
    unsafe { &mut *((VOICE_BASE + index * VOICE_SIZE) as *mut Voice) }
}

/// Silence all voices.
#[inline]
pub fn mute_all() {
    let v = voices();
    for voice in v.iter_mut() {
        voice.mute();
    }
}
