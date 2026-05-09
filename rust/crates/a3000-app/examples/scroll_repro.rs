//! Repro minimal du bug d'overlap rows / header / footer.
//!
//! 3 tabs, chacun avec une approche différente du clipping. Bordures
//! colorées pour visualiser les rects.

use eframe::egui;

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "scroll_repro",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([800.0, 500.0]),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::<App>::default())),
    )
}

#[derive(Default)]
struct App {
    tab: u8,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, name) in ["A: allocate_ui", "B: child_ui+clip", "C: TableBuilder"].iter().enumerate() {
                    if ui.selectable_label(self.tab == i as u8, *name).clicked() {
                        self.tab = i as u8;
                    }
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            0 => approach_a(ui),
            1 => approach_b(ui),
            _ => approach_c(ui),
        });
    }
}

const N_ROWS: usize = 60;
const FOOTER_H: f32 = 60.0;
const ROW_H: f32 = 28.0;
const BORDER: egui::Stroke = egui::Stroke { width: 2.0, color: egui::Color32::RED };

fn header(ui: &mut egui::Ui) {
    let bg = egui::Color32::from_rgb(30, 60, 30);
    egui::Frame::none().fill(bg).inner_margin(egui::Margin::same(6.0)).show(ui, |ui| {
        ui.label(egui::RichText::new("=== HEADER (vert sombre) ===").color(egui::Color32::WHITE));
    });
}

fn footer(ui: &mut egui::Ui) {
    let bg = egui::Color32::from_rgb(60, 30, 30);
    egui::Frame::none().fill(bg).inner_margin(egui::Margin::same(6.0)).show(ui, |ui| {
        ui.label(egui::RichText::new("=== FOOTER (rouge sombre) ===").color(egui::Color32::WHITE));
        ui.button("Send / Reset / etc.");
    });
}

fn rows(ui: &mut egui::Ui) {
    for i in 0..N_ROWS {
        ui.horizontal(|ui| {
            ui.set_min_height(ROW_H);
            ui.label(format!("Row {i:03} — content content content"));
        });
    }
}

/// A : ui.allocate_ui(size, |ui| ScrollArea inside)
fn approach_a(ui: &mut egui::Ui) {
    header(ui);
    let block_h = (ui.available_height() - FOOTER_H).max(100.0);
    let block_size = egui::vec2(ui.available_width(), block_h);
    ui.allocate_ui(block_size, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| rows(ui));
    });
    // Bordure rouge autour du rect alloué (visualisation)
    let last_rect = ui.min_rect();
    ui.painter().rect_stroke(
        egui::Rect::from_min_size(
            egui::pos2(last_rect.left(), last_rect.bottom() - block_h),
            block_size,
        ),
        0.0,
        BORDER,
    );
    footer(ui);
}

/// B : allocate_exact_size + child_ui + set_clip_rect + ScrollArea inside
fn approach_b(ui: &mut egui::Ui) {
    header(ui);
    let block_h = (ui.available_height() - FOOTER_H).max(100.0);
    let block_size = egui::vec2(ui.available_width(), block_h);
    let (block_rect, _) = ui.allocate_exact_size(block_size, egui::Sense::hover());
    let mut child = ui.child_ui(
        block_rect,
        egui::Layout::top_down(egui::Align::Min),
        None,
    );
    child.set_clip_rect(block_rect);
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(&mut child, rows);
    ui.painter().rect_stroke(block_rect, 0.0, BORDER);
    footer(ui);
}

/// C : egui_extras::TableBuilder
fn approach_c(ui: &mut egui::Ui) {
    header(ui);
    let block_h = (ui.available_height() - FOOTER_H).max(100.0);
    egui_extras::TableBuilder::new(ui)
        .striped(true)
        .resizable(false)
        .min_scrolled_height(0.0)
        .max_scroll_height(block_h)
        .column(egui_extras::Column::remainder())
        .body(|body| {
            body.rows(ROW_H, N_ROWS, |mut row| {
                let i = row.index();
                row.col(|ui| {
                    ui.label(format!("Row {i:03} — content content content"));
                });
            });
        });
    // Note : TableBuilder gère son propre footer si on en met dans le builder.
    // Ici on laisse footer tomber dessous pour comparer le comportement.
    footer(ui);
}
