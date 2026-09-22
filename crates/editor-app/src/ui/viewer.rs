//! Program and source monitors.
//!
//! With the `ffmpeg` feature and a readable file, every visible video layer
//! under the playhead is decoded and composited with the shared engine (the
//! same one Deliver encodes). Otherwise the monitor keeps the graded proxy
//! cards and explains why picture is missing. Active captions are burned into
//! that composite, and drawn on the proxy when decode is off.
//!
//! The Edit workspace shows a source monitor beside the program monitor. The
//! source plays the selected pool clip with its own playhead and in/out marks;
//! Overwrite / Insert place that marked range at the program playhead.
//!
//! The program picture is the CPU composite. When that buffer is ready it is
//! uploaded to one GPU texture for display (see `gpu_display`). Export does
//! not read the texture.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::composite::{
    active_captions, burn_captions, compose_layers, compose_layers_env, mask_window,
    media_layers_in_stack, transition_motion, ComposeEnv, FilterSample, GradeSample, LayerSource,
    MaskWindow, PictureCache, Place, ProgramLayer, StabilizeSample,
};
use editor_core::{
    active_angle, clip_relative, color_grade, multicam_target, source_frame_at, transform,
    ColorGrade, Frame, MediaAsset, MulticamGroup, TrackKind, Transform,
};
use editor_media::{fit_preview_size, preview_file, PreviewBackend, PreviewSource};
use egui::{Align, Align2, Color32, FontId, Layout, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::{MeridianApp, MonitorFocus, ScrubSource};
use crate::gpu_display::UploadedFrame;
use crate::preview::{FrameKey, FrameView, PreviewImage, PreviewQuery};
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
const MONITOR_GAP: f32 = 6.0;

/// Side-by-side source and program monitors for the Edit workspace.
pub fn dual_monitor_panel(ui: &mut egui::Ui, app: &mut MeridianApp, host: &mut eframe::Frame) {
    let total = ui.available_width();
    let height = ui.available_height();
    let half = ((total - MONITOR_GAP) * 0.5).max(160.0);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(half, height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_min_height(height);
                source_viewer_panel(ui, app);
            },
        );
        ui.add_space(MONITOR_GAP);
        ui.allocate_ui_with_layout(
            Vec2::new((total - half - MONITOR_GAP).max(160.0), height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_min_height(height);
                viewer_panel(ui, app, host);
            },
        );
    });
}

pub fn source_viewer_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    follow_source_scrub(ui, app);
    let focused = app.focused_monitor == MonitorFocus::Source;
    let media = app
        .selected_media
        .and_then(|id| app.session.project().media(id).cloned());
    let prefer_proxies = app.session.project().prefer_proxies;
    let backend = app.preview.backend().clone();
    let playing = app.source_playing;
    let scrubbing = app.preview_scrub;
    let reverse = app.source_play_rate < 0;

    let Some(media) = media else {
        widgets::panel_header(ui, "Source", |ui| {
            focus_badge(ui, focused);
            ui.label(
                egui::RichText::new("No clip")
                    .size(12.0)
                    .color(THEME.text_dim),
            );
        });
        empty_monitor(ui, "Select a pool clip, or double-click one to open it here.");
        claim_focus_on_click(ui, app, MonitorFocus::Source);
        return;
    };

    let marks = app.source_marks_for(media.id);
    let playhead = marks.playhead.clamp(0, media.duration.0.max(0));
    let end = media.duration.0.max(0);
    let (mut banner, picture, mode, used_proxy, _canvas) =
        source_picture(ui, app, &media, playhead, prefer_proxies, playing, scrubbing, reverse);

    let header_name = media.name.clone();
    let timebase = media.timebase;
    let size_label = match (media.width, media.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "—".into(),
    };

    widgets::panel_header(ui, "Source", |ui| {
        focus_badge(ui, focused);
        ui.label(
            egui::RichText::new(&header_name)
                .size(12.0)
                .color(THEME.text_dim),
        );
        ui.add_space(8.0);
        widgets::readout(ui, &size_label, 92.0, false);
        ui.add_space(6.0);
        widgets::readout(ui, mode, 78.0, mode == "Preview");
        ui.add_space(4.0);
        let resolution = if used_proxy {
            "Proxy"
        } else if prefer_proxies {
            "Full*"
        } else {
            "Full"
        };
        widgets::readout(ui, resolution, 58.0, used_proxy);
        ui.add_space(6.0);
        widgets::readout(ui, &format_tc(playhead, timebase), 118.0, true);
    });

    let monitor_h = (ui.available_height() - SCRUB_H).max(48.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), monitor_h), Sense::click());
    if response.clicked() {
        app.focus_monitor(MonitorFocus::Source);
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.stage);
    if focused {
        painter.rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(1.5_f32, THEME.accent),
            egui::StrokeKind::Inside,
        );
    }
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
    let src_w = media.width.unwrap_or(1920) as f32;
    let src_h = media.height.unwrap_or(1080) as f32;
    let frame = letterbox(glass, src_w, src_h);
    painter.rect_filled(frame.expand(1.0), 0.0, Color32::BLACK);
    painter.rect_stroke(
        frame.expand(1.0),
        0.0,
        Stroke::new(1.0_f32, THEME.border),
        egui::StrokeKind::Outside,
    );
    checker(&painter, frame);
    painter.rect_filled(frame, 0.0, Color32::BLACK);

    if let Some(image) = &picture {
        let texture = app.preview.texture(ui.ctx(), image);
        paint_decoded(&painter, frame, texture, image.width, image.height, 1.0);
    } else if media.has_video {
        paint_source_proxy(&painter, frame, &media, playhead);
        if banner.is_none() && matches!(backend, PreviewBackend::Disabled) {
            banner = Some(
                "Decoded preview is off. Run cargo run -p editor-app --features ffmpeg.".into(),
            );
        }
    } else {
        banner = Some("Audio-only clip — mark in/out, then Overwrite or Insert.".into());
    }

    monitor_corners(&painter, frame);
    let tc = format_tc(playhead, timebase);
    let dur = format_tc(end, timebase);
    painter.text(
        Pos2::new(tc_bar.left() + 12.0, tc_bar.center().y),
        egui::Align2::LEFT_CENTER,
        "SRC",
        THEME.font(9.0),
        THEME.text_mute,
    );
    painter.text(
        Pos2::new(tc_bar.left() + 40.0, tc_bar.center().y),
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
    if let Some(text) = banner {
        overlay_note(&painter, frame, &text);
    }

    source_scrubber(ui, app, media.id, end, marks.in_point, marks.out_point);
}

fn source_picture(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    media: &MediaAsset,
    playhead: i64,
    prefer_proxies: bool,
    playing: bool,
    scrubbing: bool,
    reverse: bool,
) -> (
    Option<String>,
    Option<PreviewImage>,
    &'static str,
    bool,
    (u32, u32),
) {
    let src_w = media.width.unwrap_or(1920).max(2);
    let src_h = media.height.unwrap_or(1080).max(2);
    let (canvas_w, canvas_h) =
        fit_preview_size(src_w, src_h, PREVIEW_MAX_W, PREVIEW_MAX_H).unwrap_or((960, 540));
    let source = if prefer_proxies {
        PreviewSource::Proxy
    } else {
        PreviewSource::Full
    };
    let (path, used_proxy) = preview_file(media, source);
    let backend = app.preview.backend().clone();
    let mut banner = None;
    let mut picture = None;
    let mode = match &backend {
        PreviewBackend::Disabled => {
            banner = Some(
                "Decoded preview is off. Run cargo run -p editor-app --features ffmpeg.".into(),
            );
            "Proxy"
        }
        PreviewBackend::Unavailable(message) => {
            banner = Some(format!("ffmpeg is not available — {message}"));
            "Proxy"
        }
        PreviewBackend::Cli => {
            if !media.has_video {
                "Empty"
            } else if !path.is_file() {
                banner = Some(format!("Offline — {} is not on disk", media.name));
                "Offline"
            } else {
                let last_source_frame = media.duration.0.saturating_sub(1).max(0);
                let source_frame = playhead.clamp(0, last_source_frame);
                let duration_secs = media.duration.to_seconds(media.timebase);
                let raw_time = Frame(source_frame).to_seconds(media.timebase);
                let time_secs = editor_media::clamp_preview_time(raw_time, duration_secs)
                    .unwrap_or(0.0);
                let burst = if playing && !scrubbing {
                    PLAY_BURST
                } else {
                    SCRUB_BURST
                };
                let lead = if scrubbing || reverse { burst / 3 } else { 0 };
                let query = PreviewQuery {
                    path: path.to_string_lossy().into_owned(),
                    source_frame,
                    width: canvas_w,
                    height: canvas_h,
                    time_secs,
                    frame_secs: media.timebase.frame_duration_secs().max(1.0e-4),
                    last_source_frame,
                    burst,
                    lead,
                };
                match app.preview.request(query) {
                    FrameView::Exact(image) | FrameView::Nearby(image) => {
                        picture = Some(image);
                    }
                    FrameView::Pending => {
                        banner = Some("Decoding preview…".into());
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(16));
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
                if picture.is_some() {
                    "Preview"
                } else if banner.as_deref() == Some("Decoding preview…") {
                    "Preview"
                } else {
                    "Offline"
                }
            }
        }
    };
    (banner, picture, mode, used_proxy, (canvas_w, canvas_h))
}

fn paint_source_proxy(painter: &Painter, frame: Rect, media: &MediaAsset, playhead: i64) {
    let hue = ((media.id.0 as u32).wrapping_mul(47) % 360) as f32;
    let color = Color32::from_rgb(
        ((hue / 360.0) * 80.0 + 40.0) as u8,
        70,
        ((1.0 - hue / 360.0) * 90.0 + 50.0) as u8,
    );
    painter.rect_filled(frame, 0.0, color);
    painter.text(
        frame.center(),
        egui::Align2::CENTER_CENTER,
        format!("{}\n{}", media.name, format_tc(playhead, media.timebase)),
        THEME.font(14.0),
        THEME.text,
    );
}

fn empty_monitor(ui: &mut egui::Ui, note: &str) {
    let monitor_h = (ui.available_height() - SCRUB_H).max(48.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), monitor_h), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.stage);
    let well = rect.shrink(WELL_PAD);
    painter.rect_filled(well, THEME.radius as f32, THEME.inset);
    overlay_note(&painter, well.shrink(16.0), note);
    let _ = ui.allocate_exact_size(Vec2::new(ui.available_width(), SCRUB_H), Sense::hover());
}

fn focus_badge(ui: &mut egui::Ui, focused: bool) {
    if focused {
        widgets::readout(ui, "FOCUS", 56.0, true);
        ui.add_space(6.0);
    }
}

fn claim_focus_on_click(ui: &mut egui::Ui, app: &mut MeridianApp, focus: MonitorFocus) {
    if ui.input(|input| {
        input.pointer.any_click()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| ui.max_rect().contains(pos))
    }) {
        app.focus_monitor(focus);
    }
}

fn follow_source_scrub(ui: &egui::Ui, app: &mut MeridianApp) {
    let pointer = ui.input(|input| {
        (
            input.pointer.primary_down(),
            input.pointer.primary_pressed(),
            input.pointer.interact_pos(),
        )
    });
    let (down, pressed, pos) = pointer;
    if pressed {
        if let (Some(pos), Some(bar)) = (pos, app.source_bar) {
            if bar.contains(pos) {
                app.scrub = Some(ScrubSource::Source);
                app.focus_monitor(MonitorFocus::Source);
            }
        }
    }
    if !down {
        if app.scrub == Some(ScrubSource::Source) {
            app.scrub = None;
        }
        return;
    }
    if app.scrub != Some(ScrubSource::Source) {
        return;
    }
    let (Some(pos), Some(bar)) = (pos, app.source_bar) else {
        return;
    };
    let track = scrub_track(bar);
    let span = (track.right() - track.left()).max(1.0);
    let t = ((pos.x - track.left()) / span).clamp(0.0, 1.0);
    let end = app.source_bar_end.max(0);
    let frame = if end == 0 {
        0
    } else {
        (t * end as f32).round() as i64
    };
    app.set_source_playhead(frame);
    app.preview_scrub = true;
    app.halt_source_transport();
}

fn source_scrubber(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    media_id: editor_core::MediaId,
    end: i64,
    in_point: Option<i64>,
    out_point: Option<i64>,
) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), SCRUB_H),
        Sense::click_and_drag(),
    );
    let bar = scrub_track(rect);
    app.source_bar = Some(rect);
    app.source_bar_end = end.max(0);
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, THEME.header);
    painter.hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    painter.rect_filled(bar, 2.0, THEME.inset);
    if let (Some(inn), Some(out)) = (in_point, out_point) {
        if end > 0 && out > inn {
            let x0 = bar.min.x + inn as f32 / end as f32 * bar.width();
            let x1 = bar.min.x + out as f32 / end as f32 * bar.width();
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
    let playhead = app.source_marks_for(media_id).playhead;
    let t = if end <= 0 {
        0.0
    } else {
        (playhead as f32 / end as f32).clamp(0.0, 1.0)
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
        response.clone().on_hover_text("Drag to scrub the source");
    }
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let pointer_down = ui.input(|input| input.pointer.primary_down());
    let over = pointer.is_some_and(|pos| rect.contains(pos)) && pointer_down;
    if over || response.is_pointer_button_down_on() {
        app.scrub = Some(ScrubSource::Source);
        app.focus_monitor(MonitorFocus::Source);
        if let Some(pos) = pointer.or(response.interact_pointer_pos()) {
            let span = bar.width().max(1.0);
            let local = ((pos.x - bar.min.x) / span).clamp(0.0, 1.0);
            let frame = if end <= 0 {
                0
            } else {
                (local * end as f32).round() as i64
            };
            app.set_source_playhead(frame);
            app.preview_scrub = true;
            app.halt_source_transport();
        }
    }
}

pub fn viewer_panel(ui: &mut egui::Ui, app: &mut MeridianApp, host: &mut eframe::Frame) {
    let Some(sequence) = app.session.project().active().cloned() else {
        ui.label("No sequence.");
        return;
    };
    follow_viewer_scrub(ui, app);
    let focused = app.focused_monitor == MonitorFocus::Program;
    let playhead = app.playhead;
    let playing = app.playing;
    let scrubbing = app.preview_scrub;
    let reverse = app.play_rate < 0;
    let peaks = app.audio.peaks();
    let audio_badge = app.audio.badge();
    let audio_status = app.audio.status().to_string();
    let media = app.session.project().media.clone();
    let groups = app.session.project().multicam_groups.clone();
    let sequences = app.session.project().sequences.clone();
    let prefer_proxies = app.session.project().prefer_proxies;
    let bank = angle_bank(&sequence, &groups, &app.selected, playhead);
    let plan = decode_plan(
        &sequence,
        &sequences,
        playhead,
        &media,
        &groups,
        playing,
        scrubbing,
        reverse,
        prefer_proxies,
    );
    let backend = app.preview.backend().clone();

    let mut banner: Option<String> = None;
    let mut have_picture = false;
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
                for query in &plan.media_queries {
                    match app.preview.request(query.clone()) {
                        FrameView::Exact(image) | FrameView::Nearby(image) => ready.push(image),
                        FrameView::Pending => waiting = true,
                        FrameView::Failed(message) => banner = Some(message),
                        FrameView::Unavailable => {
                            banner = Some("ffmpeg is not available.".into());
                        }
                    }
                }
                let titles_only = plan.media_queries.is_empty();
                if banner.is_none()
                    && (titles_only || ready.len() == plan.media_queries.len())
                {
                    let signature = plan.signature(playhead);
                    if app
                        .picture_cache
                        .as_ref()
                        .is_none_or(|cache| cache.signature != signature)
                    {
                        match compose_plan(
                            &plan,
                            sequence.width as f32,
                            sequence.height as f32,
                            &ready,
                        ) {
                            Ok(mut rgba) => {
                                burn_captions(
                                    &mut rgba,
                                    plan.canvas_w,
                                    plan.canvas_h,
                                    &plan.captions,
                                );
                                app.picture_cache = Some(PictureCache {
                                    signature,
                                    width: plan.canvas_w,
                                    height: plan.canvas_h,
                                    rgba,
                                });
                            }
                            Err(message) => banner = Some(message),
                        }
                    }
                    if app.picture_cache.is_some() {
                        have_picture = true;
                        paint_proxy = false;
                    }
                } else if banner.is_none() && waiting {
                    if app.picture_cache.is_some() {
                        have_picture = true;
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
    let uploaded = if paint_proxy {
        None
    } else {
        upload_program(ui.ctx(), host, app)
    };
    let mode = match (&backend, have_picture, plan.problem.is_some()) {
        (PreviewBackend::Disabled | PreviewBackend::Unavailable(_), _, _) => "Proxy",
        (_, _, true) => "Offline",
        (PreviewBackend::Cli, true, _) => "Preview",
        _ => "Empty",
    };

    widgets::panel_header(ui, "Program", |ui| {
        if !app.sequence_nav_stack.is_empty() {
            if ui
                .button(egui::RichText::new("← Parent").size(11.0).color(THEME.accent))
                .clicked()
            {
                app.close_nested_sequence();
            }
            ui.add_space(6.0);
        }
        focus_badge(ui, focused);
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
        if uploaded.is_some() {
            ui.add_space(4.0);
            let badge = app.program_display.badge();
            widgets::readout(ui, badge, 48.0, badge == "GPU");
        }
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

    let opacity = 1.0;
    let chip = (!plan.chip.is_empty() && have_picture).then(|| plan.chip.clone());

    let bank_h = if bank.is_some() { 44.0 } else { 0.0 };
    let monitor_h = (ui.available_height() - SCRUB_H - bank_h).max(48.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), monitor_h), Sense::click());
    if response.clicked() {
        app.focus_monitor(MonitorFocus::Program);
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.stage);
    if focused {
        painter.rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(1.5_f32, THEME.accent),
            egui::StrokeKind::Inside,
        );
    }
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
                if clip.is_adjustment() {
                    // Adjustment layers grade the decoded composite; proxy cards
                    // skip them because they carry no pixels of their own.
                } else if clip.is_nested() {
                    painter.text(
                        frame.center(),
                        Align2::CENTER_CENTER,
                        "Nested",
                        FontId::new(18.0, egui::FontFamily::Proportional),
                        THEME.accent,
                    );
                } else if clip.is_title() {
                    if let Some(layer) = plan
                        .layers
                        .iter()
                        .find(|layer| layer.clip_id == clip.id.0 && !layer.is_media())
                    {
                        paint_title_plate(
                            &painter,
                            app,
                            frame,
                            layer,
                            plan.canvas_w,
                            plan.canvas_h,
                            sequence.width as f32,
                            sequence.height as f32,
                        );
                    }
                } else {
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
        }
    } else if let Some(image) = uploaded {
        paint_decoded(
            &painter,
            frame,
            image.texture,
            image.width,
            image.height,
            opacity,
        );
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
    if let Some(bank) = bank {
        angle_bank_ui(ui, app, &bank);
    }
    program_scrubber(ui, app, sequence.end_frame().0.max(app.playhead));
}

struct AngleBank {
    names: Vec<String>,
    active: u32,
}

fn angle_bank(
    sequence: &editor_core::Sequence,
    groups: &[MulticamGroup],
    selected: &[editor_core::ClipId],
    playhead: i64,
) -> Option<AngleBank> {
    let clip_id = multicam_target(sequence, selected, playhead)?;
    let clip = sequence.clip(clip_id)?;
    let binding = clip.multicam.as_ref()?;
    let group = groups.iter().find(|item| item.id == binding.group)?;
    if group.angles.is_empty() {
        return None;
    }
    let group_time = source_frame_at(clip, Frame(playhead), sequence.timebase).0;
    let active = active_angle(&binding.cuts, group_time);
    let active = if (active as usize) < group.angles.len() {
        active
    } else {
        0
    };
    Some(AngleBank {
        names: group
            .angles
            .iter()
            .map(|angle| angle.name.clone())
            .collect(),
        active,
    })
}

fn angle_bank_ui(ui: &mut egui::Ui, app: &mut MeridianApp, bank: &AngleBank) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, THEME.panel);
    painter.hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 6.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(
        egui::RichText::new("ANGLES")
            .size(10.0)
            .color(THEME.text_mute),
    );
    child.add_space(8.0);
    egui::ScrollArea::horizontal()
        .id_salt("multicam_angles")
        .show(&mut child, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (index, name) in bank.names.iter().enumerate() {
                    let active = bank.active == index as u32;
                    let (cell, response) =
                        ui.allocate_exact_size(Vec2::new(108.0, 28.0), Sense::click());
                    let fill = if active {
                        THEME.accent_dim
                    } else {
                        THEME.header
                    };
                    let stroke = if active { THEME.accent } else { THEME.border };
                    ui.painter().rect_filled(cell, 3.0, fill);
                    ui.painter().rect_stroke(
                        cell,
                        3.0,
                        Stroke::new(1.0_f32, stroke),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        cell.left_center() + Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{}   {name}", index + 1),
                        THEME.font(11.0),
                        if active { THEME.accent } else { THEME.text },
                    );
                    if response.clicked() {
                        app.switch_multicam_angle(index as u32);
                    }
                    response.on_hover_text("Razor at the playhead and switch to this angle");
                }
            });
        });
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
                app.focus_monitor(MonitorFocus::Program);
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
        app.focus_monitor(MonitorFocus::Program);
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
    layers: Vec<PlanLayer>,
    media_queries: Vec<PreviewQuery>,
    compose_sequences: Vec<editor_core::Sequence>,
    compose_media: Vec<MediaAsset>,
    compose_groups: Vec<MulticamGroup>,
    compose_source: editor_media::PreviewSource,
    playhead: i64,
    problem: Option<String>,
    chip: String,
    canvas_w: u32,
    canvas_h: u32,
    captions: Vec<String>,
    used_proxy: bool,
}

enum PlanPixels {
    Media(PreviewQuery),
    Title(editor_core::Title),
    Solid([f32; 3]),
    Adjustment,
    Nested {
        sequence_id: editor_core::SequenceId,
        child_frame: i64,
    },
}

struct PlanLayer {
    pixels: PlanPixels,
    grade: GradeSample,
    filters: FilterSample,
    stabilize: StabilizeSample,
    stabilize_keyframes: Vec<editor_media::MotionSample>,
    place: Place,
    label: String,
    width: u32,
    height: u32,
    clip_id: u64,
    track_matte: Option<editor_core::TrackMatteBinding>,
}

impl PlanLayer {
    fn is_media(&self) -> bool {
        matches!(self.pixels, PlanPixels::Media(_))
    }
}

impl DecodePlan {
    fn signature(&self, playhead: i64) -> u64 {
        let mut hasher = DefaultHasher::new();
        playhead.hash(&mut hasher);
        self.canvas_w.hash(&mut hasher);
        self.canvas_h.hash(&mut hasher);
        for layer in &self.layers {
            layer.clip_id.hash(&mut hasher);
            layer.width.hash(&mut hasher);
            layer.height.hash(&mut hasher);
            match &layer.pixels {
                PlanPixels::Media(query) => {
                    0u8.hash(&mut hasher);
                    query.path.hash(&mut hasher);
                    query.source_frame.hash(&mut hasher);
                }
                PlanPixels::Title(title) => {
                    1u8.hash(&mut hasher);
                    title.text.hash(&mut hasher);
                    bits(title.font_size).hash(&mut hasher);
                    for channel in title.color {
                        bits(channel).hash(&mut hasher);
                    }
                    match title.align {
                        editor_core::TextAlign::Left => 0u8,
                        editor_core::TextAlign::Center => 1u8,
                        editor_core::TextAlign::Right => 2u8,
                    }
                    .hash(&mut hasher);
                    bits(title.x).hash(&mut hasher);
                    bits(title.y).hash(&mut hasher);
                    bits(title.plate).hash(&mut hasher);
                }
                PlanPixels::Solid(rgb) => {
                    2u8.hash(&mut hasher);
                    for channel in *rgb {
                        bits(channel).hash(&mut hasher);
                    }
                }
                PlanPixels::Adjustment => 3u8.hash(&mut hasher),
                PlanPixels::Nested {
                    sequence_id,
                    child_frame,
                } => {
                    4u8.hash(&mut hasher);
                    sequence_id.0.hash(&mut hasher);
                    child_frame.hash(&mut hasher);
                }
            }
            bits(layer.grade.exposure).hash(&mut hasher);
            bits(layer.grade.contrast).hash(&mut hasher);
            bits(layer.grade.highlights).hash(&mut hasher);
            bits(layer.grade.shadows).hash(&mut hasher);
            bits(layer.grade.temperature).hash(&mut hasher);
            bits(layer.grade.tint).hash(&mut hasher);
            bits(layer.grade.saturation).hash(&mut hasher);
            for channel in layer.grade.lift {
                bits(channel).hash(&mut hasher);
            }
            for channel in layer.grade.gamma {
                bits(channel).hash(&mut hasher);
            }
            for channel in layer.grade.gain {
                bits(channel).hash(&mut hasher);
            }
            for (input, output) in &layer.grade.luma_curve.points {
                bits(*input).hash(&mut hasher);
                bits(*output).hash(&mut hasher);
            }
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
                crate::composite::CanvasMask::Iris { edge, keep_below } => {
                    2u8.hash(&mut hasher);
                    bits(edge).hash(&mut hasher);
                    keep_below.hash(&mut hasher);
                }
            }
            bits(layer.place.blur_radius).hash(&mut hasher);
            bits(layer.filters.blur_radius).hash(&mut hasher);
            bits(layer.filters.vignette_amount).hash(&mut hasher);
            bits(layer.filters.vignette_softness).hash(&mut hasher);
            for inset in layer.filters.crop {
                bits(inset).hash(&mut hasher);
            }
            bits(layer.filters.sharpen).hash(&mut hasher);
            for channel in layer.filters.chroma_key_color {
                bits(channel).hash(&mut hasher);
            }
            bits(layer.filters.chroma_key_tolerance).hash(&mut hasher);
            bits(layer.filters.chroma_key_softness).hash(&mut hasher);
            bits(layer.filters.chroma_key_spill).hash(&mut hasher);
            if let Some(mask) = &layer.filters.shape_mask {
                match mask.shape {
                    editor_core::ShapeMaskKind::Rectangle => 0u8.hash(&mut hasher),
                    editor_core::ShapeMaskKind::Ellipse => 1u8.hash(&mut hasher),
                }
                bits(mask.center_x).hash(&mut hasher);
                bits(mask.center_y).hash(&mut hasher);
                bits(mask.width).hash(&mut hasher);
                bits(mask.height).hash(&mut hasher);
                bits(mask.feather).hash(&mut hasher);
                mask.invert.hash(&mut hasher);
            }
            bits(layer.grade.lut_mix).hash(&mut hasher);
            if let Some(lut) = &layer.grade.lut {
                lut.size.hash(&mut hasher);
                for channel in lut.domain_min {
                    bits(channel).hash(&mut hasher);
                }
                for channel in lut.domain_max {
                    bits(channel).hash(&mut hasher);
                }
                for sample in &lut.table {
                    for channel in *sample {
                        bits(channel).hash(&mut hasher);
                    }
                }
            }
            if let Some(matte) = &layer.track_matte {
                matte.source_track.hash(&mut hasher);
                match matte.mode {
                    editor_core::TrackMatteMode::Alpha => 0u8.hash(&mut hasher),
                    editor_core::TrackMatteMode::Luma => 1u8.hash(&mut hasher),
                }
                matte.invert.hash(&mut hasher);
            }
            layer.label.hash(&mut hasher);
        }
        for line in &self.captions {
            line.hash(&mut hasher);
        }
        hasher.finish()
    }
}

fn bits(value: f32) -> u32 {
    value.to_bits()
}

fn media_query_from_layer(layer: &ProgramLayer, burst: u32, lead: u32) -> Option<PreviewQuery> {
    match &layer.source {
        LayerSource::Media {
            path,
            source_frame,
            time_secs,
            frame_secs,
            last_source_frame,
        } => Some(PreviewQuery {
            path: path.clone(),
            source_frame: *source_frame,
            width: layer.width,
            height: layer.height,
            time_secs: *time_secs,
            frame_secs: *frame_secs,
            last_source_frame: *last_source_frame,
            burst,
            lead,
        }),
        _ => None,
    }
}

fn decode_plan(
    sequence: &editor_core::Sequence,
    sequences: &[editor_core::Sequence],
    playhead: i64,
    media: &[MediaAsset],
    groups: &[MulticamGroup],
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
    let stack = editor_media::program_stack_with(
        sequence,
        media,
        playhead,
        canvas_w,
        canvas_h,
        source,
        groups,
        sequences,
    );
    let compose_env = ComposeEnv {
        sequences,
        media,
        groups,
        preview_source: source,
        depth: 0,
        sequence: Some(sequence),
        playhead,
    };
    let media_programs = media_layers_in_stack(&stack.layers, compose_env);
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
        .map(|layer| {
            let pixels = match layer.source {
                LayerSource::Media {
                    path,
                    source_frame,
                    time_secs,
                    frame_secs,
                    last_source_frame,
                } => PlanPixels::Media(PreviewQuery {
                    path,
                    source_frame,
                    width: layer.width,
                    height: layer.height,
                    time_secs,
                    frame_secs,
                    last_source_frame,
                    burst,
                    lead,
                }),
                LayerSource::Title(title) => PlanPixels::Title(title),
                LayerSource::Solid { rgb } => PlanPixels::Solid(rgb),
                LayerSource::Adjustment => PlanPixels::Adjustment,
                LayerSource::Nested {
                    sequence_id,
                    child_frame,
                } => PlanPixels::Nested {
                    sequence_id,
                    child_frame,
                },
            };
            PlanLayer {
                pixels,
                grade: layer.grade,
                filters: layer.filters,
                stabilize: layer.stabilize,
                stabilize_keyframes: layer.stabilize_keyframes.clone(),
                place: layer.place,
                label: layer.label,
                width: layer.width,
                height: layer.height,
                clip_id: layer.clip_id,
                track_matte: layer.track_matte.clone(),
            }
        })
        .collect();
    let media_queries = media_programs
        .iter()
        .filter_map(|layer| media_query_from_layer(layer, burst, lead))
        .collect();
    DecodePlan {
        layers,
        media_queries,
        compose_sequences: sequences.to_vec(),
        compose_media: media.to_vec(),
        compose_groups: groups.to_vec(),
        compose_source: source,
        playhead,
        problem,
        chip,
        canvas_w,
        canvas_h,
        captions: active_captions(sequence, playhead),
        used_proxy,
    }
}

fn compose_plan(
    plan: &DecodePlan,
    seq_w: f32,
    seq_h: f32,
    media: &[PreviewImage],
) -> Result<Vec<u8>, String> {
    let programs: Vec<ProgramLayer> = plan.layers.iter().map(program_from_plan).collect();
    let env = ComposeEnv {
        sequences: &plan.compose_sequences,
        media: &plan.compose_media,
        groups: &plan.compose_groups,
        preview_source: plan.compose_source,
        depth: 0,
        sequence: plan.compose_sequences.first(),
        playhead: plan.playhead,
    };
    let mut cursor = 0;
    compose_layers_env(
        plan.canvas_w,
        plan.canvas_h,
        seq_w,
        seq_h,
        &programs,
        Some(env),
        |_| {
            let image = media
                .get(cursor)
                .ok_or_else(|| "missing decoded frame".to_string())?;
            cursor += 1;
            Ok(image.rgba.to_vec())
        },
    )
}

fn program_from_plan(layer: &PlanLayer) -> ProgramLayer {
    let source = match &layer.pixels {
        PlanPixels::Media(query) => LayerSource::Media {
            path: query.path.clone(),
            source_frame: query.source_frame,
            time_secs: query.time_secs,
            frame_secs: query.frame_secs,
            last_source_frame: query.last_source_frame,
        },
        PlanPixels::Title(title) => LayerSource::Title(title.clone()),
        PlanPixels::Solid(rgb) => LayerSource::Solid { rgb: *rgb },
        PlanPixels::Adjustment => LayerSource::Adjustment,
        PlanPixels::Nested {
            sequence_id,
            child_frame,
        } => LayerSource::Nested {
            sequence_id: *sequence_id,
            child_frame: *child_frame,
        },
    };
    ProgramLayer {
        source,
        width: layer.width,
        height: layer.height,
        grade: layer.grade.clone(),
        filters: layer.filters.clone(),
        stabilize: layer.stabilize,
        stabilize_keyframes: layer.stabilize_keyframes.clone(),
        place: layer.place,
        label: layer.label.clone(),
        using_proxy: false,
        clip_id: layer.clip_id,
        track_matte: layer.track_matte.clone(),
    }
}

fn paint_title_plate(
    painter: &Painter,
    app: &mut MeridianApp,
    frame: Rect,
    layer: &PlanLayer,
    canvas_w: u32,
    canvas_h: u32,
    seq_w: f32,
    seq_h: f32,
) {
    let program = program_from_plan(layer);
    let Ok(rgba) = compose_layers(canvas_w, canvas_h, seq_w, seq_h, &[program], |_| {
        Err("title plate has no picture".into())
    }) else {
        return;
    };
    let image = PreviewImage {
        key: FrameKey {
            path: format!("title-{}", title_signature(layer)),
            source_frame: 0,
            width: canvas_w,
            height: canvas_h,
        },
        width: canvas_w,
        height: canvas_h,
        rgba: std::sync::Arc::from(rgba.into_boxed_slice()),
    };
    let texture = app.preview.texture(painter.ctx(), &image);
    paint_decoded(painter, frame, texture, canvas_w, canvas_h, 1.0);
}

fn title_signature(layer: &PlanLayer) -> u64 {
    let mut hasher = DefaultHasher::new();
    layer.clip_id.hash(&mut hasher);
    layer.width.hash(&mut hasher);
    layer.height.hash(&mut hasher);
    if let PlanPixels::Title(title) = &layer.pixels {
        title.text.hash(&mut hasher);
        bits(title.font_size).hash(&mut hasher);
        for channel in title.color {
            bits(channel).hash(&mut hasher);
        }
        bits(title.x).hash(&mut hasher);
        bits(title.y).hash(&mut hasher);
        bits(title.plate).hash(&mut hasher);
        match title.align {
            editor_core::TextAlign::Left => 0u8,
            editor_core::TextAlign::Center => 1u8,
            editor_core::TextAlign::Right => 2u8,
        }
        .hash(&mut hasher);
    }
    bits(layer.place.opacity).hash(&mut hasher);
    bits(layer.place.pos_x).hash(&mut hasher);
    bits(layer.place.pos_y).hash(&mut hasher);
    bits(layer.place.scale_x).hash(&mut hasher);
    bits(layer.place.scale_y).hash(&mut hasher);
    hasher.finish()
}

fn upload_program(
    ctx: &egui::Context,
    host: &mut eframe::Frame,
    app: &mut MeridianApp,
) -> Option<UploadedFrame> {
    let mut display = std::mem::take(&mut app.program_display);
    let uploaded = app.picture_cache.as_ref().and_then(|cache| {
        display.upload(
            ctx,
            host,
            cache.signature,
            cache.width,
            cache.height,
            &cache.rgba,
        )
    });
    app.program_display = display;
    uploaded
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
