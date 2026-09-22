use editor_core::{
    blur, clip_relative, color_grade, crop, set_clip_gain_at, set_clip_speed, set_clip_title,
    set_filter_at, set_grade_at, set_transform_at, sharpen, source_frame_at, toggle_filter_key,
    toggle_grade_key, toggle_transform_key, toggle_volume_key, transform, vignette, ClipId,
    ClipSpeed, FilterParam, GradeParam, TextAlign, Title, TrackKind, TransformParam,
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
            if snapshot.is_title {
                ui.label(
                    RichText::new("Title generator")
                        .size(11.0)
                        .monospace()
                        .color(THEME.text_mute),
                );
            } else if snapshot.is_adjustment {
                ui.label(
                    RichText::new("Adjustment layer")
                        .size(11.0)
                        .monospace()
                        .color(THEME.text_mute),
                );
            } else {
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
            }
        });
    });

    multicam_controls(ui, app, clip_id);

    if snapshot.is_title {
        title_controls(ui, app, clip_id);
    } else if !snapshot.is_adjustment {
        speed_controls(ui, app, clip_id);
    }

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

    if snapshot.kind == TrackKind::Video {
        effects_controls(ui, app, clip_id, rel);
    }

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
    is_title: bool,
    is_adjustment: bool,
    timeline_in: i64,
    timeline_out: i64,
    source_in: i64,
    source_out: i64,
    head_handle: i64,
    tail_handle: i64,
    timebase: editor_core::Timebase,
    media_tb: editor_core::Timebase,
}

fn speed_controls(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: ClipId) {
    let Some(speed) = clip_speed(app, clip_id) else {
        return;
    };
    ui.add_space(8.0);
    widgets::section_label(ui, "Speed");
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let start = (speed.start_rate() * 100.0).round();
        let ramping = speed.ramp_end().is_some();
        for preset in [25.0_f32, 50.0, 100.0, 200.0, 400.0] {
            let on = !ramping && (start - preset).abs() < 0.5;
            if ui.selectable_label(on, format!("{preset:.0}%")).clicked() && !on {
                write_speed(
                    app,
                    clip_id,
                    ClipSpeed::constant(preset / 100.0, speed.reverse),
                    "Speed",
                );
            }
        }
    });

    let speed = clip_speed(app, clip_id).unwrap_or(speed);
    let percent = speed.start_rate() * 100.0;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| {
            let edit = widgets::value_slider(ui, "Speed %", percent, 25.0..=400.0);
            if edit.started {
                app.session.begin_interactive("Speed");
            }
            if edit.changed {
                let current = clip_speed(app, clip_id).unwrap_or_else(ClipSpeed::normal);
                let rate = edit.value.round().clamp(25.0, 400.0) / 100.0;
                let next = match current.ramp_end() {
                    Some(end) => ClipSpeed::ramp(rate, end, current.reverse),
                    None => ClipSpeed::constant(rate, current.reverse),
                };
                write_speed(app, clip_id, next, "Speed");
            }
            if edit.stopped {
                app.session.end_interactive();
            }
        });
    });

    let speed = clip_speed(app, clip_id).unwrap_or(speed);
    let mut ramp = speed.ramp_end().is_some();
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.checkbox(&mut ramp, "Ramp over clip").changed() {
            let current = clip_speed(app, clip_id).unwrap_or_else(ClipSpeed::normal);
            let start = current.start_rate();
            let next = if ramp {
                let end = (start * 2.0).clamp(0.25, 4.0);
                ClipSpeed::ramp(start, end, current.reverse)
            } else {
                ClipSpeed::constant(start, current.reverse)
            };
            write_speed(app, clip_id, next, "Speed ramp");
        }
    });
    if let Some(end) = clip_speed(app, clip_id).and_then(|speed| speed.ramp_end()) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.vertical(|ui| {
                let edit = widgets::value_slider(ui, "Ramp to %", end * 100.0, 25.0..=400.0);
                if edit.started {
                    app.session.begin_interactive("Speed ramp");
                }
                if edit.changed {
                    let current = clip_speed(app, clip_id).unwrap_or_else(ClipSpeed::normal);
                    let rate = edit.value.round().clamp(25.0, 400.0) / 100.0;
                    write_speed(
                        app,
                        clip_id,
                        ClipSpeed::ramp(current.start_rate(), rate, current.reverse),
                        "Speed ramp",
                    );
                }
                if edit.stopped {
                    app.session.end_interactive();
                }
            });
        });
    }

    let speed = clip_speed(app, clip_id).unwrap_or(speed);
    let mut reverse = speed.reverse;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.checkbox(&mut reverse, "Reverse").changed() {
            let current = clip_speed(app, clip_id).unwrap_or_else(ClipSpeed::normal);
            let next = match current.ramp_end() {
                Some(end) => ClipSpeed::ramp(current.start_rate(), end, reverse),
                None => ClipSpeed::constant(current.start_rate(), reverse),
            };
            write_speed(app, clip_id, next, "Reverse");
        }
    });

    if let Some(line) = playhead_source_line(app, clip_id) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new(line)
                    .size(11.0)
                    .monospace()
                    .color(THEME.text_dim),
            );
        });
    }
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "Duration ripples with the average speed and later sync-locked clips shift. Linked clips follow. Audio on a retimed clip is muted.",
            )
            .size(11.0)
            .color(THEME.text_mute),
        );
    });
}

fn clip_speed(app: &MeridianApp, id: ClipId) -> Option<ClipSpeed> {
    Some(app.session.project().active()?.clip(id)?.speed.clone())
}

fn write_speed(app: &mut MeridianApp, clip_id: ClipId, speed: ClipSpeed, label: &str) {
    let result = app.session.edit(label, |project| {
        let sequence = project
            .active_sequence
            .ok_or(editor_core::EditError::NoActiveSequence)?;
        set_clip_speed(project, sequence, clip_id, speed)
    });
    if let Err(err) = result {
        app.status = err.to_string();
    }
}

fn playhead_source_line(app: &MeridianApp, id: ClipId) -> Option<String> {
    let sequence = app.session.project().active()?;
    let clip = sequence.clip(id)?;
    if !clip.covers(editor_core::Frame(app.playhead)) {
        return None;
    }
    let frame = source_frame_at(clip, editor_core::Frame(app.playhead), sequence.timebase);
    Some(format!(
        "Playhead source {}",
        format_tc(frame.0, clip.media_timebase)
    ))
}

fn multicam_controls(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: ClipId) {
    let Some(info) = multicam_info(app, clip_id) else {
        return;
    };
    ui.add_space(8.0);
    widgets::section_label(ui, "Multicam");
    ui.label(
        RichText::new(format!(
            "{}    angle {} · {}",
            info.group_name,
            info.active + 1,
            info.angles
                .get(info.active as usize)
                .map(|a| a.0.as_str())
                .unwrap_or("—")
        ))
        .size(12.0)
        .color(THEME.text),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (index, (name, _)) in info.angles.iter().enumerate() {
            let active = info.active == index as u32;
            if ui
                .selectable_label(active, format!("{}  {name}", index + 1))
                .clicked()
                && !active
            {
                app.switch_multicam_angle(index as u32);
            }
        }
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new("Sync is the source frame that lines up with the start of the group.")
            .size(11.0)
            .color(THEME.text_mute),
    );
    for (index, (name, sync)) in info.angles.iter().enumerate() {
        let mut offset = *sync;
        let limit = info.limits.get(index).copied().unwrap_or(10_000).max(0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).size(12.0).color(THEME.text));
            let response = ui.add(
                egui::DragValue::new(&mut offset)
                    .range(0..=limit)
                    .speed(0.25)
                    .suffix(" fr"),
            );
            app.note_text_focus(&response);
            if response.drag_started() {
                app.session.begin_interactive("Angle sync");
            }
            if response.changed() {
                app.set_multicam_sync(info.group, index as u32, offset);
            }
            if response.drag_stopped() || response.lost_focus() {
                app.session.end_interactive();
            }
        });
    }
}

struct McInfo {
    group: editor_core::MulticamId,
    group_name: String,
    active: u32,
    angles: Vec<(String, i64)>,
    limits: Vec<i64>,
}

fn multicam_info(app: &MeridianApp, clip_id: ClipId) -> Option<McInfo> {
    let project = app.session.project();
    let sequence = project.active()?;
    let clip = sequence.clip(clip_id)?;
    let binding = clip.multicam.as_ref()?;
    let group = project.multicam_group(binding.group)?;
    let group_time =
        editor_core::source_frame_at(clip, editor_core::Frame(app.playhead), sequence.timebase).0;
    let active = if clip.covers(editor_core::Frame(app.playhead)) {
        editor_core::active_angle(&binding.cuts, group_time)
    } else {
        editor_core::active_angle(&binding.cuts, clip.source_in.0)
    };
    let active = if (active as usize) < group.angles.len() {
        active
    } else {
        0
    };
    let mut limits = Vec::new();
    let angles = group
        .angles
        .iter()
        .map(|angle| {
            let limit = project
                .media(angle.video)
                .map(|media| media.duration.0.saturating_sub(1))
                .unwrap_or(0);
            limits.push(limit);
            (angle.name.clone(), angle.sync_offset.0)
        })
        .collect();
    Some(McInfo {
        group: group.id,
        group_name: group.name.clone(),
        active,
        angles,
        limits,
    })
}

fn title_controls(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: ClipId) {
    let Some(title) = clip_title(app, clip_id) else {
        return;
    };
    ui.add_space(8.0);
    widgets::section_label(ui, "Title");
    let mut text = title.text.clone();
    let response = ui.add(
        egui::TextEdit::multiline(&mut text)
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    app.note_text_focus(&response);
    if response.gained_focus() {
        app.session.begin_interactive("Title text");
    }
    if response.changed() {
        let mut next = title.clone();
        next.text = text;
        write_title(app, clip_id, next, "Title text");
    }
    if response.lost_focus() {
        app.session.end_interactive();
    }

    let title = clip_title(app, clip_id).unwrap_or(title);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Align").size(12.0).color(THEME.text));
        for (align, label) in [
            (TextAlign::Left, "Left"),
            (TextAlign::Center, "Center"),
            (TextAlign::Right, "Right"),
        ] {
            if ui.selectable_label(title.align == align, label).clicked() && title.align != align {
                let mut next = title.clone();
                next.align = align;
                write_title(app, clip_id, next, "Title align");
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Colour").size(12.0).color(THEME.text));
        let mut color = egui::Color32::from_rgba_unmultiplied(
            (title.color[0] * 255.0).round() as u8,
            (title.color[1] * 255.0).round() as u8,
            (title.color[2] * 255.0).round() as u8,
            (title.color[3] * 255.0).round() as u8,
        );
        let response = ui.color_edit_button_srgba(&mut color);
        if response.drag_started() {
            app.session.begin_interactive("Title colour");
        }
        if response.changed() {
            let mut next = clip_title(app, clip_id).unwrap_or(title.clone());
            next.color = [
                color.r() as f32 / 255.0,
                color.g() as f32 / 255.0,
                color.b() as f32 / 255.0,
                color.a() as f32 / 255.0,
            ];
            write_title(app, clip_id, next, "Title colour");
        }
        if response.drag_stopped() {
            app.session.end_interactive();
        }
    });

    let title = clip_title(app, clip_id).unwrap_or(title);
    title_slider(
        ui,
        app,
        clip_id,
        &title,
        "Size %",
        title.font_size * 100.0,
        2.0..=18.0,
        |value| value / 100.0,
        |title, value| title.font_size = value,
    );
    title_slider(
        ui,
        app,
        clip_id,
        &title,
        "Position X",
        title.x,
        0.0..=1.0,
        |value| value,
        |title, value| {
            title.x = value;
        },
    );
    title_slider(
        ui,
        app,
        clip_id,
        &title,
        "Position Y",
        title.y,
        0.0..=1.0,
        |value| value,
        |title, value| {
            title.y = value;
        },
    );
    title_slider(
        ui,
        app,
        clip_id,
        &title,
        "Plate",
        title.plate,
        0.0..=1.0,
        |value| value,
        |title, value| {
            title.plate = value;
        },
    );
}

fn title_slider(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    clip_id: ClipId,
    title: &Title,
    label: &str,
    shown: f32,
    range: std::ops::RangeInclusive<f32>,
    store: impl Fn(f32) -> f32,
    assign: impl Fn(&mut Title, f32),
) {
    let edit = widgets::value_slider(ui, label, shown, range);
    if edit.started {
        app.session.begin_interactive(label);
    }
    if edit.changed {
        let mut next = clip_title(app, clip_id).unwrap_or_else(|| title.clone());
        assign(&mut next, store(edit.value));
        write_title(app, clip_id, next, label);
    }
    if edit.stopped {
        app.session.end_interactive();
    }
}

fn clip_title(app: &MeridianApp, id: ClipId) -> Option<Title> {
    app.session.project().active()?.clip(id)?.title.clone()
}

fn write_title(app: &mut MeridianApp, clip_id: ClipId, title: Title, label: &str) {
    let result = app.session.edit(label, |project| {
        let sequence = project
            .active_sequence
            .ok_or(editor_core::EditError::NoActiveSequence)?;
        set_clip_title(project, sequence, clip_id, title)
    });
    if let Err(err) = result {
        app.status = err.to_string();
    }
}

fn clip_snapshot(app: &MeridianApp, id: ClipId) -> Option<ClipSnap> {
    let sequence = app.session.project().active()?;
    let (ti, _) = sequence.locate_clip(id)?;
    let clip = sequence.clip(id)?;
    Some(ClipSnap {
        name: clip.name.clone(),
        track_name: sequence.tracks[ti].name.clone(),
        kind: sequence.tracks[ti].kind,
        is_title: clip.is_title(),
        is_adjustment: clip.is_adjustment(),
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

fn effects_controls(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: ClipId, rel: i64) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.vertical(|ui| {
            widgets::section_label(ui, "Effects");
        });
    });
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::BlurRadius,
        "Blur",
        0.0..=48.0,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::VignetteAmount,
        "Vignette",
        0.0..=1.0,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::VignetteSoftness,
        "Vignette softness",
        0.05..=1.0,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::CropLeft,
        "Crop left",
        0.0..=0.4,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::CropRight,
        "Crop right",
        0.0..=0.4,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::CropTop,
        "Crop top",
        0.0..=0.4,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::CropBottom,
        "Crop bottom",
        0.0..=0.4,
    );
    filter_slider(
        ui,
        app,
        clip_id,
        rel,
        FilterParam::SharpenAmount,
        "Sharpen",
        0.0..=2.0,
    );
}

fn filter_value(app: &MeridianApp, clip: ClipId, rel: i64, param: FilterParam) -> (f32, bool) {
    let neutral = match param {
        FilterParam::VignetteSoftness => 0.5,
        _ => 0.0,
    };
    let Some(sequence) = app.session.project().active() else {
        return (neutral, false);
    };
    let Some(clip) = sequence.clip(clip) else {
        return (neutral, false);
    };
    let anim = match param {
        FilterParam::BlurRadius => blur(&clip.effects).map(|f| &f.radius),
        FilterParam::VignetteAmount => vignette(&clip.effects).map(|f| &f.amount),
        FilterParam::VignetteSoftness => vignette(&clip.effects).map(|f| &f.softness),
        FilterParam::CropLeft => crop(&clip.effects).map(|f| &f.left),
        FilterParam::CropRight => crop(&clip.effects).map(|f| &f.right),
        FilterParam::CropTop => crop(&clip.effects).map(|f| &f.top),
        FilterParam::CropBottom => crop(&clip.effects).map(|f| &f.bottom),
        FilterParam::SharpenAmount => sharpen(&clip.effects).map(|f| &f.amount),
    };
    if let Some(anim) = anim {
        (anim.value_at(rel), anim.has_key(rel))
    } else {
        (neutral, false)
    }
}

fn filter_slider(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    clip: ClipId,
    rel: i64,
    param: FilterParam,
    label: &str,
    range: std::ops::RangeInclusive<f32>,
) {
    let (current, keyed) = filter_value(app, clip, rel, param);
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
                if let Err(err) = app.session.edit("Effect", |project| {
                    set_filter_at(project, seq, clip, param, rel, edit.value)
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
                if let Err(err) = app.session.edit("Effect key", |project| {
                    toggle_filter_key(project, seq, clip, param, rel)
                }) {
                    app.status = err.to_string();
                }
            }
        });
    });
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
