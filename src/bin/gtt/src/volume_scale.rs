use crate::tracker::{MAX_VOLUME, VOLUME_LEVELS};

/// Row stride of the exported table. Padding the 24 columns out to a
/// power of two lets the firmware reach a row with a shift instead of
/// a multiply.
pub const SCALE_STRIDE: usize = 32;

pub const SCALE_TABLE_SIZE: usize = VOLUME_LEVELS * SCALE_STRIDE;

// Same step size as gen_gain_tables.js, which generates the firmware's gain pages.
const STEP_DB_TOP: f64 = 1.3;
const STEP_DB_BOTTOM: f64 = 3.5;
const FLAT_STEPS: f64 = 11.0;

/// How far below full volume a level sits. The top levels are evenly spaced
/// and the spacing widens below them, so level 1 lands near -42dB.
fn db_below_max(level: u8) -> f64 {
    let mut db = 0.0;
    for k in 1..=(MAX_VOLUME - level) {
        let t = ((k as f64 - FLAT_STEPS) / (MAX_VOLUME as f64 - FLAT_STEPS)).max(0.0);
        db += STEP_DB_TOP + (STEP_DB_BOTTOM - STEP_DB_TOP) * t;
    }
    db
}

/// The level closest to target dB below full volume, or 0 when target falls
/// far enough below level 1 that silence is the nearest thing the ladder has.
fn level_for_db(target: f64) -> u8 {
    let floor = db_below_max(1);
    if target > floor + STEP_DB_BOTTOM / 2.0 {
        return 0;
    }
    let mut best = 1u8;
    let mut best_err = f64::MAX;
    for level in 1..=MAX_VOLUME {
        let err = (db_below_max(level) - target).abs();
        if err < best_err {
            best_err = err;
            best = level;
        }
    }
    best
}

/// Resolves a set volume and an envelope step into the level the channel
/// actually plays at. Both axes are the 0 to MAX_VOLUME ladder, and a step
/// says what the channel would play if its set volume were MAX_VOLUME, so the
/// two attenuations simply add.
pub fn scale_table() -> Vec<u8> {
    let mut table = vec![0u8; SCALE_TABLE_SIZE];
    for volume in 1..=MAX_VOLUME {
        let row = volume as usize * SCALE_STRIDE;
        for step in 1..=MAX_VOLUME {
            table[row + step as usize] = level_for_db(db_below_max(volume) + db_below_max(step));
        }
        // Padding columns clamp to a full-scale step rather than silence.
        for step in VOLUME_LEVELS..SCALE_STRIDE {
            table[row + step] = volume;
        }
    }
    table
}
