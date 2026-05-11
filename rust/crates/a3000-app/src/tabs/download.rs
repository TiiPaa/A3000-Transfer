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
    /// Demande d'arrêt entre 2 items : l'item courant termine normalement,
    /// mais la batch n'enchaîne pas sur le suivant.
    pub stop_requested: bool,
}

/// Réservation pour le footer (cf. upload.rs).
const FOOTER_RESERVED_H: f32 = 70.0;

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

    // Bloc table cadré strictement (allocate_exact_size + child_ui +
    // set_clip_rect) — clip VISUEL pour empêcher les rows de déborder
    // par-dessus header/footer.
    let block_h = (ui.available_height() - FOOTER_RESERVED_H).max(100.0);
    let block_size = egui::vec2(ui.available_width(), block_h);
    let (block_rect, _) = ui.allocate_exact_size(block_size, egui::Sense::hover());
    {
        let mut block_ui = ui.child_ui(
            block_rect,
            egui::Layout::top_down(egui::Align::Min),
            None,
        );
        block_ui.set_clip_rect(block_rect);
        if state.samples.is_empty() {
            block_ui.add_space(60.0);
            block_ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Aucun scan effectué").color(palette::FG_DIM).size(18.0),
                );
            });
        } else {
            show_table(&mut block_ui, state);
        }
    }

    // Footer en bas, dans l'espace réservé.
    ui.separator();
    ui.horizontal(|ui| {
        let n_checked = state.checked.len();
        ui.label(
            egui::RichText::new(format!("Sélectionnés : {n_checked}/{}", state.samples.len()))
                .color(palette::FG_DIM),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = state.current_idx.is_some();
            // Bouton Download → Stop quand un batch tourne (stop_batch
            // semantics : l'item en cours finit, pas d'enchaînement sur les
            // suivants). Cf. upload.rs : pas de cancel mid-transfert.
            let (label, fill, enabled) = if busy {
                let txt = if state.stop_requested { "Stopping…".to_string() }
                          else { "Stop".to_string() };
                (txt, palette::ACCENT_RED, !state.stop_requested)
            } else {
                (format!("Download {}", n_checked), palette::ACCENT_GREEN, n_checked > 0)
            };
            let btn = egui::Button::new(
                egui::RichText::new(label).color(egui::Color32::WHITE),
            ).fill(fill);
            let resp = ui.add_enabled_ui(enabled, |ui| {
                ui.add_sized([140.0, 32.0], btn)
            }).inner;
            if resp.clicked() {
                if busy {
                    state.stop_requested = true;
                } else {
                    state.request_download = true;
                }
            }
        });
    });
}

const ROW_H: f32 = 28.0;
// 36 px : 6 px add_space gauche (cf. upload.rs — empêche le clip du focus
// stroke d'egui qui dépasse de ~2 px le bord visible du box) + 16 px box + marge.
const COL_CHECK: f32 = 36.0;
const CHECKBOX_LEFT_PAD: f32 = 6.0;
const COL_SLOT: f32 = 60.0;
const COL_NAME: f32 = 220.0;
const COL_FORMAT: f32 = 160.0;
const COL_FRAMES: f32 = 90.0;
const COL_DUR: f32 = 70.0;
const COL_PROGRESS: f32 = 140.0;

fn cell<R>(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // Cf. upload.rs : intersection manuelle avec ui.clip_rect() pour respecter
    // le clip hérité (ScrollArea viewport).
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(w, ROW_H),
        egui::Sense::hover(),
    );
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let mut child = ui.child_ui(rect, layout, None);
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    add(&mut child)
}

fn show_table(ui: &mut egui::Ui, state: &mut DownloadState) {
    let max_h = ui.available_height();
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .max_height(max_h)
        .show(ui, |ui| {
        // Header row
        ui.horizontal(|ui| {
            cell(ui, COL_CHECK, |ui| {
                ui.add_space(CHECKBOX_LEFT_PAD);
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
                    ui.add_space(CHECKBOX_LEFT_PAD);
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
                        super::upload::paint_progress_bar(
                            ui, current_progress, COL_PROGRESS - 10.0, 14.0,
                        );
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
