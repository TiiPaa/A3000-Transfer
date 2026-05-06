//! Tab Upload — table de fichiers WAV à transférer vers le sampler.
//!
//! Phase 3d.3 :
//!   - Liste de fichiers avec checkbox multi-sélection
//!   - Drag-IN files WAV (archives .zip/.tar.gz : TODO Phase 5)
//!   - Bouton Add files (file dialog simple via `rfd` non-utilisé pour l'instant
//!     — drag-IN suffit en Phase 3d ; rfd peut être ajouté Phase 5)
//!   - Auto/Manual start slot via Config
//!   - Table : checkbox | File | Sample name (éditable) | Format | Size | Duration | Slot | State | Progress
//!   - Total summary (X/Y checked, total MB, total durée)
//!   - Boutons Upload (accent), Reset state, Clear, Remove selected
//!
//! Reste TODO Phase 3d.3 (intégration App) :
//!   - Câbler le bouton Upload à `WorkerSender::send_cmd(Cmd::Transfer{...})`
//!   - Recevoir Event::Progress / Event::Done pour mettre à jour les rows

#![allow(dead_code)] // wiring App-side viendra dans la sous-tâche suivante

use std::path::{Path, PathBuf};

use eframe::egui;

use a3000_core::wav::peek_wave_metadata;

use crate::config::Config;
use crate::theme::palette;

#[derive(Default)]
pub struct UploadState {
    pub items: Vec<UploadItem>,
    /// Demande de démarrage de batch (set par le bouton Upload, lue par l'App
    /// qui kick off le transfert et reset le flag).
    pub request_upload: bool,
    /// Index de l'item en cours de transfert (Some pendant Running).
    pub current_idx: Option<usize>,
    /// True quand on attend Event::FreeSlot après avoir envoyé FindFreeSlot.
    pub pending_find_slot: bool,
    /// Prochain slot à attribuer (auto-incrémenté entre items).
    pub next_slot: u32,
}

pub struct UploadItem {
    pub path: PathBuf,
    pub sample_name: String,
    pub format_str: String,
    pub size_bytes: u64,
    pub duration_s: f64,
    pub frame_count: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub checked: bool,
    pub slot: Option<u32>,
    pub state: UploadItemState,
    pub progress: f32,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub error_msg: Option<String>,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadItemState {
    #[default]
    Pending,
    Running,
    Done,
    Error,
}

impl UploadItem {
    /// Tente de charger les métadonnées WAV ; en cas d'échec, retourne un
    /// item en erreur (toujours ajouté à la liste, l'utilisateur voit pourquoi).
    pub fn from_path(path: PathBuf) -> Self {
        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sample");
        // Nom du sample limité à 16 chars (limite A3000).
        let sample_name: String = stem.chars().take(16).collect();

        match peek_wave_metadata(&path) {
            Ok(meta) => {
                let duration_s = if meta.sample_rate > 0 {
                    meta.frame_count as f64 / meta.sample_rate as f64
                } else { 0.0 };
                let ch_str = match meta.channels { 1 => "mono", 2 => "stereo", n => return UploadItem {
                    path: path.clone(), sample_name, format_str: format!("{n}ch (non supporté)"),
                    size_bytes, duration_s: 0.0, frame_count: 0,
                    channels: meta.channels, sample_rate: meta.sample_rate,
                    bits_per_sample: meta.bits_per_sample,
                    checked: false, slot: None, state: UploadItemState::Error,
                    progress: 0.0, sent_bytes: 0, total_bytes: 0,
                    error_msg: Some(format!("{n} canaux non supportés")),
                } };
                let format_str = format!(
                    "{}-bit {} {}Hz",
                    meta.bits_per_sample, ch_str, meta.sample_rate,
                );
                Self {
                    path,
                    sample_name,
                    format_str,
                    size_bytes,
                    duration_s,
                    frame_count: meta.frame_count,
                    channels: meta.channels,
                    sample_rate: meta.sample_rate,
                    bits_per_sample: meta.bits_per_sample,
                    checked: true,
                    slot: None,
                    state: UploadItemState::Pending,
                    progress: 0.0,
                    sent_bytes: 0,
                    total_bytes: 0,
                    error_msg: None,
                }
            }
            Err(e) => Self {
                path: path.clone(),
                sample_name,
                format_str: "?".into(),
                size_bytes,
                duration_s: 0.0,
                frame_count: 0,
                channels: 0,
                sample_rate: 0,
                bits_per_sample: 0,
                checked: false,
                slot: None,
                state: UploadItemState::Error,
                progress: 0.0,
                sent_bytes: 0,
                total_bytes: 0,
                error_msg: Some(e.to_string()),
            },
        }
    }
}

impl UploadState {
    pub fn add_path(&mut self, path: PathBuf) {
        // Évite les doublons (même chemin absolu).
        if self.items.iter().any(|it| it.path == path) {
            return;
        }
        self.items.push(UploadItem::from_path(path));
    }

    pub fn checked_count(&self) -> usize {
        self.items.iter().filter(|it| it.checked && it.state != UploadItemState::Error).count()
    }

    pub fn total_mb(&self) -> f64 {
        self.items.iter()
            .filter(|it| it.checked && it.state != UploadItemState::Error)
            .map(|it| it.size_bytes as f64)
            .sum::<f64>() / (1024.0 * 1024.0)
    }

    pub fn total_duration(&self) -> f64 {
        self.items.iter()
            .filter(|it| it.checked && it.state != UploadItemState::Error)
            .map(|it| it.duration_s)
            .sum()
    }

    /// Trouve l'index du prochain item Pending coché (et pas Done/Error).
    pub fn next_pending(&self) -> Option<usize> {
        self.items.iter().position(|it| {
            it.checked
                && it.state == UploadItemState::Pending
                && it.error_msg.is_none()
        })
    }
}

/// Drain les fichiers droppés sur la fenêtre depuis la frame courante et
/// retourne la liste de WAV à ajouter. Fichiers non-WAV ignorés silencieusement.
fn drain_dropped_wavs(ctx: &egui::Context) -> Vec<PathBuf> {
    let raw_files = ctx.input(|i| i.raw.dropped_files.clone());
    raw_files.into_iter()
        .filter_map(|f| f.path)
        .filter(|p| matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("wav") | Some("WAV")
        ))
        .collect()
}

pub fn show(ui: &mut egui::Ui, state: &mut UploadState, config: &Config) {
    // Drop : on ajoute les fichiers WAV droppés.
    let dropped = drain_dropped_wavs(ui.ctx());
    for p in dropped {
        state.add_path(p);
    }

    ui.heading("Upload");
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(format!(
            "Drag des WAV pour les ajouter à la queue. Slot de départ : {}",
            if config.auto_start_slot {
                "auto (1er libre depuis #7)".to_string()
            } else {
                format!("manuel #{}", config.manual_start_slot)
            },
        )).color(palette::FG_DIM),
    );
    ui.separator();

    if state.items.is_empty() {
        empty_drop_zone(ui);
    } else {
        show_table(ui, state);
    }

    ui.separator();
    show_footer(ui, state);
}

fn empty_drop_zone(ui: &mut egui::Ui) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("Drop des WAV ici").color(palette::FG_DIM).size(20.0),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("(8/16/24/32-bit + float ; mono ou stéréo)")
                .color(palette::FG_DIM).size(12.0),
        );
    });
    ui.add_space(60.0);
}

// Largeurs de colonnes (px) — partagées header + rows pour alignement strict.
const ROW_H: f32 = 22.0;
const COL_CHECK: f32 = 28.0;
const COL_FILE: f32 = 200.0;
const COL_NAME: f32 = 140.0;
const COL_FORMAT: f32 = 150.0;
const COL_SIZE: f32 = 80.0;
const COL_DUR: f32 = 70.0;
const COL_SLOT: f32 = 60.0;
const COL_STATE: f32 = 80.0;
const COL_PROGRESS: f32 = 140.0;
const COL_ACTION: f32 = 28.0;

/// Cellule de largeur **strictement fixe** (W × ROW_H), contenu aligné gauche
/// + centré vertical. set_min_size force la taille même si le contenu est court.
fn cell<R>(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(w, ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(w, ROW_H));
            ui.set_clip_rect(ui.max_rect());
            add(ui)
        },
    ).inner
}

fn show_table(ui: &mut egui::Ui, state: &mut UploadState) {
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        // Header row — mêmes largeurs que les rows pour alignement strict.
        ui.horizontal(|ui| {
            // Check-all
            cell(ui, COL_CHECK, |ui| {
                let any_checked = state.items.iter().any(|it| it.checked);
                let all_checked = !state.items.is_empty()
                    && state.items.iter().filter(|it| it.state != UploadItemState::Error)
                        .all(|it| it.checked);
                let mut header_check = all_checked;
                if ui.checkbox(&mut header_check, "").changed() {
                    let target = !any_checked || !all_checked;
                    for it in &mut state.items {
                        if it.state != UploadItemState::Error {
                            it.checked = target;
                        }
                    }
                }
            });
            header_label(ui, "File", COL_FILE);
            header_label(ui, "Sample name", COL_NAME);
            header_label(ui, "Format", COL_FORMAT);
            header_label(ui, "Size", COL_SIZE);
            header_label(ui, "Dur", COL_DUR);
            header_label(ui, "Slot", COL_SLOT);
            header_label(ui, "State", COL_STATE);
            header_label(ui, "Progress", COL_PROGRESS);
            header_label(ui, "", COL_ACTION);
        });
        ui.separator();

        let mut to_remove: Option<usize> = None;
        for (idx, item) in state.items.iter_mut().enumerate() {
            let row_color = match item.state {
                UploadItemState::Done => palette::ACCENT_GREEN,
                UploadItemState::Running => palette::ACCENT_YELLOW,
                UploadItemState::Error => palette::ACCENT_RED,
                UploadItemState::Pending => palette::FG_TEXT,
            };
            ui.horizontal(|ui| {
                cell(ui, COL_CHECK, |ui| {
                    ui.add_enabled(
                        item.state != UploadItemState::Error,
                        egui::Checkbox::without_text(&mut item.checked),
                    );
                });

                let file_name = item.path.file_name()
                    .and_then(|s| s.to_str()).unwrap_or("?");
                cell(ui, COL_FILE, |ui| {
                    ui.label(egui::RichText::new(file_name).color(row_color));
                });

                cell(ui, COL_NAME, |ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut item.sample_name)
                            .desired_width(COL_NAME - 10.0)
                            .char_limit(16),
                    );
                    if resp.changed() && item.sample_name.chars().count() > 16 {
                        item.sample_name = item.sample_name.chars().take(16).collect();
                    }
                });

                cell(ui, COL_FORMAT, |ui| { ui.label(&item.format_str); });
                cell(ui, COL_SIZE, |ui| { ui.label(format_size(item.size_bytes)); });
                cell(ui, COL_DUR, |ui| { ui.label(format_duration(item.duration_s)); });
                cell(ui, COL_SLOT, |ui| {
                    ui.label(match item.slot {
                        Some(n) => format!("#{n}"),
                        None => "—".into(),
                    });
                });
                cell(ui, COL_STATE, |ui| {
                    let (txt, col) = match item.state {
                        UploadItemState::Pending => ("Pending", palette::FG_DIM),
                        UploadItemState::Running => ("Running", palette::ACCENT_YELLOW),
                        UploadItemState::Done => ("Done", palette::ACCENT_GREEN),
                        UploadItemState::Error => ("Error", palette::ACCENT_RED),
                    };
                    ui.label(egui::RichText::new(txt).color(col));
                });
                cell(ui, COL_PROGRESS, |ui| {
                    ui.add(egui::ProgressBar::new(item.progress)
                        .desired_width(COL_PROGRESS - 10.0).show_percentage());
                });
                cell(ui, COL_ACTION, |ui| {
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                        to_remove = Some(idx);
                    }
                });
            });

            if let Some(err) = &item.error_msg {
                ui.horizontal(|ui| {
                    ui.add_space(COL_CHECK);
                    ui.label(
                        egui::RichText::new(format!("⚠ {err}"))
                            .color(palette::ACCENT_RED).small(),
                    );
                });
            }
        }
        if let Some(i) = to_remove {
            state.items.remove(i);
        }
    });
}

fn header_label(ui: &mut egui::Ui, text: &str, width: f32) {
    cell(ui, width, |ui| {
        ui.label(egui::RichText::new(text).color(palette::FG_DIM).strong());
    });
}

fn show_footer(ui: &mut egui::Ui, state: &mut UploadState) {
    let checked = state.checked_count();
    let total = state.items.iter()
        .filter(|it| it.state != UploadItemState::Error).count();
    let mb = state.total_mb();
    let dur = state.total_duration();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "Sélectionnés : {checked}/{total} — {mb:.1} MB — {}",
                format_duration(dur),
            )).color(palette::FG_DIM),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Clear").clicked() {
                state.items.clear();
            }
            if ui.button("Reset state").clicked() {
                for it in &mut state.items {
                    if it.state == UploadItemState::Error || it.state == UploadItemState::Done {
                        it.state = UploadItemState::Pending;
                        it.progress = 0.0;
                        it.sent_bytes = 0;
                        it.total_bytes = 0;
                        it.error_msg = None;
                        it.slot = None;
                    }
                }
            }
            let busy = state.current_idx.is_some() || state.pending_find_slot;
            let upload_enabled = checked > 0 && !busy;
            let label = if busy { "Uploading…".to_string() } else { format!("Upload {} ▶", checked) };
            let upload_btn = egui::Button::new(
                egui::RichText::new(label).color(egui::Color32::WHITE).strong(),
            ).fill(if busy { palette::ACCENT_YELLOW } else { palette::ACCENT_GREEN });
            if ui.add_enabled(upload_enabled, upload_btn).clicked() {
                state.request_upload = true;
            }
        });
    });
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 { return format!("{} B", bytes); }
    if bytes < 1024 * 1024 { return format!("{:.1} KB", bytes as f64 / 1024.0); }
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn format_duration(s: f64) -> String {
    let s = s.max(0.0);
    if s < 60.0 { format!("{:.2}s", s) }
    else if s < 3600.0 {
        let m = (s / 60.0).floor() as u32;
        let sec = s - (m as f64 * 60.0);
        format!("{}m{:.0}s", m, sec)
    } else {
        let h = (s / 3600.0).floor() as u32;
        let rest = s - (h as f64 * 3600.0);
        let m = (rest / 60.0).floor() as u32;
        format!("{}h{}m", h, m)
    }
}

// Helper inutile pour Path.is_dir / etc, mais référencé dans la doc.
#[allow(dead_code)]
fn ext_lower(p: &Path) -> Option<String> {
    p.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase())
}
