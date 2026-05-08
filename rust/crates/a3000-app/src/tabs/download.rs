//! Tab Download — scan + récupération de samples du sampler vers WAV.
//!
//! Phase 3d.4 :
//!   - Bouton Scan : envoie Cmd::ListSamples (start=0 limit=128)
//!   - Affichage progress du scan (Event::ScanProgress)
//!   - Table des samples reçus (slot/name/format/duration)
//!   - Multi-sélection via checkboxes
//!   - Output dir picker (texte simple en l'absence de file dialog)
//!   - Bouton Download : envoie Cmd::Receive séquentiellement pour chaque coché
//!   - Progression via Event::Progress / Event::Received

#![allow(dead_code)] // wiring App-side dans la sous-tâche suivante

use std::path::PathBuf;

use eframe::egui;

use crate::ipc::SampleInfo;
use crate::theme::palette;

#[derive(Default)]
pub struct DownloadState {
    pub samples: Vec<SampleInfo>,
    /// Set du slot des samples cochés pour download.
    pub checked: std::collections::BTreeSet<u32>,
    pub scan_progress: Option<(u32, u32)>, // (scanned, found)
    pub output_dir: Option<PathBuf>,
    /// Index dans `samples` du sample en cours de download.
    pub current_idx: Option<usize>,
    /// True quand l'utilisateur a cliqué Scan.
    pub request_scan: bool,
    /// True quand l'utilisateur a cliqué Download.
    pub request_download: bool,
    /// Progression du download courant.
    pub download_progress: f32,
}

pub fn show(ui: &mut egui::Ui, state: &mut DownloadState) {
    ui.add_space(6.0);
    ui.heading("Download");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Scan les slots du sampler ; sélectionne les samples à récupérer.",
        ).color(palette::FG_DIM),
    );
    ui.separator();

    // Header : bouton Scan + progress + output dir
    ui.horizontal(|ui| {
        let busy = state.current_idx.is_some();
        let scan_btn = egui::Button::new("Scan").fill(palette::BG_PANEL_LIGHT);
        let resp = ui.add_enabled_ui(!busy, |ui| {
            ui.add_sized([80.0, 32.0], scan_btn)
        }).inner;
        if resp.clicked() {
            state.request_scan = true;
        }
        if let Some((scanned, found)) = state.scan_progress {
            ui.label(
                egui::RichText::new(format!("Scan en cours : {scanned} slots, {found} samples"))
                    .color(palette::ACCENT_YELLOW),
            );
        }
        ui.separator();
        ui.label(egui::RichText::new("Dossier sortie :").color(palette::FG_DIM));
        let dir_str = state.output_dir.as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "%TEMP%/a3000_downloads".into());
        ui.label(dir_str);
    });

    ui.separator();

    // Footer ancré en bas (cf. upload.rs : sinon la ScrollArea avale tout).
    let footer_id = ui.id().with("download_footer");
    egui::TopBottomPanel::bottom(footer_id)
        .resizable(false)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.separator();
            ui.horizontal(|ui| {
                let n_checked = state.checked.len();
                ui.label(
                    egui::RichText::new(format!("Sélectionnés : {n_checked}/{}", state.samples.len()))
                        .color(palette::FG_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let busy = state.current_idx.is_some();
                    let dl_enabled = !busy && n_checked > 0;
                    let label = if busy { "Downloading…".to_string() }
                                else { format!("Download {}", n_checked) };
                    let fill = if busy { palette::ACCENT_YELLOW } else { palette::ACCENT_GREEN };
                    let btn = egui::Button::new(
                        egui::RichText::new(label).color(egui::Color32::WHITE),
                    ).fill(fill);
                    let resp = ui.add_enabled_ui(dl_enabled, |ui| {
                        ui.add_sized([140.0, 32.0], btn)
                    }).inner;
                    if resp.clicked() {
                        state.request_download = true;
                    }
                });
            });
            ui.add_space(4.0);
        });

    if state.samples.is_empty() {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Aucun scan effectué").color(palette::FG_DIM).size(18.0),
            );
        });
    } else {
        show_table(ui, state);
    }
}

const ROW_H: f32 = 28.0;
const COL_CHECK: f32 = 28.0;
const COL_SLOT: f32 = 60.0;
const COL_NAME: f32 = 220.0;
const COL_FORMAT: f32 = 160.0;
const COL_FRAMES: f32 = 90.0;
const COL_DUR: f32 = 70.0;
const COL_PROGRESS: f32 = 140.0;

fn cell<R>(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(w, ROW_H),
        egui::Sense::hover(),
    );
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let mut child = ui.child_ui(rect, layout, None);
    child.set_clip_rect(rect);
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    add(&mut child)
}

fn show_table(ui: &mut egui::Ui, state: &mut DownloadState) {
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        // Header row
        ui.horizontal(|ui| {
            cell(ui, COL_CHECK, |ui| {
                let all_checked = state.checked.len() == state.samples.len()
                    && !state.samples.is_empty();
                let mut header_check = all_checked;
                if ui.checkbox(&mut header_check, "").changed() {
                    if header_check {
                        state.checked = state.samples.iter().map(|s| s.slot).collect();
                    } else {
                        state.checked.clear();
                    }
                }
            });
            header_label(ui, "Slot", COL_SLOT);
            header_label(ui, "Name", COL_NAME);
            header_label(ui, "Format", COL_FORMAT);
            header_label(ui, "Frames", COL_FRAMES);
            header_label(ui, "Dur", COL_DUR);
            header_label(ui, "Progress", COL_PROGRESS);
        });
        ui.separator();

        let current_progress = state.download_progress;
        let current_slot = state.current_idx.and_then(|i| state.samples.get(i).map(|s| s.slot));

        for s in &state.samples {
            let is_current = current_slot == Some(s.slot);
            let mut checked = state.checked.contains(&s.slot);
            ui.horizontal(|ui| {
                cell(ui, COL_CHECK, |ui| {
                    if ui.add(egui::Checkbox::without_text(&mut checked)).changed() {
                        if checked { state.checked.insert(s.slot); }
                        else { state.checked.remove(&s.slot); }
                    }
                });
                cell(ui, COL_SLOT, |ui| { ui.label(format!("#{}", s.slot)); });
                cell(ui, COL_NAME, |ui| { ui.label(&s.name); });
                cell(ui, COL_FORMAT, |ui| {
                    let ch_str = match s.channels {
                        1 => "mono".to_string(),
                        2 => "stereo".to_string(),
                        n => format!("{n}ch"),
                    };
                    ui.label(format!("{}-bit {} {}Hz", s.bits, ch_str, s.sample_rate));
                });
                cell(ui, COL_FRAMES, |ui| { ui.label(format!("{}", s.frames)); });
                cell(ui, COL_DUR, |ui| { ui.label(format!("{:.2}s", s.duration)); });
                cell(ui, COL_PROGRESS, |ui| {
                    if is_current {
                        ui.add(egui::ProgressBar::new(current_progress)
                            .desired_width(COL_PROGRESS - 10.0).show_percentage());
                    }
                });
            });
        }
    });
}

fn header_label(ui: &mut egui::Ui, text: &str, width: f32) {
    cell(ui, width, |ui| {
        ui.label(egui::RichText::new(text).color(palette::FG_DIM).strong());
    });
}
