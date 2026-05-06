//! Tab Slicer — placeholder Phase 3a.
//!
//! TODO Phase 4 :
//!   - Custom waveform widget (peaks/RMS bins, zoom, playhead, onsets draggables)
//!   - Détection auto via `a3000_onset::detect_transients`
//!   - Selection cells (drag-select) pour marquer les slices à exporter
//!   - Bouton Reset (recharge fichier source via a3000_core::wav::load_wave)
//!   - Bouton Delete marked
//!   - ▶ Loop : playback via cpal
//!   - Spinbox Beats + génération MIDI (a3000_core::midi)
//!   - Drag-OUT MIDI vers DAW (OLE / IDataObject + DoDragDrop)

#![allow(dead_code)] // scaffolding Phase 3a

use eframe::egui;

use crate::theme::palette;

#[derive(Default)]
pub struct SlicerState {
    pub source_path: Option<std::path::PathBuf>,
    pub onsets: Vec<usize>,
    pub n_beats: u32,
}

pub fn show(ui: &mut egui::Ui, _state: &mut SlicerState) {
    ui.heading("Slicer");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Drop un WAV ; les transients sont détectés via a3000-onset \
             (port pur Rust de librosa.onset_detect)."
        ).color(palette::FG_DIM)
    );
    ui.separator();

    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("Phase 4 — TODO : waveform widget + drag MIDI")
                .color(palette::FG_DIM)
                .size(16.0)
        );
    });
    ui.add_space(40.0);
}
