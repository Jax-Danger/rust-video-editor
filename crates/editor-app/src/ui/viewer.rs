//! Program monitor.
//!
//! With the `ffmpeg` feature and a readable file, every visible video layer
//! under the playhead is decoded and composited with the shared engine (the
//! same one Deliver encodes). Otherwise the monitor keeps the graded proxy
//! cards and explains why picture is missing. Active captions are burned into
//! that composite, and drawn on the proxy when decode is off.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::composite::{
    active_captions, burn_captions, composite, mask_window, transition_motion, BlitLayer,
    GradeSample, MaskWindow, PictureCache, Place,
};
use editor_core::{
    clip_relative, color_grade, transform, ColorGrade, Frame, MediaAsset, TrackKind, Transform,
};
use editor_media::{fit_preview_size, PreviewBackend};
use egui::{Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::{MeridianApp, ScrubSource};
use crate::preview::{FrameView, PreviewImage, PreviewQuery};
use crate::theme::THEME;
use crate::ui::format_tc;
use crate::ui::widgets;

const PREVIEW_MAX_W: u32 = 960;
const PREVIEW_MAX_H: u32 = 540;
const PLAY_BURST: u32 = 12;
const SCRUB_BURST: u32 = 8;
const SCRUB_H: f32 = 28.0;
const SCRUB_INSET_X: f32 = 16.0;
const WELL_PAD: f32 = 12.0;
const WELL_TC_H: f32 = 30.0;

pub fn viewer_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let Some(sequence) = app.session.project().active().cloned() else {
        ui.label("No sequence.");
        return;
    };
    follow_viewer_scrub(ui, app);
    let playhead = app.playhead;
    let playing = app.playing;
    let scrubbing = app.preview_scrub;
    let reverse = app.play_rate < 0;
    let peaks = app.audio.peaks();
    let audio_badge = app.audio.badge();
    let audio_status = app.audio.status().to_string();
    let media = app.session.project().media.clone();
    let prefer_proxies = app.session.project().prefer_proxies;
    let plan = decode_plan(
        &sequence,
        playhead,
        &media,
        playing,
        scrubbing,
        reverse,
        prefer_proxies,
    );
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
            if let Some(problem) = &plan.problem {
                banner = Some(problem.clone());
            } else if plan.layers.is_empty() {
                paint_proxy = true;
            } else {
                let mut ready = Vec::new();
                let mut waiting = false;
                for layer in &plan.layers {
                    match app.preview.request(layer.query.clone()) {
                        FrameView::Exact(image) | FrameView::Nearby(image) => ready.push(image),
                        FrameView::Pending => waiting = true,
                        FrameView::Failed(message) => banner = Some(message),
                        FrameView::Unavailable => {
                            banner = Some("ffmpeg is not available.".into());
                        }
                    }
                }
                if banner.is_none() && ready.len() == plan.layers.len() {
                    let signature = plan.signature(playhead);
                    if app
                        .picture_cache
                        .as_ref()
                        .is_none_or(|cache| cache.signature != signature)
                    {
                        let blits: Vec<BlitLayer<'_>> = ready
                            .iter()
                            .zip(plan.layers.iter())
                            .map(|(image, layer)| BlitLayer {
                                rgba: &image.rgba,
                                width: image.width,
                                height: image.height,
                                grade: layer.grade,
                                place: layer.place,
                            })
                            .collect();
                        let mut rgba = composite(
                            plan.canvas_w,
                            plan.canvas_h,
                            sequence.width as f32,
                            sequence.height as f32,
                            &blits,
                        );
                        burn_captions(&mut rgba, plan.canvas_w, plan.canvas_h, &plan.captions);
                        app.picture_cache = Some(PictureCache {
                            signature,
                            width: plan.canvas_w,
                            height: plan.canvas_h,
                            rgba,
                        });
                    }
                    if let Some(cache) = &app.picture_cache {
                        picture = Some(PreviewImage {
                            key: crate::preview::FrameKey {
                                path: format!("composite-{signature}"),
                                source_frame: playhead,
                                width: cache.width,
                                height: cache.height,
                            },
                            width: cache.width,
                            height: cache.height,
                            rgba: std::sync::Arc::from(cache.rgba.clone().into_boxed_slice()),
                        });
                        paint_proxy = false;
                    }
                } else if banner.is_none() && waiting {
                    if let Some(cache) = &app.picture_cache {
                        picture = Some(PreviewImage {
                            key: crate::preview::FrameKey {
                                path: format!("composite-{}", cache.signature),
                                source_frame: playhead,
                                width: cache.width,
                                height: cache.height,
                            },
                            width: cache.width,
                            height: cache.height,
                            rgba: std::sync::Arc::from(cache.rgba.clone().into_boxed_slice()),
                        });
                        paint_proxy = false;
                    } else {
                        banner = Some("Decoding preview…".into());
                    }
                }
                if app.preview.busy() || waiting {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(16));
                }
            }
        }
    }
    let mode = match (&backend, picture.is_some(), plan.problem.is_some()) {
        (PreviewBackend::Disabled | PreviewBackend::Unavailable(_), _, _) => "Proxy",
        (_, _, true) => "Offline",
        (PreviewBackend::Cli, true, _) => "Preview",
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
        ui.add_space(4.0);
        let resolution = if plan.used_proxy {
            "Proxy"
        } else if prefer_proxies {
            "Full*"
        } else {
            "Full"
        };
        widgets::readout(ui, resolution, 58.0, plan.used_proxy);
        ui.add_space(6.0);
        widgets::readout(ui, &format_tc(playhead, sequence.timebase), 118.0, true);
        ui.add_space(8.0);
        audio_meters(ui, peaks, audio_badge, &audio_status);
    });

    let texture = picture.as_ref().map(|image| {
        let id = app.preview.texture(ui.ctx(), image);
        (id, image.width, image.height)
    });
    let opacity = 1.0;
    let chip = (!plan.chip.is_empty() && picture.is_some()).then(|| plan.chip.clone());

    let monitor_h = (ui.available_height() - SCRUB_H).max(48.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), monitor_h), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.stage);
    let well = rect.shrink(WELL_PAD);
    painter.rect_filled(well, THEME.radius as f32, THEME.inset);
    painter.rect_stroke(
        well,
        THEME.radius as f32,
        Stroke::new(1.0_f32, THEME.border),
        egui::StrokeKind::Inside,
    );
    let tc_h = WELL_TC_H.min((well.height() * 0.16).max(22.0));
    let tc_bar = Rect::from_min_max(
        Pos2::new(well.left(), well.bottom() - tc_h),
        well.right_bottom(),
    );
    painter.hline(
        tc_bar.x_range(),
        tc_bar.top(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    let glass = Rect::from_min_max(
        well.min + Vec2::new(8.0, 8.0),
        Pos2::new(well.right() - 8.0, tc_bar.top() - 8.0),
    );
    let frame = letterbox(glass, sequence.width as f32, sequence.height as f32);
    painter.rect_filled(frame.expand(1.0), 0.0, Color32::BLACK);
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
            .filter(|t| editor_media::video_track_visible(t, &sequence.tracks))
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

    if paint_proxy {
        paint_caption_burn(&painter, frame, &active_captions(&sequence, app.playhead));
    }

    monitor_corners(&painter, frame);
    let tc = format_tc(app.playhead, sequence.timebase);
    let dur = format_tc(sequence.end_frame().0, sequence.timebase);
    painter.text(
        Pos2::new(tc_bar.left() + 12.0, tc_bar.center().y),
        egui::Align2::LEFT_CENTER,
        "TC",
        THEME.font(9.0),
        THEME.text_mute,
    );
    painter.text(
        Pos2::new(tc_bar.left() + 32.0, tc_bar.center().y),
        egui::Align2::LEFT_CENTER,
        &tc,
        THEME.mono(15.0),
        THEME.accent,
    );
    painter.text(
        Pos2::new(tc_bar.right() - 12.0, tc_bar.center().y),
        egui::Align2::RIGHT_CENTER,
        &dur,
        THEME.mono(12.0),
        THEME.text_dim,
    );
    painter.text(
        Pos2::new(tc_bar.right() - 12.0 - 108.0, tc_bar.center().y),
        egui::Align2::RIGHT_CENTER,
        "DUR",
        THEME.font(9.0),
        THEME.text_mute,
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
    program_scrubber(ui, app, sequence.end_frame().0.max(app.playhead));
}

fn follow_viewer_scrub(ui: &egui::Ui, app: &mut MeridianApp) {
    let pointer = ui.input(|input| {
        (
            input.pointer.primary_down(),
            input.pointer.primary_pressed(),
            input.pointer.interact_pos(),
        )
    });
    let (down, pressed, pos) = pointer;
    if pressed {
        if let (Some(pos), Some(bar)) = (pos, app.viewer_bar) {
            if bar.contains(pos) {
                app.scrub = Some(ScrubSource::Viewer);
            }
        }
    }
    if !down {
        if app.scrub == Some(ScrubSource::Viewer) {
            app.scrub = None;
        }
        return;
    }
    if app.scrub != Some(ScrubSource::Viewer) {
        return;
    }
    let (Some(pos), Some(bar)) = (pos, app.viewer_bar) else {
        return;
    };
    let track = scrub_track(bar);
    let track_left = track.left();
    let track_right = track.right();
    let span = (track_right - track_left).max(1.0);
    let t = ((pos.x - track_left) / span).clamp(0.0, 1.0);
    let end = app.viewer_bar_end.max(0);
    app.playhead = if end == 0 {
        0
    } else {
        (t * end as f32).round() as i64
    };
    app.preview_scrub = true;
    app.halt_transport();
}

fn scrub_track(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left() + SCRUB_INSET_X, rect.center().y - 5.0),
        Pos2::new(rect.right() - SCRUB_INSET_X, rect.center().y + 5.0),
    )
}

fn monitor_corners(painter: &Painter, frame: Rect) {
    let len = 9.0;
    let stroke = Stroke::new(1.0_f32, THEME.text_mute);
    let marks = [
        (frame.left_top(), Vec2::new(len, 0.0), Vec2::new(0.0, len)),
        (frame.right_top(), Vec2::new(-len, 0.0), Vec2::new(0.0, len)),
        (
            frame.left_bottom(),
            Vec2::new(len, 0.0),
            Vec2::new(0.0, -len),
        ),
        (
            frame.right_bottom(),
            Vec2::new(-len, 0.0),
            Vec2::new(0.0, -len),
        ),
    ];
    for (origin, horizontal, vertical) in marks {
        painter.line_segment([origin, origin + horizontal], stroke);
        painter.line_segment([origin, origin + vertical], stroke);
    }
}

fn program_scrubber(ui: &mut egui::Ui, app: &mut MeridianApp, end: i64) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), SCRUB_H),
        Sense::click_and_drag(),
    );
    let bar = scrub_track(rect);
    app.viewer_bar = Some(rect);
    app.viewer_bar_end = end.max(0);
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, THEME.header);
    painter.hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    painter.rect_filled(bar, 2.0, THEME.inset);
    if let Some(sequence) = app.session.project().active() {
        if let (Some(inn), Some(out)) = (sequence.in_point, sequence.out_point) {
            if end > 0 {
                let x0 = bar.min.x + inn.0 as f32 / end as f32 * bar.width();
                let x1 = bar.min.x + out.0 as f32 / end as f32 * bar.width();
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(x0, bar.min.y),
                        Pos2::new(x1.max(x0 + 2.0), bar.max.y),
                    ),
                    2.0,
                    THEME.accent_dim,
                );
            }
        }
    }
    let t = if end <= 0 {
        0.0
    } else {
        (app.playhead as f32 / end as f32).clamp(0.0, 1.0)
    };
    let x = bar.min.x + t * bar.width();
    painter.rect_filled(
        Rect::from_min_max(bar.min, Pos2::new(x, bar.max.y)),
        2.0,
        Color32::from_white_alpha(28),
    );
    painter.vline(
        x,
        bar.y_range().expand(3.0),
        Stroke::new(2.0_f32, THEME.playhead),
    );
    painter.circle_filled(Pos2::new(x, bar.center().y), 6.0, THEME.playhead);
    if response.hovered() || response.dragged() {
        response
            .clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.clone().on_hover_text("Drag to scrub the program");
    }
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let pointer_down = ui.input(|input| input.pointer.primary_down());
    let over = pointer.is_some_and(|pos| rect.contains(pos)) && pointer_down;
    if over || response.is_pointer_button_down_on() {
        app.scrub = Some(ScrubSource::Viewer);
        if let Some(pos) = pointer.or(response.interact_pointer_pos()) {
            let span = bar.width().max(1.0);
            let local = ((pos.x - bar.min.x) / span).clamp(0.0, 1.0);
            app.playhead = if end <= 0 {
                0
            } else {
                (local * end as f32).round() as i64
            };
            app.preview_scrub = true;
            app.halt_transport();
        }
    }
}

fn audio_meters(ui: &mut egui::Ui, peaks: [f32; 2], badge: &str, status: &str) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(28.0, 18.0), Sense::hover());
    let painter = ui.painter();
    for (index, peak) in peaks.iter().enumerate() {
        let x = rect.left() + index as f32 * 8.0;
        let column = Rect::from_min_size(Pos2::new(x, rect.top() + 1.0), Vec2::new(5.0, 16.0));
        painter.rect_filled(column, 1.0, THEME.inset);
        let level = peak.clamp(0.0, 1.0);
        let fill_h = column.height() * level;
        let color = if level > 0.92 {
            THEME.danger
        } else if level > 0.7 {
            THEME.amber
        } else {
            THEME.audio
        };
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(column.left(), column.bottom() - fill_h),
                column.right_bottom(),
            ),
            1.0,
            color,
        );
    }
    response.on_hover_text(format!("{badge} — {status}"));
    ui.label(egui::RichText::new(badge).size(11.0).color(THEME.text_mute));
}

struct DecodePlan {
    layers: Vec<DecodedLayer>,
    problem: Option<String>,
    chip: String,
    canvas_w: u32,
    canvas_h: u32,
    captions: Vec<String>,
    used_proxy: bool,
}

impl DecodePlan {
    fn signature(&self, playhead: i64) -> u64 {
        let mut hasher = DefaultHasher::new();
        playhead.hash(&mut hasher);
        self.canvas_w.hash(&mut hasher);
        self.canvas_h.hash(&mut hasher);
        for layer in &self.layers {
            layer.query.path.hash(&mut hasher);
            layer.query.source_frame.hash(&mut hasher);
            layer.query.width.hash(&mut hasher);
            layer.query.height.hash(&mut hasher);
            bits(layer.grade.exposure).hash(&mut hasher);
            bits(layer.grade.contrast).hash(&mut hasher);
            bits(layer.grade.highlights).hash(&mut hasher);
            bits(layer.grade.shadows).hash(&mut hasher);
            bits(layer.grade.temperature).hash(&mut hasher);
            bits(layer.grade.tint).hash(&mut hasher);
            bits(layer.grade.saturation).hash(&mut hasher);
            bits(layer.place.scale_x).hash(&mut hasher);
            bits(layer.place.scale_y).hash(&mut hasher);
            bits(layer.place.pos_x).hash(&mut hasher);
            bits(layer.place.pos_y).hash(&mut hasher);
            bits(layer.place.rotation).hash(&mut hasher);
            bits(layer.place.opacity).hash(&mut hasher);
            bits(layer.place.shift_x).hash(&mut hasher);
            bits(layer.place.shift_y).hash(&mut hasher);
            bits(layer.place.anchor_x).hash(&mut hasher);
            bits(layer.place.anchor_y).hash(&mut hasher);
            match layer.place.mask {
                crate::composite::CanvasMask::None => 0u8.hash(&mut hasher),
                crate::composite::CanvasMask::Wipe {
                    angle_deg,
                    edge,
                    keep_below,
                } => {
                    1u8.hash(&mut hasher);
                    bits(angle_deg).hash(&mut hasher);
                    bits(edge).hash(&mut hasher);
                    keep_below.hash(&mut hasher);
                }
            }
            layer.label.hash(&mut hasher);
        }
        for line in &self.captions {
            line.hash(&mut hasher);
        }
        hasher.finish()
    }
}

struct DecodedLayer {
    query: PreviewQuery,
    grade: GradeSample,
    place: Place,
    label: String,
}

fn bits(value: f32) -> u32 {
    value.to_bits()
}

fn decode_plan(
    sequence: &editor_core::Sequence,
    playhead: i64,
    media: &[MediaAsset],
    playing: bool,
    scrubbing: bool,
    reverse: bool,
    prefer_proxies: bool,
) -> DecodePlan {
    let (canvas_w, canvas_h) = fit_preview_size(
        sequence.width.max(2),
        sequence.height.max(2),
        PREVIEW_MAX_W,
        PREVIEW_MAX_H,
    )
    .unwrap_or((PREVIEW_MAX_W, PREVIEW_MAX_H));
    let source = if prefer_proxies {
        editor_media::PreviewSource::Proxy
    } else {
        editor_media::PreviewSource::Full
    };
    let stack =
        editor_media::program_stack_with(sequence, media, playhead, canvas_w, canvas_h, source);
    let used_proxy = stack.layers.iter().any(|layer| layer.using_proxy);
    let problem = if stack.layers.is_empty() {
        stack.errors.into_iter().next()
    } else {
        None
    };
    let burst = if playing && !scrubbing {
        PLAY_BURST
    } else {
        SCRUB_BURST
    };
    let lead = if scrubbing || reverse { burst / 3 } else { 0 };
    let chip = stack
        .layers
        .last()
        .map(|layer| layer.label.clone())
        .unwrap_or_default();
    let layers = stack
        .layers
        .into_iter()
        .filter(|layer| layer.place.contributes())
        .map(|layer| DecodedLayer {
            query: PreviewQuery {
                path: layer.path,
                source_frame: layer.source_frame,
                width: layer.width,
                height: layer.height,
                time_secs: layer.time_secs,
                frame_secs: layer.frame_secs,
                last_source_frame: layer.last_source_frame,
                burst,
                lead,
            },
            grade: layer.grade,
            place: layer.place,
            label: layer.label,
        })
        .collect();
    DecodePlan {
        layers,
        problem,
        chip,
        canvas_w,
        canvas_h,
        captions: active_captions(sequence, playhead),
        used_proxy,
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
    let view_x = frame.width() / w.max(1.0);
    let view_y = frame.height() / h.max(1.0);
    for (clip, outgoing) in [(hit.left, true), (hit.right, false)] {
        let motion = transition_motion(&hit.kind, hit.progress, outgoing, w, h);
        let shifted = frame.translate(Vec2::new(motion.shift_x * view_x, -motion.shift_y * view_y));
        let window = match mask_window(motion.mask) {
            MaskWindow::Empty => continue,
            MaskWindow::All | MaskWindow::PerPixel => frame,
            MaskWindow::Uv { u0, v0, u1, v1 } => Rect::from_min_max(
                Pos2::new(
                    frame.left() + u0.clamp(0.0, 1.0) * frame.width(),
                    frame.top() + v0.clamp(0.0, 1.0) * frame.height(),
                ),
                Pos2::new(
                    frame.left() + u1.clamp(0.0, 1.0) * frame.width(),
                    frame.top() + v1.clamp(0.0, 1.0) * frame.height(),
                ),
            ),
        };
        if window.width() < 1.0 || window.height() < 1.0 {
            continue;
        }
        paint_clip_clipped(
            painter,
            shifted,
            window,
            w,
            h,
            clip,
            motion.opacity_scale,
            playhead,
        );
    }
}

fn paint_caption_burn(painter: &Painter, frame: Rect, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let margin = (frame.height() * 48.0 / 1080.0).clamp(8.0, 72.0);
    let size = (frame.height() * 32.0 / 1080.0).clamp(13.0, 42.0);
    let font = FontId::proportional(size);
    let mut baseline = frame.bottom() - margin;
    for line in lines.iter().rev() {
        let pos = Pos2::new(frame.center().x, baseline);
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            painter.text(
                pos + Vec2::new(dx, dy),
                egui::Align2::CENTER_BOTTOM,
                line,
                font.clone(),
                Color32::BLACK,
            );
        }
        painter.text(
            pos,
            egui::Align2::CENTER_BOTTOM,
            line,
            font.clone(),
            Color32::WHITE,
        );
        baseline -= size + 4.0;
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
    GradeSample::from_grade(grade, rel).apply(rgb)
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
