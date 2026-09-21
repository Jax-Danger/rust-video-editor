//! Timeline: ruler, track headers, clip lanes, transport.

use std::collections::HashSet;

use editor_core::{
    collect_snap_points, expand_linked, snap_span, ClipId, Frame, MediaId, TrackFlag, TrackKind,
    TrimEdge,
};
use egui::{pos2, Align2, Color32, CursorIcon, FontId, Id, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::{note_track_flag, Drag, DragKind, MeridianApp, ScrubSource, Tool};
use crate::theme::{self, THEME};
use crate::ui::format_tc;
use crate::ui::widgets;

const HEADER_W: f32 = 156.0;
const RULER_H: f32 = 28.0;
const ROW_H: f32 = 36.0;

pub fn timeline_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    transport(ui, app);
    ui.separator();
    let Some(sequence) = app.session.project().active().cloned() else {
        ui.label("No sequence.");
        return;
    };
    let visual = sequence.visual_track_indices();
    let end = (sequence.end_frame().0 + 48).max(app.playhead + 24).max(96);
    let content_w = (end as f32 * app.pixels_per_frame).max(400.0);
    let offline = offline_media(app);
    let body_h = ui.available_height();

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
        ui.allocate_ui_with_layout(
            Vec2::new(body_w, body_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                nudge_timeline_scroll(ui, app);
                let output = egui::ScrollArea::horizontal()
                    .id_salt("timeline_body")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            app.timeline_width = content_w;
                            ruler(ui, app, &sequence, content_w, end);
                            for index in &visual {
                                lane(ui, app, &sequence, *index, content_w, &offline);
                            }
                        });
                    });
                app.timeline_view = Some(output.inner_rect);
            },
        );
    });
    follow_ruler_scrub(ui, app);
}

fn timeline_scroll_id(ui: &egui::Ui) -> Id {
    ui.make_persistent_id(Id::new("timeline_body"))
}

fn nudge_timeline_scroll(ui: &egui::Ui, app: &mut MeridianApp) {
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
    let (dx, dy, zoom, command) = ui.input(|input| {
        (
            input.smooth_scroll_delta.x,
            input.smooth_scroll_delta.y,
            input.zoom_delta(),
            input.modifiers.command || input.modifiers.ctrl,
        )
    });
    let scrolling = dx.abs() + dy.abs() > 0.0;
    let zooming = (zoom - 1.0).abs() > 0.01 || (command && dy.abs() > 0.0);
    if !scrolling && !zooming {
        return;
    }
    ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
    let id = timeline_scroll_id(ui);
    let Some(mut state) = egui::scroll_area::State::load(ui.ctx(), id) else {
        return;
    };
    if zooming {
        let factor = if (zoom - 1.0).abs() > 0.01 {
            zoom
        } else {
            (dy * 0.0016).exp()
        };
        let old = app.pixels_per_frame;
        let new = (old * factor).clamp(0.2, 64.0);
        let content_x = state.offset.x + (pointer.x - view.min.x);
        let frame = if old > 0.0 { content_x / old } else { 0.0 };
        state.offset.x = (frame * new - (pointer.x - view.min.x)).max(0.0);
        app.pixels_per_frame = new;
    } else {
        state.offset.x = (state.offset.x - (dx + dy)).max(0.0);
    }
    state.store(ui.ctx(), id);
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
    app.playhead = x_to_frame(pos.x, ruler.min.x, app.pixels_per_frame).max(0);
    app.preview_scrub = true;
    app.halt_transport();
}

fn transport_scrub(ui: &mut egui::Ui, playhead: i64, end: i64) -> Option<i64> {
    let width = ui.available_width().max(80.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 6.0));
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
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, THEME.header);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    let controls = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 38.0));
    let scrub_row = Rect::from_min_max(pos2(rect.left(), rect.top() + 38.0), rect.max);
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(controls.shrink2(Vec2::new(8.0, 4.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 4.0;
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
    bar.add_space(10.0);
    widgets::readout(&mut bar, &format_tc(app.playhead, timebase), 118.0, true);
    bar.add_space(4.0);
    widgets::readout(&mut bar, &format_tc(end, timebase), 118.0, false);
    bar.add_space(8.0);
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
    if widgets::ghost_button(&mut bar, &in_label) {
        app.mark_in();
    }
    if widgets::ghost_button(&mut bar, &out_label) {
        app.mark_out();
    }
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::ghost_button(ui, "Fit") {
            app.zoom_to_fit();
        }
        if widgets::ghost_button(ui, "+") {
            app.zoom_by(1.25);
        }
        if let Some(zoom) = widgets::mini_slider(ui, app.pixels_per_frame, 0.2..=64.0) {
            app.pixels_per_frame = zoom;
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
                .size(11.0)
                .monospace()
                .color(if app.playing { THEME.accent } else { THEME.text_mute }),
        );
    });
    let mut scrub_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(scrub_row.shrink2(Vec2::new(8.0, 2.0)))
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
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
        0.0,
        theme::track_color(track.kind),
    );
    painter.text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        &track.name,
        FontId::new(12.0, egui::FontFamily::Proportional),
        THEME.text,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 4.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 3.0;
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
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, THEME.hairline),
    );
    let ppf = app.pixels_per_frame;
    let fps = sequence.timebase.timecode_fps().max(1);
    let major = fps;
    if let (Some(inn), Some(out)) = (sequence.in_point, sequence.out_point) {
        let x0 = rect.min.x + inn.0 as f32 * ppf;
        let x1 = rect.min.x + out.0 as f32 * ppf;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x0, rect.bottom() - 3.0),
                pos2(x1.max(x0 + 2.0), rect.bottom()),
            ),
            0.0,
            THEME.accent,
        );
    }
    let step = if ppf < 1.2 { major * 2 } else { major };
    let mut frame = 0;
    while frame <= end {
        let x = rect.min.x + frame as f32 * ppf;
        if x > rect.max.x + 40.0 {
            break;
        }
        let height = if frame % (major * 5) == 0 {
            14.0
        } else if frame % major == 0 {
            9.0
        } else {
            0.0
        };
        if height > 0.0 {
            painter.vline(
                x,
                rect.bottom() - height..=rect.bottom(),
                Stroke::new(1.0_f32, THEME.border),
            );
        }
        if frame % step == 0 && ppf > 0.8 {
            painter.text(
                pos2(x + 4.0, rect.top() + 3.0),
                Align2::LEFT_TOP,
                format_tc(frame, sequence.timebase),
                FontId::monospace(10.0),
                THEME.text_mute,
            );
        }
        frame += major.max(1);
    }
    for marker in &sequence.markers {
        let x = rect.min.x + marker.frame.0 as f32 * ppf;
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
            app.playhead = x_to_frame(pos.x, rect.min.x, ppf).max(0);
            app.preview_scrub = true;
            app.halt_transport();
        }
    }
    if app.reveal_playhead {
        let x = rect.min.x + app.playhead as f32 * ppf;
        let target = Rect::from_center_size(pos2(x, rect.center().y), Vec2::new(48.0, rect.height()));
        ui.scroll_to_rect(target, None);
        app.reveal_playhead = false;
    }
    paint_playhead(&painter, rect, app.playhead, ppf);
}

fn lane(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    sequence: &editor_core::Sequence,
    index: usize,
    content_w: f32,
    offline: &HashSet<MediaId>,
) {
    let track = &sequence.tracks[index];
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(content_w, ROW_H), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let bg = if index % 2 == 0 {
        theme::LANE
    } else {
        theme::LANE_ALT
    };
    painter.rect_filled(rect, 0.0, bg);
    let ppf = app.pixels_per_frame;

    if track.kind == TrackKind::Caption {
        for cue in &track.cues {
            let x0 = rect.min.x + cue.timeline_in.0 as f32 * ppf;
            let x1 = rect.min.x + cue.timeline_out.0 as f32 * ppf;
            let crect = Rect::from_min_max(
                pos2(x0, rect.min.y + 4.0),
                pos2(x1.max(x0 + 4.0), rect.max.y - 4.0),
            );
            paint_clip_body(&painter, crect, theme::CAPTION, false, false);
            painter.with_clip_rect(crect.shrink(4.0)).text(
                crect.left_center() + Vec2::new(6.0, 0.0),
                Align2::LEFT_CENTER,
                &cue.text,
                FontId::new(11.0, egui::FontFamily::Proportional),
                Color32::WHITE,
            );
        }
    }

    for clip in &track.clips {
        let (start, end) = preview_span(app, clip.id, clip.timeline_in.0, clip.timeline_out.0);
        let x0 = rect.min.x + start as f32 * ppf;
        let x1 = rect.min.x + end as f32 * ppf;
        let crect = Rect::from_min_max(
            pos2(x0, rect.min.y + 3.0),
            pos2(x1.max(x0 + 3.0), rect.max.y - 3.0),
        );
        let selected = app.selected.contains(&clip.id);
        let fill = theme::label_fill(clip.label, track.kind);
        let clip_offline = clip.media_id.is_some_and(|id| offline.contains(&id));
        paint_clip_body(
            &painter,
            crect,
            fill,
            selected,
            track.kind == TrackKind::Audio,
        );
        if clip_offline {
            painter.rect_filled(crect, 3.0, Color32::from_black_alpha(90));
            painter.text(
                crect.right_center() - Vec2::new(8.0, 0.0),
                Align2::RIGHT_CENTER,
                "Offline",
                FontId::new(10.0, egui::FontFamily::Proportional),
                THEME.amber,
            );
        }
        painter.with_clip_rect(crect.shrink(4.0)).text(
            crect.left_center() + Vec2::new(7.0, 0.0),
            Align2::LEFT_CENTER,
            &clip.name,
            FontId::new(11.0, egui::FontFamily::Proportional),
            Color32::WHITE,
        );
    }

    for transition in &track.transitions {
        let Some(left) = track.clips.iter().find(|c| c.id == transition.left_clip) else {
            continue;
        };
        let (start, end) = transition.range(left.timeline_out);
        let x0 = rect.min.x + start.0 as f32 * ppf;
        let x1 = rect.min.x + end.0 as f32 * ppf;
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

    paint_playhead(&painter, rect, app.playhead, ppf);

    if let Some(pos) = response.hover_pos() {
        if let Some(hit) = hit_test(track, rect, pos, ppf) {
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
            let frame = x_to_frame(pos.x, rect.min.x, ppf).max(0);
            if let Some(hit) = hit_test(track, rect, pos, ppf) {
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
            let frame = x_to_frame(pos.x, rect.min.x, ppf).max(0);
            razor_at(app, track, frame);
        }
    } else if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(hit) = hit_test(track, rect, pos, ppf) {
                let shift = ui.input(|i| i.modifiers.shift);
                let toggle = ui.input(|i| i.modifiers.command);
                select_clip(app, sequence, hit.clip_id, shift, toggle);
            } else if app.dragging_media.is_none() {
                app.playhead = x_to_frame(pos.x, rect.min.x, ppf).max(0);
                app.halt_transport();
                app.preview_scrub = true;
                if !ui.input(|i| i.modifiers.shift) {
                    app.selected.clear();
                }
            }
        }
    }

    if ui.input(|i| i.pointer.any_released()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if rect.contains(pos) {
                if let Some(id) = app.dragging_media.take() {
                    let frame = x_to_frame(pos.x, rect.min.x, ppf).max(0);
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

fn hit_test(track: &editor_core::Track, rect: Rect, pos: egui::Pos2, ppf: f32) -> Option<Hit> {
    for clip in track.clips.iter().rev() {
        let x0 = rect.min.x + clip.timeline_in.0 as f32 * ppf;
        let x1 = rect.min.x + clip.timeline_out.0 as f32 * ppf;
        let crect = Rect::from_min_max(
            pos2(x0, rect.min.y + 3.0),
            pos2(x1.max(x0 + 4.0), rect.max.y - 3.0),
        );
        if crect.contains(pos) {
            let edge = if (pos.x - crect.min.x).abs() <= 6.0 {
                Some(TrimEdge::Head)
            } else if (pos.x - crect.max.x).abs() <= 6.0 {
                Some(TrimEdge::Tail)
            } else {
                None
            };
            return Some(Hit {
                clip_id: clip.id,
                edge,
            });
        }
    }
    None
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
    if let Some(clip) = track.clips.iter().find(|c| c.contains_frame(Frame(frame))) {
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

fn paint_playhead(painter: &egui::Painter, rect: Rect, frame: i64, ppf: f32) {
    let x = rect.min.x + frame as f32 * ppf;
    painter.vline(x, rect.y_range(), Stroke::new(1.5_f32, THEME.playhead));
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
    painter.rect_filled(rect, 3.0, fill);
    let sheen = Rect::from_min_max(
        rect.min,
        pos2(rect.right(), rect.top() + rect.height() * 0.42),
    );
    painter.rect_filled(sheen, 3.0, Color32::from_white_alpha(28));
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
        0.0,
        Color32::from_white_alpha(50),
    );
    if waveform {
        let mut x = rect.left() + 8.0;
        let mut seed = (rect.left() as u32)
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        while x < rect.right() - 4.0 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let h = 3.0 + ((seed >> 16) % 100) as f32 / 100.0 * (rect.height() * 0.28);
            painter.vline(
                x,
                (rect.center().y - h)..=(rect.center().y + h),
                Stroke::new(1.0_f32, Color32::from_white_alpha(70)),
            );
            x += 3.5;
        }
    }
    if selected {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.6_f32, THEME.selection),
            egui::StrokeKind::Inside,
        );
    }
}

fn x_to_frame(x: f32, origin: f32, ppf: f32) -> i64 {
    if ppf <= 0.0 {
        0
    } else {
        ((x - origin) / ppf).round() as i64
    }
}
