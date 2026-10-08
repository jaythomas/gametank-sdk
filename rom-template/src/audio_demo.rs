//! Audio demo module - example chord progressions and sequencing.
//!
//! Works with wavetable and FM synthesis cargo features:
//! - `audio-wavetable` - 7 voices, 11 PCM instruments
//! - `audio-fm-4ch` - 4 FM channels, 4 operators each, full ADSR per operator

/// Sequencer state for the FM demo
#[cfg(feature = "audio-fm-4ch")]
pub struct DemoSequencer {
    frame: u16,
    step:  u8,
}

/// Sequencer state for the wavetable demo
#[cfg(feature = "audio-wavetable")]
pub struct DemoSequencer {
    frame:               u16,
    step:                u8,
    bg_level:            u8,
    melody_level:        u8,
    bg_fade_counter:     u8,
    melody_fade_counter: u8,
}

#[cfg(feature = "audio-fm-4ch")]
impl DemoSequencer {
    pub const fn new() -> Self { Self { frame: 0, step: 0 } }

    /// Call once per frame (60 fps). Advances ADSR envelopes and the sequence.
    pub fn tick(&mut self) {
        use gametank::audio::{channels, flush_params, MidiNote, silence_all};

        let mut ch = channels();

        // Advance ADSR envelopes every frame (mirrors tick_music() decay loop)
        for c in ch.iter_mut() {
            c.tick();
        }

        match self.step {
            // Build Cmaj7 chord, one note per second across channels 0-2
            1 => { if self.frame == 0 { ch[0].set_note(MidiNote::C4); ch[0].note_on(); } }
            2 => { if self.frame == 0 { ch[1].set_note(MidiNote::E4); ch[1].note_on(); } }
            3 => { if self.frame == 0 { ch[1].set_note(MidiNote::G4); ch[1].note_on(); } }
            4 => { if self.frame == 0 { ch[2].set_note(MidiNote::B4); ch[2].note_on(); } }

            // Arpeggio melody on channel 3 (GUITAR decays to silence naturally)
            5..=8 => {
                if self.step == 7 {
                    match self.frame {
                        0  => { ch[3].set_note(MidiNote::E5); ch[3].note_on(); }
                        20 => { ch[3].set_note(MidiNote::B4); ch[3].note_on(); }
                        40 => { ch[3].set_note(MidiNote::G4); ch[3].note_on(); }
                        _  => {}
                    }
                }
            }

            9 => { if self.frame == 0 { silence_all(); } }
            _ => {}
        }

        flush_params();

        self.frame += 1;
        if self.frame >= 60 {
            self.frame = 0;
            self.step += 1;
        }
    }
}

#[cfg(feature = "audio-wavetable")]
impl DemoSequencer {
    pub const fn new() -> Self {
        Self {
            frame: 0, step: 0,
            bg_level: 12, melody_level: 16,
            bg_fade_counter: 0, melody_fade_counter: 0,
        }
    }

    // Fade the background chord one increment every BG_FAGE_INTERVAL
    fn fade_bg(&mut self, v: &[gametank::audio::Voice]) {
        const BG_FADE_INTERVAL: u8 = 20;
        if self.bg_level == 0 {
            return;
        }
        self.bg_fade_counter += 1;
        if self.bg_fade_counter >= BG_FADE_INTERVAL {
            self.bg_fade_counter = 0;
            self.bg_level -= 1;
            for voice in &v[0..=4] {
                voice.set_volume(self.bg_level);
            }
        }
    }

    // Fade the melody voice down by one level every MELODY_FADE_INTERVAL
    fn fade_melody(&mut self, melody_voice: gametank::audio::Voice) {
        const MELODY_FADE_INTERVAL: u8 = 3;
        if self.melody_level == 0 {
            return;
        }
        self.melody_fade_counter += 1;
        if self.melody_fade_counter >= MELODY_FADE_INTERVAL {
            self.melody_fade_counter = 0;
            self.melody_level -= 1;
            melody_voice.set_volume(self.melody_level);
        }
    }

    // Call once per frame (60 fps). Advances the sequence.
    pub fn tick(&mut self) {
        use gametank::audio::{voices, MidiNote, VOICE_COUNT};

        let v = voices();
        let melody_voice = v[VOICE_COUNT - 1];

        match self.step {
            // Build up Cmaj7 chord, one note per second
            1 => { if self.frame == 0 { v[0].set_note(MidiNote::C4); v[0].set_volume(self.bg_level); } }
            2 => { if self.frame == 0 { v[1].set_note(MidiNote::E4); v[1].set_volume(self.bg_level); } }
            3 => { if self.frame == 0 { v[2].set_note(MidiNote::G4); v[2].set_volume(self.bg_level); } }
            4 => { if self.frame == 0 { v[3].set_note(MidiNote::B4); v[3].set_volume(self.bg_level); } }
            5 => { if self.frame == 0 { v[4].set_note(MidiNote::D5); v[4].set_volume(self.bg_level); } }

            // Steps 6-9: arpeggio melody while the background chord fades
            6..=9 => {
                if self.step == 8 {
                    match self.frame {
                        0  => { melody_voice.set_volume(self.melody_level); melody_voice.set_note(MidiNote::E5); }
                        20 => melody_voice.set_note(MidiNote::B4),
                        40 => melody_voice.set_note(MidiNote::G4),
                        _  => {}
                    }
                }
                self.fade_bg(&v);
            }

            // Steps 10+: fade out the melody (and finish the background
            // fade, in case it hasn't reached silence yet) until every
            // voice has come to rest.
            10..=255 => {
                self.fade_bg(&v);
                self.fade_melody(melody_voice);
            }

            _ => {}
        }

        self.frame += 1;
        if self.frame >= 60 {
            self.frame = 0;
            self.step += 1;
        }
    }
}

#[cfg(feature = "audio-fm-4ch")]
pub fn init_demo() -> DemoSequencer {
    use gametank::audio::{channels, flush_params, silence_all};
    use gametank::audio::fm_4ch::instruments::{PIANO, SITAR};

    let mut ch = channels();
    ch[0].load_instrument(&PIANO);
    ch[1].load_instrument(&PIANO);
    ch[2].load_instrument(&PIANO);
    ch[3].load_instrument(&SITAR);

    silence_all();
    flush_params();

    DemoSequencer::new()
}

#[cfg(feature = "audio-wavetable")]
pub fn init_demo() -> DemoSequencer {
    use gametank::audio::{voices, WAVETABLE};

    let v = voices();
    let mut wavetable_idx = 0;
    for voice in v.iter() {
        voice.set_wavetable(WAVETABLE[wavetable_idx]);
        voice.set_volume(0);
        wavetable_idx += 1;
    }
    DemoSequencer::new()
}
