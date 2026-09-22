//! Timeline: ruler, track headers, clip lanes, transport.

use std::collections::HashSet;

use editor_core::{
    align_frame, clamp_timeline_zoom, clip_index_at, collect_snap_points, expand_linked,
    frame_at_x, ruler_step, snap_span, stacked_hits, timeline_x, visible_clip_span, visible_span,
    ClipId, Frame, MediaId, TrackFlag, TrackKind, TrimEdge, MAX_PIXELS_PER_FRAME,
    MIN_PIXELS_PER_FRAME,
};
use egui::{pos2, Align2, Color32, CursorIcon, FontId, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::{note_track_flag, Drag, DragKind, MeridianApp, ScrubSource, Tool};
use crate::theme::{self, THEME};
use crate::ui::format_tc;
use crate::ui::widgets;

const HEADER_W: f32 = theme::HEADER_COL_W;
const RULER_H: f32 = theme::RULER_H;
const ROW_H: f32 = theme::LANE_H;
const CLIP_PAD_Y: f32 = 2.0;

pub fn timeline_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    transport(ui, app);
    let Some(sequence) = app.session.project().active().cloned() else {
        ui.label("No sequence.");
        return;
    };
    let visual = sequence.visual_track_indices();
    let end = (sequence.end_frame().0 + 48).max(app.playhead + 24).max(96);
    let offline = offline_media(app);
    let body_h = ui.available_height();

    let scroll_h = 14.0;
    ui.horizontal(|ui| {
        ui.set_min_height(body_h);
        ui.vertical(|ui| {
            ui.set_width(HEADER_W);
            ui.allocate_exact_size(Vec2::new(HEADER_W, RULER_H), Sense::hover());
            for index in &visual {
                header_row(ui, app, &sequence, *index);
            }
        });
        let body_w = (ui.available_width() - 4.0).max(80.0);
        let (body, _) = ui.allocate_exact_size(Vec2::new(body_w, body_h), Sense::hover());
        app.timeline_view = Some(body);
        app.timeline_width = body_w;
        if app.reveal_playhead {
            reveal_playhead(app, body_w, end);
            app.reveal_playhead = false;
        }
        app.clamp_timeline_origin(end);
        nudge_timeline_scroll(ui, app, end);
        let scroll = Rect::from_min_max(pos2(body.left(), body.bottom() - scroll_h), body.max);
        let lanes = Rect::from_min_max(body.min, pos2(body.right(), scroll.top()));
        let mut lanes_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(lanes)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        lanes_ui.set_clip_rect(lanes);
        ruler(&mut lanes_ui, app, &sequence, body_w, end);
        for index in &visual {
            lane(&mut lanes_ui, app, &sequence, *index, body_w, end, &offline);
        }
        timeline_scrollbar(ui, app, end, scroll);
    });
    follow_ruler_scrub(ui, app);
}

fn reveal_playhead(app: &mut MeridianApp, view_w: f32, end: i64) {
    let ppf = f64::from(app.pixels_per_frame.max(MIN_PIXELS_PER_FRAME));
    let view_frames = (f64::from(view_w) / ppf).max(1.0);
    let frame = app.playhead as f64;
    let left = app.timeline_origin;
    let right = left + view_frames;
    if frame < left + view_frames * 0.08 || frame > right - view_frames * 0.12 {
        app.timeline_origin = frame - view_frames * 0.33;
    }
    app.clamp_timeline_origin(end);
}

fn nudge_timeline_scroll(ui: &egui::Ui, app: &mut MeridianApp, end: i64) {
    let Some(view) = app.timeline_view else {
        return;
    };
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let Some(pointer) = pointer else {
        return;
    };
    if !view.contains(pointer) {
        return;
    }
    let (dx, dy, zoom, command, alt, middle, middle_dy) = ui.input(|input| {
        (
            input.smooth_scroll_delta.x,
            input.smooth_scroll_delta.y,
            input.zoom_delta(),
            input.modifiers.command || input.modifiers.ctrl,
            input.modifiers.alt,
            input.pointer.middle_down(),
            input.pointer.delta().y,
        )
    });
    let scrolling = dx.abs() + dy.abs() > 0.0;
    let zooming = (zoom - 1.0).abs() > 0.01
        || (command && dy.abs() > 0.0)
        || (alt && dy.abs() > 0.0)
        || (middle && middle_dy.abs() > 0.0);
    if !scrolling && !zooming {
        return;
    }
    ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
    if zooming {
        let factor = if (zoom - 1.0).abs() > 0.01 {
            zoom
        } else if middle && middle_dy.abs() > 0.0 {
            (-middle_dy * 0.01).exp()
        } else {
            (dy * 0.0016).exp()
        };
        let old = app.pixels_per_frame;
        let new = clamp_timeline_zoom(old * factor);
        app.rebase_timeline_zoom(old, new, pointer.x - view.min.x);
    } else {
        let ppf = f64::from(app.pixels_per_frame.max(MIN_PIXELS_PER_FRAME));
        app.timeline_origin -= f64::from(dx + dy) / ppf;
        app.clamp_timeline_origin(end);
    }
}

fn timeline_scrollbar(ui: &mut egui::Ui, app: &mut MeridianApp, end: i64, rect: Rect) {
    let bar = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let response = bar.interact(
        rect,
        egui::Id::new("timeline_scroll"),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.inset);
    let ppf = f64::from(app.pixels_per_frame.max(MIN_PIXELS_PER_FRAME));
    let view_frames = (f64::from(rect.width()) / ppf).max(1.0);
    let total = (end as f64).max(view_frames);
    let max_origin = (total - view_frames).max(0.0);
    let thumb_w = (f64::from(rect.width()) * (view_frames / total))
        .clamp(24.0, f64::from(rect.width())) as f32;
    let travel = (rect.width() - thumb_w).max(1.0);
    let thumb_x = if max_origin <= 0.0 {
        rect.left()
    } else {
        rect.left() + (app.timeline_origin / max_origin).clamp(0.0, 1.0) as f32 * travel
    };
    let thumb = Rect::from_min_size(
        pos2(thumb_x, rect.top() + 2.0),
        Vec2::new(thumb_w, (rect.height() - 4.0).max(4.0)),
    );
    painter.rect_filled(
        thumb,
        2.0,
        if response.dragged() || response.hovered() {
            THEME.accent
        } else {
            THEME.border
        },
    );
    if response.dragged() || response.clicked() {
        if let Some(pos) = ui.input(|input| input.pointer.interact_pos()) {
            let t = ((pos.x - rect.left() - thumb_w * 0.5) / travel).clamp(0.0, 1.0);
            app.timeline_origin = if max_origin <= 0.0 {
                0.0
            } else {
                f64::from(t) * max_origin
            };
            app.clamp_timeline_origin(end);
        }
    }
}

fn ruler_visible(app: &MeridianApp) -> Option<Rect> {
    let ruler = app.ruler_rect?;
    let Some(view) = app.timeline_view else {
        return Some(ruler);
    };
    Some(Rect::from_min_max(
        pos2(view.min.x.max(ruler.min.x), ruler.min.y),
        pos2(view.max.x.min(ruler.max.x), ruler.max.y),
    ))
}

fn offline_media(app: &MeridianApp) -> HashSet<MediaId> {
    app.session
        .project()
        .media
        .iter()
        .filter(|media| super::media_missing(&media.path))
        .map(|media| media.id)
        .collect()
}

fn follow_ruler_scrub(ui: &egui::Ui, app: &mut MeridianApp) {
    let (down, pressed, pos) = ui.input(|input| {
        (
            input.pointer.primary_down(),
            input.pointer.primary_pressed(),
            input.pointer.interact_pos(),
        )
    });
    if pressed {
        if let Some(pos) = pos {
            if ruler_visible(app).is_some_and(|ruler| ruler.contains(pos)) {
                app.scrub = Some(ScrubSource::Ruler);
            }
        }
    }
    if !down {
        if app.scrub == Some(ScrubSource::Ruler) {
            app.scrub = None;
        }
        return;
    }
    if app.scrub != Some(ScrubSource::Ruler) {
        return;
    }
    let (Some(pos), Some(ruler)) = (pos, app.ruler_rect) else {
        return;
    };
    app.playhead = x_to_frame(
        pos.x,
        ruler.min.x,
        app.timeline_origin,
        app.pixels_per_frame,
    )
    .max(0);
    app.preview_scrub = true;
    app.halt_transport();
}

fn transport_scrub(ui: &mut egui::Ui, playhead: i64, end: i64) -> Option<i64> {
    let width = ui.available_width().max(80.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 14.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
    let painter = ui.painter();
    painter.rect_filled(track, 3.0, THEME.inset);
    let span = end.max(1) as f32;
    let t = (playhead as f32 / span).clamp(0.0, 1.0);
    let x = track.left() + track.width() * t;
    painter.rect_filled(
        Rect::from_min_max(track.min, pos2(x, track.bottom())),
        3.0,
        THEME.accent_dim,
    );
    painter.circle_filled(pos2(x, track.center().y), 6.0, THEME.playhead);
    if response.hovered() || response.dragged() {
        response.clone().on_hover_cursor(CursorIcon::PointingHand);
        response.clone().on_hover_text("Drag to scrub");
    }
    if !(response.dragged() || response.clicked()) {
        return None;
    }
    let pos = response.interact_pointer_pos()?;
    let nt = ((pos.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
    Some((nt * end.max(0) as f32).round() as i64)
}

fn transport(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let timebase = app.timebase();
    let end = app.sequence_end();
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::TRANSPORT_H),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, THEME.header);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), theme::hairline_stroke());
    let controls = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 36.0));
    let scrub_row = Rect::from_min_max(pos2(rect.left(), rect.top() + 36.0), rect.max);
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(controls.shrink2(Vec2::new(theme::SPACE_MD, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = theme::SPACE_XS;
    if widgets::transport_glyph(&mut bar, "Go to start", widgets::bar_left) {
        app.playhead = 0;
        app.halt_transport();
        app.reveal_playhead = true;
    }
    if widgets::transport_glyph(&mut bar, "Step back", widgets::tri_left) {
        app.step_playhead(-1);
    }
    if widgets::play_button(&mut bar, app.playing) {
        app.toggle_play();
    }
    if widgets::transport_glyph(&mut bar, "Step forward", widgets::tri_right) {
        app.step_playhead(1);
    }
    if widgets::transport_glyph(&mut bar, "Go to end", widgets::bar_right) {
        app.playhead = end;
        app.halt_transport();
        app.reveal_playhead = true;
    }
    bar.add_space(theme::SPACE_SM);
    widgets::timecode_well(
        &mut bar,
        "TIMECODE",
        &format_tc(app.playhead, timebase),
        true,
    );
    bar.add_space(theme::SPACE_XS);
    widgets::timecode_well(&mut bar, "DURATION", &format_tc(end, timebase), false);
    bar.add_space(theme::SPACE_SM);
    let (inn, out) = app
        .session
        .project()
        .active()
        .map(|s| (s.in_point.map(|f| f.0), s.out_point.map(|f| f.0)))
        .unwrap_or((None, None));
    let in_label = match inn {
        Some(frame) => format!("In {}", format_tc(frame, timebase)),
        None => "Mark In".into(),
    };
    let out_label = match out {
        Some(frame) => format!("Out {}", format_tc(frame, timebase)),
        None => "Mark Out".into(),
    };
    if widgets::chip(&mut bar, &in_label, inn.is_some()) {
        app.mark_in();
    }
    if widgets::chip(&mut bar, &out_label, out.is_some()) {
        app.mark_out();
    }
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::ghost_button(ui, "Fit") {
            app.zoom_to_fit();
        }
        if widgets::ghost_button(ui, "+") {
            app.zoom_by(1.25);
        }
        if let Some(zoom) = widgets::mini_slider_log(
            ui,
            app.pixels_per_frame,
            MIN_PIXELS_PER_FRAME..=MAX_PIXELS_PER_FRAME,
        ) {
            let old = app.pixels_per_frame;
            let new = clamp_timeline_zoom(zoom);
            let anchor = app.zoom_anchor_px();
            app.rebase_timeline_zoom(old, new, anchor);
        }
        if widgets::ghost_button(ui, "−") {
            app.zoom_by(1.0 / 1.25);
        }
        let rate = if app.playing {
            format!("{}×", app.play_rate)
        } else {
            "Stop".into()
        };
        ui.label(
            egui::RichText::new(rate)
                .font(THEME.mono(11.0))
                .color(if app.playing {
                    THEME.accent
                } else {
                    THEME.text_mute
                }),
        );
    });
    let mut scrub_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(scrub_row.shrink2(Vec2::new(theme::SPACE_MD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if let Some(frame) = transport_scrub(&mut scrub_ui, app.playhead, end) {
        app.playhead = frame;
        app.preview_scrub = true;
        app.halt_transport();
    }
}

fn header_row(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    sequence: &editor_core::Sequence,
    index: usize,
) {
    let track = &sequence.tracks[index];
    let (rect, _) = ui.allocate_exact_size(Vec2::new(HEADER_W, ROW_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, THEME.header);
    painter.hline(rect.x_range(), rect.bottom(), theme::hairline_stroke());
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
        0.0,
        theme::track_color(track.kind),
    );
    let name_clip = Rect::from_min_max(rect.min, pos2(rect.right() - 80.0, rect.bottom()));
    painter.with_clip_rect(name_clip).text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        &track.name,
        THEME.font(11.0),
        THEME.text,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 4.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 3.0;
    let targeted = app.is_track_targeted(track.id)
        && (track.kind == TrackKind::Video || track.kind == TrackKind::Audio);
    if track.kind == TrackKind::Video || track.kind == TrackKind::Audio {
        if widgets::icon_toggle(&mut child, "T", targeted, THEME.accent) {
            app.toggle_track_target(track.id);
        }
    }
    if widgets::icon_toggle(&mut child, "S", track.solo, THEME.amber) {
        note_track_flag(app, track.id, TrackFlag::Solo, !track.solo);
    }
    if widgets::icon_toggle(&mut child, "M", track.muted, THEME.danger) {
        note_track_flag(app, track.id, TrackFlag::Mute, !track.muted);
    }
    if widgets::icon_toggle(&mut child, "L", track.locked, THEME.control_hover) {
        note_track_flag(app, track.id, TrackFlag::Lock, !track.locked);
    }
}

fn ruler(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    sequence: &editor_core::Sequence,
    content_w: f32,
    end: i64,
) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(content_w, RULER_H), Sense::click_and_drag());
    app.ruler_rect = Some(rect);
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::RULER);
    painter.hline(rect.x_range(), rect.bottom(), theme::hairline_stroke());
    let ppf = app.pixels_per_frame;
    let origin = app.timeline_origin;
    let fps = sequence.timebase.timecode_fps().max(1);
    let (view_start, view_end) = visible_frame_range(origin, rect.width(), ppf, end);
    let step = ruler_step(ppf, fps);
    let mut frame = align_frame(view_start, step.minor).max(0);
    let mut drawn = 0;
    while frame <= view_end && drawn < 800 {
        let x = fx(frame, origin, rect.min.x, ppf);
        let major = step.major > 0 && frame % step.major == 0;
        if major {
            painter.vline(
                x,
                rect.bottom() - 10.0..=rect.bottom(),
                Stroke::new(1.0_f32, THEME.border),
            );
        } else if frame % step.minor.max(1) == 0 {
            painter.vline(
                x,
                (rect.bottom() - 4.0)..=rect.bottom(),
                Stroke::new(1.0_f32, THEME.hairline),
            );
        }
        if step.label > 0 && frame % step.label == 0 {
            painter.text(
                pos2(x + 3.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                format_tc(frame, sequence.timebase),
                THEME.mono(9.0),
                THEME.text_mute,
            );
        }
        frame += step.minor.max(1);
        drawn += 1;
    }
    if let (Some(inn), Some(out)) = (sequence.in_point, sequence.out_point) {
        let x0 = fx(inn.0, origin, rect.min.x, ppf);
        let x1 = fx(out.0, origin, rect.min.x, ppf);
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x0, rect.bottom() - 3.0),
                pos2(x1.max(x0 + 2.0), rect.bottom()),
            ),
            0.0,
            THEME.accent,
        );
    }
    for marker in &sequence.markers {
        if marker.frame.0 < view_start.saturating_sub(2)
            || marker.frame.0 > view_end.saturating_add(2)
        {
            continue;
        }
        let x = fx(marker.frame.0, origin, rect.min.x, ppf);
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x, rect.bottom() - 8.0),
                pos2(x + 4.0, rect.bottom() - 2.0),
                pos2(x - 4.0, rect.bottom() - 2.0),
            ],
            THEME.amber,
            Stroke::NONE,
        ));
    }
    if response.is_pointer_button_down_on() {
        app.scrub = Some(ScrubSource::Ruler);
        if let Some(pos) = response.interact_pointer_pos() {
            app.playhead = x_to_frame(pos.x, rect.min.x, origin, ppf).max(0);
            app.preview_scrub = true;
            app.halt_transport();
        }
    }
    if response.double_clicked() {
        app.toggle_play();
    }
    paint_playhead(&painter, rect, app.playhead, origin, ppf);
}

fn lane(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    sequence: &editor_core::Sequence,
    index: usize,
    view_w: f32,
    end: i64,
    offline: &HashSet<MediaId>,
) {
    let track = &sequence.tracks[index];
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(view_w, ROW_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let bg = if index % 2 == 0 {
        theme::LANE
    } else {
        theme::LANE_ALT
    };
    painter.rect_filled(rect, 0.0, bg);
    let ppf = app.pixels_per_frame;
    let origin = app.timeline_origin;
    let (view_start, view_end) = visible_frame_range(origin, rect.width(), ppf, end);

    if track.kind == TrackKind::Caption {
        let span = visible_span(
            &track.cues,
            view_start,
            view_end,
            |cue| cue.timeline_in.0,
            |cue| cue.timeline_out.0,
        );
        for cue in &track.cues[span] {
            let x0 = fx(cue.timeline_in.0, origin, rect.min.x, ppf);
            let x1 = fx(cue.timeline_out.0, origin, rect.min.x, ppf);
            let crect = Rect::from_min_max(
                pos2(x0, rect.min.y + CLIP_PAD_Y),
                pos2(x1.max(x0 + 4.0), rect.max.y - CLIP_PAD_Y),
            );
            paint_clip_body(&painter, crect, theme::CAPTION, false, false);
            if crect.width() >= 18.0 {
                painter.with_clip_rect(crect.shrink(3.0)).text(
                    crect.left_center() + Vec2::new(5.0, 0.0),
                    Align2::LEFT_CENTER,
                    &cue.text,
                    THEME.font(10.5),
                    Color32::WHITE,
                );
            }
        }
    }

    let mut span = visible_clip_span(&track.clips, view_start, view_end);
    if let Some(drag) = &app.drag {
        if drag.track_id == track.id {
            if let Some(index) = track.clips.iter().position(|clip| clip.id == drag.clip_id) {
                if index < span.start {
                    span.start = index;
                } else if index >= span.end {
                    span.end = index + 1;
                }
            }
        }
    }
    let mut draw: Vec<usize> = stacked_hits(
        &track.clips,
        &track.stacked_clips,
        view_start,
        view_end,
        span.start,
    );
    draw.extend(span);
    for index in draw {
        let clip = &track.clips[index];
        let (start, end) = preview_span(app, clip.id, clip.timeline_in.0, clip.timeline_out.0);
        let x0 = fx(start, origin, rect.min.x, ppf);
        let x1 = fx(end, origin, rect.min.x, ppf);
        let crect = Rect::from_min_max(
            pos2(x0, rect.min.y + CLIP_PAD_Y),
            pos2(x1.max(x0 + 3.0), rect.max.y - CLIP_PAD_Y),
        );
        let selected = app.selected.contains(&clip.id);
        let fill = theme::label_fill(clip.label, track.kind);
        let clip_offline = clip.media_id.is_some_and(|id| offline.contains(&id));
        if crect.width() < 2.0 {
            painter.rect_filled(crect, 0.0, fill);
            continue;
        }
        paint_clip_body(
            &painter,
            crect,
            fill,
            selected,
            track.kind == TrackKind::Audio,
        );
        if clip_offline {
            painter.rect_filled(crect, 3.0, Color32::from_black_alpha(90));
            if crect.width() >= 48.0 {
                painter.text(
                    crect.right_center() - Vec2::new(8.0, 0.0),
                    Align2::RIGHT_CENTER,
                    "Offline",
                    FontId::new(10.0, egui::FontFamily::Proportional),
                    THEME.amber,
                );
            }
        }
        if crect.width() >= 18.0 {
            let clip_label = if clip.is_title() {
                format!("T  {}", clip.name)
            } else if clip.is_adjustment() {
                format!("A  {}", clip.name)
            } else if let Some(badge) = clip.speed.badge() {
                format!("{}  {badge}", clip.name)
            } else {
                clip.name.clone()
            };
            painter.with_clip_rect(crect.shrink(3.0)).text(
                crect.left_center() + Vec2::new(5.0, 0.0),
                Align2::LEFT_CENTER,
                &clip_label,
                THEME.font(10.5),
                Color32::WHITE,
            );
        }
    }

    if !track.transitions.is_empty() {
        let mut by_id = std::collections::HashMap::with_capacity(track.clips.len());
        for clip in &track.clips {
            by_id.insert(clip.id, clip);
        }
        paint_transitions(&painter, track, &by_id, rect, origin, ppf);
    }

    paint_playhead(&painter, rect, app.playhead, origin, ppf);

    if let Some(pos) = response.hover_pos() {
        if let Some(hit) = hit_test(track, rect, pos, origin, ppf) {
            let icon = match (app.tool, hit.edge) {
                (Tool::Slip | Tool::Slide, _) => CursorIcon::ResizeHorizontal,
                (_, Some(_)) => CursorIcon::ResizeHorizontal,
                (Tool::Razor, _) => CursorIcon::Crosshair,
                _ => CursorIcon::Grab,
            };
            response.clone().on_hover_cursor(icon);
        }
    }

    if response.drag_started() && app.tool != Tool::Razor {
        if let Some(pos) = response.interact_pointer_pos() {
            let frame = x_to_frame(pos.x, rect.min.x, origin, ppf).max(0);
            if let Some(hit) = hit_test(track, rect, pos, origin, ppf) {
                select_clip(app, sequence, hit.clip_id, false, false);
                if let Some(clip) = track.clips.iter().find(|c| c.id == hit.clip_id) {
                    let kind = drag_kind(app.tool, hit.edge);
                    app.drag = Some(Drag {
                        kind,
                        clip_id: clip.id,
                        track_id: track.id,
                        origin_in: clip.timeline_in.0,
                        origin_out: clip.timeline_out.0,
                        press_x: pos.x,
                        current_x: pos.x,
                        ppf,
                    });
                }
            } else {
                app.selected.clear();
                app.playhead = frame;
                app.halt_transport();
                app.preview_scrub = true;
            }
        }
    } else if let Some(pos) = response.interact_pointer_pos() {
        if let Some(drag) = &mut app.drag {
            if drag.track_id == track.id {
                drag.current_x = pos.x;
                drag.ppf = ppf;
            }
        }
    }

    if response.clicked() && app.tool == Tool::Razor {
        if let Some(pos) = response.interact_pointer_pos() {
            let frame = x_to_frame(pos.x, rect.min.x, origin, ppf).max(0);
            razor_at(app, track, frame);
        }
    } else if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(hit) = hit_test(track, rect, pos, origin, ppf) {
                let shift = ui.input(|i| i.modifiers.shift);
                let toggle = ui.input(|i| i.modifiers.command);
                select_clip(app, sequence, hit.clip_id, shift, toggle);
            } else if app.dragging_media.is_none() {
                app.playhead = x_to_frame(pos.x, rect.min.x, origin, ppf).max(0);
                app.halt_transport();
                app.preview_scrub = true;
                if !ui.input(|i| i.modifiers.shift) {
                    app.selected.clear();
                }
            }
        }
    }

    if response.double_clicked() && track.kind != TrackKind::Caption {
        if let Some(pos) = response.interact_pointer_pos() {
            if hit_test(track, rect, pos, origin, ppf).is_none() {
                app.toggle_play();
            }
        }
    }

    if ui.input(|i| i.pointer.any_released()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if rect.contains(pos) {
                if let Some(id) = app.dragging_media.take() {
                    let frame = x_to_frame(pos.x, rect.min.x, origin, ppf).max(0);
                    app.selected_media = Some(id);
                    app.playhead = frame;
                    app.halt_transport();
                    let insert = ui.input(|i| i.modifiers.shift);
                    app.place_selected_media(insert);
                }
            }
        }
    }
    if app.dragging_media.is_some() {
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            if rect.contains(pos) {
                painter.rect_filled(rect, 0.0, Color32::from_white_alpha(16));
            }
        }
    }

    if response.drag_stopped() {
        commit_drag(app, sequence);
    }
}

fn preview_span(app: &MeridianApp, id: ClipId, start: i64, end: i64) -> (i64, i64) {
    let Some(drag) = &app.drag else {
        return (start, end);
    };
    if drag.clip_id != id {
        return (start, end);
    }
    let delta = drag.delta_frames();
    match drag.kind {
        DragKind::Move | DragKind::Slide => {
            let duration = end - start;
            let snapped = maybe_snap(app, start + delta, duration, id);
            (snapped, snapped + duration)
        }
        DragKind::TrimTail | DragKind::RippleTail | DragKind::RollTail => {
            (start, (end + delta).max(start + 1))
        }
        DragKind::TrimHead | DragKind::RollHead => ((start + delta).min(end - 1), end),
        DragKind::RippleHead => (start, (end - delta).max(start + 1)),
        DragKind::Slip => (start, end),
    }
}

fn maybe_snap(app: &MeridianApp, start: i64, duration: i64, clip: ClipId) -> i64 {
    if !app.snap_enabled {
        return start.max(0);
    }
    let Some(sequence) = app.session.project().active() else {
        return start.max(0);
    };
    let exclude = if app.linked_selection {
        expand_linked(sequence, &[clip])
    } else {
        vec![clip]
    };
    let targets = collect_snap_points(sequence, &exclude, Some(Frame(app.playhead)));
    let threshold = snap_threshold(app.pixels_per_frame);
    snap_span(Frame(start), duration, &targets, threshold)
        .0
        .max(0)
}

fn snap_threshold(ppf: f32) -> i64 {
    ((10.0 / ppf).round() as i64).clamp(1, 12)
}

fn drag_kind(tool: Tool, edge: Option<TrimEdge>) -> DragKind {
    match (tool, edge) {
        (Tool::Ripple, Some(TrimEdge::Head)) => DragKind::RippleHead,
        (Tool::Ripple, _) => DragKind::RippleTail,
        (Tool::Roll, Some(TrimEdge::Head)) => DragKind::RollHead,
        (Tool::Roll, _) => DragKind::RollTail,
        (Tool::Slip, _) => DragKind::Slip,
        (Tool::Slide, _) => DragKind::Slide,
        (_, Some(TrimEdge::Head)) => DragKind::TrimHead,
        (_, Some(TrimEdge::Tail)) => DragKind::TrimTail,
        _ => DragKind::Move,
    }
}

struct Hit {
    clip_id: ClipId,
    edge: Option<TrimEdge>,
}

fn hit_test(
    track: &editor_core::Track,
    rect: Rect,
    pos: egui::Pos2,
    origin: f64,
    ppf: f32,
) -> Option<Hit> {
    let frame = x_to_frame(pos.x, rect.min.x, origin, ppf);
    if let Some(index) = clip_index_at(&track.clips, frame, &track.stacked_clips) {
        return Some(hit_from_clip(&track.clips[index], rect, pos, origin, ppf));
    }
    let slop = ((8.0 / ppf.max(0.001)).ceil() as i64).clamp(1, 64);
    let span = visible_clip_span(
        &track.clips,
        frame.saturating_sub(slop),
        frame.saturating_add(slop + 1),
    );
    for index in stacked_hits(
        &track.clips,
        &track.stacked_clips,
        frame.saturating_sub(slop),
        frame.saturating_add(slop + 1),
        span.start,
    ) {
        let clip = &track.clips[index];
        let crect = clip_rect(clip, rect, origin, ppf);
        if crect.contains(pos) {
            return Some(hit_from_clip(clip, rect, pos, origin, ppf));
        }
    }
    for clip in track.clips[span].iter().rev() {
        let crect = clip_rect(clip, rect, origin, ppf);
        if crect.contains(pos) {
            return Some(hit_from_clip(clip, rect, pos, origin, ppf));
        }
    }
    None
}

fn hit_from_clip(
    clip: &editor_core::Clip,
    rect: Rect,
    pos: egui::Pos2,
    origin: f64,
    ppf: f32,
) -> Hit {
    let crect = clip_rect(clip, rect, origin, ppf);
    let edge = if crect.width() >= 16.0 && (pos.x - crect.min.x).abs() <= 6.0 {
        Some(TrimEdge::Head)
    } else if crect.width() >= 16.0 && (pos.x - crect.max.x).abs() <= 6.0 {
        Some(TrimEdge::Tail)
    } else {
        None
    };
    Hit {
        clip_id: clip.id,
        edge,
    }
}

fn clip_rect(clip: &editor_core::Clip, rect: Rect, origin: f64, ppf: f32) -> Rect {
    let x0 = fx(clip.timeline_in.0, origin, rect.min.x, ppf);
    let x1 = fx(clip.timeline_out.0, origin, rect.min.x, ppf);
    Rect::from_min_max(
        pos2(x0, rect.min.y + CLIP_PAD_Y),
        pos2(x1.max(x0 + 4.0), rect.max.y - CLIP_PAD_Y),
    )
}

fn visible_frame_range(origin: f64, width: f32, ppf: f32, end: i64) -> (i64, i64) {
    let start = frame_at_x(-24.0, origin, 0.0, ppf).saturating_sub(1);
    let stop = frame_at_x(width + 24.0, origin, 0.0, ppf).saturating_add(2);
    (start.max(0), stop.max(start).min(end.max(0) + 8))
}

fn select_clip(
    app: &mut MeridianApp,
    sequence: &editor_core::Sequence,
    id: ClipId,
    extend: bool,
    toggle: bool,
) {
    let group = if app.linked_selection {
        expand_linked(sequence, &[id])
    } else {
        vec![id]
    };
    if toggle {
        let removing = group.iter().all(|id| app.selected.contains(id));
        if removing {
            app.selected.retain(|id| !group.contains(id));
        } else {
            for id in group {
                if !app.selected.contains(&id) {
                    app.selected.push(id);
                }
            }
        }
    } else if extend {
        for id in group {
            if !app.selected.contains(&id) {
                app.selected.push(id);
            }
        }
    } else {
        app.selected = group;
    }
}

fn razor_at(app: &mut MeridianApp, track: &editor_core::Track, frame: i64) {
    app.playhead = frame;
    app.halt_transport();
    let clip = clip_index_at(&track.clips, frame, &track.stacked_clips)
        .map(|index| &track.clips[index])
        .filter(|clip| clip.contains_frame(Frame(frame)));
    if let Some(clip) = clip {
        match app.session.razor_clip(clip.id, Frame(frame)) {
            Ok(()) => app.status = "Split clip.".into(),
            Err(err) => app.status = err.to_string(),
        }
    } else {
        app.status = "Playhead is not inside a clip.".into();
    }
}

fn commit_drag(app: &mut MeridianApp, sequence: &editor_core::Sequence) {
    let Some(drag) = app.drag.take() else {
        return;
    };
    let raw = drag.delta_frames();
    if raw == 0 && !matches!(drag.kind, DragKind::Slip) {
        return;
    }
    let result = match drag.kind {
        DragKind::Move => {
            let ids = if app.linked_selection {
                expand_linked(sequence, &[drag.clip_id])
            } else {
                vec![drag.clip_id]
            };
            let duration = drag.origin_out - drag.origin_in;
            let snapped = maybe_snap(app, drag.origin_in + raw, duration, drag.clip_id);
            let delta = snapped - drag.origin_in;
            if delta == 0 {
                return;
            }
            app.session.move_clips(ids, delta, None)
        }
        DragKind::TrimHead => app.session.trim(drag.clip_id, TrimEdge::Head, raw),
        DragKind::TrimTail => app.session.trim(drag.clip_id, TrimEdge::Tail, raw),
        DragKind::RippleHead => app.session.ripple_trim(drag.clip_id, TrimEdge::Head, -raw),
        DragKind::RippleTail => app.session.ripple_trim(drag.clip_id, TrimEdge::Tail, raw),
        DragKind::RollTail => app.session.roll(drag.clip_id, raw),
        DragKind::RollHead => {
            let Some(prev) = previous_clip(sequence, drag.clip_id) else {
                app.status = "Roll needs an adjacent clip.".into();
                return;
            };
            app.session.roll(prev, raw)
        }
        DragKind::Slip => {
            if raw == 0 {
                return;
            }
            app.session.slip(drag.clip_id, raw)
        }
        DragKind::Slide => {
            if raw == 0 {
                return;
            }
            app.session.slide(drag.clip_id, raw)
        }
    };
    app.status = match result {
        Ok(()) => format!("{} applied.", kind_name(drag.kind)),
        Err(err) => err.to_string(),
    };
}

fn previous_clip(sequence: &editor_core::Sequence, id: ClipId) -> Option<ClipId> {
    let (ti, ci) = sequence.locate_clip(id)?;
    if ci == 0 {
        return None;
    }
    let prev = &sequence.tracks[ti].clips[ci - 1];
    let clip = &sequence.tracks[ti].clips[ci];
    if prev.timeline_out == clip.timeline_in {
        Some(prev.id)
    } else {
        None
    }
}

fn kind_name(kind: DragKind) -> &'static str {
    match kind {
        DragKind::Move => "Move",
        DragKind::TrimHead | DragKind::TrimTail => "Trim",
        DragKind::RippleHead | DragKind::RippleTail => "Ripple",
        DragKind::RollHead | DragKind::RollTail => "Roll",
        DragKind::Slip => "Slip",
        DragKind::Slide => "Slide",
    }
}

fn paint_playhead(painter: &egui::Painter, rect: Rect, frame: i64, origin: f64, ppf: f32) {
    let x = fx(frame, origin, rect.min.x, ppf);
    if x < rect.left() - 8.0 || x > rect.right() + 8.0 {
        return;
    }
    painter.vline(x, rect.y_range(), Stroke::new(1.25_f32, THEME.playhead));
    if rect.height() <= RULER_H + 2.0 {
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x - 5.5, rect.top()),
                pos2(x + 5.5, rect.top()),
                pos2(x, rect.top() + 8.0),
            ],
            THEME.playhead,
            Stroke::NONE,
        ));
    }
}

fn paint_clip_body(
    painter: &egui::Painter,
    rect: Rect,
    fill: Color32,
    selected: bool,
    waveform: bool,
) {
    if rect.width() < 1.0 {
        return;
    }
    let radius = THEME.radius as f32;
    painter.rect_filled(rect, radius, fill);
    let sheen = Rect::from_min_max(
        rect.min,
        pos2(rect.right(), rect.top() + rect.height() * 0.34),
    );
    painter.rect_filled(sheen, radius, Color32::from_white_alpha(18));
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
        0.0,
        Color32::from_white_alpha(46),
    );
    if waveform {
        let visible = rect.intersect(painter.clip_rect());
        let mut x = visible.left().max(rect.left() + 8.0);
        let mut seed = (x as u32).wrapping_mul(1664525).wrapping_add(1013904223);
        let right = visible.right().min(rect.right() - 4.0);
        while x < right {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let h = 2.0 + ((seed >> 16) % 100) as f32 / 100.0 * (rect.height() * 0.32);
            painter.vline(
                x,
                (rect.center().y - h)..=(rect.center().y + h),
                Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
            );
            x += 3.0;
        }
    }
    if selected {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.5_f32, THEME.selection),
            egui::StrokeKind::Inside,
        );
    }
}

fn fx(frame: i64, origin: f64, left: f32, ppf: f32) -> f32 {
    timeline_x(frame, origin, left, ppf)
}

fn x_to_frame(x: f32, left: f32, origin: f64, ppf: f32) -> i64 {
    frame_at_x(x, origin, left, ppf)
}

fn paint_transitions(
    painter: &egui::Painter,
    track: &editor_core::Track,
    by_id: &std::collections::HashMap<ClipId, &editor_core::Clip>,
    rect: Rect,
    origin: f64,
    ppf: f32,
) {
    let view = painter.clip_rect();
    for transition in &track.transitions {
        let Some(left) = by_id.get(&transition.left_clip) else {
            continue;
        };
        let (start, end) = transition.range(left.timeline_out);
        let x0 = fx(start.0, origin, rect.min.x, ppf);
        let x1 = fx(end.0, origin, rect.min.x, ppf);
        if x1 < view.min.x - 8.0 || x0 > view.max.x + 8.0 {
            continue;
        }
        let mid_y = rect.center().y;
        let mid_x = (x0 + x1) * 0.5;
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x0, rect.min.y + 5.0),
                pos2(mid_x, mid_y),
                pos2(x0, rect.max.y - 5.0),
            ],
            Color32::from_white_alpha(50),
            Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
        ));
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x1, rect.min.y + 5.0),
                pos2(mid_x, mid_y),
                pos2(x1, rect.max.y - 5.0),
            ],
            Color32::from_white_alpha(50),
            Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
        ));
    }
}
