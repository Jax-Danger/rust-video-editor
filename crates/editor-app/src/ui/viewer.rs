//! Program monitor.
//!
//! With the `ffmpeg` feature and a readable file, the topmost visible video
//! clip is a decoded frame. Otherwise the monitor keeps the graded proxy cards
//! and explains why picture is missing.

use editor_core::{
    clip_relative, color_grade, source_frame_at, transform, ColorGrade, Frame, MediaAsset,
    Timebase, TrackKind, Transform,
};
use editor_media::{clamp_preview_time, fit_preview_size, resolve_media_path, PreviewBackend};
use egui::{Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::preview::{FrameView, PreviewImage, PreviewQuery};
use crate::theme::THEME;
use crate::ui::format_tc;
use crate::ui::widgets;

const PREVIEW_MAX_W: u32 = 960;
const PREVIEW_MAX_H: u32 = 540;
const PLAY_BURST: u32 = 12;

pub fn viewer_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let Some(sequence) = app.session.project().active().cloned() else {
        ui.label("No sequence.");
        return;
    };
    let playhead = app.playhead;
    let playing = app.playing;
    let media = app.session.project().media.clone();
    let source = top_picture(&sequence, playhead, &media);
    let backend = app.preview.backend().clone();

    let mut banner: Option<String> = None;
    let mut picture: Option<PreviewImage> = None;
    let mut paint_proxy = true;
    match &backend {
        PreviewBackend::Disabled => {
            banner = Some(
                "Decoded preview is off. Run cargo run -p editor-app --features ffmpeg.".into(),
            );
        }
        PreviewBackend::Unavailable(message) => {
            banner = Some(format!("ffmpeg is not available — {message}"));
        }
        PreviewBackend::Cli => {
            if let Some(source) = &source {
                if let Some(problem) = &source.problem {
                    banner = Some(problem.clone());
                } else {
                    let query = source.query(playing);
                    match app.preview.request(query) {
                        FrameView::Exact(image) | FrameView::Nearby(image) => {
                            picture = Some(image);
                            paint_proxy = false;
                        }
                        FrameView::Pending => {
                            banner = Some("Decoding preview…".into());
                        }
                        FrameView::Failed(message) => banner = Some(message),
                        FrameView::Unavailable => {
                            banner = Some("ffmpeg is not available.".into());
                        }
                    }
                    if app.preview.busy() {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(16));
                    }
                }
            }
        }
    }
    let mode = match (&backend, picture.is_some(), source.as_ref()) {
        (PreviewBackend::Disabled | PreviewBackend::Unavailable(_), _, _) => "Proxy",
        (_, _, Some(source)) if source.problem.is_some() => "Offline",
        (PreviewBackend::Cli, _, Some(_)) => "Preview",
        _ => "Empty",
    };

    widgets::panel_header(ui, "Program", |ui| {
        ui.label(
            egui::RichText::new(&sequence.name)
                .size(12.0)
                .color(THEME.text_dim),
        );
        ui.add_space(8.0);
        widgets::readout(
            ui,
            &format!("{}×{}", sequence.width, sequence.height),
            92.0,
            false,
        );
        ui.add_space(6.0);
        widgets::readout(ui, mode, 78.0, mode == "Preview");
        ui.add_space(6.0);
        widgets::readout(ui, &format_tc(playhead, sequence.timebase), 118.0, true);
    });

    let texture = picture.as_ref().map(|image| {
        let id = app.preview.texture(ui.ctx(), image);
        (id, image.width, image.height)
    });
    let opacity = source.as_ref().map(|item| item.opacity).unwrap_or(1.0);
    let chip = source
        .as_ref()
        .filter(|_| picture.is_some())
        .map(|item| format!("{}  {}", item.track_name, item.clip_name));

    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.stage);
    let frame = letterbox(
        rect.shrink(18.0),
        sequence.width as f32,
        sequence.height as f32,
    );
    painter.rect_stroke(
        frame.expand(1.0),
        0.0,
        Stroke::new(1.0_f32, THEME.border),
        egui::StrokeKind::Outside,
    );
    checker(&painter, frame);
    painter.rect_filled(frame, 0.0, Color32::BLACK);

    if paint_proxy {
        let video: Vec<_> = sequence
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .filter(|t| track_visible(t, &sequence.tracks))
            .collect();
        for track in video {
            if let Some(hit) = transition_hit(track, playhead) {
                paint_transition(&painter, frame, &sequence, hit, playhead);
            } else if let Some(clip) = track
                .clips
                .iter()
                .find(|c| c.covers(Frame(playhead)) && c.enabled)
            {
                paint_clip(
                    &painter,
                    frame,
                    sequence.width as f32,
                    sequence.height as f32,
                    clip,
                    1.0,
                    playhead,
                );
            }
        }
    } else if let Some((texture, width, height)) = texture {
        paint_decoded(&painter, frame, texture, width, height, opacity);
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
        painter.rect_filled(box_rect, 3.0, Color32::from_rgba_unmultiplied(0, 0, 0, 180));
        painter.text(
            box_rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            FontId::new(15.0, egui::FontFamily::Proportional),
            Color32::WHITE,
        );
    }

    let tc = format_tc(app.playhead, sequence.timebase);
    let pill = Rect::from_min_size(
        frame.left_top() + Vec2::new(8.0, 8.0),
        Vec2::new(108.0, 20.0),
    );
    painter.rect_filled(pill, 3.0, Color32::from_rgba_unmultiplied(0, 0, 0, 160));
    painter.text(
        pill.center(),
        egui::Align2::CENTER_CENTER,
        tc,
        FontId::monospace(12.0),
        Color32::from_rgb(255, 214, 160),
    );
    if let Some(label) = chip {
        let chip_rect = Rect::from_min_size(
            Pos2::new(frame.right() - 148.0, frame.top() + 8.0),
            Vec2::new(140.0, 20.0),
        );
        painter.rect_filled(
            chip_rect,
            3.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 160),
        );
        painter.text(
            chip_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(11.0),
            THEME.accent,
        );
    }
    if let Some(text) = banner {
        overlay_note(&painter, frame, &text);
    }
}

struct PictureSource {
    track_name: String,
    clip_name: String,
    path: String,
    source_frame: i64,
    time_secs: f64,
    frame_secs: f64,
    width: u32,
    height: u32,
    last_source_frame: i64,
    opacity: f32,
    problem: Option<String>,
}

impl PictureSource {
    fn query(&self, playing: bool) -> PreviewQuery {
        PreviewQuery {
            path: self.path.clone(),
            source_frame: self.source_frame,
            width: self.width,
            height: self.height,
            time_secs: self.time_secs,
            frame_secs: self.frame_secs,
            last_source_frame: self.last_source_frame,
            burst: if playing { PLAY_BURST } else { 1 },
        }
    }
}

fn top_picture(
    sequence: &editor_core::Sequence,
    playhead: i64,
    media: &[MediaAsset],
) -> Option<PictureSource> {
    let video: Vec<_> = sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .collect();
    for track in video.into_iter().rev() {
        if !track_visible(track, &sequence.tracks) {
            continue;
        }
        let Some(clip) = track
            .clips
            .iter()
            .find(|clip| clip.enabled && clip.covers(Frame(playhead)))
        else {
            continue;
        };
        let rel = clip_relative(Frame(playhead), clip.timeline_in);
        let xform = transform(&clip.effects)
            .cloned()
            .unwrap_or_else(Transform::identity);
        let opacity = xform.opacity.value_at(rel).clamp(0.0, 1.0);
        if opacity <= 0.001 {
            continue;
        }
        let Some(media_id) = clip.media_id else {
            return Some(unreadable(
                track,
                clip,
                opacity,
                format!("No media linked to {}", clip.name),
            ));
        };
        let Some(asset) = media.iter().find(|item| item.id == media_id) else {
            return Some(unreadable(
                track,
                clip,
                opacity,
                format!("Missing media for {}", clip.name),
            ));
        };
        if !asset.has_video {
            return Some(unreadable(
                track,
                clip,
                opacity,
                format!("{} has no picture", asset.name),
            ));
        }
        let resolved = resolve_media_path(&asset.path);
        if !resolved.is_file() {
            return Some(unreadable(
                track,
                clip,
                opacity,
                format!("Offline — {} is not on disk", asset.name),
            ));
        }
        let (src_w, src_h) = match (asset.width, asset.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
            _ => (PREVIEW_MAX_W, PREVIEW_MAX_H),
        };
        let (width, height) = fit_preview_size(src_w, src_h, PREVIEW_MAX_W, PREVIEW_MAX_H)
            .unwrap_or((
                PREVIEW_MAX_W.min(src_w).max(2),
                PREVIEW_MAX_H.min(src_h).max(2),
            ));
        let mut source_frame = source_frame_at(clip, Frame(playhead), sequence.timebase)
            .0
            .max(0);
        let last_source_frame = asset.duration.0.saturating_sub(1).max(0);
        if source_frame > last_source_frame {
            source_frame = last_source_frame;
        }
        let duration_secs = asset.duration.to_seconds(asset.timebase);
        let raw_time = Frame(source_frame).to_seconds(clip.media_timebase);
        let time_secs = clamp_preview_time(raw_time, duration_secs).unwrap_or(0.0);
        let frame_secs = clip.media_timebase.frame_duration_secs();
        return Some(PictureSource {
            track_name: track.name.clone(),
            clip_name: clip.name.clone(),
            path: resolved.to_string_lossy().into_owned(),
            source_frame,
            time_secs,
            frame_secs,
            width,
            height,
            last_source_frame,
            opacity,
            problem: None,
        });
    }
    None
}

fn unreadable(
    track: &editor_core::Track,
    clip: &editor_core::Clip,
    opacity: f32,
    problem: String,
) -> PictureSource {
    PictureSource {
        track_name: track.name.clone(),
        clip_name: clip.name.clone(),
        path: String::new(),
        source_frame: 0,
        time_secs: 0.0,
        frame_secs: Timebase::fps_24().frame_duration_secs(),
        width: 2,
        height: 2,
        last_source_frame: 0,
        opacity,
        problem: Some(problem),
    }
}

fn paint_decoded(
    painter: &Painter,
    frame: Rect,
    texture: egui::TextureId,
    width: u32,
    height: u32,
    opacity: f32,
) {
    let dest = letterbox(frame, width as f32, height as f32);
    let alpha = (opacity * 255.0).round().clamp(0.0, 255.0) as u8;
    let tint = Color32::from_white_alpha(alpha);
    let painter = painter.with_clip_rect(frame);
    painter.image(
        texture,
        dest,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        tint,
    );
}

fn overlay_note(painter: &Painter, frame: Rect, text: &str) {
    let galley = painter.layout(
        text.to_owned(),
        FontId::proportional(13.0),
        THEME.text,
        (frame.width() - 36.0).max(40.0),
    );
    let pad = Vec2::new(12.0, 7.0);
    let size = galley.size() + pad * 2.0;
    let rect = Rect::from_center_size(
        Pos2::new(frame.center().x, frame.top() + 40.0 + size.y * 0.5),
        size,
    );
    painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(8, 10, 12, 220));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0_f32, THEME.border),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, THEME.text);
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
