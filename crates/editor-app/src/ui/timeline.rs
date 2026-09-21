//! Timeline: ruler, track headers, clip lanes, transport.

use editor_core::{
    collect_snap_points, expand_linked, snap_span, ClipId, Frame, TrackFlag, TrackId, TrackKind,
    TrimEdge,
};
use egui::{pos2, Color32, CursorIcon, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::{note_track_flag, Drag, DragKind, MeridianApp, Tool};
use crate::theme;
use crate::ui::format_tc;

const HEADER_W: f32 = 168.0;
const RULER_H: f32 = 26.0;
const ROW_H: f32 = 34.0;

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

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(HEADER_W);
            ui.allocate_exact_size(Vec2::new(HEADER_W, RULER_H), Sense::hover());
            for index in &visual {
                header_row(ui, app, &sequence, *index);
            }
        });
        let scroll = egui::ScrollArea::horizontal()
            .id_salt("timeline_body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    app.timeline_width = ui.available_width().max(app.timeline_width);
                    ruler(ui, app, &sequence, content_w, end);
                    for index in &visual {
                        lane(ui, app, &sequence, *index, content_w);
                    }
                });
            });
        let _ = scroll;
    });
}

fn transport(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let timebase = app.timebase();
    let end = app.sequence_end();
    ui.horizontal(|ui| {
        if ui.small_button("|◀").clicked() {
            app.playhead = 0;
            app.playing = false;
        }
        if ui.small_button("◀").clicked() {
            app.playhead = (app.playhead - 1).max(0);
            app.playing = false;
        }
        let play_label = if app.playing { "Pause" } else { "Play" };
        if ui.button(play_label).clicked() {
            app.playing = !app.playing;
            app.play_accum = 0.0;
        }
        if ui.small_button("▶").clicked() {
            app.playhead += 1;
            app.playing = false;
        }
        if ui.small_button("▶|").clicked() {
            app.playhead = end;
            app.playing = false;
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(format_tc(app.playhead, timebase))
                .monospace()
                .size(16.0)
                .color(theme::AMBER),
        );
        ui.label(
            RichText::new(format!("/ {}", format_tc(end, timebase)))
                .monospace()
                .color(theme::DIM),
        );
        ui.separator();
        let (inn, out) = app
            .session
            .project()
            .active()
            .map(|s| (s.in_point.map(|f| f.0), s.out_point.map(|f| f.0)))
            .unwrap_or((None, None));
        ui.label(
            RichText::new(match inn {
                Some(f) => format!("In {}", format_tc(f, timebase)),
                None => "In —".into(),
            })
            .small()
            .monospace(),
        );
        ui.label(
            RichText::new(match out {
                Some(f) => format!("Out {}", format_tc(f, timebase)),
                None => "Out —".into(),
            })
            .small()
            .monospace(),
        );
        if ui.small_button("In").clicked() {
            app.mark_in();
        }
        if ui.small_button("Out").clicked() {
            app.mark_out();
        }
        ui.separator();
        if ui.small_button("−").clicked() {
            app.pixels_per_frame = (app.pixels_per_frame / 1.25).max(0.35);
        }
        let mut zoom = app.pixels_per_frame;
        if ui
            .add(
                egui::Slider::new(&mut zoom, 0.35..=24.0)
                    .show_value(false)
                    .text("zoom"),
            )
            .changed()
        {
            app.pixels_per_frame = zoom;
        }
        if ui.small_button("+").clicked() {
            app.pixels_per_frame = (app.pixels_per_frame * 1.25).min(24.0);
        }
        if ui.small_button("Fit").clicked() {
            app.zoom_to_fit();
        }
    });
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
    painter.rect_filled(rect, 0.0, theme::HEADER);
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
        0.0,
        theme::track_color(track.kind),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(6.0, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(RichText::new(&track.name).small().strong());
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        flag_button(
            ui,
            app,
            track.id,
            "S",
            track.solo,
            TrackFlag::Solo,
            theme::AMBER,
        );
        flag_button(
            ui,
            app,
            track.id,
            "M",
            track.muted,
            TrackFlag::Mute,
            theme::DANGER,
        );
        flag_button(
            ui,
            app,
            track.id,
            "L",
            track.locked,
            TrackFlag::Lock,
            theme::AMBER,
        );
    });
}

fn flag_button(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    track: TrackId,
    label: &str,
    on: bool,
    flag: TrackFlag,
    color: Color32,
) {
    let button = egui::Button::new(RichText::new(label).small().color(if on {
        theme::BG
    } else {
        theme::DIM
    }))
    .min_size(Vec2::new(18.0, 16.0))
    .fill(if on { color } else { theme::PANEL });
    if ui.add(button).clicked() {
        note_track_flag(app, track, flag, !on);
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
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::RULER);
    let ppf = app.pixels_per_frame;
    let fps = sequence.timebase.timecode_fps().max(1);
    let major = fps;
    let mut frame = 0;
    while frame <= end {
        let x = rect.min.x + frame as f32 * ppf;
        if x > rect.max.x + 20.0 {
            break;
        }
        let height = if frame % (major * 5) == 0 {
            12.0
        } else if frame % major == 0 {
            8.0
        } else {
            0.0
        };
        if height > 0.0 {
            painter.vline(
                x,
                rect.bottom() - height..=rect.bottom(),
                Stroke::new(1.0_f32, theme::BORDER),
            );
        }
        if frame % major == 0 && ppf > 1.2 {
            painter.text(
                pos2(x + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                format_tc(frame, sequence.timebase),
                egui::FontId::monospace(10.0),
                theme::DIM,
            );
        }
        frame += if ppf < 1.5 { major } else { major.max(1) };
        if ppf >= 4.0 && frame % major != 0 {
            // already stepping by major
        }
    }
    for marker in &sequence.markers {
        let x = rect.min.x + marker.frame.0 as f32 * ppf;
        painter.text(
            pos2(x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            "▼",
            egui::FontId::proportional(10.0),
            theme::AMBER,
        );
    }
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            app.playhead = x_to_frame(pos.x, rect.min.x, ppf).max(0);
            app.playing = false;
        }
    }
    paint_playhead(&painter, rect, app.playhead, ppf);
}

fn lane(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    sequence: &editor_core::Sequence,
    index: usize,
    content_w: f32,
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
            painter.rect_filled(crect, 2.0, theme::CAPTION);
            painter.with_clip_rect(crect).text(
                crect.left_center() + Vec2::new(4.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &cue.text,
                egui::FontId::proportional(11.0),
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
        painter.rect_filled(crect, 2.0, theme::label_fill(clip.label, track.kind));
        if selected {
            painter.rect_stroke(
                crect,
                2.0,
                Stroke::new(1.5_f32, theme::AMBER),
                egui::StrokeKind::Inside,
            );
        }
        painter.with_clip_rect(crect.shrink(3.0)).text(
            crect.left_center() + Vec2::new(5.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &clip.name,
            egui::FontId::proportional(11.0),
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
        painter.line_segment(
            [pos2(x0, rect.min.y + 4.0), pos2((x0 + x1) * 0.5, mid_y)],
            Stroke::new(1.0_f32, theme::TEXT),
        );
        painter.line_segment(
            [pos2((x0 + x1) * 0.5, mid_y), pos2(x1, rect.min.y + 4.0)],
            Stroke::new(1.0_f32, theme::TEXT),
        );
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
                select_clip(app, sequence, hit.clip_id, false);
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
                app.playing = false;
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
                select_clip(app, sequence, hit.clip_id, ui.input(|i| i.modifiers.shift));
            } else {
                app.playhead = x_to_frame(pos.x, rect.min.x, ppf).max(0);
                app.playing = false;
                if !ui.input(|i| i.modifiers.shift) {
                    app.selected.clear();
                }
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

fn select_clip(app: &mut MeridianApp, sequence: &editor_core::Sequence, id: ClipId, extend: bool) {
    let group = if app.linked_selection {
        expand_linked(sequence, &[id])
    } else {
        vec![id]
    };
    if extend {
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
    app.playing = false;
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
    painter.vline(x, rect.y_range(), Stroke::new(1.0_f32, theme::PLAYHEAD));
}

fn x_to_frame(x: f32, origin: f32, ppf: f32) -> i64 {
    if ppf <= 0.0 {
        0
    } else {
        ((x - origin) / ppf).round() as i64
    }
}
