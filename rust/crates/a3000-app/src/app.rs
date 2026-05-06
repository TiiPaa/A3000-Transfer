//! Shell `eframe::App` principal.
//!
//! Layout :
//!  - TopBottomPanel "tabs" : sélecteur Upload / Download / Slicer + bouton Settings
//!  - CentralPanel : contenu de la tab active
//!  - TopBottomPanel "status" en bas : statut connexion worker, file de progress

use eframe::egui;

use crate::config::Config;
use crate::tabs::{self, Tab};
use crate::theme::palette;

pub struct A3000App {
    pub active_tab: Tab,
    pub config: Config,
    pub upload: tabs::upload::UploadState,
    pub download: tabs::download::DownloadState,
    pub slicer: tabs::slicer::SlicerState,
    pub status: String,
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
        }
    }
}

impl eframe::App for A3000App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top bar : tabs + settings
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(4.0);
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
                        // TODO 3b : modal egui::Window pour HA/Bus/Target/LUN
                    }
                    ui.label(format!(
                        "HA{} BUS{} ID{} LUN{}",
                        self.config.ha, self.config.bus,
                        self.config.target, self.config.lun,
                    ));
                });
            });
            ui.add_space(2.0);
        });

        // Bottom status bar
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.status).color(palette::FG_DIM));
            });
        });

        // Tab content
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                Tab::Upload => tabs::upload::show(ui, &mut self.upload),
                Tab::Download => tabs::download::show(ui, &mut self.download),
                Tab::Slicer => tabs::slicer::show(ui, &mut self.slicer),
            }
        });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let _ = self.config.save();
    }
}
