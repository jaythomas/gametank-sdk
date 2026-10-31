use std::collections::HashSet;

use crate::{file::TrackerFile, tracker::SequencerCmd};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidationError {
    pub pattern: usize,
    pub beat: usize,
}

const BEAT_CHECKS: &[fn(&TrackerFile, usize, usize) -> bool] = &[is_invalid_jump];

pub fn validate_track(file: &TrackerFile) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for pattern in 0..file.patterns.len() {
        for beat in 0..file.beats_for(pattern as u8) as usize {
            if BEAT_CHECKS.iter().any(|check| check(file, pattern, beat)) {
                errors.push(ValidationError { pattern, beat });
            }
        }
    }
    errors
}

fn is_invalid_jump(file: &TrackerFile, pattern_idx: usize, row: usize) -> bool {
    let Some((target, beat, is_jump)) = jump_at(file, pattern_idx, row) else {
        return false;
    };
    beat >= file.beats_for(target as u8) || (is_jump && jump_loops_back(file, pattern_idx, row))
}

fn jump_at(file: &TrackerFile, pattern_idx: usize, row: usize) -> Option<(usize, u8, bool)> {
    let (target, beat, is_jump) = match file.patterns.get(pattern_idx)?[0].get(row)?.sqc {
        Some(SequencerCmd::CountJump(p, b)) => (p, b, false),
        Some(SequencerCmd::Jump(p, b)) => (p, b, true),
        _ => return None,
    };
    Some(((target as usize).min(file.patterns.len() - 1), beat, is_jump))
}

fn jump_loops_back(file: &TrackerFile, pattern_idx: usize, row: usize) -> bool {
    let start = (pattern_idx, row);
    let mut visited = HashSet::new();
    let mut cell = start;
    while let Some((target, beat, true)) = jump_at(file, cell.0, cell.1) {
        let last_beat = (file.beats_for(target as u8) as usize).saturating_sub(1);
        cell = (target, (beat as usize).min(last_beat));
        if cell == start {
            return true;
        }
        if !visited.insert(cell) {
            return false;
        }
    }
    false
}
