use std::path::PathBuf;

use crate::file::TuningData;

pub enum ComponentAction {
    RequestFocus,
    OpenInstrumentEditor(usize),
    OpenEnvelopeEditor(usize),
    OpenTuningEditor,
    OpenFileBrowser,
    OpenQuitConfirm,
    SaveFile,
    Export,
    ConfirmExport(PathBuf),
    Play,
    Quit,
    SaveAndQuit,
    InstrumentSaved(usize, [u8; 256]),
    EnvelopeSaved(usize, Vec<u8>),
    TuningSaved(TuningData),
    OpenFile(PathBuf),
    CreateNewFile(PathBuf),
    PatternPrev,
    PatternNext,
    PatternNew,
    PatternCopy,
    PatternDelete,
    PatternZap,
    OpenPatternDeleteConfirm,
    MuteChannel,
    UnmuteChannel,
    ToggleMuteChannel,
    SoloChannel,
}
