//! Lift / gamma / gain wheels and the luma curve editor for the Colour workspace.

use editor_core::{
    clear_lut, color_grade, lut, set_filter_at, set_luma_curve_point, set_lut_look,
    set_wheel_offsets_at, toggle_filter_key, FilterParam, WheelKind,
};
use egui::{Color32, RichText, Sense, Shape, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::widgets;

const WHEEL_RADIUS: f32 = 54.0;
const WHEEL_SIZE: f32 = 128.0;

pub fn colour_controls(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: editor_core::ClipId, rel: i64) {
    widgets::section_label(ui, "Wheels");
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        offset_wheel(ui, app, clip_id, rel, WheelKind::Lift, "Lift");
        offset_wheel(ui, app, clip_id, rel, WheelKind::Gamma, "Gamma");
        offset_wheel(ui, app, clip_id, rel, WheelKind::Gain, "Gain");
    });

    widgets::section_label(ui, "Luma curve");
    luma_curve_editor(ui, app, clip_id);

    widgets::section_label(ui, "White balance");
    temp_tint_wheel(ui, app, clip_id, rel);

    widgets::section_label(ui, "3D LUT");
    lut_controls(ui, app, clip_id, rel);
}

fn offset_wheel(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    clip_id: editor_core::ClipId,
    rel: i64,
    kind: WheelKind,
    label: &str,
) {
    let (r, g, b) = wheel_values(app, clip_id, rel, kind);
    let (u, v) = rgb_offsets_to_wheel(r, g, b);
    let size = Vec2::splat(WHEEL_SIZE);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let spare = (ui.available_width() - size.x).max(0.0) * 0.5;
            ui.add_space(spare);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            let center = rect.center();
            let radius = WHEEL_RADIUS;
            for step in 0..48 {
                let a0 = step as f32 / 48.0 * std::f32::consts::TAU;
                let a1 = (step + 1) as f32 / 48.0 * std::f32::consts::TAU;
                let p0 = center + Vec2::new(a0.cos(), a0.sin()) * radius;
                let p1 = center + Vec2::new(a1.cos(), a1.sin()) * radius;
                painter.add(Shape::convex_polygon(
                    vec![center, p0, p1],
                    wheel_hue(a0),
                    Stroke::NONE,
                ));
            }
            painter.circle_filled(center, 18.0, THEME.inset);
            painter.circle_stroke(center, radius, Stroke::new(1.0, THEME.border));
            painter.hline(
                (center.x - radius)..=(center.x + radius),
                center.y,
                Stroke::new(1.0, Color32::from_white_alpha(24)),
            );
            painter.vline(
                center.x,
                (center.y - radius)..=(center.y + radius),
                Stroke::new(1.0, Color32::from_white_alpha(24)),
            );
            let point = center + Vec2::new(u * radius, -v * radius);
            painter.circle_filled(point, 5.0, Color32::WHITE);
            painter.circle_stroke(point, 5.0, Stroke::new(1.5, THEME.bg));

            let wheel_label = match kind {
                WheelKind::Lift => "Colour offset",
                WheelKind::Gamma => "Colour offset",
                WheelKind::Gain => "Colour offset",
            };
            if response.drag_started() {
                app.session.begin_interactive(wheel_label);
            }
            if response.dragged() || response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let nu = ((pos.x - center.x) / radius).clamp(-1.0, 1.0);
                    let nv = (-(pos.y - center.y) / radius).clamp(-1.0, 1.0);
                    let (nr, ng, nb) = wheel_pos_to_rgb_offsets(nu, nv);
                    apply_wheel(app, clip_id, rel, kind, nr, ng, nb);
                }
            }
            if response.drag_stopped() {
                app.session.end_interactive();
            }
        });
        ui.horizontal(|ui| {
            let spare = (ui.available_width() - 72.0).max(0.0) * 0.5;
            ui.add_space(spare);
            ui.label(
                RichText::new(label)
                    .size(11.0)
                    .strong()
                    .color(THEME.text_dim),
            );
        });
        ui.horizontal(|ui| {
            let spare = (ui.available_width() - 96.0).max(0.0) * 0.5;
            ui.add_space(spare);
            ui.label(
                RichText::new(format!("{r:+.2} {g:+.2} {b:+.2}"))
                    .size(10.0)
                    .monospace()
                    .color(THEME.text_mute),
            );
        });
    });
}

fn luma_curve_editor(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: editor_core::ClipId) {
    let points = curve_points(app, clip_id);
    let size = Vec2::new(ui.available_width() - 16.0, 120.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, THEME.inset);
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, THEME.border), egui::StrokeKind::Inside);

        for level in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let y = rect.bottom() - level * rect.height();
            let x = rect.left() + level * rect.width();
            painter.hline(rect.x_range(), y, Stroke::new(1.0, Color32::from_white_alpha(18)));
            painter.vline(x, rect.y_range(), Stroke::new(1.0, Color32::from_white_alpha(18)));
        }

        let mut curve_pts = Vec::new();
        for (input, output) in &points {
            curve_pts.push(map_curve_point(rect, *input, *output));
        }
        if curve_pts.len() >= 2 {
            painter.add(Shape::line(
                curve_pts,
                Stroke::new(2.0, THEME.accent),
            ));
        }

        for (index, (input, output)) in points.iter().enumerate() {
            let p = map_curve_point(rect, *input, *output);
            let fixed = index == 0 || index + 1 == points.len();
            let radius = if fixed { 3.0 } else { 5.0 };
            let color = if fixed {
                THEME.border
            } else {
                Color32::WHITE
            };
            painter.circle_filled(p, radius, color);
            if !fixed {
                painter.circle_stroke(p, radius, Stroke::new(1.5, THEME.bg));
            }
        }

        if response.drag_started() {
            app.session.begin_interactive("Luma curve");
        }
        if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let mut best = None;
                for (index, (input, output)) in points.iter().enumerate() {
                    if index == 0 || index + 1 == points.len() {
                        continue;
                    }
                    let p = map_curve_point(rect, *input, *output);
                    let dist = (pos - p).length();
                    if dist < 12.0 && best.map_or(true, |(_, d)| dist < d) {
                        best = Some((index, dist));
                    }
                }
                if let Some((index, _)) = best {
                    let y = 1.0 - ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                    apply_curve_point(app, clip_id, index, y);
                }
            }
        }
        if response.drag_stopped() {
            app.session.end_interactive();
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new("Drag the interior points. Endpoints stay fixed.")
                .size(10.0)
                .color(THEME.text_mute),
        );
    });
}

fn temp_tint_wheel(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: editor_core::ClipId, rel: i64) {
    use editor_core::{GradeParam, set_grade_at};
    let (temp, _) = grade_scalar(app, clip_id, rel, GradeParam::Temperature);
    let (tint, _) = grade_scalar(app, clip_id, rel, GradeParam::Tint);
    let size = Vec2::splat(176.0);
    ui.horizontal(|ui| {
        let spare = (ui.available_width() - size.x).max(0.0) * 0.5;
        ui.add_space(spare);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let center = rect.center();
        let radius = 72.0;
        for step in 0..64 {
            let a0 = step as f32 / 64.0 * std::f32::consts::TAU;
            let a1 = (step + 1) as f32 / 64.0 * std::f32::consts::TAU;
            let p0 = center + Vec2::new(a0.cos(), a0.sin()) * radius;
            let p1 = center + Vec2::new(a1.cos(), a1.sin()) * radius;
            painter.add(Shape::convex_polygon(
                vec![center, p0, p1],
                temp_tint_hue(a0),
                Stroke::NONE,
            ));
        }
        painter.circle_filled(center, 28.0, THEME.inset);
        painter.circle_stroke(center, radius, Stroke::new(1.0, THEME.border));
        let point = center + Vec2::new(temp * radius, -tint * radius);
        painter.circle_filled(point, 6.0, Color32::WHITE);
        painter.circle_stroke(point, 6.0, Stroke::new(2.0, THEME.bg));
        if response.drag_started() {
            app.session.begin_interactive("White balance");
        }
        if response.dragged() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let next_temp = ((pos.x - center.x) / radius).clamp(-1.0, 1.0);
                let next_tint = (-(pos.y - center.y) / radius).clamp(-1.0, 1.0);
                let Ok(seq) = app.session.active_id() else {
                    return;
                };
                if let Err(err) = app.session.edit("Temperature", |project| {
                    set_grade_at(project, seq, clip_id, GradeParam::Temperature, rel, next_temp)
                }) {
                    app.status = err.to_string();
                }
                if let Err(err) = app.session.edit("Tint", |project| {
                    set_grade_at(project, seq, clip_id, GradeParam::Tint, rel, next_tint)
                }) {
                    app.status = err.to_string();
                }
            }
        }
        if response.drag_stopped() {
            app.session.end_interactive();
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(format!("Temp {temp:+.2}     Tint {tint:+.2}"))
                .size(11.0)
                .monospace()
                .color(THEME.text_dim),
        );
    });
}

fn map_curve_point(rect: egui::Rect, input: f32, output: f32) -> egui::Pos2 {
    egui::pos2(
        rect.left() + input * rect.width(),
        rect.bottom() - output * rect.height(),
    )
}

fn wheel_values(
    app: &MeridianApp,
    clip_id: editor_core::ClipId,
    rel: i64,
    kind: WheelKind,
) -> (f32, f32, f32) {
    let Some(sequence) = app.session.project().active() else {
        return (0.0, 0.0, 0.0);
    };
    let Some(clip) = sequence.clip(clip_id) else {
        return (0.0, 0.0, 0.0);
    };
    if let Some(grade) = color_grade(&clip.effects) {
        let wheel = grade.wheel(kind);
        let values = wheel.values_at(rel);
        return (values[0], values[1], values[2]);
    }
    (0.0, 0.0, 0.0)
}

fn curve_points(app: &MeridianApp, clip_id: editor_core::ClipId) -> Vec<(f32, f32)> {
    let Some(sequence) = app.session.project().active() else {
        return editor_core::ToneCurve::identity().points;
    };
    let Some(clip) = sequence.clip(clip_id) else {
        return editor_core::ToneCurve::identity().points;
    };
    if let Some(grade) = color_grade(&clip.effects) {
        return grade.luma_curve.points.clone();
    }
    editor_core::ToneCurve::identity().points
}

fn apply_wheel(
    app: &mut MeridianApp,
    clip_id: editor_core::ClipId,
    rel: i64,
    kind: WheelKind,
    red: f32,
    green: f32,
    blue: f32,
) {
    let Ok(seq) = app.session.active_id() else {
        return;
    };
    if let Err(err) = app.session.edit("Wheel", |project| {
        set_wheel_offsets_at(project, seq, clip_id, kind, rel, red, green, blue)
    }) {
        app.status = err.to_string();
    }
}

fn apply_curve_point(app: &mut MeridianApp, clip_id: editor_core::ClipId, index: usize, y: f32) {
    let Ok(seq) = app.session.active_id() else {
        return;
    };
    if let Err(err) = app.session.edit("Luma curve", |project| {
        set_luma_curve_point(project, seq, clip_id, index, y)
    }) {
        app.status = err.to_string();
    }
}

fn grade_scalar(
    app: &MeridianApp,
    clip_id: editor_core::ClipId,
    rel: i64,
    param: editor_core::GradeParam,
) -> (f32, bool) {
    let neutral = match param {
        editor_core::GradeParam::Contrast | editor_core::GradeParam::Saturation => 1.0,
        _ => 0.0,
    };
    let Some(sequence) = app.session.project().active() else {
        return (neutral, false);
    };
    let Some(clip) = sequence.clip(clip_id) else {
        return (neutral, false);
    };
    if let Some(grade) = color_grade(&clip.effects) {
        let anim = grade.param(param);
        return (anim.value_at(rel), anim.has_key(rel));
    }
    (neutral, false)
}

fn wheel_pos_to_rgb_offsets(u: f32, v: f32) -> (f32, f32, f32) {
    let strength = (u * u + v * v).sqrt().min(1.0);
    if strength < 0.01 {
        return (0.0, 0.0, 0.0);
    }
    let angle = f32::atan2(v, u);
    let r = strength * (angle.cos() * 0.5 + 0.5) - strength * 0.5;
    let g = strength * ((angle + 2.094395).cos() * 0.5 + 0.5) - strength * 0.5;
    let b = strength * ((angle - 2.094395).cos() * 0.5 + 0.5) - strength * 0.5;
    (r.clamp(-0.5, 0.5), g.clamp(-0.5, 0.5), b.clamp(-0.5, 0.5))
}

fn rgb_offsets_to_wheel(r: f32, g: f32, b: f32) -> (f32, f32) {
    let strength = (r * r + g * g + b * b).sqrt();
    if strength < 0.01 {
        return (0.0, 0.0);
    }
    let u = (r * 0.6 + g * -0.3 + b * -0.3) / strength;
    let v = (g * 0.6 + r * -0.3 + b * -0.3) / strength;
    let len = (u * u + v * v).sqrt().max(1.0e-4);
    let scale = strength.min(1.0) / len;
    (u * scale, v * scale)
}

fn wheel_hue(angle: f32) -> Color32 {
    let warm = (angle.cos() * 0.5 + 0.5).clamp(0.0, 1.0);
    let cool = (angle.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    Color32::from_rgb(
        (50.0 + warm * 160.0) as u8,
        (60.0 + (1.0 - warm) * 70.0 + cool * 30.0) as u8,
        (55.0 + (1.0 - warm) * 120.0) as u8,
    )
}

fn temp_tint_hue(angle: f32) -> Color32 {
    let warm = (angle.cos() * 0.5 + 0.5).clamp(0.0, 1.0);
    let magenta = (angle.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    Color32::from_rgb(
        (40.0 + warm * 180.0) as u8,
        (70.0 + (1.0 - magenta) * 90.0) as u8,
        (50.0 + (1.0 - warm) * 140.0 + magenta * 40.0) as u8,
    )
}

fn lut_controls(ui: &mut egui::Ui, app: &mut MeridianApp, clip_id: editor_core::ClipId, rel: i64) {
    let (label, path, offline) = lut_status(app, clip_id);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            RichText::new(label)
                .size(11.0)
                .color(if offline {
                    THEME.amber
                } else if path.is_some() {
                    THEME.text_dim
                } else {
                    THEME.text_mute
                }),
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.button("Choose LUT…").clicked() {
            if let Some(file) = crate::dialogs::pick_lut_file() {
                match editor_media::parse_cube_file(&file) {
                    Ok(parsed) => {
                        let embedded = parsed.should_embed().then(|| parsed.to_embedded());
                        let title = parsed.title.clone();
                        let path = file.to_string_lossy().into_owned();
                        let Ok(seq) = app.session.active_id() else {
                            return;
                        };
                        if let Err(err) = app.session.edit("3D LUT", |project| {
                            set_lut_look(project, seq, clip_id, path, title, embedded)
                        }) {
                            app.status = err.to_string();
                        }
                    }
                    Err(err) => app.status = err.to_string(),
                }
            }
        }
        if path.is_some() && ui.button("Clear").clicked() {
            let Ok(seq) = app.session.active_id() else {
                return;
            };
            if let Err(err) = app.session.edit("Clear LUT", |project| {
                clear_lut(project, seq, clip_id)
            }) {
                app.status = err.to_string();
            }
        }
    });
    lut_mix_slider(ui, app, clip_id, rel);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new("Tetrahedral sampling in the shared compositor. Small LUTs embed in the project JSON.")
                .size(10.0)
                .color(THEME.text_mute),
        );
    });
}

fn lut_status(app: &MeridianApp, clip_id: editor_core::ClipId) -> (String, Option<String>, bool) {
    let Some(sequence) = app.session.project().active() else {
        return ("No LUT".into(), None, false);
    };
    let Some(clip) = sequence.clip(clip_id) else {
        return ("No LUT".into(), None, false);
    };
    let Some(filter) = lut(&clip.effects) else {
        return ("No LUT".into(), None, false);
    };
    if !filter.has_source() {
        return ("No LUT".into(), None, false);
    }
    let title = filter
        .title
        .clone()
        .or_else(|| {
            if filter.path.is_empty() {
                None
            } else {
                std::path::Path::new(&filter.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            }
        })
        .unwrap_or_else(|| "LUT".into());
    let offline = filter.embedded.is_none()
        && !filter.path.is_empty()
        && !std::path::Path::new(&filter.path).is_file();
    let detail = if offline {
        format!("{title} (offline)")
    } else if filter.path.is_empty() {
        format!("{title} (embedded)")
    } else {
        title
    };
    (detail, Some(filter.path.clone()), offline)
}

fn lut_mix_slider(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    clip_id: editor_core::ClipId,
    rel: i64,
) {
    let (current, keyed) = lut_mix_value(app, clip_id, rel);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| {
            let edit = widgets::param_slider(ui, "LUT mix", current, 0.0..=1.0, keyed);
            if edit.started {
                app.session.begin_interactive("LUT mix");
            }
            if edit.changed {
                let Ok(seq) = app.session.active_id() else {
                    return;
                };
                if let Err(err) = app.session.edit("LUT mix", |project| {
                    set_filter_at(project, seq, clip_id, FilterParam::LutMix, rel, edit.value)
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
                if let Err(err) = app.session.edit("LUT mix key", |project| {
                    toggle_filter_key(project, seq, clip_id, FilterParam::LutMix, rel)
                }) {
                    app.status = err.to_string();
                }
            }
        });
    });
}

fn lut_mix_value(app: &MeridianApp, clip_id: editor_core::ClipId, rel: i64) -> (f32, bool) {
    let Some(sequence) = app.session.project().active() else {
        return (1.0, false);
    };
    let Some(clip) = sequence.clip(clip_id) else {
        return (1.0, false);
    };
    if let Some(filter) = lut(&clip.effects) {
        return (filter.mix.value_at(rel), filter.mix.has_key(rel));
    }
    (1.0, false)
}
