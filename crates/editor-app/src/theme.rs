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
    bg: Color32::from_rgb(9, 10, 12),
    stage: Color32::from_rgb(4, 5, 7),
    panel: Color32::from_rgb(20, 22, 26),
    header: Color32::from_rgb(27, 30, 35),
    inset: Color32::from_rgb(12, 13, 16),
    control: Color32::from_rgb(34, 38, 44),
    control_hover: Color32::from_rgb(46, 52, 60),
    hairline: Color32::from_rgb(40, 44, 52),
    border: Color32::from_rgb(64, 70, 80),
    text: Color32::from_rgb(236, 238, 241),
    text_dim: Color32::from_rgb(166, 174, 184),
    text_mute: Color32::from_rgb(108, 116, 128),
    accent: Color32::from_rgb(64, 214, 188),
    accent_dim: Color32::from_rgb(14, 48, 44),
    accent_text: Color32::from_rgb(8, 24, 22),
    amber: Color32::from_rgb(224, 168, 78),
    playhead: Color32::from_rgb(255, 72, 64),
    danger: Color32::from_rgb(214, 86, 78),
    video: Color32::from_rgb(52, 114, 186),
    audio: Color32::from_rgb(42, 138, 102),
    caption: Color32::from_rgb(184, 132, 52),
    lane: Color32::from_rgb(13, 14, 17),
    lane_alt: Color32::from_rgb(17, 19, 23),
    ruler: Color32::from_rgb(16, 18, 22),
    selection: Color32::from_rgb(242, 196, 72),
    radius: 2,
};

/// Shared chrome metrics. Panels and painted controls read these instead of
/// inventing their own spacing.
pub const HEADER_H: f32 = 26.0;
pub const LANE_H: f32 = 26.0;
pub const RULER_H: f32 = 22.0;
pub const HEADER_COL_W: f32 = 152.0;
pub const TRANSPORT_H: f32 = 52.0;
pub const TOOLBAR_H: f32 = 46.0;
pub const MENU_H: f32 = 34.0;
pub const STATUS_H: f32 = 24.0;
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 6.0;
pub const SPACE_MD: f32 = 8.0;
pub const SPACE_LG: f32 = 12.0;

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
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, THEME.border);
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
        style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_XS);
        style.spacing.button_padding = egui::vec2(SPACE_MD, 3.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.slider_width = 160.0;
        style.spacing.window_margin = Margin::same(8);
        style.spacing.menu_margin = Margin::same(4);
        style.spacing.indent = 12.0;
        style.interaction.resize_grab_radius_side = 6.0;
        style.text_styles.insert(TextStyle::Body, THEME.font(12.5));
        style
            .text_styles
            .insert(TextStyle::Button, THEME.font(12.0));
        style.text_styles.insert(TextStyle::Small, THEME.font(10.5));
        style
            .text_styles
            .insert(TextStyle::Heading, THEME.font(16.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, THEME.mono(12.0));
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
        .stroke(Stroke::NONE)
}

pub fn hairline_stroke() -> Stroke {
    Stroke::new(1.0_f32, THEME.hairline)
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
    pub fn font(self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }

    pub fn mono(self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }

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
