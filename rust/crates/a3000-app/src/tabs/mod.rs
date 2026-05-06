//! 3 tabs : Upload (envoi WAV → sampler), Download (récupération sampler →
//! WAV), Slicer (découpe par transients + drag MIDI).
//!
//! Phase 3a : placeholders avec layout de base. Le contenu détaillé
//! (tables, drag-drop, widgets custom) viendra dans les phases 3b-3c-4.

pub mod upload;
pub mod download;
pub mod slicer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Upload,
    Download,
    Slicer,
}

impl Tab {
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Upload => "Upload",
            Tab::Download => "Download",
            Tab::Slicer => "Slicer",
        }
    }
}
