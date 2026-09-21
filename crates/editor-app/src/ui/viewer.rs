//! Program monitor. Clips are drawn as graded, transformed cards — a stand-in
//! for a GPU viewer. Colour, opacity, dissolves, wipes, and pushes are real.

use editor_core::{clip_relative, color_grade, transform, ColorGrade, Frame, TrackKind, Transform};
use egui::{Color32, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme;
use crate::ui::format_tc;

pub fn viewer_panel(ui: &mut egui::Ui, app: &MeridianApp) {
    let Some(sequence) = app.session.project().active().cloned() else {
        ui.label("No sequence.");
        return;
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("PROGRAM").small().color(theme::DIM));
        ui.label(egui::RichText::new(&sequence.name).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format_tc(app.playhead, sequence.timebase))
                    .monospace()
                    .color(theme::AMBER),
            );
            ui.label(
                egui::RichText::new(format!("{}×{}", sequence.width, sequence.height))
                    .small()
                    .color(theme::DIM),
            );
        });
    });
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(8, 9, 11));
    let frame = letterbox(
        rect.shrink(16.0),
        sequence.width as f32,
        sequence.height as f32,
    );
    checker(&painter, frame);
    painter.rect_filled(frame, 0.0, Color32::BLACK);

    let video: Vec<_> = sequence
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .filter(|t| track_visible(t, &sequence.tracks))
        .collect();

    for track in video {
        if let Some(hit) = transition_hit(track, app.playhead) {
            paint_transition(&painter, frame, &sequence, hit, app.playhead);
        } else if let Some(clip) = track
            .clips
            .iter()
            .find(|c| c.covers(Frame(app.playhead)) && c.enabled)
        {
            paint_clip(
                &painter,
                frame,
                sequence.width as f32,
                sequence.height as f32,
                clip,
                1.0,
                app.playhead,
            );
        }
    }

    let captions: Vec<_> = sequence
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Caption && !t.muted)
        .flat_map(|t| t.cues.iter())
        .filter(|c| app.playhead >= c.timeline_in.0 && app.playhead < c.timeline_out.0)
        .map(|c| c.text.clone())
        .collect();
    if let Some(text) = captions.last() {
        let box_rect = Rect::from_min_max(
            Pos2::new(frame.left() + 16.0, frame.bottom() - 42.0),
            Pos2::new(frame.right() - 16.0, frame.bottom() - 12.0),
        );
        painter.rect_filled(box_rect, 2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 170));
        painter.text(
            box_rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(15.0),
            Color32::WHITE,
        );
    }

    painter.text(
        frame.left_top() + Vec2::new(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        format_tc(app.playhead, sequence.timebase),
        egui::FontId::monospace(13.0),
        Color32::from_rgba_unmultiplied(255, 220, 160, 220),
    );
}

fn track_visible(track: &editor_core::Track, tracks: &[editor_core::Track]) -> bool {
    if track.muted {
        return false;
    }
    let any_solo = tracks.iter().any(|t| t.kind == track.kind && t.solo);
    if any_solo {
        track.solo
    } else {
        true
    }
}

struct TransHit<'a> {
    kind: editor_core::TransitionKind,
    progress: f32,
    left: &'a editor_core::Clip,
    right: &'a editor_core::Clip,
}

fn transition_hit(track: &editor_core::Track, playhead: i64) -> Option<TransHit<'_>> {
    for transition in &track.transitions {
        let Some(left) = track.clips.iter().find(|c| c.id == transition.left_clip) else {
            continue;
        };
        let Some(right) = track.clips.iter().find(|c| c.id == transition.right_clip) else {
            continue;
        };
        let Some(progress) = transition.progress(left.timeline_out, Frame(playhead)) else {
            continue;
        };
        return Some(TransHit {
            kind: transition.kind.clone(),
            progress,
            left,
            right,
        });
    }
    None
}

fn paint_transition(
    painter: &Painter,
    frame: Rect,
    sequence: &editor_core::Sequence,
    hit: TransHit<'_>,
    playhead: i64,
) {
    let w = sequence.width as f32;
    let h = sequence.height as f32;
    match &hit.kind {
        editor_core::TransitionKind::CrossDissolve => {
            paint_clip(painter, frame, w, h, hit.left, 1.0 - hit.progress, playhead);
            paint_clip(painter, frame, w, h, hit.right, hit.progress, playhead);
        }
        editor_core::TransitionKind::Wipe { .. } => {
            let split = frame.left() + frame.width() * hit.progress;
            let left_rect = Rect::from_min_max(frame.min, Pos2::new(split, frame.bottom()));
            let right_rect = Rect::from_min_max(Pos2::new(split, frame.top()), frame.max);
            paint_clip_clipped(painter, frame, left_rect, w, h, hit.left, 1.0, playhead);
            paint_clip_clipped(painter, frame, right_rect, w, h, hit.right, 1.0, playhead);
        }
        editor_core::TransitionKind::PushSlide { .. } => {
            let shift = frame.width() * hit.progress;
            let left_frame = frame.translate(Vec2::new(-shift, 0.0));
            let right_frame = frame.translate(Vec2::new(frame.width() - shift, 0.0));
            paint_clip_clipped(painter, left_frame, frame, w, h, hit.left, 1.0, playhead);
            paint_clip_clipped(painter, right_frame, frame, w, h, hit.right, 1.0, playhead);
        }
    }
}

fn paint_clip_clipped(
    painter: &Painter,
    frame: Rect,
    clip_rect: Rect,
    seq_w: f32,
    seq_h: f32,
    clip: &editor_core::Clip,
    mix: f32,
    playhead: i64,
) {
    let painter = painter.with_clip_rect(clip_rect);
    paint_clip(&painter, frame, seq_w, seq_h, clip, mix, playhead);
}

fn paint_clip(
    painter: &Painter,
    frame: Rect,
    seq_w: f32,
    seq_h: f32,
    clip: &editor_core::Clip,
    mix: f32,
    playhead: i64,
) {
    let rel = clip_relative(Frame(playhead), clip.timeline_in);
    let xform = transform(&clip.effects)
        .cloned()
        .unwrap_or_else(Transform::identity);
    let grade = color_grade(&clip.effects)
        .cloned()
        .unwrap_or_else(ColorGrade::neutral);
    let opacity = (xform.opacity.value_at(rel) * mix).clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }
    let scale_x = xform.scale_x.value_at(rel).max(0.01);
    let scale_y = xform.scale_y.value_at(rel).max(0.01);
    let rotation = xform.rotation_deg.value_at(rel);
    let pos_x = xform.position_x.value_at(rel);
    let pos_y = xform.position_y.value_at(rel);
    let anchor_x = xform.anchor_x.value_at(rel);
    let anchor_y = xform.anchor_y.value_at(rel);

    let view_x = frame.width() / seq_w.max(1.0);
    let view_y = frame.height() / seq_h.max(1.0);
    let size = Vec2::new(frame.width() * scale_x, frame.height() * scale_y);
    let center = frame.center() + Vec2::new(pos_x * view_x, -pos_y * view_y);
    let local_anchor = Vec2::new((anchor_x - 0.5) * size.x, (0.5 - anchor_y) * size.y);
    let rotated_anchor = rotate(local_anchor, rotation);
    let origin = center - rotated_anchor;
    let quad = quad_around(origin, size, rotation);
    let rgb = base_rgb(clip);
    let graded = apply_grade(rgb, &grade, rel);
    let alpha = (opacity * 255.0).round() as u8;
    let fill = Color32::from_rgba_unmultiplied(
        (graded[0] * 255.0).round() as u8,
        (graded[1] * 255.0).round() as u8,
        (graded[2] * 255.0).round() as u8,
        alpha,
    );
    painter.add(Shape::convex_polygon(
        quad.clone(),
        fill,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 40)),
    ));
    if let (Some(a), Some(b)) = (quad.first(), quad.get(1)) {
        painter.line_segment(
            [*a, *b],
            Stroke::new(3.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 50)),
        );
    }
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        &clip.name,
        egui::FontId::proportional(13.0),
        Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
    );
}

fn base_rgb(clip: &editor_core::Clip) -> [f32; 3] {
    match clip.label {
        editor_core::LabelColor::Teal => [0.22, 0.55, 0.58],
        editor_core::LabelColor::Amber => [0.72, 0.48, 0.18],
        editor_core::LabelColor::Violet => [0.42, 0.32, 0.68],
        editor_core::LabelColor::Blue => [0.24, 0.42, 0.72],
        editor_core::LabelColor::Green => [0.2, 0.5, 0.36],
        editor_core::LabelColor::Rose => [0.7, 0.28, 0.36],
        editor_core::LabelColor::Neutral => [0.35, 0.4, 0.48],
    }
}

fn apply_grade(rgb: [f32; 3], grade: &ColorGrade, rel: i64) -> [f32; 3] {
    let exposure = grade.exposure.value_at(rel);
    let contrast = grade.contrast.value_at(rel);
    let highlights = grade.highlights.value_at(rel);
    let shadows = grade.shadows.value_at(rel);
    let temperature = grade.temperature.value_at(rel);
    let tint = grade.tint.value_at(rel);
    let saturation = grade.saturation.value_at(rel);
    let mut c = rgb.map(|channel| channel * 2.0_f32.powf(exposure));
    c = c.map(|channel| ((channel - 0.5) * contrast + 0.5).clamp(0.0, 1.5));
    c = c.map(|channel| {
        let shadow_w = (1.0 - channel).clamp(0.0, 1.0);
        let high_w = channel.clamp(0.0, 1.0);
        (channel + shadows * 0.35 * shadow_w + highlights * 0.35 * high_w).clamp(0.0, 1.5)
    });
    c[0] = (c[0] + temperature * 0.18).clamp(0.0, 1.5);
    c[2] = (c[2] - temperature * 0.18).clamp(0.0, 1.5);
    c[1] = (c[1] - tint * 0.14).clamp(0.0, 1.5);
    c[0] = (c[0] + tint * 0.06).clamp(0.0, 1.5);
    c[2] = (c[2] + tint * 0.06).clamp(0.0, 1.5);
    let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    c.map(|channel| (luma + (channel - luma) * saturation).clamp(0.0, 1.0))
}

fn rotate(v: Vec2, degrees: f32) -> Vec2 {
    let r = degrees.to_radians();
    let (s, c) = r.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn quad_around(center: Pos2, size: Vec2, degrees: f32) -> Vec<Pos2> {
    let half = size * 0.5;
    let corners = [
        Vec2::new(-half.x, -half.y),
        Vec2::new(half.x, -half.y),
        Vec2::new(half.x, half.y),
        Vec2::new(-half.x, half.y),
    ];
    corners
        .into_iter()
        .map(|corner| center + rotate(corner, degrees))
        .collect()
}

fn letterbox(bounds: Rect, width: f32, height: f32) -> Rect {
    if width <= 0.0 || height <= 0.0 || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return bounds;
    }
    let aspect = width / height;
    let bounds_aspect = bounds.width() / bounds.height();
    if bounds_aspect > aspect {
        let w = bounds.height() * aspect;
        Rect::from_center_size(bounds.center(), Vec2::new(w, bounds.height()))
    } else {
        let h = bounds.width() / aspect;
        Rect::from_center_size(bounds.center(), Vec2::new(bounds.width(), h))
    }
}

fn checker(painter: &Painter, rect: Rect) {
    let cell = 12.0;
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left();
        let mut col = 0;
        while x < rect.right() {
            let color = if (row + col) % 2 == 0 {
                Color32::from_rgb(28, 28, 30)
            } else {
                Color32::from_rgb(18, 18, 20)
            };
            let cell_rect = Rect::from_min_max(
                Pos2::new(x, y),
                Pos2::new((x + cell).min(rect.right()), (y + cell).min(rect.bottom())),
            );
            painter.rect_filled(cell_rect, 0.0, color);
            x += cell;
            col += 1;
        }
        y += cell;
        row += 1;
    }
}
