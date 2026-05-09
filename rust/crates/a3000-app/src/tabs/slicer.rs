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

use a3000_core::midi::generate_midi;
use a3000_core::wav::{load_wave, WaveError};
use a3000_onset::{detect_transients, DetectOptions};

use crate::audio::{pcm16_le_to_mono_f32, Playback};
use crate::theme::palette;

#[derive(Default)]
pub struct SlicerState {
    pub source_path: Option<PathBuf>,
    pub audio: Option<AudioData>,
    pub onsets: Vec<usize>,
    /// Une entrée par slice. `true` = marqué pour suppression (rouge).
    /// Mutuellement exclusif avec `selected`.
    pub marked: Vec<bool>,
    /// Une entrée par slice. `true` = sélectionné pour export (vert).
    /// Mutuellement exclusif avec `marked`. Si non vide, `Send to Upload`
    /// n'exporte QUE les slices sélectionnées (mode "filter").
    pub selected: Vec<bool>,
    pub n_beats: u32,
    pub error: Option<String>,
    /// Cache des bins de peaks. Invalidé quand la largeur du widget change.
    peaks_cache: Option<PeaksCache>,
    /// Onset actuellement en cours de drag (idx dans `onsets`).
    dragging_onset: Option<usize>,
    /// Pan en cours : (x à l'instant du press, view.start à l'instant du press).
    dragging_pan: Option<(f32, usize)>,
    /// Onset focalisé pour la navigation Space / Shift+Space.
    current_onset: Option<usize>,
    /// Dernier path MIDI généré (pour afficher / drag-out futur).
    pub last_midi_path: Option<PathBuf>,
    /// Flag : l'utilisateur a cliqué Send to Upload — l'app fait l'export
    /// (découpe + WAV) et bascule sur le tab Upload.
    pub request_send_to_upload: bool,
    /// Drag-select : slice idx où le drag a commencé + valeur cible.
    drag_select_start: Option<usize>,
    drag_select_target: bool,
    /// Drag-select : opère sur `selected` (left) ou `marked` (right).
    drag_select_kind: DragKind,
    /// Playback en cours (audio output stream cpal). Drop le stream pour stop.
    /// `cpal::Stream` est `!Send` sur Windows → vit sur le thread GUI.
    playback: Option<Playback>,
    /// Si une preview de slice est en cours, l'index de la slice (None pour
    /// le Loop full audio). Sert à highlight la cellule sans dessiner de
    /// playhead (qui n'aurait pas de sens : le playback joue un buffer
    /// extrait, pas le buffer global).
    previewing_slice: Option<usize>,
    /// Fenêtre visible sur la waveform (zoom + pan). [start, end) en samples.
    view: ViewWindow,
}

/// Distinguer un drag click-gauche (selected) d'un drag click-droit (marked).
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    #[default]
    Selected,
    Marked,
}

/// Fenêtre visible : intervalle [start, end) de samples du buffer audio.
/// Plein zoom-out → start=0, end=audio_len. Zoom in → fenêtre rétrécie.
#[derive(Clone, Copy, Default)]
pub struct ViewWindow {
    pub start: usize,
    pub end: usize,
}

impl ViewWindow {
    fn len(&self) -> usize { self.end.saturating_sub(self.start) }
    fn x_to_sample(&self, x: f32, rect: egui::Rect) -> usize {
        let frac = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        self.start + (frac * self.len() as f32) as usize
    }
    /// Renvoie la position x correspondant au sample `s`. None si hors de la
    /// fenêtre visible.
    fn sample_to_x(&self, s: usize, rect: egui::Rect) -> Option<f32> {
        if s < self.start || s >= self.end { return None; }
        let frac = (s - self.start) as f32 / self.len().max(1) as f32;
        Some(rect.left() + frac * rect.width())
    }
    fn contains(&self, s: usize) -> bool { s >= self.start && s < self.end }
}

pub struct AudioData {
    pub sample_rate: u32,
    pub channels: u16,
    pub mono: Vec<f32>,
    pub duration_s: f64,
}

struct PeaksCache {
    width_px: u32,
    view_start: usize,
    view_end: usize,
    /// Pour chaque pixel : (min, max) amplitude dans [-1, 1].
    bins: Vec<(f32, f32)>,
}

// `Playback` et `pcm16_le_to_mono_f32` sont dans `crate::audio` (partagé
// avec le tab Upload pour la preview WAV).

impl SlicerState {
    pub fn load(&mut self, path: PathBuf) {
        self.error = None;
        self.peaks_cache = None;
        self.onsets.clear();
        self.marked.clear();
        self.selected.clear();
        self.dragging_onset = None;
        self.dragging_pan = None;
        self.drag_select_start = None;
        self.current_onset = None;
        self.playback = None;
        self.previewing_slice = None;
        self.view = ViewWindow::default();
        match load_wave(&path) {
            Ok(payload) => {
                let mono = pcm16_le_to_mono_f32(&payload);
                let sample_rate = payload.sample_rate;
                let channels = payload.channels.max(1);
                let mono_len = mono.len();
                let duration_s = mono_len as f64 / sample_rate.max(1) as f64;
                let opts = DetectOptions::default();
                self.onsets = detect_transients(&mono, sample_rate, &opts);
                self.marked = vec![false; self.onsets.len()];
                self.selected = vec![false; self.onsets.len()];
                self.source_path = Some(path);
                self.audio = Some(AudioData {
                    sample_rate,
                    channels,
                    mono,
                    duration_s,
                });
                self.view = ViewWindow { start: 0, end: mono_len };
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

    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|&&m| m).count()
    }

    /// Mutuellement exclusif : marquer une slice désélectionne et inversement.
    fn set_selected(&mut self, idx: usize, value: bool) {
        if let Some(s) = self.selected.get_mut(idx) { *s = value; }
        if value {
            if let Some(m) = self.marked.get_mut(idx) { *m = false; }
        }
    }

    fn set_marked(&mut self, idx: usize, value: bool) {
        if let Some(m) = self.marked.get_mut(idx) { *m = value; }
        if value {
            if let Some(s) = self.selected.get_mut(idx) { *s = false; }
        }
    }

    /// Reconstruit le buffer audio en concaténant les slices NON marquées,
    /// puis recalcule les onsets dans le nouveau buffer. Les indices de
    /// `marked` étant invalidés, on reset tout à false. Reset le cache de
    /// peaks et l'éventuel onset en cours de drag.
    pub fn delete_marked(&mut self) {
        let Some(audio) = self.audio.as_mut() else { return; };
        if self.marked.iter().all(|&m| !m) {
            return;
        }
        let total = audio.mono.len();
        let mut new_audio: Vec<f32> = Vec::with_capacity(total);
        let mut new_onsets: Vec<usize> = Vec::with_capacity(self.onsets.len());

        for i in 0..self.onsets.len() {
            let start = self.onsets[i];
            let end = self.onsets.get(i + 1).copied().unwrap_or(total);
            let marked = self.marked.get(i).copied().unwrap_or(false);
            if !marked {
                new_onsets.push(new_audio.len());
                if start < end && start < total {
                    let end = end.min(total);
                    new_audio.extend_from_slice(&audio.mono[start..end]);
                }
            }
        }

        // Si on a supprimé absolument tout, on évite de laisser audio.mono
        // empty avec onsets[0]=0 incohérents : on assure au moins qu'onsets
        // est consistant.
        audio.mono = new_audio;
        audio.duration_s = audio.mono.len() as f64 / audio.sample_rate.max(1) as f64;

        self.onsets = new_onsets;
        self.marked = vec![false; self.onsets.len()];
        self.selected = vec![false; self.onsets.len()];
        self.peaks_cache = None;
        self.dragging_onset = None;
        self.dragging_pan = None;
        self.drag_select_start = None;
        self.current_onset = None;
        // L'audio buffer du Playback est cloné au moment de start ; on coupe
        // pour rester cohérent avec le nouveau contenu visible.
        self.playback = None;
        self.previewing_slice = None;
        self.view = ViewWindow { start: 0, end: audio.mono.len() };
    }

    /// Cycle vers l'onset suivant (direction +1) ou précédent (-1) et centre
    /// la vue dessus en préservant le zoom courant.
    pub fn cycle_onset(&mut self, direction: i32) {
        if self.onsets.is_empty() { return; }
        let n = self.onsets.len() as i32;
        let next = match self.current_onset {
            None => if direction > 0 { 0 } else { (n - 1) as usize },
            Some(cur) => ((cur as i32 + direction).rem_euclid(n)) as usize,
        };
        self.current_onset = Some(next);
        let target_sample = self.onsets[next];
        self.center_view_on(target_sample);
    }

    fn center_view_on(&mut self, sample: usize) {
        let Some(audio) = &self.audio else { return; };
        let total = audio.mono.len();
        let len = self.view.len().max(1);
        if total == 0 { return; }
        let new_start = (sample as i64 - (len / 2) as i64)
            .max(0)
            .min(total.saturating_sub(len) as i64) as usize;
        self.view.start = new_start;
        self.view.end = (new_start + len).min(total);
        self.peaks_cache = None;
    }

    /// Découpe le buffer audio aux onsets et écrit chaque slice (non marquée)
    /// comme un fichier WAV mono 16-bit dans
    /// `%TEMP%/a3000_slicer_slices/<stem>_slice_NNN.wav`. Retourne la liste
    /// des paths générés (ou une erreur stringifiée).
    pub fn export_slices_to_wavs(&self) -> Result<Vec<PathBuf>, String> {
        let audio = self.audio.as_ref().ok_or("Aucun fichier audio chargé")?;
        if self.onsets.is_empty() {
            return Err("Aucun onset à exporter".into());
        }
        let stem = self.source_path.as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("slicer")
            .to_string();
        let safe_stem: String = stem.chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let dir = std::env::temp_dir().join("a3000_slicer_slices");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create_dir: {e}"))?;

        let total = audio.mono.len();
        let n = self.onsets.len();
        let n_digits = (n.to_string().len()).max(3);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: audio.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        // Sémantique d'export (port Python view.py) :
        //   - Si au moins une slice est SELECTED (vert) → export = SEULEMENT
        //     les selected (mode "filter").
        //   - Sinon → export = TOUT sauf les marked (rouge).
        let any_selected = self.selected.iter().any(|&s| s);

        let mut paths: Vec<PathBuf> = Vec::new();
        for (i, &start) in self.onsets.iter().enumerate() {
            let end = self.onsets.get(i + 1).copied().unwrap_or(total).min(total);
            if start >= end {
                continue;
            }
            let include = if any_selected {
                self.selected.get(i).copied().unwrap_or(false)
            } else {
                !self.marked.get(i).copied().unwrap_or(false)
            };
            if !include {
                continue;
            }
            let path = dir.join(format!(
                "{safe_stem}_slice_{:0width$}.wav", i + 1, width = n_digits,
            ));
            let mut writer = hound::WavWriter::create(&path, spec)
                .map_err(|e| format!("WAV {}: {e}", path.display()))?;
            for &s in &audio.mono[start..end] {
                let s16 = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                writer.write_sample(s16)
                    .map_err(|e| format!("write_sample: {e}"))?;
            }
            writer.finalize()
                .map_err(|e| format!("finalize: {e}"))?;
            paths.push(path);
        }
        Ok(paths)
    }

    /// Génère un fichier MIDI dans `%TEMP%/a3000_slicer_midi/<stem>.mid` à
    /// partir des onsets courants. Retourne le path écrit, ou une erreur
    /// stringifiée à afficher dans le status.
    pub fn generate_midi_file(&mut self) -> Result<PathBuf, String> {
        let audio = self.audio.as_ref().ok_or("Aucun fichier audio chargé")?;
        if self.onsets.is_empty() {
            return Err("Aucun onset à exporter".into());
        }
        let stem = self.source_path.as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("slicer")
            .to_string();
        let track_name: String = stem.chars().take(32).collect();
        let onsets_i64: Vec<i64> = self.onsets.iter().map(|&o| o as i64).collect();
        let bytes = generate_midi(
            &onsets_i64,
            audio.mono.len() as u64,
            audio.sample_rate,
            self.n_beats.max(1),
            &track_name,
        ).map_err(|e| format!("MIDI : {e}"))?;

        let dir = std::env::temp_dir().join("a3000_slicer_midi");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create_dir: {e}"))?;
        let safe_stem: String = stem.chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let path = dir.join(format!("{safe_stem}.mid"));
        std::fs::write(&path, &bytes).map_err(|e| format!("write: {e}"))?;
        self.last_midi_path = Some(path.clone());
        Ok(path)
    }

    pub fn reset_view(&mut self) {
        if let Some(audio) = &self.audio {
            self.view = ViewWindow { start: 0, end: audio.mono.len() };
            self.peaks_cache = None;
        }
    }

    /// Zoom in/out par un facteur donné, centré sur le sample `anchor`.
    /// `factor` > 1 zoom in (réduit la fenêtre), < 1 zoom out.
    pub fn zoom_at(&mut self, factor: f32, anchor: usize) {
        let Some(audio) = &self.audio else { return };
        let total = audio.mono.len();
        if total == 0 { return; }
        let cur_len = self.view.len().max(1) as f32;
        let new_len = (cur_len / factor).clamp(64.0, total as f32) as usize;
        // Conserve `anchor` à la même position fractionnelle dans la fenêtre.
        let anchor_frac = if self.view.len() > 0 {
            (anchor.saturating_sub(self.view.start)) as f32 / self.view.len() as f32
        } else { 0.5 };
        let new_start_f = anchor as f32 - anchor_frac * new_len as f32;
        let new_start = new_start_f.max(0.0) as usize;
        let new_start = new_start.min(total.saturating_sub(new_len));
        let new_end = (new_start + new_len).min(total);
        self.view = ViewWindow { start: new_start, end: new_end };
        self.peaks_cache = None;
    }

    pub fn pan_by(&mut self, delta_samples: i64) {
        let Some(audio) = &self.audio else { return };
        let total = audio.mono.len();
        let len = self.view.len();
        let new_start = (self.view.start as i64 + delta_samples)
            .max(0)
            .min(total.saturating_sub(len) as i64) as usize;
        self.view = ViewWindow { start: new_start, end: (new_start + len).min(total) };
        self.peaks_cache = None;
    }

    pub fn is_playing(&self) -> bool { self.playback.is_some() }

    pub fn toggle_playback(&mut self) {
        if self.playback.is_some() {
            self.playback = None;
            self.previewing_slice = None;
            return;
        }
        let Some(audio) = self.audio.as_ref() else { return; };
        match Playback::start_loop(audio.mono.clone(), audio.sample_rate) {
            Ok(p) => {
                self.playback = Some(p);
                self.previewing_slice = None; // Loop full = pas de slice spécifique
            }
            Err(e) => self.error = Some(format!("Playback : {e}")),
        }
    }

    pub fn stop_playback(&mut self) {
        self.playback = None;
        self.previewing_slice = None;
    }

    /// Joue la slice contenant le sample `pos` (lecture one-shot, pas en boucle).
    /// Stop la lecture en cours si elle existe. La cellule correspondante
    /// est mise en surbrillance via `previewing_slice`.
    pub fn play_slice_at_sample(&mut self, pos: usize) {
        let Some(audio) = self.audio.as_ref() else { return; };
        let Some(idx) = self.slice_at_sample(pos) else { return; };
        let total = audio.mono.len();
        let start = self.onsets[idx];
        let end = self.onsets.get(idx + 1).copied().unwrap_or(total).min(total);
        if start >= end { return; }
        let slice: Vec<f32> = audio.mono[start..end].to_vec();
        let sr = audio.sample_rate;
        self.playback = None;
        match Playback::start_oneshot(slice, sr) {
            Ok(p) => {
                self.playback = Some(p);
                self.previewing_slice = Some(idx);
            }
            Err(e) => self.error = Some(format!("× preview : {e}")),
        }
    }

    /// Détecte qu'une preview oneshot est arrivée à sa fin et nettoie l'état.
    /// À appeler en début de frame (avant le rendu).
    fn poll_preview_end(&mut self) {
        if self.previewing_slice.is_some() {
            if let Some(pb) = &self.playback {
                if pb.position_fraction() >= 1.0 - 1e-3 {
                    self.playback = None;
                    self.previewing_slice = None;
                }
            }
        }
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

fn format_wav_err(e: &WaveError) -> String { format!("× WAV : {e}") }

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
    // Détecte la fin d'une preview oneshot (slice) avant le rendu.
    state.poll_preview_end();
    // Pendant la lecture, repaint régulier pour l'animation de la playhead /
    // pour détecter la fin d'une preview oneshot.
    if state.is_playing() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }

    // Navigation clavier : Space → onset suivant, Shift+Space ou Ctrl+Space
    // → onset précédent. On ne consomme pas la touche si une TextEdit a le focus.
    if !ui.ctx().wants_keyboard_input() && state.audio.is_some() {
        let (space, back) = ui.input(|i| (
            i.key_pressed(egui::Key::Space),
            i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command,
        ));
        if space {
            state.cycle_onset(if back { -1 } else { 1 });
        }
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

    // Ligne de message à hauteur **strictement fixe** (allocate_exact_size,
    // pas allocate_ui_with_layout qui rétrécit à la taille du contenu) :
    // la waveform ne bouge plus à l'apparition / disparition du message.
    let (msg_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 22.0),
        egui::Sense::hover(),
    );
    if let Some(msg) = &state.error {
        let mut child = ui.child_ui(
            msg_rect,
            egui::Layout::left_to_right(egui::Align::Center),
            None,
        );
        child.set_clip_rect(msg_rect);
        let is_error = msg.starts_with('×') || msg.starts_with('!');
        let color = if is_error { palette::ACCENT_RED } else { palette::ACCENT_GREEN };
        child.label(egui::RichText::new(msg).color(color).strong());
    }

    // Bloc canvas cadré strictement (allocate_exact_size + child_ui +
    // set_clip_rect) — cohérent avec upload/download.
    // Footer slicer = 2 lignes (info + spinbox Beats / boutons) + séparateur.
    const FOOTER_RESERVED_H: f32 = 110.0;
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
        if state.audio.is_some() {
            block_ui.add_space(8.0);
            show_canvas(&mut block_ui, state);
        } else if state.error.is_none() {
            block_ui.add_space(60.0);
            block_ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Drop un WAV ici").color(palette::FG_DIM).size(20.0),
                );
            });
        }
    }

    // Footer en bas, dans l'espace réservé.
    ui.separator();
    show_footer(ui, state);
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let has_audio = state.audio.is_some();
            let playing = state.is_playing();
            let label = if playing { "Stop" } else { "Loop" };
            let fill = if playing { palette::ACCENT_ORANGE } else { palette::ACCENT_GREEN };
            let btn = egui::Button::new(
                egui::RichText::new(label).color(egui::Color32::WHITE),
            ).fill(fill);
            if ui.add_enabled_ui(has_audio, |ui| {
                ui.add_sized([90.0, 32.0], btn)
            }).inner.clicked() {
                state.toggle_playback();
            }

            // Indicateur de zoom + bouton "Fit" (reset view).
            if let Some(audio) = &state.audio {
                let total = audio.mono.len().max(1);
                let zoom = total as f32 / state.view.len().max(1) as f32;
                let zoom_str = if (zoom - 1.0).abs() < 0.05 {
                    "1.0×".to_string()
                } else if zoom < 10.0 {
                    format!("{zoom:.1}×")
                } else {
                    format!("{zoom:.0}×")
                };
                let fit_disabled = (zoom - 1.0).abs() < 0.05;
                if ui.add_enabled_ui(!fit_disabled, |ui| {
                    ui.add_sized([60.0, 32.0], egui::Button::new("Fit"))
                }).inner.clicked() {
                    state.reset_view();
                }
                ui.label(egui::RichText::new(format!("Zoom : {zoom_str}"))
                    .color(palette::FG_DIM));
            }
        });
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

    // Zoom / pan via molette + Shift+molette + double-click reset.
    handle_zoom_pan(ui, state, &wave_rect, &wave_resp);
    handle_waveform_interaction(ui, state, &wave_rect, &wave_resp);

    // Recalcule le cache si largeur OU view ont changé.
    let width_px = wave_rect.width().round() as u32;
    let view = state.view;
    let need_recompute = state.peaks_cache.as_ref()
        .map(|c| c.width_px != width_px || c.view_start != view.start || c.view_end != view.end)
        .unwrap_or(true);
    if need_recompute {
        if let Some(audio) = &state.audio {
            state.peaks_cache = Some(compute_peaks(&audio.mono, view.start, view.end, width_px));
        }
    }

    paint_waveform(ui, state, wave_rect);
}

/// Wheel sur la waveform → zoom (anchor = curseur).
/// Shift+wheel → pan horizontal (egui reroute alors la molette sur l'axe X
/// donc on lit `delta.x` au lieu de `delta.y` ; on tolère aussi des deltas
/// arrivant sur l'axe « inattendu » selon les drivers souris).
/// Double-click → reset view (zoom out complet).
fn handle_zoom_pan(
    ui: &egui::Ui,
    state: &mut SlicerState,
    rect: &egui::Rect,
    resp: &egui::Response,
) {
    let (scroll_x, scroll_y, shift) = ui.input(|i| {
        let hovered = i.pointer.hover_pos().map(|p| rect.contains(p)).unwrap_or(false);
        if hovered {
            (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y, i.modifiers.shift)
        } else { (0.0, 0.0, false) }
    });

    if shift {
        // Pan : Shift+molette → axe horizontal d'egui ; on accepte aussi un
        // delta vertical au cas où certains drivers ne reroutent pas l'axe.
        let raw = if scroll_x.abs() > scroll_y.abs() { scroll_x } else { scroll_y };
        if raw.abs() > 0.5 {
            let len = state.view.len() as i64;
            let direction = if raw > 0.0 { -1 } else { 1 };
            state.pan_by(direction * (len / 20).max(1));
        }
    } else if scroll_y.abs() > 0.5 {
        // Zoom : facteur 1.1× par cran (subtil ; plusieurs crans pour zoomer fort).
        let factor = if scroll_y > 0.0 { 1.1 } else { 1.0 / 1.1 };
        let anchor = ui.ctx().pointer_hover_pos()
            .map(|p| state.view.x_to_sample(p.x, *rect))
            .unwrap_or(state.view.start + state.view.len() / 2);
        state.zoom_at(factor, anchor);
    }

    if resp.double_clicked() {
        state.reset_view();
    }
}

fn handle_cells_interaction(
    state: &mut SlicerState,
    rect: &egui::Rect,
    resp: &egui::Response,
) {
    if state.audio.is_none() { return; }
    let view = state.view;

    // Helper inline (function pointer pour éviter le double borrow de state).
    fn slice_at(state: &SlicerState, view: ViewWindow, rect: egui::Rect, p: egui::Pos2) -> Option<usize> {
        if !rect.x_range().contains(p.x) { return None; }
        let sample = view.x_to_sample(p.x, rect);
        state.slice_at_sample(sample)
    }

    use egui::PointerButton::{Primary, Secondary};

    // === DRAG START : capture la cellule + l'opération (selected/marked) ===
    let primary_drag_start = resp.drag_started_by(Primary);
    let secondary_drag_start = resp.drag_started_by(Secondary);
    if primary_drag_start || secondary_drag_start {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(idx) = slice_at(state, view, *rect, p) {
                state.drag_select_start = Some(idx);
                if primary_drag_start {
                    state.drag_select_kind = DragKind::Selected;
                    let cur = state.selected.get(idx).copied().unwrap_or(false);
                    state.drag_select_target = !cur;
                    state.set_selected(idx, state.drag_select_target);
                } else {
                    state.drag_select_kind = DragKind::Marked;
                    let cur = state.marked.get(idx).copied().unwrap_or(false);
                    state.drag_select_target = !cur;
                    state.set_marked(idx, state.drag_select_target);
                }
            }
        }
    }

    // === DRAG CONTINUATION : étend l'opération sur les cells visitées ===
    if resp.dragged_by(Primary) || resp.dragged_by(Secondary) {
        if let Some(start) = state.drag_select_start {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(cur_idx) = slice_at(state, view, *rect, p) {
                    let (lo, hi) = if cur_idx < start { (cur_idx, start) }
                                   else { (start, cur_idx) };
                    let target = state.drag_select_target;
                    let kind = state.drag_select_kind;
                    for i in lo..=hi {
                        match kind {
                            DragKind::Selected => state.set_selected(i, target),
                            DragKind::Marked => state.set_marked(i, target),
                        }
                    }
                }
            }
        }
    }

    // === DRAG END ===
    if resp.drag_stopped_by(Primary) || resp.drag_stopped_by(Secondary) {
        state.drag_select_start = None;
    }

    // === CLICK SIMPLE (sans drag) ===
    // Click gauche → toggle selected (vert, à garder pour export).
    // Click droit  → toggle marked  (rouge, à supprimer).
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(idx) = slice_at(state, view, *rect, p) {
                let cur = state.selected.get(idx).copied().unwrap_or(false);
                state.set_selected(idx, !cur);
            }
        }
    }
    if resp.clicked_by(Secondary) {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(idx) = slice_at(state, view, *rect, p) {
                let cur = state.marked.get(idx).copied().unwrap_or(false);
                state.set_marked(idx, !cur);
            }
        }
    }
}

fn paint_cells(ui: &egui::Ui, state: &SlicerState, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, palette::BG_PANEL_LIGHT);

    let total = match &state.audio { Some(a) => a.mono.len(), None => return };
    let view = state.view;
    if view.len() == 0 { return; }

    // Cellules visibles uniquement (clamp slice extent à la fenêtre).
    for (i, &start) in state.onsets.iter().enumerate() {
        let end = state.onsets.get(i + 1).copied().unwrap_or(total);
        // Skip slices entièrement hors view.
        if end <= view.start || start >= view.end { continue; }
        let s_clamped = start.max(view.start);
        let e_clamped = end.min(view.end);
        let x0 = rect.left()
            + (s_clamped - view.start) as f32 / view.len() as f32 * rect.width();
        let x1 = rect.left()
            + (e_clamped - view.start) as f32 / view.len() as f32 * rect.width();
        if x1 - x0 < 1.0 { continue; }
        let cell = egui::Rect::from_min_max(
            egui::pos2(x0 + 1.0, rect.top() + 2.0),
            egui::pos2(x1 - 1.0, rect.bottom() - 2.0),
        );
        let selected = state.selected.get(i).copied().unwrap_or(false);
        let marked = state.marked.get(i).copied().unwrap_or(false);
        let previewing = state.previewing_slice == Some(i);
        let fill = if selected {
            palette::ACCENT_GREEN  // à garder pour export
        } else if marked {
            palette::ACCENT_RED    // à supprimer
        } else {
            palette::BG_PANEL
        };
        painter.rect_filled(cell, 1.5, fill);
        // Surbrillance jaune épaisse autour de la slice en cours de preview.
        if previewing {
            painter.rect_stroke(
                cell,
                1.5,
                egui::Stroke::new(2.5, palette::ACCENT_YELLOW),
            );
        }

        if cell.width() >= 24.0 {
            let txt_color = if selected || marked {
                egui::Color32::WHITE
            } else {
                palette::FG_DIM
            };
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
    let view = state.view;
    if view.len() == 0 { return; }

    let x_to_sample = |x: f32| -> usize { view.x_to_sample(x, *rect) };
    let sample_to_x = |s: usize| -> Option<f32> { view.sample_to_x(s, *rect) };

    // Hover seulement pour le curseur. Le drag est piloté par
    // is_pointer_button_down_on (capture dès le press, pas drag_started — le
    // drag_started se déclenche après ~6 px de déplacement, donc le curseur
    // a déjà quitté la zone de hit ±ONSET_HIT_PX du marker).
    let hover_pos = ui.ctx().pointer_hover_pos();
    let mut hovered_onset: Option<usize> = None;
    if let Some(p) = hover_pos {
        if rect.contains(p) {
            for (i, &o) in state.onsets.iter().enumerate() {
                if let Some(ox) = sample_to_x(o) {
                    if (ox - p.x).abs() < ONSET_HIT_PX {
                        hovered_onset = Some(i);
                        break;
                    }
                }
            }
        }
    }
    // Curseur : ↔ sur un onset ou pendant son drag, ✋ (Grab/Grabbing) sinon
    // sur la zone pannable.
    if hovered_onset.is_some() || state.dragging_onset.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if state.dragging_pan.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if hover_pos.map(|p| rect.contains(p)).unwrap_or(false) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    // Première frame de press : on capture soit l'onset sous le curseur,
    // soit on initie un pan-drag si on est sur une zone vide.
    if resp.is_pointer_button_down_on()
        && state.dragging_onset.is_none()
        && state.dragging_pan.is_none()
    {
        if let Some(p) = resp.interact_pointer_pos() {
            let mut found_onset = None;
            for (i, &o) in state.onsets.iter().enumerate() {
                if let Some(ox) = sample_to_x(o) {
                    if (ox - p.x).abs() < ONSET_HIT_PX {
                        found_onset = Some(i);
                        break;
                    }
                }
            }
            if let Some(idx) = found_onset {
                state.dragging_onset = Some(idx);
            } else {
                // Pan : on enregistre la position du curseur ET la view au moment
                // du press. Ensuite on calcule new_start en fonction du delta.
                state.dragging_pan = Some((p.x, view.start));
            }
        }
    }

    // Mise à jour pendant que la souris bouge.
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
        } else if let Some((press_x, press_start)) = state.dragging_pan {
            if let Some(p) = resp.interact_pointer_pos() {
                let dx = p.x - press_x;
                // Drag mouse right → contenu sous curseur reste sous curseur →
                // view.start diminue → delta négatif.
                let samples_per_px = view.len() as f32 / rect.width().max(1.0);
                let delta_samples = -(dx * samples_per_px) as i64;
                let view_len = view.len();
                let new_start = (press_start as i64 + delta_samples)
                    .max(0)
                    .min(total.saturating_sub(view_len) as i64) as usize;
                if new_start != state.view.start {
                    state.view.start = new_start;
                    state.view.end = (new_start + view_len).min(total);
                    state.peaks_cache = None;
                }
            }
        }
    } else {
        // Bouton relâché — fin du drag.
        state.dragging_onset = None;
        state.dragging_pan = None;
    }

    // Click simple sur la waveform (sans drag) hors d'un onset → joue la slice
    // sous le curseur. resp.clicked() ne fire que si le mouvement est resté
    // sous le drag-threshold (~6 px), donc pas de conflit avec drag-onset/pan.
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let was_on_onset = state.onsets.iter().any(|&o| {
                sample_to_x(o).map(|x| (x - p.x).abs() < ONSET_HIT_PX).unwrap_or(false)
            });
            if !was_on_onset {
                let sample = x_to_sample(p.x);
                state.play_slice_at_sample(sample);
            }
        }
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

    // Onset markers verticaux (uniquement ceux dans la fenêtre visible).
    let view = state.view;
    if view.len() > 0 {
        for (i, &s) in state.onsets.iter().enumerate() {
            if let Some(x) = view.sample_to_x(s, rect) {
                let dragging = state.dragging_onset == Some(i);
                let current = state.current_onset == Some(i);
                let (width, color) = if dragging {
                    (2.5, palette::ACCENT_ORANGE)
                } else if current {
                    (2.5, egui::Color32::WHITE)
                } else {
                    (1.5, palette::ACCENT_YELLOW)
                };
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(width, color),
                );
            }
        }
    }

    // Playhead (Loop full audio uniquement). En preview de slice on ne
    // dessine PAS la playhead — la position serait dans le buffer de la
    // slice extraite, pas du buffer global. À la place, la cellule est
    // highlight (cf. paint_cells).
    if state.previewing_slice.is_none() {
        if let Some(pb) = &state.playback {
            if let Some(audio) = &state.audio {
                let total = audio.mono.len();
                let pos_sample = (pb.position_fraction() * total as f32) as usize;
                if let Some(x) = view.sample_to_x(pos_sample, rect) {
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(2.0, palette::ACCENT_ORANGE),
                    );
                }
            }
        }
    }
}

fn show_footer(ui: &mut egui::Ui, state: &mut SlicerState) {
    const BTN_H: f32 = 32.0;
    let total = state.slice_count();
    let marked = state.marked_count();
    let selected = state.selected_count();
    let has_audio = state.audio.is_some();
    let has_slices = !state.onsets.is_empty();
    // Nombre de slices qui seront exportées (sémantique Python) :
    //   - si au moins une selected (vert) → seulement selected
    //   - sinon → tout sauf marked (rouge)
    let export_count = if selected > 0 { selected } else { total.saturating_sub(marked) };
    let midi_enabled = has_audio && has_slices;

    // Ligne 1 : compteurs sélection / suppression / total.
    ui.horizontal(|ui| {
        if total > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "{} onsets — sélectionnées (vert) : {selected}/{total} — marquées suppression (rouge) : {marked}/{total}",
                    state.onsets.len(),
                )).color(palette::FG_DIM),
            );
        }
    });

    ui.add_space(2.0);

    // Ligne 2 : tous les boutons d'action.
    ui.horizontal(|ui| {
        // Slice management (gauche → droite : Reset, Delete, None, Select all).
        if ui.add_enabled_ui(has_audio, |ui| {
            ui.add_sized([100.0, BTN_H], egui::Button::new("Reset"))
        }).inner.clicked() {
            state.reset();
        }
        let delete_btn = egui::Button::new(
            egui::RichText::new(format!("Delete {marked}")).color(egui::Color32::WHITE),
        ).fill(palette::ACCENT_RED);
        if ui.add_enabled_ui(has_audio && marked > 0 && marked < total, |ui| {
            ui.add_sized([110.0, BTN_H], delete_btn)
        }).inner.clicked() {
            state.delete_marked();
        }
        // None — décoche tout (selected ET marked).
        if ui.add_enabled_ui(has_audio && (selected > 0 || marked > 0), |ui| {
            ui.add_sized([80.0, BTN_H], egui::Button::new("None"))
        }).inner.clicked() {
            for s in &mut state.selected { *s = false; }
            for m in &mut state.marked { *m = false; }
        }
        // Select all — sélectionne toutes les slices (vert) en effaçant les
        // suppressions au passage (mutuellement exclusif).
        if ui.add_enabled_ui(has_audio && selected < total, |ui| {
            ui.add_sized([90.0, BTN_H], egui::Button::new("Select all"))
        }).inner.clicked() {
            for s in &mut state.selected { *s = true; }
            for m in &mut state.marked { *m = false; }
        }

        // Action primaire + MIDI export à droite.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Send to Upload — primaire (verte) flush right. Le compteur
            // affiche le nombre exact de slices qui seront exportées selon
            // la sémantique filter/trash.
            let send_btn = egui::Button::new(
                egui::RichText::new(format!("Send to Upload ({export_count})"))
                    .color(egui::Color32::WHITE),
            ).fill(palette::ACCENT_GREEN);
            if ui.add_enabled_ui(has_audio && has_slices && export_count > 0, |ui| {
                ui.add_sized([170.0, BTN_H], send_btn)
            }).inner.clicked() {
                state.request_send_to_upload = true;
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Drag MIDI (custom Sense::drag — régénère TOUJOURS le fichier
            // au début du drag pour refléter la valeur actuelle de Beats /
            // les déplacements d'onsets).
            #[cfg(windows)]
            {
                let drag_resp = drag_midi_button(ui, midi_enabled, BTN_H);
                if drag_resp.drag_started() {
                    match state.generate_midi_file() {
                        Ok(p) => match crate::ole_drag::drag_file(&p) {
                            Ok(eff) if eff.0 != 0 => {
                                state.error = Some(format!(
                                    "MIDI déposé dans le DAW ({})", p.display(),
                                ));
                            }
                            Ok(_) => state.error = Some("Drag annulé".into()),
                            Err(e) => state.error = Some(format!("× drag : {e}")),
                        },
                        Err(e) => state.error = Some(format!("× {e}")),
                    }
                }
            }
            ui.add_space(4.0);

            // Save MIDI (couleur cuivre/ambre BUTTON_MIDI plus douce que
            // l'ancien ACCENT_YELLOW agressif).
            let midi_btn = egui::Button::new(
                egui::RichText::new("Save MIDI").color(egui::Color32::WHITE),
            ).fill(palette::BUTTON_MIDI);
            if ui.add_enabled_ui(midi_enabled, |ui| {
                ui.add_sized([110.0, BTN_H], midi_btn)
            }).inner.clicked() {
                match state.generate_midi_file() {
                    Ok(p) => state.error = Some(format!("MIDI sauvé : {}", p.display())),
                    Err(e) => state.error = Some(format!("× {e}")),
                }
            }
            ui.add_space(4.0);

            // Spinbox Beats — placé juste à gauche des boutons MIDI pour le
            // contexte (il sert UNIQUEMENT à la génération MIDI).
            ui.add_enabled_ui(has_audio, |ui| {
                ui.add(egui::DragValue::new(&mut state.n_beats).range(1..=64));
            });
            ui.label(egui::RichText::new("Beats :").color(palette::FG_DIM));
        });
    });
}

/// Bouton custom "Drag MIDI" peint à la main pour pouvoir capter `Sense::drag()`
/// (le widget `Button` standard d'egui n'expose qu'un click sense).
#[cfg(windows)]
fn drag_midi_button(ui: &mut egui::Ui, enabled: bool, h: f32) -> egui::Response {
    let size = egui::vec2(110.0, h);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    if !enabled {
        resp = resp.on_disabled_hover_text("Charger un WAV avant");
    }
    let visuals = ui.style().interact(&resp);
    let fill = if !enabled {
        palette::BG_PANEL_LIGHT
    } else if resp.is_pointer_button_down_on() {
        palette::ACCENT_ORANGE
    } else if resp.hovered() {
        palette::BG_HOVER
    } else {
        palette::BUTTON_MIDI
    };
    ui.painter().rect(rect, 4.0, fill, visuals.bg_stroke);
    let text_color = if enabled { egui::Color32::WHITE } else { palette::FG_DIM };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Drag MIDI",
        egui::FontId::proportional(13.0),
        text_color,
    );
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    resp
}

/// Compute peaks (min, max amplitude) par pixel-bin sur la fenêtre
/// `[view_start, view_end)`. Style 8-bit (pas de smoothing).
fn compute_peaks(samples: &[f32], view_start: usize, view_end: usize, width_px: u32) -> PeaksCache {
    let width = width_px.max(1) as usize;
    let view_end = view_end.min(samples.len());
    let view_start = view_start.min(view_end);
    let n = view_end.saturating_sub(view_start);
    let mut bins: Vec<(f32, f32)> = Vec::with_capacity(width);
    if n == 0 || width == 0 {
        return PeaksCache { width_px, view_start, view_end, bins };
    }
    let slice = &samples[view_start..view_end];
    for i in 0..width {
        let lo = (i as u64 * n as u64 / width as u64) as usize;
        let hi = ((i as u64 + 1) * n as u64 / width as u64) as usize;
        let hi = hi.min(n).max(lo + 1);
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for &s in &slice[lo..hi] {
            if s < mn { mn = s; }
            if s > mx { mx = s; }
        }
        bins.push((mn, mx));
    }
    PeaksCache { width_px, view_start, view_end, bins }
}
