//! Waveform and vectorscope fed from the program-frame cache.

use egui::{Color32, Pos2, Rect, RichText, Shape, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::widgets;

pub fn scopes_panel(ui: &mut egui::Ui, app: &MeridianApp) {
    widgets::panel_header(ui, "Scopes", |_| {});
    let cache = app.picture_cache.as_ref();
    if cache.is_none() || cache.unwrap().rgba.is_empty() {
        widgets::empty_note(ui, "Scopes read the composited program frame.");
        widgets::empty_note(ui, "Scrub the timeline or play to fill them.");
        return;
    }
    let cache = cache.unwrap();
    let width = cache.width as usize;
    let height = cache.height as usize;
    let rgba = &cache.rgba;
    if width < 2 || height < 2 {
        return;
    }

    ui.add_space(4.0);
    let total_h = ui.available_height();
    let waveform_h = (total_h * 0.52).clamp(100.0, 280.0);
    let vectorscope_h = (total_h - waveform_h - 24.0).max(100.0);

    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new("Waveform").size(11.0).color(THEME.text_dim));
    });
    let wf_size = Vec2::new(ui.available_width() - 16.0, waveform_h);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(wf_size, egui::Sense::hover());
        paint_waveform(ui, rect, rgba, width, height);
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new("Vectorscope").size(11.0).color(THEME.text_dim));
    });
    let vs_size = Vec2::new(ui.available_width() - 16.0, vectorscope_h);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(vs_size, egui::Sense::hover());
        paint_vectorscope(ui, rect, rgba, width, height);
    });
}

fn paint_waveform(ui: &egui::Ui, rect: Rect, rgba: &[u8], width: usize, height: usize) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.inset);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, THEME.border), egui::StrokeKind::Inside);

    let scope_w = rect.width() as usize;
    let scope_h = rect.height() as usize;
    if scope_w < 2 || scope_h < 2 {
        return;
    }

    let step_x = width as f32 / scope_w as f32;
    let mut peaks = vec![0.0f32; scope_w];
    for sx in 0..scope_w {
        let x0 = (sx as f32 * step_x).floor() as usize;
        let x1 = ((sx + 1) as f32 * step_x).ceil() as usize;
        let mut peak = 0.0f32;
        for x in x0..x1.min(width) {
            for y in 0..height {
                let index = (y * width + x) * 4;
                if index + 2 < rgba.len() {
                    let luma = rec709_luma_u8(rgba[index], rgba[index + 1], rgba[index + 2]);
                    peak = peak.max(luma);
                }
            }
        }
        peaks[sx] = peak;
    }

    let grid_y = [
        (0.0, THEME.hairline),
        (0.25, Color32::from_white_alpha(20)),
        (0.5, Color32::from_white_alpha(36)),
        (0.75, Color32::from_white_alpha(20)),
        (1.0, THEME.hairline),
    ];
    for (level, color) in grid_y {
        let y = rect.bottom() - level * rect.height();
        painter.hline(rect.x_range(), y, Stroke::new(1.0, color));
    }

    let mut points = Vec::with_capacity(scope_w + 2);
    points.push(Pos2::new(rect.left(), rect.bottom()));
    for (sx, peak) in peaks.iter().enumerate() {
        let x = rect.left() + (sx as f32 + 0.5) / scope_w as f32 * rect.width();
        let y = rect.bottom() - peak * rect.height();
        points.push(Pos2::new(x, y));
    }
    points.push(Pos2::new(rect.right(), rect.bottom()));
    painter.add(Shape::convex_polygon(
        points,
        Color32::from_rgba_unmultiplied(64, 214, 188, 48),
        Stroke::new(1.5, THEME.accent),
    ));
}

fn paint_vectorscope(ui: &egui::Ui, rect: Rect, rgba: &[u8], width: usize, height: usize) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.inset);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, THEME.border), egui::StrokeKind::Inside);

    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.44;
    painter.circle_stroke(center, radius, Stroke::new(1.0, THEME.hairline));
    painter.circle_stroke(center, radius * 0.66, Stroke::new(1.0, Color32::from_white_alpha(16)));
    painter.circle_stroke(center, radius * 0.33, Stroke::new(1.0, Color32::from_white_alpha(12)));
    painter.line_segment(
        [center + Vec2::new(-radius, 0.0), center + Vec2::new(radius, 0.0)],
        Stroke::new(1.0, Color32::from_white_alpha(16)),
    );
    painter.line_segment(
        [center + Vec2::new(0.0, -radius), center + Vec2::new(0.0, radius)],
        Stroke::new(1.0, Color32::from_white_alpha(16)),
    );

    let stride = ((width * height) / 12_000).max(1);
    let mut dots = Vec::new();
    for (index, chunk) in rgba.chunks(4).enumerate() {
        if index % stride != 0 || chunk[3] < 8 {
            continue;
        }
        let r = chunk[0] as f32 / 255.0;
        let g = chunk[1] as f32 / 255.0;
        let b = chunk[2] as f32 / 255.0;
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if luma < 0.02 {
            continue;
        }
        let cb = (b - luma) / 1.8556;
        let cr = (r - luma) / 1.5748;
        let sat = (cb * cb + cr * cr).sqrt().min(0.5);
        let angle = cr.atan2(cb);
        let px = center.x + angle.cos() * sat * radius * 2.0;
        let py = center.y + angle.sin() * sat * radius * 2.0;
        let alpha = (luma * 180.0 + 40.0).clamp(40.0, 220.0) as u8;
        dots.push((Pos2::new(px, py), Color32::from_rgba_unmultiplied(64, 214, 188, alpha)));
    }
    for (pos, color) in dots {
        painter.circle_filled(pos, 1.2, color);
    }
}

fn rec709_luma_u8(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}
