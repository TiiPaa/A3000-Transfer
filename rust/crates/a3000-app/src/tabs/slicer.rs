//! Tab Slicer — découpe par transients + drag-out MIDI.
//!
//! Phase 4a (waveform + auto-onsets) :
//!   - Drop d'un WAV → load_wave → conversion mono f32
//!   - Auto-détection des transients via `a3000_onset::detect_transients`
//!   - Custom waveform widget : peaks par pixel-bin, rendu via `egui::Painter`
//!   - Render des onset markers verticaux
//!
//! Phase 4b (cette itération) :
//!   - Bandeau de selection cells (1 par slice) au-dessus de la waveform
//!   - Click cell → toggle marked
//!   - Drag-select : range mark/unmark
//!   - Drag des onset markers (±4 px hit zone) pour déplacer un onset
//!   - Footer : Reset, Select all, None
//!
//! TODO Phase 4c-e :
//!   - ▶ Loop playback via cpal + playhead animée
//!   - Bouton Delete marked → reconstruit l'audio buffer + réajuste les onsets
//!   - Bouton Send to Upload (cross-tab)
//!   - Spinbox Beats + génération MIDI via `a3000_core::midi::generate_midi`
//!   - Drag-OUT MIDI vers DAW (OLE / IDataObject + DoDragDrop)

#![allow(dead_code)] // wiring complet à venir Phase 4c-e

use std::path::PathBuf;

use eframe::egui;

use a3000_core::wav::{load_wave, WaveError, WavePayload};
use a3000_onset::{detect_transients, DetectOptions};

use crate::theme::palette;

#[derive(Default)]
pub struct SlicerState {
    pub source_path: Option<PathBuf>,
    pub audio: Option<AudioData>,
    pub onsets: Vec<usize>,
    /// Une entrée par slice (= par onset). `true` = marqué pour suppression.
    pub marked: Vec<bool>,
    pub n_beats: u32,
    pub error: Option<String>,
    /// Cache des bins de peaks. Invalidé quand la largeur du widget change.
    peaks_cache: Option<PeaksCache>,
    /// Onset actuellement en cours de drag (idx dans `onsets`).
    dragging_onset: Option<usize>,
    /// Drag-select : slice idx où le drag a commencé + valeur cible.
    drag_select_start: Option<usize>,
    drag_select_target: bool,
}

pub struct AudioData {
    pub sample_rate: u32,
    pub channels: u16,
    pub mono: Vec<f32>,
    pub duration_s: f64,
}

struct PeaksCache {
    width_px: u32,
    /// Pour chaque pixel : (min, max) amplitude dans [-1, 1].
    bins: Vec<(f32, f32)>,
}

impl SlicerState {
    pub fn load(&mut self, path: PathBuf) {
        self.error = None;
        self.peaks_cache = None;
        self.onsets.clear();
        self.marked.clear();
        self.dragging_onset = None;
        self.drag_select_start = None;
        match load_wave(&path) {
            Ok(payload) => {
                let audio = pcm16_le_to_mono_f32(&payload);
                let duration_s = audio.mono.len() as f64 / audio.sample_rate.max(1) as f64;
                let opts = DetectOptions::default();
                self.onsets = detect_transients(&audio.mono, audio.sample_rate, &opts);
                self.marked = vec![false; self.onsets.len()];
                self.source_path = Some(path);
                self.audio = Some(AudioData {
                    sample_rate: audio.sample_rate,
                    channels: audio.channels,
                    mono: audio.mono,
                    duration_s,
                });
                if self.n_beats == 0 {
                    self.n_beats = 16;
                }
            }
            Err(e) => {
                self.error = Some(format_wav_err(&e));
                self.source_path = Some(path);
                self.audio = None;
            }
        }
    }

    pub fn reset(&mut self) {
        if let Some(path) = self.source_path.clone() {
            self.load(path);
        }
    }

    pub fn slice_count(&self) -> usize { self.onsets.len() }

    pub fn marked_count(&self) -> usize {
        self.marked.iter().filter(|&&m| m).count()
    }

    /// slice_idx tel que `onsets[idx] <= sample < onsets[idx+1]`.
    fn slice_at_sample(&self, sample: usize) -> Option<usize> {
        if self.onsets.is_empty() {
            return None;
        }
        match self.onsets.binary_search(&sample) {
            Ok(i) => Some(i),
            Err(0) => Some(0), // avant le 1er onset = slice 0
            Err(i) => Some(i - 1),
        }
    }
}

fn format_wav_err(e: &WaveError) -> String { format!("{e}") }

fn pcm16_le_to_mono_f32(payload: &WavePayload) -> AudioData {
    let channels = payload.channels.max(1);
    let frames = payload.frame_count as usize;
    let inv_max = 1.0 / 32768.0_f32;
    let inv_n = 1.0 / channels as f32;
    let mut mono = Vec::with_capacity(frames);
    let data = &payload.pcm_data;
    for f in 0..frames {
        let mut sum = 0.0_f32;
        for c in 0..channels as usize {
            let off = (f * channels as usize + c) * 2;
            if off + 1 < data.len() {
                let s = i16::from_le_bytes([data[off], data[off + 1]]);
                sum += (s as f32) * inv_max;
            }
        }
        mono.push(sum * inv_n);
    }
    AudioData {
        sample_rate: payload.sample_rate,
        channels,
        mono,
        duration_s: 0.0,
    }
}

fn drain_dropped_wav(ctx: &egui::Context) -> Option<PathBuf> {
    ctx.input(|i| {
        i.raw.dropped_files.iter()
            .filter_map(|f| f.path.clone())
            .find(|p| matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("wav") | Some("WAV"),
            ))
    })
}

pub fn show(ui: &mut egui::Ui, state: &mut SlicerState) {
    if let Some(path) = drain_dropped_wav(ui.ctx()) {
        state.load(path);
    }

    ui.add_space(6.0);
    ui.heading("Slicer");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Drop un WAV ; les transients sont détectés via a3000-onset \
             (port Rust de librosa.onset_detect).",
        ).color(palette::FG_DIM),
    );
    ui.separator();

    show_top_bar(ui, state);

    if let Some(err) = &state.error {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!("! Erreur : {err}"))
                .color(palette::ACCENT_RED),
        );
    }

    // Footer ancré en bas (cf. upload/download).
    let footer_id = ui.id().with("slicer_footer");
    egui::TopBottomPanel::bottom(footer_id)
        .resizable(false)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.separator();
            show_footer(ui, state);
            ui.add_space(4.0);
        });

    if state.audio.is_some() {
        ui.add_space(8.0);
        show_canvas(ui, state);
    } else if state.error.is_none() {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Drop un WAV ici").color(palette::FG_DIM).size(20.0),
            );
        });
    }
}

fn show_top_bar(ui: &mut egui::Ui, state: &mut SlicerState) {
    ui.horizontal(|ui| {
        if let Some(audio) = &state.audio {
            let stem = state.source_path.as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("?");
            let ch_str = match audio.channels {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                n => format!("{n}ch"),
            };
            ui.label(egui::RichText::new(stem).color(palette::FG_TEXT).strong());
            ui.separator();
            ui.label(egui::RichText::new(format!(
                "{} {}Hz — {:.2}s — {} onsets",
                ch_str, audio.sample_rate, audio.duration_s, state.onsets.len(),
            )).color(palette::FG_DIM));
        } else {
            ui.label(egui::RichText::new("Aucun fichier chargé").color(palette::FG_DIM));
        }
    });
}

const CELL_H: f32 = 22.0;
const WAVEFORM_H: f32 = 200.0;
const ONSET_HIT_PX: f32 = 5.0;

fn show_canvas(ui: &mut egui::Ui, state: &mut SlicerState) {
    let avail_w = ui.available_width().max(100.0);

    // 1. Bandeau de selection cells (au-dessus)
    let (cells_rect, cells_resp) = ui.allocate_exact_size(
        egui::vec2(avail_w, CELL_H),
        egui::Sense::click_and_drag(),
    );
    handle_cells_interaction(state, &cells_rect, &cells_resp);
    paint_cells(ui, state, cells_rect);

    // 2. Waveform (en dessous)
    ui.add_space(2.0);
    let (wave_rect, wave_resp) = ui.allocate_exact_size(
        egui::vec2(avail_w, WAVEFORM_H),
        egui::Sense::click_and_drag(),
    );
    handle_waveform_interaction(ui, state, &wave_rect, &wave_resp);

    // Recalcule le cache si la largeur a changé.
    let width_px = wave_rect.width().round() as u32;
    let need_recompute = state.peaks_cache.as_ref()
        .map(|c| c.width_px != width_px)
        .unwrap_or(true);
    if need_recompute {
        if let Some(audio) = &state.audio {
            state.peaks_cache = Some(compute_peaks(&audio.mono, width_px));
        }
    }

    paint_waveform(ui, state, wave_rect);
}

fn handle_cells_interaction(
    state: &mut SlicerState,
    rect: &egui::Rect,
    resp: &egui::Response,
) {
    let total = match &state.audio { Some(a) => a.mono.len(), None => return };
    let total_f = total as f32;

    let pointer_pos_to_slice = |p: egui::Pos2| -> Option<usize> {
        if !rect.x_range().contains(p.x) { return None; }
        let frac = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let sample = (frac * total_f) as usize;
        state.slice_at_sample(sample)
    };

    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(idx) = pointer_pos_to_slice(p) {
                state.drag_select_start = Some(idx);
                let cur = state.marked.get(idx).copied().unwrap_or(false);
                state.drag_select_target = !cur;
                if let Some(m) = state.marked.get_mut(idx) {
                    *m = state.drag_select_target;
                }
            }
        }
    } else if resp.dragged() {
        if let Some(start) = state.drag_select_start {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(cur_idx) = pointer_pos_to_slice(p) {
                    let (lo, hi) = if cur_idx < start { (cur_idx, start) }
                                   else { (start, cur_idx) };
                    for i in lo..=hi {
                        if let Some(m) = state.marked.get_mut(i) {
                            *m = state.drag_select_target;
                        }
                    }
                }
            }
        }
    } else if resp.drag_stopped() {
        state.drag_select_start = None;
    } else if resp.clicked() {
        // Click simple sans drag : toggle juste la cell sous le curseur.
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(idx) = pointer_pos_to_slice(p) {
                if let Some(m) = state.marked.get_mut(idx) {
                    *m = !*m;
                }
            }
        }
    }
}

fn paint_cells(ui: &egui::Ui, state: &SlicerState, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, palette::BG_PANEL_LIGHT);

    let total = match &state.audio { Some(a) => a.mono.len(), None => return };
    let total_f = total as f32;
    let width = rect.width();

    for (i, &start) in state.onsets.iter().enumerate() {
        let end = state.onsets.get(i + 1).copied().unwrap_or(total) as f32;
        let x0 = rect.left() + (start as f32 / total_f).clamp(0.0, 1.0) * width;
        let x1 = rect.left() + (end / total_f).clamp(0.0, 1.0) * width;
        if x1 - x0 < 1.0 { continue; }
        let cell = egui::Rect::from_min_max(
            egui::pos2(x0 + 1.0, rect.top() + 2.0),
            egui::pos2(x1 - 1.0, rect.bottom() - 2.0),
        );
        let marked = state.marked.get(i).copied().unwrap_or(false);
        let fill = if marked { palette::ACCENT_GREEN } else { palette::BG_PANEL };
        painter.rect_filled(cell, 1.5, fill);

        // Numéro de slice (uniquement si la cellule est assez large).
        if cell.width() >= 24.0 {
            let txt_color = if marked { egui::Color32::WHITE } else { palette::FG_DIM };
            painter.text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}", i + 1),
                egui::FontId::proportional(11.0),
                txt_color,
            );
        }
    }
}

fn handle_waveform_interaction(
    ui: &egui::Ui,
    state: &mut SlicerState,
    rect: &egui::Rect,
    resp: &egui::Response,
) {
    let total = match &state.audio { Some(a) => a.mono.len(), None => return };
    let total_f = total as f32;

    let x_to_sample = |x: f32| -> usize {
        let frac = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        (frac * total_f) as usize
    };
    let sample_to_x = |s: usize| -> f32 {
        rect.left() + (s as f32 / total_f).clamp(0.0, 1.0) * rect.width()
    };

    // Hover seulement pour le curseur. Le drag est piloté par
    // is_pointer_button_down_on (capture dès le press, pas drag_started — le
    // drag_started se déclenche après ~6 px de déplacement, donc le curseur
    // a déjà quitté la zone de hit ±ONSET_HIT_PX du marker).
    let hover_pos = ui.ctx().pointer_hover_pos();
    let mut hovered_onset: Option<usize> = None;
    if let Some(p) = hover_pos {
        if rect.contains(p) {
            for (i, &o) in state.onsets.iter().enumerate() {
                if (sample_to_x(o) - p.x).abs() < ONSET_HIT_PX {
                    hovered_onset = Some(i);
                    break;
                }
            }
        }
    }
    if hovered_onset.is_some() || state.dragging_onset.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // Première frame de press : on capture l'onset sous le curseur en utilisant
    // la position de press (avant le drag_threshold).
    if resp.is_pointer_button_down_on() && state.dragging_onset.is_none() {
        if let Some(p) = resp.interact_pointer_pos() {
            for (i, &o) in state.onsets.iter().enumerate() {
                if (sample_to_x(o) - p.x).abs() < ONSET_HIT_PX {
                    state.dragging_onset = Some(i);
                    break;
                }
            }
        }
    }

    // Mise à jour position pendant que la souris bouge (et que le bouton est
    // toujours pressé sur ce widget).
    if resp.is_pointer_button_down_on() {
        if let Some(idx) = state.dragging_onset {
            if let Some(p) = resp.interact_pointer_pos() {
                let new_sample = x_to_sample(p.x);
                let lo = if idx > 0 { state.onsets[idx - 1].saturating_add(1) } else { 0 };
                let hi = state.onsets.get(idx + 1).copied()
                    .unwrap_or(total).saturating_sub(1);
                if let Some(o) = state.onsets.get_mut(idx) {
                    *o = new_sample.clamp(lo, hi);
                }
            }
        }
    } else {
        // Bouton relâché — fin du drag.
        state.dragging_onset = None;
    }
}

fn paint_waveform(ui: &egui::Ui, state: &SlicerState, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, palette::BG_DEEP);

    let mid_y = rect.center().y;
    painter.line_segment(
        [egui::pos2(rect.left(), mid_y), egui::pos2(rect.right(), mid_y)],
        egui::Stroke::new(1.0, palette::SEPARATOR),
    );

    if let Some(cache) = &state.peaks_cache {
        let h = rect.height();
        let half_h = h * 0.5 - 2.0;
        for (i, (mn, mx)) in cache.bins.iter().enumerate() {
            let x = rect.left() + i as f32;
            let y_top = (mid_y - mx.clamp(-1.0, 1.0) * half_h).min(mid_y);
            let y_bot = (mid_y - mn.clamp(-1.0, 1.0) * half_h).max(mid_y);
            painter.line_segment(
                [egui::pos2(x, y_top), egui::pos2(x, y_bot)],
                egui::Stroke::new(1.0, palette::ACCENT_GREEN),
            );
        }
    }

    // Onset markers verticaux.
    let total = match &state.audio { Some(a) => a.mono.len(), None => return };
    let total_f = total as f32;
    if total_f > 0.0 {
        for (i, &s) in state.onsets.iter().enumerate() {
            let frac = (s as f32 / total_f).clamp(0.0, 1.0);
            let x = rect.left() + frac * rect.width();
            let dragging = state.dragging_onset == Some(i);
            let stroke = egui::Stroke::new(
                if dragging { 2.5 } else { 1.5 },
                if dragging { palette::ACCENT_ORANGE } else { palette::ACCENT_YELLOW },
            );
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                stroke,
            );
        }
    }
}

fn show_footer(ui: &mut egui::Ui, state: &mut SlicerState) {
    const BTN_H: f32 = 32.0;
    let total = state.slice_count();
    let marked = state.marked_count();
    ui.horizontal(|ui| {
        if total > 0 {
            ui.label(
                egui::RichText::new(format!("Marqués : {marked}/{total}"))
                    .color(palette::FG_DIM),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let has_audio = state.audio.is_some();
            // Reset (recharge fichier source)
            if ui.add_enabled_ui(has_audio, |ui| {
                ui.add_sized([100.0, BTN_H], egui::Button::new("Reset"))
            }).inner.clicked() {
                state.reset();
            }
            ui.add_space(8.0);
            // None → décoche tout
            if ui.add_enabled_ui(has_audio && marked > 0, |ui| {
                ui.add_sized([80.0, BTN_H], egui::Button::new("None"))
            }).inner.clicked() {
                for m in &mut state.marked { *m = false; }
            }
            // Select all
            if ui.add_enabled_ui(has_audio && marked < total, |ui| {
                ui.add_sized([90.0, BTN_H], egui::Button::new("Select all"))
            }).inner.clicked() {
                for m in &mut state.marked { *m = true; }
            }
        });
    });
}

/// Compute peaks (min, max amplitude) per pixel-bin. Style 8-bit (pas de smoothing).
fn compute_peaks(samples: &[f32], width_px: u32) -> PeaksCache {
    let width = width_px.max(1) as usize;
    let n = samples.len();
    let mut bins: Vec<(f32, f32)> = Vec::with_capacity(width);
    if n == 0 || width == 0 {
        return PeaksCache { width_px, bins };
    }
    for i in 0..width {
        let lo = (i as u64 * n as u64 / width as u64) as usize;
        let hi = ((i as u64 + 1) * n as u64 / width as u64) as usize;
        let hi = hi.min(n).max(lo + 1);
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for &s in &samples[lo..hi] {
            if s < mn { mn = s; }
            if s > mx { mx = s; }
        }
        bins.push((mn, mx));
    }
    PeaksCache { width_px, bins }
}
