//! Dense dark chrome for Meridian. Teal is the editorial accent; amber marks
//! the playhead, selection, and finishing controls.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Visuals,
};

pub const BG: Color32 = Color32::from_rgb(16, 17, 20);
pub const PANEL: Color32 = Color32::from_rgb(24, 26, 30);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(32, 35, 40);
pub const HEADER: Color32 = Color32::from_rgb(28, 31, 36);
pub const BORDER: Color32 = Color32::from_rgb(48, 53, 60);
pub const TEXT: Color32 = Color32::from_rgb(226, 228, 232);
pub const DIM: Color32 = Color32::from_rgb(142, 150, 160);
pub const ACCENT: Color32 = Color32::from_rgb(72, 196, 186);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(28, 78, 76);
pub const AMBER: Color32 = Color32::from_rgb(224, 164, 90);
pub const PLAYHEAD: Color32 = Color32::from_rgb(255, 92, 84);
pub const VIDEO: Color32 = Color32::from_rgb(46, 96, 148);
pub const AUDIO: Color32 = Color32::from_rgb(32, 110, 84);
pub const CAPTION: Color32 = Color32::from_rgb(148, 112, 42);
pub const DANGER: Color32 = Color32::from_rgb(196, 82, 74);
pub const LANE: Color32 = Color32::from_rgb(20, 22, 26);
pub const LANE_ALT: Color32 = Color32::from_rgb(26, 28, 33);
pub const RULER: Color32 = Color32::from_rgb(18, 19, 22);

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut visuals = Visuals::dark();
    visuals.window_fill = BG;
    visuals.panel_fill = PANEL;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = PANEL_RAISED;
    visuals.code_bg_color = Color32::from_rgb(14, 15, 18);
    visuals.window_corner_radius = CornerRadius::same(3);
    visuals.menu_corner_radius = CornerRadius::same(3);
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow::NONE;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(2);
    visuals.widgets.inactive.bg_fill = PANEL_RAISED;
    visuals.widgets.inactive.weak_bg_fill = HEADER;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(2);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 48, 56);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(40, 46, 54);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(2);
    visuals.widgets.active.bg_fill = ACCENT_DIM;
    visuals.widgets.active.weak_bg_fill = ACCENT_DIM;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.corner_radius = CornerRadius::same(2);
    visuals.widgets.open.bg_fill = PANEL_RAISED;
    visuals.widgets.open.corner_radius = CornerRadius::same(2);
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.hyperlink_color = ACCENT;
    visuals.override_text_color = Some(TEXT);
    ctx.set_visuals(visuals);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(7.0, 3.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.slider_width = 140.0;
        style.spacing.window_margin = egui::Margin::same(8);
        style.spacing.menu_margin = egui::Margin::same(6);
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(12.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(16.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        );
    });
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let proportional = [
        "/usr/share/fonts/truetype/macos/Inter-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let mono = [
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    ];
    if let Some(bytes) = first_font(&proportional) {
        fonts.font_data.insert(
            "MeridianSans".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "MeridianSans".into());
    }
    if let Some(bytes) = first_font(&mono) {
        fonts.font_data.insert(
            "MeridianMono".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "MeridianMono".into());
    }
    ctx.set_fonts(fonts);
}

fn first_font(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|path| std::fs::read(path).ok())
}

pub fn track_color(kind: editor_core::TrackKind) -> Color32 {
    match kind {
        editor_core::TrackKind::Video => VIDEO,
        editor_core::TrackKind::Audio => AUDIO,
        editor_core::TrackKind::Caption => CAPTION,
    }
}

pub fn label_fill(label: editor_core::LabelColor, kind: editor_core::TrackKind) -> Color32 {
    match label {
        editor_core::LabelColor::Neutral => track_color(kind),
        editor_core::LabelColor::Rose => Color32::from_rgb(140, 64, 78),
        editor_core::LabelColor::Amber => Color32::from_rgb(140, 104, 48),
        editor_core::LabelColor::Green => Color32::from_rgb(36, 112, 78),
        editor_core::LabelColor::Teal => Color32::from_rgb(32, 112, 118),
        editor_core::LabelColor::Blue => Color32::from_rgb(46, 90, 150),
        editor_core::LabelColor::Violet => Color32::from_rgb(96, 72, 150),
    }
}
