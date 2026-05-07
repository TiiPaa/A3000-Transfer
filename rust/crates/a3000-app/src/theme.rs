//! Palette dark sample-editor (cohérent avec l'icône A3K vert/jaune/orange).
//!
//! Thème volontairement sombre, faible chroma sauf pour les accents :
//! background quasi noir, panneaux gris-bleu très sombres, accent vert-jaune
//! pour la sélection, orange pour les warnings.

use eframe::egui::{self, Color32, Stroke};

/// Couleurs nommées (constantes pour réutilisation par les widgets custom).
pub mod palette {
    use super::Color32;
    pub const BG_DEEP: Color32 = Color32::from_rgb(15, 17, 20);
    pub const BG_PANEL: Color32 = Color32::from_rgb(20, 22, 25);
    pub const BG_PANEL_LIGHT: Color32 = Color32::from_rgb(28, 31, 36);
    pub const BG_HOVER: Color32 = Color32::from_rgb(38, 42, 48);
    pub const FG_TEXT: Color32 = Color32::from_rgb(220, 222, 215);
    pub const FG_DIM: Color32 = Color32::from_rgb(140, 146, 142);
    pub const ACCENT_GREEN: Color32 = Color32::from_rgb(120, 180, 95);
    pub const ACCENT_YELLOW: Color32 = Color32::from_rgb(220, 200, 80);
    pub const ACCENT_ORANGE: Color32 = Color32::from_rgb(220, 140, 60);
    pub const ACCENT_RED: Color32 = Color32::from_rgb(220, 90, 80);
    pub const SELECTION: Color32 = Color32::from_rgb(60, 110, 65);
    pub const SEPARATOR: Color32 = Color32::from_rgb(45, 48, 54);
}

pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    use palette::*;

    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_PANEL;
    visuals.faint_bg_color = BG_PANEL_LIGHT;
    visuals.extreme_bg_color = BG_DEEP;
    visuals.window_stroke = Stroke::new(1.0, SEPARATOR);

    visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG_TEXT);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SEPARATOR);

    visuals.widgets.inactive.bg_fill = BG_PANEL_LIGHT;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, FG_TEXT);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, SEPARATOR);

    visuals.widgets.hovered.bg_fill = BG_HOVER;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, FG_TEXT);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, ACCENT_GREEN);

    visuals.widgets.active.bg_fill = SELECTION;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, FG_TEXT);
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT_GREEN);

    visuals.selection.bg_fill = SELECTION;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_GREEN);
    visuals.hyperlink_color = ACCENT_YELLOW;
    visuals.warn_fg_color = ACCENT_ORANGE;
    visuals.error_fg_color = ACCENT_RED;

    ctx.set_visuals(visuals);

    // Spacing — défauts egui trop serrés (button_padding=(4,1) par défaut).
    // On écarte horizontalement + verticalement pour laisser respirer le texte.
    ctx.style_mut(|style| {
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        // Hauteur minimum d'un widget interactif (incluant boutons / DragValue /
        // TextEdit / Checkbox) — assure une ligne de base cohérente entre cellules.
        style.spacing.interact_size = egui::vec2(40.0, 24.0);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}
