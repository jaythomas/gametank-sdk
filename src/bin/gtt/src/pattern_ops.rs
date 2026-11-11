use crate::{
    action::PatternSlot,
    file::{MAX_PATTERNS, TrackerFile},
    tracker::{DEFAULT_PATTERN_BEATS, Pattern, SequencerCmd, empty_pattern},
};

fn remap_jump_patterns(patterns: &mut [Pattern], remap: impl Fn(u8) -> u8) {
    for pattern in patterns {
        for beat in &mut pattern[0] {
            if let Some(SequencerCmd::Jump(pat, _) | SequencerCmd::CountJump(pat, _)) =
                &mut beat.sqc
            {
                *pat = remap(*pat);
            }
        }
    }
}

fn shift_jumps_from(file: &mut TrackerFile, at: usize) {
    remap_jump_patterns(&mut file.patterns, |p| {
        if p as usize >= at {
            p.saturating_add(1)
        } else {
            p
        }
    });
}

fn resolve_slot(file: &TrackerFile, src: usize, slot: PatternSlot) -> Option<usize> {
    let len = file.patterns.len();
    let at = match slot {
        PatternSlot::AfterCurrent => src + 1,
        PatternSlot::End => len,
        PatternSlot::At(n) => n,
    };
    (src < len && at <= len).then_some(at)
}

pub fn copy_pattern(file: &mut TrackerFile, src: usize, slot: PatternSlot) -> Option<usize> {
    if file.patterns.len() >= MAX_PATTERNS {
        return None;
    }
    let at = resolve_slot(file, src, slot)?;
    file.pattern_beats
        .resize(file.patterns.len(), DEFAULT_PATTERN_BEATS as u8);
    let beats = file.beats_for(src as u8);
    let copy = file.patterns[src].clone();
    file.patterns.insert(at, copy);
    file.pattern_beats.insert(at, beats);
    shift_jumps_from(file, at);
    let src_idx = if src >= at { src + 1 } else { src };
    remap_jump_patterns(std::slice::from_mut(&mut file.patterns[at]), |p| {
        if p as usize == src_idx { at as u8 } else { p }
    });
    Some(at)
}

pub fn new_pattern(file: &mut TrackerFile, current: usize, slot: PatternSlot) -> Option<usize> {
    let len = file.patterns.len();
    if len >= MAX_PATTERNS {
        return None;
    }
    let at = match slot {
        PatternSlot::AfterCurrent => (current + 1).min(len),
        PatternSlot::End => len,
        PatternSlot::At(n) => n,
    };
    if at > len {
        return None;
    }
    file.pattern_beats.resize(len, DEFAULT_PATTERN_BEATS as u8);
    file.patterns.insert(at, empty_pattern());
    file.pattern_beats.insert(at, DEFAULT_PATTERN_BEATS as u8);
    shift_jumps_from(file, at);
    Some(at)
}

pub fn zap_pattern(file: &mut TrackerFile, idx: usize) {
    *file.current_pattern_mut(idx as u8) = empty_pattern();
}

pub fn delete_pattern(file: &mut TrackerFile, idx: usize) -> Option<usize> {
    if file.patterns.len() <= 1 {
        zap_pattern(file, 0);
        file.set_beats_for(0, DEFAULT_PATTERN_BEATS as u8);
        return Some(0);
    }
    if idx >= file.patterns.len() {
        return None;
    }
    file.pattern_beats
        .resize(file.patterns.len(), DEFAULT_PATTERN_BEATS as u8);
    file.patterns.remove(idx);
    file.pattern_beats.remove(idx);
    remap_jump_patterns(
        &mut file.patterns,
        |p| {
            if p as usize > idx { p - 1 } else { p }
        },
    );
    Some(idx.min(file.patterns.len() - 1))
}

pub fn move_pattern(file: &mut TrackerFile, src: usize, slot: PatternSlot) -> Option<usize> {
    let at = resolve_slot(file, src, slot)?;
    let dest = if at > src { at - 1 } else { at };
    if dest == src {
        return Some(src);
    }
    file.pattern_beats
        .resize(file.patterns.len(), DEFAULT_PATTERN_BEATS as u8);
    let pattern = file.patterns.remove(src);
    let beats = file.pattern_beats.remove(src);
    file.patterns.insert(dest, pattern);
    file.pattern_beats.insert(dest, beats);
    remap_jump_patterns(&mut file.patterns, |p| {
        if p as usize == src {
            return dest as u8;
        }
        let shifted = if p as usize > src { p - 1 } else { p };
        if shifted as usize >= dest {
            shifted + 1
        } else {
            shifted
        }
    });
    Some(dest)
}
