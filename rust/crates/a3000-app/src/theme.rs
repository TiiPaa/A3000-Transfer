//! Palette dark sample-editor (cohérent avec l'icône A3K vert/jaune/orange).
//!
//! Tous les paramètres de design (couleurs, tailles, fontes, espacements)
//! sont centralisés ici pour faciliter le re-skinning visuel.

use eframe::egui::{self, Color32, Stroke};

/// Couleurs nommées.
///
/// Convention :
/// - `BG_*` : fonds (du plus foncé au plus clair).
/// - `FG_*` : textes.
/// - `ACCENT_*` : actions et états (sémantique : vert OK / orange warning /
///   rouge erreur ; jaune réservé aux états transitoires "loading").
/// - `BUTTON_*` : couleurs spécifiques aux boutons d'action.
pub mod palette {
    use super::Color32;

    // Fonds
    pub const BG_DEEP: Color32 = Color32::from_rgb(15, 17, 20);
    pub const BG_PANEL: Color32 = Color32::from_rgb(20, 22, 25);
    pub const BG_PANEL_LIGHT: Color32 = Color32::from_rgb(28, 31, 36);
    pub const BG_HOVER: Color32 = Color32::from_rgb(38, 42, 48);
    pub const SEPARATOR: Color32 = Color32::from_rgb(45, 48, 54);

    // Textes
    pub const FG_TEXT: Color32 = Color32::from_rgb(220, 222, 215);
    pub const FG_DIM: Color32 = Color32::from_rgb(140, 146, 142);

    // Accents sémantiques
    pub const ACCENT_GREEN: Color32 = Color32::from_rgb(120, 180, 95);
    pub const ACCENT_YELLOW: Color32 = Color32::from_rgb(220, 200, 80);
    pub const ACCENT_ORANGE: Color32 = Color32::from_rgb(220, 140, 60);
    pub const ACCENT_RED: Color32 = Color32::from_rgb(220, 90, 80);
    pub const SELECTION: Color32 = Color32::from_rgb(60, 110, 65);

    // Boutons d'action — réservés aux fills de boutons pour pouvoir les
    // ré-accorder sans toucher la sémantique des accents.
    #[allow(dead_code)]
    /// Bouton primaire (alias d'ACCENT_GREEN — utilisable explicitement
    /// quand on veut signifier "action principale" sans dépendre du
    /// sémantique de l'accent).
    pub const BUTTON_PRIMARY: Color32 = ACCENT_GREEN;
    /// Bouton MIDI export (teinte cuivre/ambre, plus douce que ACCENT_YELLOW
    /// qui est trop saturé pour de larges surfaces).
    pub const BUTTON_MIDI: Color32 = Color32::from_rgb(178, 134, 60);
    #[allow(dead_code)]
    /// Bouton destructif — alias d'ACCENT_RED.
    pub const BUTTON_DANGER: Color32 = ACCENT_RED;
    #[allow(dead_code)]
    /// Bouton transitoire — alias d'ACCENT_ORANGE.
    pub const BUTTON_BUSY: Color32 = ACCENT_ORANGE;
}

/// Tailles standards (px). À utiliser plutôt que des magic numbers.
#[allow(dead_code)] // tokens design centralisés, pas tous référencés actuellement
pub mod size {
    /// Hauteur d'un bouton d'action (footer, top bar).
    pub const BTN_H: f32 = 32.0;
    /// Hauteur d'une row de table (Upload/Download).
    pub const ROW_H: f32 = 28.0;
    /// Hauteur du bandeau de selection cells au-dessus de la waveform.
    pub const CELLS_H: f32 = 22.0;
    /// Hauteur du canvas waveform.
    pub const WAVEFORM_H: f32 = 200.0;
    /// Hauteur réservée pour le footer (pour `allocate_exact_size` du bloc table).
    pub const FOOTER_RESERVED_H: f32 = 70.0;
    /// Idem pour le slicer dont le footer est sur 2 lignes.
    pub const SLICER_FOOTER_RESERVED_H: f32 = 110.0;
    /// Zone de hit (px) autour d'un onset marker pour le drag.
    pub const ONSET_HIT_PX: f32 = 5.0;
}

/// Tailles de fontes (en pixels logiques).
#[allow(dead_code)]
pub mod font {
    pub const HEADING: f32 = 18.0;
    pub const BODY: f32 = 13.0;
    pub const SMALL: f32 = 11.0;
    pub const LARGE_PROMPT: f32 = 20.0;  // pour les "Drop ici" prompts vides
}

/// Espacements standards.
#[allow(dead_code)]
pub mod space {
    /// Padding horizontal d'un bouton.
    pub const BTN_PAD_X: f32 = 12.0;
    /// Padding vertical d'un bouton.
    pub const BTN_PAD_Y: f32 = 6.0;
    /// Espace vertical entre éléments (default item_spacing.y).
    pub const ITEM_Y: f32 = 6.0;
    /// Espace horizontal entre éléments (default item_spacing.x).
    pub const ITEM_X: f32 = 8.0;
    /// Espace au-dessus d'un heading de tab.
    pub const TAB_TOP: f32 = 6.0;
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
