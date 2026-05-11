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

use a3000_core::wav::{load_wave, peek_wave_metadata};

use crate::audio::{pcm16_le_to_mono_f32, Playback};
use crate::config::Config;
use crate::theme::palette;

#[derive(Default)]
pub struct UploadState {
    pub items: Vec<UploadItem>,
    /// Demande de démarrage de batch (set par le bouton Upload, lue par l'App
    /// qui kick off le transfert et reset le flag).
    pub request_upload: bool,
    /// Demande d'arrêt entre 2 items : l'item courant termine normalement, mais
    /// la batch n'enchaîne pas sur le suivant. Set par le bouton Stop, reset
    /// quand le batch se termine. Pas de cancel mid-transfert (cf. analyse :
    /// risque de slot stuck sur le sampler + UAC re-prompt à chaque cancel).
    pub stop_requested: bool,
    /// Index de l'item en cours de transfert (Some pendant Running).
    pub current_idx: Option<usize>,
    /// True quand on attend Event::FreeSlot après avoir envoyé FindFreeSlot.
    pub pending_find_slot: bool,
    /// Prochain slot à attribuer (auto-incrémenté entre items).
    pub next_slot: u32,
    /// Audio playback en cours (preview d'un item de la queue, oneshot).
    /// `cpal::Stream` est `!Send` sur Windows → vit sur le thread GUI.
    playback: Option<Playback>,
    /// Index de l'item en cours de preview (None si rien en lecture).
    playing_idx: Option<usize>,
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

    /// Toggle le preview audio d'un item : si déjà en lecture sur cet item,
    /// stop ; sinon stop l'éventuel preview en cours et joue celui-ci.
    pub fn toggle_play(&mut self, idx: usize) {
        if self.playing_idx == Some(idx) {
            self.playback = None;
            self.playing_idx = None;
            return;
        }
        let Some(item) = self.items.get(idx) else { return; };
        if item.state == UploadItemState::Error || item.error_msg.is_some() {
            return;
        }
        // Charge le WAV via a3000_core::wav::load_wave (PCM 16-bit LE) puis
        // convertit en mono f32 via le helper partagé.
        match load_wave(&item.path) {
            Ok(payload) => {
                let mono = pcm16_le_to_mono_f32(&payload);
                let sr = payload.sample_rate;
                self.playback = None;
                match Playback::start_oneshot(mono, sr) {
                    Ok(p) => {
                        self.playback = Some(p);
                        self.playing_idx = Some(idx);
                    }
                    Err(e) => eprintln!("upload play: {e}"),
                }
            }
            Err(e) => eprintln!("upload play load_wave: {e}"),
        }
    }

    /// Détecte la fin du oneshot et nettoie. À appeler avant le rendu.
    pub fn poll_play_end(&mut self) {
        if let Some(pb) = &self.playback {
            if pb.position_fraction() >= 1.0 - 1e-3 {
                self.playback = None;
                self.playing_idx = None;
            }
        }
    }

    pub fn is_playing(&self) -> bool { self.playing_idx.is_some() }
}

/// Drain les fichiers droppés sur la fenêtre depuis la frame courante et
/// retourne (paths_wav, errors_archives). Les WAV directs sont passés tels
/// quels ; les archives .zip/.tar.gz/.tgz/.tar sont extraites dans %TEMP%
/// et leurs WAV ajoutés à la liste. Les autres extensions sont ignorées.
fn drain_dropped_wavs(ctx: &egui::Context) -> (Vec<PathBuf>, Vec<String>) {
    let raw_files = ctx.input(|i| i.raw.dropped_files.clone());
    let mut wavs = Vec::new();
    let mut errors = Vec::new();
    for f in raw_files {
        let Some(p) = f.path else { continue; };
        // .wav direct ?
        if matches!(p.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()).as_deref(), Some("wav")) {
            wavs.push(p);
            continue;
        }
        // Archive ?
        if let Some(result) = crate::archive::try_extract_archive(&p) {
            match result {
                Ok(extracted) => {
                    if extracted.is_empty() {
                        errors.push(format!(
                            "{} : aucun .wav dans l'archive",
                            p.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                        ));
                    } else {
                        wavs.extend(extracted);
                    }
                }
                Err(e) => errors.push(format!(
                    "{} : {e}",
                    p.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                )),
            }
        }
        // Autre extension : ignoré silencieusement.
    }
    (wavs, errors)
}

/// Réservation pour le footer (séparateur + ligne boutons + spacing).
const FOOTER_RESERVED_H: f32 = 70.0;

pub fn show(ui: &mut egui::Ui, state: &mut UploadState, config: &Config) {
    // Drop : on ajoute les fichiers WAV droppés (incluant les .wav extraits
    // d'éventuels .zip / .tar.gz droppés).
    let (dropped, archive_errors) = drain_dropped_wavs(ui.ctx());
    for p in dropped {
        state.add_path(p);
    }

    // Détecte la fin du oneshot de preview audio + repaint pendant la lecture
    // pour mettre à jour le bouton P/S et la surbrillance.
    state.poll_play_end();
    if state.is_playing() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    for e in archive_errors {
        // Archive en erreur → on l'affiche comme un item Error pour info.
        let placeholder = std::path::PathBuf::from(format!("[archive] {e}"));
        let mut item = UploadItem::from_path(placeholder);
        item.error_msg = Some(e);
        state.items.push(item);
    }

    ui.add_space(6.0);
    ui.heading("Upload");
    ui.add_space(4.0);
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

    // Bloc table cadré strictement via allocate_exact_size + child_ui +
    // set_clip_rect : pareil que pour les cells de table, c'est la SEULE
    // façon en egui d'obtenir un clipping VISUEL qui empêche les rows de
    // déborder par-dessus le header/footer (ui.allocate_ui borne le layout
    // cursor mais pas le painter).
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
        if state.items.is_empty() {
            empty_drop_zone(&mut block_ui);
        } else {
            show_table(&mut block_ui, state);
        }
    }

    // Footer en bas, dans l'espace réservé.
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
// Hauteur de ligne ≥ interact_size (24) + 4 px de respiration verticale.
const ROW_H: f32 = 28.0;
// 36 px : 6 px add_space gauche (cf. cell — empêche le clip du focus stroke
// d'egui qui dépasse de ~2 px le bord visible du box) + 16 px box + marge.
const COL_CHECK: f32 = 36.0;
/// Décalage à gauche de la checkbox dans son cell : sans ça le focus
/// stroke / hover ring d'egui (qui dépasse de ~2 px le box) est tronqué par
/// le clip_rect strict du cell.
const CHECKBOX_LEFT_PAD: f32 = 6.0;
const COL_FILE: f32 = 200.0;
const COL_NAME: f32 = 140.0;
const COL_FORMAT: f32 = 150.0;
const COL_SIZE: f32 = 80.0;
const COL_DUR: f32 = 70.0;
const COL_SLOT: f32 = 60.0;
const COL_STATE: f32 = 80.0;
const COL_PROGRESS: f32 = 140.0;
const COL_ACTION: f32 = 40.0;

/// Cellule de largeur **strictement fixe** (W × ROW_H) : `allocate_exact_size`
/// réserve un rect immuable au parent, on construit un child_ui bordé par ce
/// rect avec clipping + wrap=Truncate. Les labels trop longs sont tronqués
/// au lieu de pousser le layout.
///
/// IMPORTANT : `set_clip_rect` REMPLACE le clip parent au lieu de l'intersecter.
/// On intersecte manuellement avec `ui.clip_rect()` pour conserver le clip
/// hérité (ex : viewport d'une ScrollArea parente) — sinon les rows hors-viewport
/// dessinent par-dessus header/footer.
fn cell<R>(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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

fn show_table(ui: &mut egui::Ui, state: &mut UploadState) {
    // max_height explicite : sans ça la ScrollArea déborde sous le footer
    // quand le nombre d'items dépasse la hauteur disponible.
    let max_h = ui.available_height();
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .max_height(max_h)
        .show(ui, |ui| {
        // Header row — mêmes largeurs que les rows pour alignement strict.
        ui.horizontal(|ui| {
            // Check-all
            cell(ui, COL_CHECK, |ui| {
                ui.add_space(CHECKBOX_LEFT_PAD);
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

        // Capture playing_idx avant l'iter mutable (split borrow).
        let playing_idx = state.playing_idx;
        let mut to_remove: Option<usize> = None;
        let mut to_play: Option<usize> = None;
        for (idx, item) in state.items.iter_mut().enumerate() {
            let row_color = if playing_idx == Some(idx) {
                palette::ACCENT_YELLOW  // surbrillance preview
            } else {
                match item.state {
                    UploadItemState::Done => palette::ACCENT_GREEN,
                    UploadItemState::Running => palette::ACCENT_YELLOW,
                    UploadItemState::Error => palette::ACCENT_RED,
                    UploadItemState::Pending => palette::FG_TEXT,
                }
            };
            ui.horizontal(|ui| {
                cell(ui, COL_CHECK, |ui| {
                    ui.add_space(CHECKBOX_LEFT_PAD);
                    ui.add_enabled(
                        item.state != UploadItemState::Error,
                        egui::Checkbox::without_text(&mut item.checked),
                    );
                });

                let file_name = item.path.file_name()
                    .and_then(|s| s.to_str()).unwrap_or("?");
                // Le file name est cliquable → toggle preview audio. Tooltip
                // hover indique la sémantique + curseur PointingHand pour
                // signaler que c'est interactif (convention web).
                cell(ui, COL_FILE, |ui| {
                    let is_playing_this = playing_idx == Some(idx);
                    let tip = if is_playing_this { "Stop preview" } else { "Play preview" };
                    let resp = ui.add(
                        egui::Label::new(egui::RichText::new(file_name).color(row_color))
                            .sense(egui::Sense::click()),
                    ).on_hover_text(tip);
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        to_play = Some(idx);
                    }
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
                    paint_progress_bar(ui, item.progress, COL_PROGRESS - 10.0, 14.0);
                });
                cell(ui, COL_ACTION, |ui| {
                    if ui.small_button("×").on_hover_text("Remove").clicked() {
                        to_remove = Some(idx);
                    }
                });
            });

            if let Some(err) = &item.error_msg {
                ui.horizontal(|ui| {
                    ui.add_space(COL_CHECK);
                    ui.label(
                        egui::RichText::new(format!("! {err}"))
                            .color(palette::ACCENT_RED).small(),
                    );
                });
            }
        }
        if let Some(i) = to_remove {
            state.items.remove(i);
        }
        if let Some(i) = to_play {
            state.toggle_play(i);
        }
    });
}

fn header_label(ui: &mut egui::Ui, text: &str, width: f32) {
    cell(ui, width, |ui| {
        ui.label(egui::RichText::new(text).color(palette::FG_DIM).strong());
    });
}

/// Progress bar peinte manuellement : rect strict (pas d'auto-resize comme
/// `egui::ProgressBar` qui étend la largeur pour faire rentrer le texte
/// `X%` et déborde la cellule). Texte centré horizontalement dans le rect
/// quel que soit le progrès.
pub(super) fn paint_progress_bar(ui: &mut egui::Ui, progress: f32, w: f32, h: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let painter = ui.painter();
    // Fond de la barre
    painter.rect_filled(rect, 3.0, palette::BG_DEEP);
    // Remplissage progressif
    let progress = progress.clamp(0.0, 1.0);
    if progress > 0.0 {
        let fill_w = (rect.width() * progress).max(2.0);
        let fill_rect = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(fill_w, rect.height()),
        );
        painter.rect_filled(fill_rect, 3.0, palette::ACCENT_GREEN);
    }
    // Texte X% centré dans le rect global, jamais hors bornes (afficher
    // depuis 1% pour éviter "0%" sur barre vide).
    if progress > 0.005 {
        let pct = (progress * 100.0).round() as i32;
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{pct}%"),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
    }
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

        // right_to_left : le 1er pushé est le PLUS À DROITE → on met l'action
        // primaire (Upload) en premier pour qu'elle soit flush right.
        // Tous les boutons utilisent `add_sized([_, BTN_H])` pour passer par
        // le même chemin layout (`centered_and_justified`) et garantir un Y
        // cohérent. Pas de `RichText::strong()` ni de glyph ▶ (galley
        // asymétrique) sur Upload : la couleur verte suffit à marquer
        // l'action primaire.
        const BTN_H: f32 = 32.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = state.current_idx.is_some() || state.pending_find_slot;
            // Bouton Upload (devient Stop pendant un batch). Quand busy,
            // click → set stop_requested : la batch s'arrête après l'item
            // en cours (cf. design : pas de cancel mid-transfert).
            let (label, fill, enabled) = if busy {
                let txt = if state.stop_requested {
                    "Stopping…".to_string()
                } else {
                    "Stop".to_string()
                };
                (txt, palette::ACCENT_RED, !state.stop_requested)
            } else {
                (format!("Upload {}", checked), palette::ACCENT_GREEN, checked > 0)
            };
            let upload_btn = egui::Button::new(
                egui::RichText::new(label).color(egui::Color32::WHITE),
            ).fill(fill);
            let resp = ui.add_enabled_ui(enabled, |ui| {
                ui.add_sized([120.0, BTN_H], upload_btn)
            }).inner;
            if resp.clicked() {
                if busy {
                    state.stop_requested = true;
                } else {
                    state.request_upload = true;
                }
            }
            ui.add_space(8.0);
            if ui.add_sized([0.0, BTN_H], egui::Button::new("Reset state")).clicked() {
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
            if ui.add_sized([0.0, BTN_H], egui::Button::new("Clear")).clicked() {
                state.items.clear();
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
