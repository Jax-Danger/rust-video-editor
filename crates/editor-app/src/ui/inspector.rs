use editor_core::{
    clip_relative, color_grade, set_clip_gain_at, set_grade_at, set_transform_at, toggle_grade_key,
    toggle_transform_key, toggle_volume_key, transform, ClipId, GradeParam, TrackKind,
    TransformParam,
};
use egui::RichText;

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::format_tc;
use crate::ui::widgets;

pub fn inspector_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let title = match app.workspace {
        crate::app::Workspace::Colour => "Colour",
        _ => "Inspector",
    };
    widgets::panel_header(ui, title, |_| {});

    let Some(clip_id) = app.selected.first().copied() else {
        sequence_summary(ui, app);
        return;
    };
    let Some(snapshot) = clip_snapshot(app, clip_id) else {
        sequence_summary(ui, app);
        return;
    };

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("inspector_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    inspector_body(ui, app, clip_id, &snapshot);
                });
        });
}

fn inspector_body(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: ClipId, snapshot: &ClipSnap) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(&snapshot.name).size(15.0).strong());
            ui.label(
                RichText::new(format!(
                    "{}    {} – {}    {} frames",
                    snapshot.track_name,
                    format_tc(snapshot.timeline_in, snapshot.timebase),
                    format_tc(snapshot.timeline_out, snapshot.timebase),
                    snapshot.timeline_out - snapshot.timeline_in
                ))
                .size(11.0)
                .monospace()
                .color(THEME.text_dim),
            );
            ui.label(
                RichText::new(format!(
                    "Source {} – {}    head {}    tail {}",
                    format_tc(snapshot.source_in, snapshot.media_tb),
                    format_tc(snapshot.source_out, snapshot.media_tb),
                    snapshot.head_handle,
                    snapshot.tail_handle
                ))
                .size(11.0)
                .monospace()
                .color(THEME.text_mute),
            );
        });
    });

    let rel = clip_relative(
        editor_core::Frame(app.playhead),
        editor_core::Frame(snapshot.timeline_in),
    )
    .max(0);

    if snapshot.kind == TrackKind::Audio {
        ui.add_space(8.0);
        widgets::section_label(ui, "Sound");
        let (current, keyed) = clip_gain(app, clip_id, rel);
        let edit = widgets::param_slider(ui, "Clip gain", current, 0.0..=2.0, keyed);
        if edit.started {
            app.session.begin_interactive("Clip gain");
        }
        if edit.changed {
            let value = edit.value;
            let result = app.session.edit("Clip gain", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                set_clip_gain_at(project, seq, clip_id, rel, value)
            });
            if let Err(err) = result {
                app.status = err.to_string();
            }
        }
        if edit.stopped {
            app.session.end_interactive();
        }
        if edit.key_clicked {
            let result = app.session.edit("Clip gain key", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                toggle_volume_key(project, seq, clip_id, rel)
            });
            if let Err(err) = result {
                app.status = err.to_string();
            }
        }
    }

    if matches!(app.workspace, crate::app::Workspace::Colour) {
        crate::ui::colour::colour_controls(ui, app, clip_id, rel);
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.vertical(|ui| {
            widgets::section_label(ui, "Lighting");
        });
    });
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

    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.vertical(|ui| {
            widgets::section_label(ui, "Transform");
        });
    });
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

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "A diamond sets a key at the playhead. Until then, the slider is a constant.",
            )
            .size(11.0)
            .color(THEME.text_mute),
        );
    });
    ui.add_space(8.0);
}

struct ClipSnap {
    name: String,
    track_name: String,
    kind: TrackKind,
    timeline_in: i64,
    timeline_out: i64,
    source_in: i64,
    source_out: i64,
    head_handle: i64,
    tail_handle: i64,
    timebase: editor_core::Timebase,
    media_tb: editor_core::Timebase,
}

fn clip_snapshot(app: &MeridianApp, id: ClipId) -> Option<ClipSnap> {
    let sequence = app.session.project().active()?;
    let (ti, _) = sequence.locate_clip(id)?;
    let clip = sequence.clip(id)?;
    Some(ClipSnap {
        name: clip.name.clone(),
        track_name: sequence.tracks[ti].name.clone(),
        kind: sequence.tracks[ti].kind,
        timeline_in: clip.timeline_in.0,
        timeline_out: clip.timeline_out.0,
        source_in: clip.source_in.0,
        source_out: clip.source_out.0,
        head_handle: clip.head_handle(),
        tail_handle: clip.tail_handle(),
        timebase: sequence.timebase,
        media_tb: clip.media_timebase,
    })
}

fn sequence_summary(ui: &mut egui::Ui, app: &MeridianApp) {
    let Some(sequence) = app.session.project().active() else {
        widgets::empty_note(ui, "No sequence is open.");
        return;
    };
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(&sequence.name).size(15.0).strong());
            ui.label(
                RichText::new(format!(
                    "{}×{}    {:.3} fps",
                    sequence.width,
                    sequence.height,
                    sequence.timebase.fps_f64()
                ))
                .monospace()
                .size(12.0)
                .color(THEME.text_dim),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("Select a clip to grade it, move it, or set keyframes.")
                    .size(12.0)
                    .color(THEME.text_mute),
            );
        });
    });
}

fn clip_gain(app: &MeridianApp, clip: ClipId, rel: i64) -> (f32, bool) {
    let Some(sequence) = app.session.project().active() else {
        return (1.0, false);
    };
    let Some(clip) = sequence.clip(clip) else {
        return (1.0, false);
    };
    (clip.volume.value_at(rel), clip.volume.has_key(rel))
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
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| {
            let edit = widgets::param_slider(ui, label, current, range, keyed);
            if edit.started {
                app.session.begin_interactive(label);
            }
            if edit.changed {
                apply_grade(app, clip, rel, param, edit.value);
            }
            if edit.stopped {
                app.session.end_interactive();
            }
            if edit.key_clicked {
                toggle_grade(app, clip, rel, param);
            }
        });
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
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| {
            let edit = widgets::param_slider(ui, label, current, range, keyed);
            if edit.started {
                app.session.begin_interactive(label);
            }
            if edit.changed {
                let Ok(seq) = app.session.active_id() else {
                    return;
                };
                if let Err(err) = app.session.edit("Transform", |project| {
                    set_transform_at(project, seq, clip, param, rel, edit.value)
                }) {
                    app.status = err.to_string();
                }
            }
            if edit.stopped {
                app.session.end_interactive();
            }
            if edit.key_clicked {
                let Ok(seq) = app.session.active_id() else {
                    return;
                };
                if let Err(err) = app.session.edit("Keyframe", |project| {
                    toggle_transform_key(project, seq, clip, param, rel)
                }) {
                    app.status = err.to_string();
                }
            }
        });
    });
}
