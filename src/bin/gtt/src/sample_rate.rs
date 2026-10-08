//! Hardware sample-rate register lookup.

/// Audio coprocessor clock frequency (NTSC colorburst-derived).
pub const CPU_FREQ: f64 = 3_579_545.0;

/// $A8 = ~44191Hz
pub const SAMPLE_RATE_REG: u8 = 0xA8;

/// Resolve a hardware `audio_freq` register value to the number of ACP
/// clocks between samples.
///
/// Note this is *not* the register value itself - only its low 7 bits
/// participate, and odd values are rounded up to the next even divisor.
pub const fn sample_rate_divisor(reg: u8) -> u32 {
    let low7 = (reg & 0x7F) as u32;
    2 * low7 + 1 + (low7 & 1)
}

/// Resolve a hardware `audio_freq` register value to its real
/// output sample rate in Hz.
pub const fn sample_rate_reg_to_hz(reg: u8) -> f64 {
    CPU_FREQ / sample_rate_divisor(reg) as f64
}

const FS: u32 = sample_rate_reg_to_hz(SAMPLE_RATE_REG) as u32;

#[inline(always)]
pub const fn hz_to_inc_q16(hz_q16: u32) -> u16 {
    ((hz_q16 as u64 + (FS as u64 / 2)) / (FS as u64)) as u16
}
