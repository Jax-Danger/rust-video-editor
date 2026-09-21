//! MeridianTheme — the only colour, radius, and type scale the shell should use.
//!
//! Teal is reserved for the active mode, selection accents, and primary actions.
//! The playhead is the only red. Clip bodies carry their own label colours.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Shadow,
    Stroke, TextStyle, Visuals,
};

use editor_core::{LabelColor, TrackKind};

#[derive(Clone, Copy, Debug)]
pub struct MeridianTheme {
    pub bg: Color32,
    pub stage: Color32,
    pub panel: Color32,
    pub header: Color32,
    pub inset: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub hairline: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_mute: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub accent_text: Color32,
    pub amber: Color32,
    pub playhead: Color32,
    pub danger: Color32,
    pub video: Color32,
    pub audio: Color32,
    pub caption: Color32,
    pub lane: Color32,
    pub lane_alt: Color32,
    pub ruler: Color32,
    pub selection: Color32,
    pub radius: u8,
}

pub const THEME: MeridianTheme = MeridianTheme {
    bg: Color32::from_rgb(12, 13, 16),
    stage: Color32::from_rgb(7, 8, 10),
    panel: Color32::from_rgb(22, 24, 28),
    header: Color32::from_rgb(28, 31, 36),
    inset: Color32::from_rgb(14, 15, 18),
    control: Color32::from_rgb(36, 40, 46),
    control_hover: Color32::from_rgb(48, 54, 62),
    hairline: Color32::from_rgb(42, 46, 54),
    border: Color32::from_rgb(58, 64, 74),
    text: Color32::from_rgb(232, 234, 237),
    text_dim: Color32::from_rgb(154, 163, 173),
    text_mute: Color32::from_rgb(108, 116, 128),
    accent: Color32::from_rgb(61, 214, 186),
    accent_dim: Color32::from_rgb(18, 58, 54),
    accent_text: Color32::from_rgb(10, 28, 26),
    amber: Color32::from_rgb(232, 176, 92),
    playhead: Color32::from_rgb(255, 78, 72),
    danger: Color32::from_rgb(214, 92, 84),
    video: Color32::from_rgb(47, 108, 176),
    audio: Color32::from_rgb(36, 128, 96),
    caption: Color32::from_rgb(176, 128, 48),
    lane: Color32::from_rgb(16, 17, 20),
    lane_alt: Color32::from_rgb(20, 22, 26),
    ruler: Color32::from_rgb(18, 20, 24),
    selection: Color32::from_rgb(242, 193, 78),
    radius: 3,
};

pub const DIM: Color32 = THEME.text_dim;
pub const AMBER: Color32 = THEME.amber;
pub const CAPTION: Color32 = THEME.caption;
pub const DANGER: Color32 = THEME.danger;
pub const LANE: Color32 = THEME.lane;
pub const LANE_ALT: Color32 = THEME.lane_alt;
pub const RULER: Color32 = THEME.ruler;

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut visuals = Visuals::dark();
    visuals.window_fill = THEME.panel;
    visuals.panel_fill = THEME.panel;
    visuals.extreme_bg_color = THEME.bg;
    visuals.faint_bg_color = THEME.header;
    visuals.code_bg_color = THEME.inset;
    visuals.window_corner_radius = CornerRadius::same(THEME.radius);
    visuals.menu_corner_radius = CornerRadius::same(THEME.radius);
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow::NONE;
    visuals.window_stroke = Stroke::new(1.0_f32, THEME.border);
    visuals.widgets.noninteractive.bg_fill = THEME.panel;
    visuals.widgets.noninteractive.weak_bg_fill = THEME.panel;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, THEME.text);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, THEME.hairline);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(THEME.radius);
    visuals.widgets.inactive.bg_fill = THEME.control;
    visuals.widgets.inactive.weak_bg_fill = THEME.header;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, THEME.text);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, THEME.hairline);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(THEME.radius);
    visuals.widgets.hovered.bg_fill = THEME.control_hover;
    visuals.widgets.hovered.weak_bg_fill = THEME.control_hover;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, THEME.text);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, THEME.border);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(THEME.radius);
    visuals.widgets.active.bg_fill = THEME.accent_dim;
    visuals.widgets.active.weak_bg_fill = THEME.accent_dim;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, THEME.text);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, THEME.accent);
    visuals.widgets.active.corner_radius = CornerRadius::same(THEME.radius);
    visuals.widgets.open.bg_fill = THEME.header;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, THEME.text);
    visuals.widgets.open.corner_radius = CornerRadius::same(THEME.radius);
    visuals.selection.bg_fill = THEME.accent_dim;
    visuals.selection.stroke = Stroke::new(1.0_f32, THEME.accent);
    visuals.hyperlink_color = THEME.accent;
    visuals.override_text_color = Some(THEME.text);
    ctx.set_visuals(visuals);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size.y = 22.0;
        style.spacing.slider_width = 160.0;
        style.spacing.window_margin = Margin::same(10);
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.indent = 12.0;
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(18.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        );
    });
}

pub fn chrome_frame() -> Frame {
    Frame::new()
        .fill(THEME.bg)
        .inner_margin(Margin::same(0))
        .stroke(Stroke::NONE)
}

pub fn panel_frame() -> Frame {
    Frame::new()
        .fill(THEME.panel)
        .inner_margin(Margin::same(0))
        .stroke(Stroke::new(1.0_f32, THEME.hairline))
}

pub fn dialog_frame() -> Frame {
    Frame::new()
        .fill(THEME.panel)
        .inner_margin(Margin::same(14))
        .stroke(Stroke::new(1.0_f32, THEME.border))
        .corner_radius(CornerRadius::same(6))
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

impl MeridianTheme {
    pub fn track_color(self, kind: TrackKind) -> Color32 {
        match kind {
            TrackKind::Video => self.video,
            TrackKind::Audio => self.audio,
            TrackKind::Caption => self.caption,
        }
    }

    pub fn label_fill(self, label: LabelColor, kind: TrackKind) -> Color32 {
        match label {
            LabelColor::Neutral => self.track_color(kind),
            LabelColor::Rose => Color32::from_rgb(168, 72, 88),
            LabelColor::Amber => Color32::from_rgb(176, 122, 48),
            LabelColor::Green => Color32::from_rgb(42, 138, 96),
            LabelColor::Teal => Color32::from_rgb(32, 140, 146),
            LabelColor::Blue => Color32::from_rgb(52, 112, 186),
            LabelColor::Violet => Color32::from_rgb(118, 86, 184),
        }
    }
}

pub fn track_color(kind: TrackKind) -> Color32 {
    THEME.track_color(kind)
}

pub fn label_fill(label: LabelColor, kind: TrackKind) -> Color32 {
    THEME.label_fill(label, kind)
}
