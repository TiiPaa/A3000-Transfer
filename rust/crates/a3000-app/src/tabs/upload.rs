//! Tab Upload — placeholder Phase 3a.
//!
//! TODO Phase 3b-3c :
//!   - Liste de fichiers (WAV) avec checkbox multi-sélection
//!   - Drag-IN files + archives (.zip / .tar.gz)
//!   - Auto/Manual start slot (toggle)
//!   - Boutons Upload (accent), Reset state, Retry, Clear, Add files,
//!     Preview, Rename, Remove
//!   - Total summary (X/Y checked, MB, durée)
//!   - Progress bar par item

#![allow(dead_code)] // scaffolding Phase 3a

use eframe::egui;

use crate::theme::palette;

#[derive(Default)]
pub struct UploadState {
    /// Files queued for upload (display path + checkbox state).
    pub items: Vec<UploadItem>,
}

pub struct UploadItem {
    pub path: std::path::PathBuf,
    pub checked: bool,
    pub state: UploadItemState,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadItemState {
    #[default]
    Pending,
    Running,
    Done,
    Error,
}

pub fn show(ui: &mut egui::Ui, state: &mut UploadState) {
    ui.heading("Upload");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Drag des WAV / .zip / .tar.gz pour les ajouter à la queue, ou clique \"Add files\"."
        ).color(palette::FG_DIM)
    );
    ui.separator();

    if state.items.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Aucun fichier — drop ou Add files")
                    .color(palette::FG_DIM)
                    .size(16.0)
            );
        });
        ui.add_space(40.0);
    } else {
        // Phase 3b : remplacer par un table widget custom avec colonnes
        // (Checkbox / File name / Sample name / Format / Size / Duration / Slot / State / Progress).
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in &mut state.items {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut item.checked, "");
                    ui.label(item.path.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("?"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("{:?}", item.state));
                    });
                });
            }
        });
    }

    ui.separator();
    ui.horizontal(|ui| {
        let upload_btn = egui::Button::new(
            egui::RichText::new("Upload").color(egui::Color32::WHITE).strong()
        ).fill(palette::ACCENT_GREEN);
        if ui.add_enabled(!state.items.is_empty(), upload_btn).clicked() {
            // TODO 3c : kick off transfer batch via WorkerClient.
        }
        if ui.button("Add files…").clicked() {
            // TODO 3b : file dialog
        }
        if ui.button("Clear").clicked() {
            state.items.clear();
        }
    });
}
