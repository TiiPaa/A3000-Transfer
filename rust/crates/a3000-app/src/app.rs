//! Shell `eframe::App` principal + état worker.
//!
//! Layout :
//!  - TopBottomPanel "tabs" : sélecteur Upload / Download / Slicer + bouton Settings + état worker
//!  - CentralPanel : contenu de la tab active
//!  - TopBottomPanel "status" en bas : statut et messages d'erreur

use std::sync::mpsc;
use std::time::Duration;

use eframe::egui;

#[cfg(windows)]
use crate::client::{WorkerError, WorkerHandle};
use crate::config::Config;
use crate::ipc::{Cmd, Event};
use crate::tabs::{self, upload::UploadItemState, Tab};
use crate::theme::palette;

#[cfg(windows)]
pub enum WorkerState {
    /// Pas encore tenté de se connecter.
    Idle,
    /// UAC popup affiché + accept en cours dans un thread.
    Connecting(mpsc::Receiver<Result<WorkerHandle, WorkerError>>),
    /// Connecté, prêt à recevoir des Cmd.
    Connected(WorkerHandle),
    /// Échec dernière tentative (l'utilisateur peut retry).
    Error(String),
}

#[cfg(not(windows))]
pub enum WorkerState {
    Unsupported,
}

#[cfg(windows)]
impl WorkerState {
    pub fn label(&self) -> String {
        match self {
            WorkerState::Idle => "Worker : non démarré".into(),
            WorkerState::Connecting(_) => "Worker : UAC en cours…".into(),
            // Pas de glyph Unicode (✓ rend en ☐ avec la font egui par défaut).
            WorkerState::Connected(_) => "Worker : connecté".into(),
            WorkerState::Error(msg) => format!("Worker : ERREUR — {msg}"),
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            WorkerState::Idle => palette::FG_DIM,
            WorkerState::Connecting(_) => palette::ACCENT_YELLOW,
            WorkerState::Connected(_) => palette::ACCENT_GREEN,
            WorkerState::Error(_) => palette::ACCENT_RED,
        }
    }
}

#[cfg(not(windows))]
impl WorkerState {
    pub fn label(&self) -> String { "Worker : non supporté hors Windows".into() }
    pub fn color(&self) -> egui::Color32 { palette::FG_DIM }
}

pub struct A3000App {
    pub active_tab: Tab,
    pub config: Config,
    pub upload: tabs::upload::UploadState,
    pub download: tabs::download::DownloadState,
    pub slicer: tabs::slicer::SlicerState,
    pub status: String,
    pub worker: WorkerState,
    pub settings_open: bool,
}

impl A3000App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        Self {
            active_tab: Tab::default(),
            config: Config::load(),
            upload: Default::default(),
            download: Default::default(),
            slicer: Default::default(),
            status: "Prêt".into(),
            #[cfg(windows)]
            worker: WorkerState::Idle,
            #[cfg(not(windows))]
            worker: WorkerState::Unsupported,
            settings_open: false,
        }
    }

    /// Lance la connexion au worker élevé (non-bloquant : popup UAC + accept
    /// dans un thread, l'état Connecting passe à Connected ou Error sur le
    /// prochain `poll_worker`).
    #[cfg(windows)]
    pub fn start_worker(&mut self, ctx: &egui::Context) {
        if !matches!(self.worker, WorkerState::Idle | WorkerState::Error(_)) {
            return;
        }
        let (tx, rx) = mpsc::channel::<Result<WorkerHandle, WorkerError>>();
        let ctx_clone = ctx.clone();
        let ctx_for_reader = ctx.clone();
        std::thread::Builder::new()
            .name("worker-start".into())
            .spawn(move || {
                let result = WorkerHandle::start(Duration::from_secs(60), ctx_for_reader);
                let _ = tx.send(result);
                ctx_clone.request_repaint();
            })
            .ok();
        self.worker = WorkerState::Connecting(rx);
        self.status = "Lancement du worker élevé (UAC)…".into();
    }

    /// Vérifie l'état du worker (à appeler à chaque update).
    #[cfg(windows)]
    fn poll_worker(&mut self, ctx: &egui::Context) {
        // Transition Connecting → Connected/Error
        let new_state = match &self.worker {
            WorkerState::Connecting(rx) => match rx.try_recv() {
                Ok(Ok(handle)) => Some(WorkerState::Connected(handle)),
                Ok(Err(e)) => Some(WorkerState::Error(e.to_string())),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(WorkerState::Error("worker start channel closed".into()))
                }
            },
            _ => None,
        };
        if let Some(s) = new_state {
            match &s {
                WorkerState::Connected(_) => self.status = "Worker élevé connecté.".into(),
                WorkerState::Error(e) => self.status = format!("Worker : {e}"),
                _ => {}
            }
            self.worker = s;
        }

        // Drain events si connecté (collect d'abord, dispatch ensuite pour
        // éviter le double-borrow de self).
        let events: Vec<Event> = if let WorkerState::Connected(handle) = &self.worker {
            handle.events.try_iter().collect()
        } else {
            Vec::new()
        };
        if !events.is_empty() {
            for ev in events {
                self.on_event(ev);
            }
            ctx.request_repaint();
        }
    }

    #[cfg(not(windows))]
    fn poll_worker(&mut self, _ctx: &egui::Context) {}

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Ready => self.status = "Worker ready".into(),
            Event::FreeSlot { slot } => {
                self.status = format!("1er slot libre = #{slot}");
                if self.upload.pending_find_slot {
                    self.upload.next_slot = slot;
                    self.upload.pending_find_slot = false;
                    self.try_start_next_upload();
                }
            }
            Event::Progress { sent, total } => {
                if let Some(idx) = self.upload.current_idx {
                    if let Some(it) = self.upload.items.get_mut(idx) {
                        it.sent_bytes = sent;
                        it.total_bytes = total;
                        it.progress = if total > 0 { (sent as f32) / (total as f32) } else { 0.0 };
                    }
                }
                if self.download.current_idx.is_some() {
                    self.download.download_progress = if total > 0 {
                        (sent as f32) / (total as f32)
                    } else { 0.0 };
                }
                self.status = format!("Transfert : {sent}/{total} bytes");
            }
            Event::Done { sample_number, bytes_sent, packet_count } => {
                self.status = format!(
                    "Slot #{sample_number} OK : {bytes_sent} octets en {packet_count} packets",
                );
                if let Some(idx) = self.upload.current_idx.take() {
                    if let Some(it) = self.upload.items.get_mut(idx) {
                        it.state = UploadItemState::Done;
                        it.progress = 1.0;
                    }
                    self.upload.next_slot = self.upload.next_slot.saturating_add(1);
                }
                self.try_start_next_upload();
            }
            Event::Error { msg, .. } => {
                self.status = format!("× {msg}");
                if let Some(idx) = self.upload.current_idx.take() {
                    if let Some(it) = self.upload.items.get_mut(idx) {
                        it.state = UploadItemState::Error;
                        it.error_msg = Some(msg);
                    }
                    // Continue avec le suivant : on saute juste cet item.
                    self.try_start_next_upload();
                }
            }
            Event::ScanProgress { scanned, found } => {
                self.download.scan_progress = Some((scanned, found));
                self.status = format!("Scan : {scanned} slots, {found} samples trouvés");
            }
            Event::SamplesList { samples } => {
                self.status = format!("Liste reçue : {} samples", samples.len());
                self.download.samples = samples;
                self.download.scan_progress = None;
            }
            Event::Received { sample_number, output_path, name, .. } => {
                self.status = format!(
                    "Sample #{sample_number} '{name}' téléchargé : {output_path}",
                );
                self.download.checked.remove(&sample_number);
                self.download.current_idx = None;
                self.download.download_progress = 0.0;
                self.try_start_next_download();
            }
        }
    }

    /// Pilotage du batch upload : 1er coup envoie FindFreeSlot ou démarre direct,
    /// les coups suivants démarrent l'item Pending suivant.
    #[cfg(windows)]
    fn try_start_next_upload(&mut self) {
        let WorkerState::Connected(handle) = &self.worker else { return; };
        let sender = handle.sender();
        let Some(idx) = self.upload.next_pending() else {
            // Plus rien à faire, batch terminé.
            return;
        };
        let slot = self.upload.next_slot;
        let item = &mut self.upload.items[idx];
        item.slot = Some(slot);
        item.state = UploadItemState::Running;
        item.progress = 0.0;
        item.sent_bytes = 0;
        item.total_bytes = 0;
        self.upload.current_idx = Some(idx);

        let cmd = Cmd::Transfer {
            ha: self.config.ha,
            bus: self.config.bus,
            target: self.config.target,
            lun: self.config.lun,
            sample_number: slot,
            name: item.sample_name.clone(),
            wave_path: item.path.to_string_lossy().to_string(),
        };
        if let Err(e) = sender.send_cmd(&cmd) {
            self.status = format!("× send Transfer: {e}");
            self.upload.current_idx = None;
            if let Some(it) = self.upload.items.get_mut(idx) {
                it.state = UploadItemState::Error;
                it.error_msg = Some(e.to_string());
            }
        }
    }

    #[cfg(not(windows))]
    fn try_start_next_upload(&mut self) {}

    /// Si l'utilisateur a cliqué "Upload" dans le tab et qu'on est connecté,
    /// kick off la batch. Sinon laisse le flag pour la prochaine frame.
    #[cfg(windows)]
    fn poll_upload_request(&mut self) {
        if !self.upload.request_upload {
            return;
        }
        // Doit être connecté.
        if !matches!(self.worker, WorkerState::Connected(_)) {
            self.status = "Worker non connecté — clique Connect…".into();
            self.upload.request_upload = false;
            return;
        }
        if self.upload.current_idx.is_some() || self.upload.pending_find_slot {
            self.upload.request_upload = false;
            return; // déjà en cours
        }
        self.upload.request_upload = false;

        // Auto vs manual start slot
        if self.config.auto_start_slot {
            // Demande au worker le 1er slot libre puis on enchaînera dans on_event.
            let WorkerState::Connected(handle) = &self.worker else { return; };
            let sender = handle.sender();
            let cmd = Cmd::FindFreeSlot {
                ha: self.config.ha,
                bus: self.config.bus,
                target: self.config.target,
                lun: self.config.lun,
                start: 7,
            };
            self.upload.pending_find_slot = true;
            if let Err(e) = sender.send_cmd(&cmd) {
                self.status = format!("× send FindFreeSlot: {e}");
                self.upload.pending_find_slot = false;
            } else {
                self.status = "Recherche du 1er slot libre…".into();
            }
        } else {
            self.upload.next_slot = self.config.manual_start_slot;
            self.try_start_next_upload();
        }
    }

    #[cfg(not(windows))]
    fn poll_upload_request(&mut self) {}

    /// Pilotage Download : Scan + Download batch (séquentiel).
    #[cfg(windows)]
    fn poll_download_request(&mut self) {
        if self.download.request_scan {
            self.download.request_scan = false;
            if let WorkerState::Connected(handle) = &self.worker {
                let cmd = Cmd::ListSamples {
                    ha: self.config.ha,
                    bus: self.config.bus,
                    target: self.config.target,
                    lun: self.config.lun,
                    start: 0,
                    limit: 256,
                };
                self.download.scan_progress = Some((0, 0));
                self.download.samples.clear();
                self.download.checked.clear();
                if let Err(e) = handle.sender().send_cmd(&cmd) {
                    self.status = format!("× Scan : {e}");
                    self.download.scan_progress = None;
                } else {
                    self.status = "Scan en cours…".into();
                }
            } else {
                self.status = "Worker non connecté — clique Connect…".into();
            }
        }

        if self.download.request_download {
            self.download.request_download = false;
            if !matches!(self.worker, WorkerState::Connected(_)) {
                self.status = "Worker non connecté — clique Connect…".into();
                return;
            }
            self.try_start_next_download();
        }
    }

    #[cfg(not(windows))]
    fn poll_download_request(&mut self) {}

    /// Lance le download du prochain sample coché. Return None si batch vide.
    #[cfg(windows)]
    fn try_start_next_download(&mut self) {
        let WorkerState::Connected(handle) = &self.worker else { return; };
        let sender = handle.sender();
        // Trouve le 1er sample coché qui n'a pas été tagué Done dans cette
        // session (on n'a pas encore d'état per-sample côté Download — pour
        // l'instant on enlève simplement le slot du set checked à mesure).
        let next_slot = match self.download.checked.iter().next().copied() {
            Some(s) => s,
            None => {
                self.status = "Batch download terminé.".into();
                self.download.current_idx = None;
                self.download.download_progress = 0.0;
                return;
            }
        };
        let idx = match self.download.samples.iter().position(|s| s.slot == next_slot) {
            Some(i) => i,
            None => {
                // Sample disparu de la liste : skip.
                self.download.checked.remove(&next_slot);
                self.try_start_next_download();
                return;
            }
        };
        let sample = &self.download.samples[idx];
        let out_dir = self.download.output_dir.clone()
            .unwrap_or_else(|| std::env::temp_dir().join("a3000_downloads"));
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            self.status = format!("× create_dir {}: {e}", out_dir.display());
            return;
        }
        let safe_name: String = sample.name.chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let stem = if safe_name.trim().is_empty() { format!("slot_{next_slot:04}") }
                   else { format!("slot_{next_slot:04}_{safe_name}") };
        let output_path = out_dir.join(format!("{stem}.wav"));

        self.download.current_idx = Some(idx);
        self.download.download_progress = 0.0;

        let cmd = Cmd::Receive {
            ha: self.config.ha,
            bus: self.config.bus,
            target: self.config.target,
            lun: self.config.lun,
            sample_number: next_slot,
            output_path: output_path.to_string_lossy().to_string(),
        };
        if let Err(e) = sender.send_cmd(&cmd) {
            self.status = format!("× send Receive: {e}");
            self.download.current_idx = None;
        } else {
            self.status = format!("Téléchargement #{next_slot} → {}", output_path.display());
        }
    }

    #[cfg(not(windows))]
    fn try_start_next_download(&mut self) {}

    /// Slicer → Upload : exporte les slices non marquées en WAV et les ajoute
    /// à la queue Upload, puis bascule sur le tab Upload.
    fn poll_slicer_send_to_upload(&mut self) {
        if !self.slicer.request_send_to_upload {
            return;
        }
        self.slicer.request_send_to_upload = false;
        match self.slicer.export_slices_to_wavs() {
            Ok(paths) => {
                let n = paths.len();
                for p in paths {
                    self.upload.add_path(p);
                }
                self.active_tab = Tab::Upload;
                self.status = format!(
                    "{n} slice{} envoyée{} dans le tab Upload",
                    if n > 1 { "s" } else { "" },
                    if n > 1 { "s" } else { "" },
                );
            }
            Err(e) => {
                self.slicer.error = Some(format!("× Send to Upload : {e}"));
            }
        }
    }

    /// Modal Settings : édite HA/BUS/TARGET/LUN + auto/manual start slot.
    /// Pas de Save/Cancel ; les changements sont live et persistés au save()
    /// d'eframe. Bouton Close ferme la fenêtre.
    fn show_settings(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(320.0)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Cible SCSI").color(palette::ACCENT_YELLOW).strong());
                ui.add_space(4.0);
                egui::Grid::new("scsi_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Host adapter (HA)");
                        ui.add(egui::DragValue::new(&mut self.config.ha).range(0..=15));
                        ui.end_row();

                        ui.label("Bus (PathId)");
                        ui.add(egui::DragValue::new(&mut self.config.bus).range(0..=15));
                        ui.end_row();

                        ui.label("Target ID");
                        ui.add(egui::DragValue::new(&mut self.config.target).range(0..=15));
                        ui.end_row();

                        ui.label("LUN");
                        ui.add(egui::DragValue::new(&mut self.config.lun).range(0..=7));
                        ui.end_row();
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                ui.label(egui::RichText::new("Slot de départ (Upload)").color(palette::ACCENT_YELLOW).strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.config.auto_start_slot, true, "Auto (1er libre)");
                    ui.radio_value(&mut self.config.auto_start_slot, false, "Manuel :");
                    ui.add_enabled(
                        !self.config.auto_start_slot,
                        egui::DragValue::new(&mut self.config.manual_start_slot).range(0..=999),
                    );
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save now").clicked() {
                        match self.config.save() {
                            Ok(()) => self.status = "Config sauvée.".into(),
                            Err(e) => self.status = format!("× Erreur sauvegarde config : {e}"),
                        }
                    }
                    ui.label(
                        egui::RichText::new("(les changements sont auto-sauvés à la fermeture de l'app)")
                            .color(palette::FG_DIM)
                            .small(),
                    );
                });
            });
        self.settings_open = open;
    }
}

impl eframe::App for A3000App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker(ctx);
        self.poll_upload_request();
        self.poll_download_request();
        self.poll_slicer_send_to_upload();

        // Si on est en train de se connecter, on repaint régulièrement pour
        // récupérer la transition.
        #[cfg(windows)]
        if matches!(self.worker, WorkerState::Connecting(_)) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Top bar : tabs + worker status + settings
        egui::TopBottomPanel::top("tabs")
            .show_separator_line(true)
            .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for t in [Tab::Upload, Tab::Download, Tab::Slicer] {
                    let selected = self.active_tab == t;
                    let label = egui::RichText::new(t.label())
                        .size(15.0)
                        .color(if selected { palette::ACCENT_GREEN } else { palette::FG_TEXT });
                    if ui.selectable_label(selected, label).clicked() {
                        self.active_tab = t;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                    ui.label(format!(
                        "HA{} BUS{} ID{} LUN{}",
                        self.config.ha, self.config.bus,
                        self.config.target, self.config.lun,
                    ));
                    ui.separator();
                    // Worker status + bouton connect
                    #[cfg(windows)]
                    {
                        let label = self.worker.label();
                        let color = self.worker.color();
                        ui.label(egui::RichText::new(&label).color(color));
                        if matches!(self.worker, WorkerState::Idle | WorkerState::Error(_))
                            && ui.button("Connect…").clicked()
                        {
                            self.start_worker(ctx);
                        }
                    }
                });
            });
            ui.add_space(6.0);
        });

        // Settings modal
        if self.settings_open {
            self.show_settings(ctx);
        }

        // Bottom status bar
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.status).color(palette::FG_DIM));
            });
        });

        // Tab content
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                Tab::Upload => tabs::upload::show(ui, &mut self.upload, &self.config),
                Tab::Download => tabs::download::show(ui, &mut self.download),
                Tab::Slicer => tabs::slicer::show(ui, &mut self.slicer),
            }
        });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let _ = self.config.save();
    }
}
