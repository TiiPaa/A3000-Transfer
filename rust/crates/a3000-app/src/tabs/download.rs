//! Tab Download — placeholder Phase 3a.
//!
//! TODO Phase 3c :
//!   - Bouton Scan (lance ListSamples côté worker)
//!   - Treeview/table des samples détectés
//!   - Multi-sélection + Download selection (Receive cmd batch)
//!   - Output dir picker

#![allow(dead_code)] // scaffolding Phase 3a

use eframe::egui;

use crate::ipc::SampleInfo;
use crate::theme::palette;

#[derive(Default)]
pub struct DownloadState {
    pub samples: Vec<SampleInfo>,
    pub scan_progress: Option<(u32, u32)>, // (scanned, found)
    pub output_dir: Option<std::path::PathBuf>,
}

pub fn show(ui: &mut egui::Ui, state: &mut DownloadState) {
    ui.heading("Download");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Scanne les slots du sampler et télécharge ceux sélectionnés."
        ).color(palette::FG_DIM)
    );
    ui.separator();

    ui.horizontal(|ui| {
        if ui.button("Scan").clicked() {
            // TODO 3c : send Cmd::ListSamples
        }
        if let Some((scanned, found)) = state.scan_progress {
            ui.label(format!("Scanning… {scanned} slots, {found} samples trouvés"));
        }
    });

    ui.separator();

    if state.samples.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Pas de scan effectué")
                    .color(palette::FG_DIM)
                    .size(16.0)
            );
        });
        ui.add_space(40.0);
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for s in &state.samples {
                ui.horizontal(|ui| {
                    ui.label(format!("#{}", s.slot));
                    ui.label(&s.name);
                    ui.label(format!("{}b {}ch {}Hz", s.bits, s.channels, s.sample_rate));
                    ui.label(format!("{:.2}s", s.duration));
                });
            }
        });
    }
}
