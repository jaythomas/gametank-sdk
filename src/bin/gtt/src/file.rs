use std::{io, path::Path};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::tracker::{Beat, Pattern, empty_pattern};

pub const NUM_INSTRUMENTS: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
pub struct InstrumentData {
    pub name: String,
    pub waveform: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Interval {
    pub name: String,
    pub cents: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningData {
    pub notes: IndexMap<String, f64>,
    #[serde(skip)]
    pub key_assignments: IndexMap<String, Vec<String>>,
    #[serde(default)]
    pub scale: Vec<Interval>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackerFile {
    pub schema_version: u8,
    pub instruments: [InstrumentData; NUM_INSTRUMENTS],
    #[serde(default = "default_tuning")]
    pub tuning: TuningData,
    #[serde(
        serialize_with = "serialize_patterns",
        deserialize_with = "deserialize_patterns"
    )]
    pub patterns: Vec<Pattern>,
    #[serde(default = "default_pattern_beats")]
    pub pattern_beats: Vec<u8>,
    pub bpm: u16,
    pub fx_speed: u8,
}

fn serialize_patterns<S>(patterns: &Vec<Pattern>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(patterns.len()))?;
    for pattern in patterns {
        let as_vecs: Vec<Vec<Beat>> = pattern.iter().map(|row| row.to_vec()).collect();
        seq.serialize_element(&as_vecs)?;
    }
    seq.end()
}

fn deserialize_patterns<'de, D>(deserializer: D) -> Result<Vec<Pattern>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let vecs: Vec<Vec<Vec<Beat>>> = Vec::deserialize(deserializer)?;
    let mut patterns = Vec::new();
    for pattern_vec in vecs {
        if pattern_vec.len() != 8 {
            return Err(D::Error::custom(format!(
                "Expected 8 channels, got {}",
                pattern_vec.len()
            )));
        }
        let mut pattern: Pattern = empty_pattern();
        for (ch_idx, channel) in pattern_vec.iter().enumerate() {
            if channel.len() != 64 {
                return Err(D::Error::custom(format!(
                    "Expected 64 beats in channel {}, got {}",
                    ch_idx,
                    channel.len()
                )));
            }
            for (beat_idx, beat) in channel.iter().enumerate() {
                pattern[ch_idx][beat_idx] = beat.clone();
            }
        }
        patterns.push(pattern);
    }
    Ok(patterns)
}

pub fn default_tuning() -> TuningData {
    let degree_names = [
        "A", "A♯", "B", "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯",
    ];
    let degree_cents: [f64; 12] = [
        0.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 1100.0,
    ];

    const BASE_FREQ: f64 = 440.0;
    const PERIOD_RATIO: f64 = 2.0;
    const FREQ_MIN: f64 = 7.0;
    const FREQ_MAX: f64 = 4200.0;
    const PIVOT_HZ: f64 = 510.0;
    const PIVOT_NUM: i32 = 5;
    const N: usize = 12;

    let ln_ratio = PERIOD_RATIO.ln();
    let start_unison = ((FREQ_MIN / BASE_FREQ).ln() / ln_ratio).floor() as i32 - 1;
    let end_unison = ((FREQ_MAX / BASE_FREQ).ln() / ln_ratio).ceil() as i32 + 1;

    let mut rows: Vec<(&str, f64, usize)> = Vec::new();
    for unison in start_unison..=end_unison {
        for degree in 0..N {
            let freq =
                BASE_FREQ * PERIOD_RATIO.powi(unison) * 2.0_f64.powf(degree_cents[degree] / 1200.0);
            if (FREQ_MIN..=FREQ_MAX).contains(&freq) {
                rows.push((degree_names[degree], freq, degree));
            }
        }
    }
    rows.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });

    let pivot = rows
        .iter()
        .position(|(_, freq, _)| *freq > PIVOT_HZ)
        .unwrap_or(rows.len()) as i32;

    let mut notes = IndexMap::new();
    for (j, (name, freq, _)) in rows.iter().enumerate() {
        let repeat_num = PIVOT_NUM + (j as i32 - pivot).div_euclid(N as i32);
        notes.insert(format!("{}{}", name, repeat_num), *freq);
    }

    let key_map: &[(&str, char)] = &[
        // Ideally capture key code to make the layout agnostic, but the
        // terminal doesn't pass that infomation through to the application
        // layer. All these are ANSI US QWERTY but reassignable.
        ("C5", 'q'),
        ("C♯5", '2'),
        ("D5", 'w'),
        ("D♯5", '3'),
        ("E5", 'e'),
        ("F5", 'r'),
        ("F♯5", '5'),
        ("G5", 't'),
        ("G♯5", '6'),
        ("A5", 'y'),
        ("A♯5", '7'),
        ("B5", 'u'),
        ("C6", 'i'),
        ("C♯6", '9'),
        ("D6", 'o'),
        ("D♯6", '0'),
        ("E6", 'p'),
        ("F6", '['),
        ("F♯6", '='),
        ("G6", ']'),
        // Shift keys
        ("C6", 'Q'),
        ("C♯6", '@'),
        ("D6", 'W'),
        ("D♯6", '#'),
        ("E6", 'E'),
        ("F6", 'R'),
        ("F♯6", '%'),
        ("G6", 'T'),
        ("G♯6", '^'),
        ("A6", 'Y'),
        ("A♯6", '&'),
        ("B6", 'U'),
        ("C7", 'I'),
        ("C♯7", '('),
        ("D7", 'O'),
        ("D♯7", ')'),
        ("E7", 'P'),
        ("F7", '{'),
        ("F♯7", '+'),
        ("G7", '}'),
    ];
    let mut key_assignments: IndexMap<String, Vec<String>> = IndexMap::new();
    for (name, ch) in key_map.iter() {
        key_assignments
            .entry(name.to_string())
            .or_default()
            .push(ch.to_string());
    }

    let mut scale: Vec<Interval> = (1..N)
        .map(|i| Interval {
            name: degree_names[i].to_string(),
            cents: degree_cents[i],
        })
        .collect();
    scale.push(Interval {
        name: degree_names[0].to_string(),
        cents: 1200.0,
    });

    TuningData {
        notes,
        key_assignments,
        scale,
    }
}

fn default_pattern_beats() -> Vec<u8> {
    vec![64]
}

/// The instrument that new tracks fill with noise: the one the last voice (6)
/// plays by default, giving a ready-made percussion channel.
const NOISE_INSTRUMENT_INDEX: usize = 6;

/// White noise: a shuffle of every byte value, so it is flat in level and has
/// no DC offset.
const NOISE_WAVEFORM: [u8; 256] = [
    0x71, 0x8e, 0x48, 0xdb, 0x44, 0x16, 0x8b, 0x56, 0x5d, 0x68, 0xb7, 0x4e, 0x70, 0xae, 0xb3, 0x98,
    0x7f, 0x14, 0x55, 0xbf, 0xd6, 0x54, 0xdd, 0xdc, 0x41, 0x93, 0xe7, 0x99, 0xd8, 0x75, 0xd4, 0x11,
    0x47, 0x8d, 0xf6, 0xa5, 0xcc, 0x0b, 0x9b, 0xac, 0x02, 0x45, 0x74, 0x18, 0x4f, 0x6b, 0xf0, 0xa8,
    0xa0, 0xe3, 0xa6, 0xf3, 0x95, 0x01, 0xe1, 0x35, 0x4d, 0xfc, 0xec, 0xaf, 0x1b, 0xa7, 0x32, 0x3b,
    0xc3, 0x1a, 0x8f, 0xb1, 0x00, 0xa3, 0x83, 0x0a, 0xf1, 0xb2, 0x73, 0xdf, 0x2f, 0x12, 0xe2, 0x9a,
    0x38, 0x59, 0x69, 0x07, 0x5b, 0xcd, 0x80, 0x7a, 0x6a, 0xe5, 0xbd, 0xda, 0x78, 0xff, 0x0c, 0x03,
    0x2e, 0xce, 0x2c, 0x65, 0x52, 0x79, 0x7b, 0x3f, 0xc0, 0x81, 0x5c, 0xc5, 0xca, 0xfb, 0xa2, 0x08,
    0x88, 0xf4, 0x82, 0xd5, 0xd7, 0xef, 0xf8, 0x6c, 0x63, 0x91, 0x61, 0x9e, 0xc4, 0x30, 0x85, 0x34,
    0xcb, 0x19, 0x64, 0x6f, 0x62, 0x84, 0x90, 0xd2, 0x9c, 0x0e, 0x46, 0xbb, 0x49, 0x05, 0xba, 0x04,
    0xfe, 0x24, 0xb5, 0x72, 0x15, 0xd9, 0x60, 0x92, 0x67, 0x5f, 0x37, 0x1f, 0x76, 0x25, 0x13, 0x8c,
    0x10, 0x97, 0x7d, 0x9f, 0xb0, 0x09, 0xa4, 0xc8, 0x36, 0xaa, 0x3c, 0xed, 0xfa, 0xe0, 0x51, 0xa1,
    0xc7, 0x57, 0x9d, 0x06, 0x2a, 0x58, 0x8a, 0xf2, 0x3e, 0xe9, 0x39, 0xe4, 0x2d, 0x50, 0x20, 0x6d,
    0x0f, 0x0d, 0x4b, 0xb6, 0x42, 0xf7, 0xa9, 0xbe, 0x4c, 0xd1, 0xb9, 0xe6, 0xd3, 0x66, 0x77, 0x6e,
    0x7c, 0xc1, 0x5a, 0xe8, 0xcf, 0x23, 0x7e, 0x53, 0xc2, 0x29, 0x1c, 0x31, 0x17, 0x1e, 0x1d, 0x27,
    0x43, 0xea, 0xb4, 0xee, 0x3d, 0x2b, 0xab, 0x33, 0xf9, 0x86, 0xc6, 0x4a, 0x5e, 0xeb, 0x3a, 0xfd,
    0xd0, 0x26, 0x21, 0xb8, 0x40, 0xf5, 0xc9, 0x96, 0x89, 0x87, 0xad, 0xde, 0x22, 0x94, 0x28, 0xbc,
];

impl TrackerFile {
    pub fn empty() -> Self {
        let mut square = vec![0xFFu8; 256];
        for b in square[128..].iter_mut() {
            *b = 0x00;
        }
        Self {
            schema_version: 0,
            instruments: std::array::from_fn(|i| InstrumentData {
                name: format!("instrument_{}", i),
                waveform: if i == NOISE_INSTRUMENT_INDEX {
                    NOISE_WAVEFORM.to_vec()
                } else {
                    square.clone()
                },
            }),
            tuning: default_tuning(),
            patterns: vec![empty_pattern()],
            pattern_beats: vec![64],
            bpm: 120,
            fx_speed: 6,
        }
    }

    pub fn current_pattern(&self, pattern_idx: u8) -> &Pattern {
        let idx = pattern_idx as usize;
        if idx >= self.patterns.len() {
            &self.patterns[0]
        } else {
            &self.patterns[idx]
        }
    }

    pub fn current_pattern_mut(&mut self, pattern_idx: u8) -> &mut Pattern {
        let idx = pattern_idx as usize;
        while idx >= self.patterns.len() {
            self.patterns.push(empty_pattern());
        }
        while idx >= self.pattern_beats.len() {
            self.pattern_beats.push(64);
        }
        &mut self.patterns[idx]
    }

    pub fn beats_for(&self, pattern_idx: u8) -> u8 {
        let idx = pattern_idx as usize;
        self.pattern_beats.get(idx).copied().unwrap_or(64)
    }

    pub fn set_beats_for(&mut self, pattern_idx: u8, beats: u8) {
        let idx = pattern_idx as usize;
        while idx >= self.pattern_beats.len() {
            self.pattern_beats.push(64);
        }
        self.pattern_beats[idx] = beats.clamp(1, crate::tracker::PATTERN_BEATS as u8);
    }

    pub fn instrument_names(&self) -> [String; NUM_INSTRUMENTS] {
        std::array::from_fn(|i| self.instruments[i].name.clone())
    }

    pub fn instrument_waveform(&self, idx: usize) -> [u8; 256] {
        let mut arr = [0u8; 256];
        let src = &self.instruments[idx].waveform;
        let len = src.len().min(256);
        arr[..len].copy_from_slice(&src[..len]);
        arr
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 0 {
            return Err(format!(
                "unsupported schema version {} (expected 0)",
                self.schema_version
            ));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = std::fs::read(path)?;
        let mut file: TrackerFile = rmp_serde::from_slice(&bytes)?;
        file.validate()
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        while file.pattern_beats.len() < file.patterns.len() {
            file.pattern_beats.push(64);
        }
        file.pattern_beats.truncate(file.patterns.len().max(1));
        Ok(file)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = rmp_serde::to_vec_named(self).map_err(io::Error::other)?;
        std::fs::write(path, bytes)
    }
}

