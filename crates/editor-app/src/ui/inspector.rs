use editor_core::{
    clip_relative, color_grade, set_grade_at, set_transform_at, toggle_grade_key,
    toggle_transform_key, transform, ClipId, EditError, Effect, GradeParam, TransformParam,
};
use egui::{RichText, Slider};

use crate::app::MeridianApp;
use crate::theme;
use crate::ui::format_tc;

pub fn inspector_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let title = match app.workspace {
        crate::app::Workspace::Colour => "COLOUR",
        _ => "INSPECTOR",
    };
    ui.label(RichText::new(title).small().strong());
    ui.add_space(4.0);

    let Some(clip_id) = app.selected.first().copied() else {
        sequence_summary(ui, app);
        return;
    };
    let Some(snapshot) = clip_snapshot(app, clip_id) else {
        sequence_summary(ui, app);
        return;
    };

    ui.label(RichText::new(&snapshot.name).strong());
    ui.label(
        RichText::new(format!(
            "{}   {} – {}   {} frames",
            snapshot.track_name,
            format_tc(snapshot.timeline_in, snapshot.timebase),
            format_tc(snapshot.timeline_out, snapshot.timebase),
            snapshot.timeline_out - snapshot.timeline_in
        ))
        .small()
        .color(theme::DIM),
    );
    ui.label(
        RichText::new(format!(
            "Source {} – {}   head {}   tail {}",
            format_tc(snapshot.source_in, snapshot.media_tb),
            format_tc(snapshot.source_out, snapshot.media_tb),
            snapshot.head_handle,
            snapshot.tail_handle
        ))
        .small()
        .color(theme::DIM),
    );

    let rel = clip_relative(
        editor_core::Frame(app.playhead),
        editor_core::Frame(snapshot.timeline_in),
    )
    .max(0);

    if matches!(app.workspace, crate::app::Workspace::Colour) {
        colour_wheel(ui, app, clip_id, rel);
    }

    ui.add_space(6.0);
    ui.label(
        RichText::new("LIGHTING / COLOUR")
            .small()
            .color(theme::ACCENT),
    );
    grade_slider(
        ui,
        app,
        clip_id,
        rel,
        GradeParam::Exposure,
        "Exposure",
        -3.0..=3.0,
    );
    grade_slider(
        ui,
        app,
        clip_id,
        rel,
        GradeParam::Contrast,
        "Contrast",
        0.0..=2.0,
    );
    grade_slider(
        ui,
        app,
        clip_id,
        rel,
        GradeParam::Highlights,
        "Highlights",
        -1.0..=1.0,
    );
    grade_slider(
        ui,
        app,
        clip_id,
        rel,
        GradeParam::Shadows,
        "Shadows",
        -1.0..=1.0,
    );
    grade_slider(
        ui,
        app,
        clip_id,
        rel,
        GradeParam::Temperature,
        "Temperature",
        -1.0..=1.0,
    );
    grade_slider(ui, app, clip_id, rel, GradeParam::Tint, "Tint", -1.0..=1.0);
    grade_slider(
        ui,
        app,
        clip_id,
        rel,
        GradeParam::Saturation,
        "Saturation",
        0.0..=2.0,
    );

    ui.add_space(6.0);
    ui.label(RichText::new("TRANSFORM").small().color(theme::ACCENT));
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::PositionX,
        "Position X",
        -1600.0..=1600.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::PositionY,
        "Position Y",
        -900.0..=900.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::ScaleX,
        "Scale X",
        0.05..=4.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::ScaleY,
        "Scale Y",
        0.05..=4.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::Rotation,
        "Rotation",
        -180.0..=180.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::AnchorX,
        "Anchor X",
        0.0..=1.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::AnchorY,
        "Anchor Y",
        0.0..=1.0,
    );
    xform_slider(
        ui,
        app,
        clip_id,
        rel,
        TransformParam::Opacity,
        "Opacity",
        0.0..=1.0,
    );

    ui.add_space(6.0);
    ui.label(
        RichText::new("Diamond adds or removes a keyframe at the playhead. Sliders write the constant until a key exists, then they key the current frame.")
            .small()
            .color(theme::DIM),
    );

    if !snapshot.effects.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("EFFECTS").small().color(theme::DIM));
        for effect in &snapshot.effects {
            let name = match effect {
                Effect::Color(_) => "Colour",
                Effect::Transform(_) => "Transform",
            };
            ui.label(name);
        }
    }
}

struct ClipSnap {
    name: String,
    track_name: String,
    timeline_in: i64,
    timeline_out: i64,
    source_in: i64,
    source_out: i64,
    head_handle: i64,
    tail_handle: i64,
    timebase: editor_core::Timebase,
    media_tb: editor_core::Timebase,
    effects: Vec<Effect>,
}

fn clip_snapshot(app: &MeridianApp, id: ClipId) -> Option<ClipSnap> {
    let sequence = app.session.project().active()?;
    let (ti, _) = sequence.locate_clip(id)?;
    let clip = sequence.clip(id)?;
    Some(ClipSnap {
        name: clip.name.clone(),
        track_name: sequence.tracks[ti].name.clone(),
        timeline_in: clip.timeline_in.0,
        timeline_out: clip.timeline_out.0,
        source_in: clip.source_in.0,
        source_out: clip.source_out.0,
        head_handle: clip.head_handle(),
        tail_handle: clip.tail_handle(),
        timebase: sequence.timebase,
        media_tb: clip.media_timebase,
        effects: clip.effects.clone(),
    })
}

fn sequence_summary(ui: &mut egui::Ui, app: &MeridianApp) {
    let Some(sequence) = app.session.project().active() else {
        ui.label("No sequence.");
        return;
    };
    ui.label(RichText::new(&sequence.name).strong());
    ui.label(format!(
        "{}×{}   {:.3} fps",
        sequence.width,
        sequence.height,
        sequence.timebase.fps_f64()
    ));
    ui.label(
        RichText::new("Select a clip to grade, transform, or keyframe it.")
            .small()
            .color(theme::DIM),
    );
}

fn grade_value(app: &MeridianApp, clip: ClipId, rel: i64, param: GradeParam) -> (f32, bool) {
    let neutral = match param {
        GradeParam::Contrast | GradeParam::Saturation => 1.0,
        _ => 0.0,
    };
    let Some(sequence) = app.session.project().active() else {
        return (neutral, false);
    };
    let Some(clip) = sequence.clip(clip) else {
        return (neutral, false);
    };
    if let Some(grade) = color_grade(&clip.effects) {
        let anim = grade.param(param);
        (anim.value_at(rel), anim.has_key(rel))
    } else {
        (neutral, false)
    }
}

fn grade_slider(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    clip: ClipId,
    rel: i64,
    param: GradeParam,
    label: &str,
    range: std::ops::RangeInclusive<f32>,
) {
    let (current, keyed) = grade_value(app, clip, rel, param);
    let mut value = current;
    ui.horizontal(|ui| {
        let response = ui.add(Slider::new(&mut value, range).text(label));
        if response.drag_started() {
            app.session.begin_interactive(label);
        }
        if response.changed() {
            apply_grade(app, clip, rel, param, value);
        }
        if response.drag_stopped() {
            app.session.end_interactive();
        }
        let mark = if keyed { "◆" } else { "◇" };
        if ui
            .small_button(mark)
            .on_hover_text("Keyframe at playhead")
            .clicked()
        {
            toggle_grade(app, clip, rel, param);
        }
    });
}

fn apply_grade(app: &mut MeridianApp, clip: ClipId, rel: i64, param: GradeParam, value: f32) {
    let Ok(seq) = app.session.active_id() else {
        return;
    };
    if let Err(err) = app.session.edit("Grade", |project| {
        set_grade_at(project, seq, clip, param, rel, value)
    }) {
        app.status = err.to_string();
    }
}

fn toggle_grade(app: &mut MeridianApp, clip: ClipId, rel: i64, param: GradeParam) {
    let Ok(seq) = app.session.active_id() else {
        return;
    };
    if let Err(err) = app.session.edit("Keyframe", |project| {
        toggle_grade_key(project, seq, clip, param, rel)
    }) {
        app.status = err.to_string();
    }
}

fn xform_value(app: &MeridianApp, clip: ClipId, rel: i64, param: TransformParam) -> (f32, bool) {
    let neutral = match param {
        TransformParam::ScaleX | TransformParam::ScaleY | TransformParam::Opacity => 1.0,
        TransformParam::AnchorX | TransformParam::AnchorY => 0.5,
        _ => 0.0,
    };
    let Some(sequence) = app.session.project().active() else {
        return (neutral, false);
    };
    let Some(clip) = sequence.clip(clip) else {
        return (neutral, false);
    };
    if let Some(transform) = transform(&clip.effects) {
        let anim = transform.param(param);
        (anim.value_at(rel), anim.has_key(rel))
    } else {
        (neutral, false)
    }
}

fn xform_slider(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    clip: ClipId,
    rel: i64,
    param: TransformParam,
    label: &str,
    range: std::ops::RangeInclusive<f32>,
) {
    let (current, keyed) = xform_value(app, clip, rel, param);
    let mut value = current;
    ui.horizontal(|ui| {
        let response = ui.add(Slider::new(&mut value, range).text(label));
        if response.drag_started() {
            app.session.begin_interactive(label);
        }
        if response.changed() {
            let Ok(seq) = app.session.active_id() else {
                return;
            };
            if let Err(err) = app.session.edit("Transform", |project| {
                set_transform_at(project, seq, clip, param, rel, value)
            }) {
                app.status = err.to_string();
            }
        }
        if response.drag_stopped() {
            app.session.end_interactive();
        }
        let mark = if keyed { "◆" } else { "◇" };
        if ui.small_button(mark).clicked() {
            let Ok(seq) = app.session.active_id() else {
                return;
            };
            if let Err(EditError::ClipNotFound) = app.session.edit("Keyframe", |project| {
                toggle_transform_key(project, seq, clip, param, rel)
            }) {
                app.status = "Clip not found.".into();
            }
        }
    });
}

fn colour_wheel(ui: &mut egui::Ui, app: &mut MeridianApp, clip: ClipId, rel: i64) {
    ui.label(RichText::new("OFFSET").small().color(theme::DIM));
    let (temp, _) = grade_value(app, clip, rel, GradeParam::Temperature);
    let (tint, _) = grade_value(app, clip, rel, GradeParam::Tint);
    let size = egui::vec2(168.0, 168.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.circle_filled(rect.center(), 78.0, theme::PANEL_RAISED);
    painter.circle_stroke(
        rect.center(),
        78.0,
        egui::Stroke::new(1.0_f32, theme::BORDER),
    );
    painter.hline(
        (rect.center().x - 70.0)..=(rect.center().x + 70.0),
        rect.center().y,
        egui::Stroke::new(1.0_f32, theme::BORDER),
    );
    painter.vline(
        rect.center().x,
        (rect.center().y - 70.0)..=(rect.center().y + 70.0),
        egui::Stroke::new(1.0_f32, theme::BORDER),
    );
    let point = rect.center() + egui::vec2(temp * 70.0, -tint * 70.0);
    painter.circle_filled(point, 6.0, theme::AMBER);
    if response.drag_started() {
        app.session.begin_interactive("Colour offset");
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let next_temp = ((pos.x - rect.center().x) / 70.0).clamp(-1.0, 1.0);
            let next_tint = (-(pos.y - rect.center().y) / 70.0).clamp(-1.0, 1.0);
            apply_grade(app, clip, rel, GradeParam::Temperature, next_temp);
            apply_grade(app, clip, rel, GradeParam::Tint, next_tint);
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
    ui.label(
        RichText::new(format!("Temp {temp:+.2}   Tint {tint:+.2}"))
            .small()
            .monospace()
            .color(theme::DIM),
    );
}
