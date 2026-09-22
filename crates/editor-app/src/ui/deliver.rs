use egui::{CornerRadius, Frame, Margin, RichText, Stroke, TextEdit};

use crate::app::MeridianApp;
use crate::theme::{self, THEME};
use crate::ui::format_tc;
use crate::ui::widgets;

const CODECS: &[&str] = &["H.264", "H.265", "ProRes 422", "ProRes 4444", "DNxHR HQ", "PCM"];
const CONTAINERS: &[&str] = &["mp4", "mov", "mxf", "wav"];

pub fn deliver_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let running = app.export_running();
    widgets::panel_header(ui, "Deliver", |ui| {
        if running && widgets::ghost_button(ui, "Cancel") {
            app.cancel_export();
        }
        if widgets::action_button(ui, "Export", !running) {
            app.start_export();
        }
    });
    let sequence = app.session.project().active().cloned();
    let Some(sequence) = sequence else {
        widgets::empty_note(ui, "Open a sequence before delivering.");
        return;
    };

    egui::ScrollArea::vertical()
        .id_salt("deliver_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            deliver_body(ui, app, &sequence);
        });
}

fn deliver_body(ui: &mut egui::Ui, app: &mut MeridianApp, sequence: &editor_core::Sequence) {
    ui.add_space(theme::SPACE_LG);
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 600.0).max(24.0) * 0.5);
        ui.vertical(|ui| {
            ui.set_width(600.0);
            Frame::new()
                .fill(THEME.panel)
                .stroke(Stroke::new(1.0_f32, THEME.hairline))
                .corner_radius(CornerRadius::same(THEME.radius))
                .inner_margin(Margin::same(16))
                .show(ui, |ui| {
                    deliver_card(ui, app, sequence);
                });
        });
    });
}

fn deliver_card(
    ui: &mut egui::Ui,
    app: &mut crate::app::MeridianApp,
    sequence: &editor_core::Sequence,
) {
    ui.label(
        RichText::new(&sequence.name)
            .font(THEME.font(18.0))
            .strong(),
    );
    ui.label(
        RichText::new(format!(
            "{}×{}    {:.3} fps    {}",
            sequence.width,
            sequence.height,
            sequence.timebase.fps_f64(),
            format_tc(sequence.end_frame().0, sequence.timebase)
        ))
        .monospace()
        .size(12.0)
        .color(THEME.text_dim),
    );
    if let (Some(inn), Some(out)) = (sequence.in_point, sequence.out_point) {
        ui.label(
            RichText::new(format!(
                "In {}    Out {}",
                format_tc(inn.0, sequence.timebase),
                format_tc(out.0, sequence.timebase)
            ))
            .size(12.0)
            .monospace()
            .color(THEME.text_mute),
        );
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Export runs the same composite as the program monitor — stacked tracks, grade, transform, dissolves, wipes, pushes, and burned captions — then ffmpeg encodes it. H.264 / AAC in an mp4 is the tested path.",
        )
        .size(12.5)
        .color(THEME.text_dim),
    );

    ui.add_space(16.0);
    widgets::section_label(ui, "Stills");
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Save the composited program frame as PNG or JPEG. Grade, LUT, titles, transitions, and burned captions match the video export path.",
        )
        .size(12.5)
        .color(THEME.text_dim),
    );
    ui.add_space(8.0);
    let running = app.export_running();
    ui.horizontal(|ui| {
        if widgets::action_button(ui, "Export Still at Playhead", !running) {
            app.export_still_at_playhead();
        }
        if widgets::action_button(ui, "Export Stills at Markers", !running) {
            app.export_stills_at_markers();
        }
    });

    ui.add_space(16.0);
    widgets::section_label(ui, "Preset");
    ui.add_space(8.0);
    let presets = app.deliver_presets();
    let selected_name = presets
        .iter()
        .find(|preset| preset.id == app.deliver.preset_id)
        .map(|preset| preset.name.as_str())
        .unwrap_or("Custom");
    let mut preset_changed = None;
    egui::ComboBox::from_id_salt("deliver_preset")
        .width(320.0)
        .selected_text(selected_name)
        .show_ui(ui, |ui| {
            for preset in &presets {
                if ui
                    .selectable_value(
                        &mut app.deliver.preset_id,
                        preset.id.clone(),
                        &preset.name,
                    )
                    .clicked()
                {
                    preset_changed = Some(preset.id.clone());
                }
            }
            if ui
                .selectable_value(&mut app.deliver.preset_id, "custom".into(), "Custom")
                .clicked()
            {
                preset_changed = Some("custom".into());
            }
        });
    if let Some(preset_id) = preset_changed {
        if preset_id != "custom" {
            app.apply_deliver_preset(&preset_id);
        } else {
            app.deliver.report = "Custom settings — tweak codec and path below.".into();
        }
    }
    if let Some(preset) = presets.iter().find(|p| p.id == app.deliver.preset_id) {
        ui.label(
            RichText::new(&preset.description)
                .size(12.0)
                .color(THEME.text_mute),
        );
    }

    ui.add_space(16.0);
    widgets::section_label(ui, "Format");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Codec").color(THEME.text_dim));
        egui::ComboBox::from_id_salt("codec")
            .width(180.0)
            .selected_text(&app.deliver.codec)
            .show_ui(ui, |ui| {
                for codec in CODECS {
                    ui.selectable_value(&mut app.deliver.codec, (*codec).to_string(), *codec);
                }
            });
        ui.add_space(12.0);
        ui.label(RichText::new("Container").color(THEME.text_dim));
        egui::ComboBox::from_id_salt("container")
            .width(100.0)
            .selected_text(&app.deliver.container)
            .show_ui(ui, |ui| {
                for container in CONTAINERS {
                    ui.selectable_value(
                        &mut app.deliver.container,
                        (*container).to_string(),
                        *container,
                    );
                }
            });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut app.deliver.use_in_out,
            "Limit to the marked in and out",
        );
        ui.add_space(12.0);
        ui.checkbox(
            &mut app.deliver.burn_captions,
            "Burn captions into the picture",
        );
    });
    ui.checkbox(
        &mut app.deliver.audio_only,
        "Audio only (skip picture encode)",
    );
    if app.deliver.audio_only {
        ui.label(
            RichText::new("Audio-only exports mix the timeline bus to WAV via ffmpeg.")
                .size(12.0)
                .color(THEME.text_mute),
        );
    } else if matches!(
        app.deliver.codec.to_ascii_lowercase().as_str(),
        "prores 422" | "prores 4444" | "dnxhr hq"
    ) {
        ui.label(
            RichText::new(format!(
                "{} needs a ffmpeg build with that encoder — otherwise Export reports the ffmpeg error.",
                app.deliver.codec
            ))
            .size(12.0)
            .color(THEME.text_mute),
        );
    }
    if let Some(video_kbps) = app.deliver.video_bitrate_kbps {
        ui.label(
            RichText::new(format!("Video bitrate hint: {video_kbps} kbps"))
                .size(12.0)
                .color(THEME.text_mute),
        );
    }
    if let Some(audio_kbps) = app.deliver.audio_bitrate_kbps {
        ui.label(
            RichText::new(format!("Audio bitrate hint: {audio_kbps} kbps"))
                .size(12.0)
                .color(THEME.text_mute),
        );
    }

    ui.add_space(12.0);
    widgets::section_label(ui, "Output");
    ui.add_space(8.0);
    ui.label(RichText::new("File path").size(11.0).color(THEME.text_mute));
    let response = ui.add(TextEdit::singleline(&mut app.deliver.output_path).desired_width(520.0));
    app.note_text_focus(&response);
    ui.add_space(12.0);
    let running = app.export_running();
    if running || app.deliver.progress > 0.0 {
        ui.add_space(8.0);
        let width = ui.available_width().min(520.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 8.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, THEME.header);
        let fill = rect.shrink2(egui::vec2(0.0, 0.0));
        let mut done = fill;
        done.set_width(fill.width() * app.deliver.progress.clamp(0.0, 1.0));
        ui.painter().rect_filled(done, 2.0, THEME.accent);
    }
    if !app.deliver.report.is_empty() {
        ui.add_space(10.0);
        ui.label(
            RichText::new(&app.deliver.report)
                .size(12.0)
                .color(THEME.text_dim),
        );
    }

    ui.add_space(16.0);
    widgets::section_label(ui, "Save preset");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").size(11.0).color(THEME.text_mute));
        ui.add(
            TextEdit::singleline(&mut app.deliver.save_preset_name)
                .hint_text("My web export")
                .desired_width(180.0),
        );
        ui.add_space(8.0);
        if widgets::ghost_button(ui, "Save JSON") {
            if let Err(err) = app.save_custom_deliver_preset() {
                app.deliver.report = err;
            }
        }
    });
    ui.add(
        TextEdit::singleline(&mut app.deliver.save_preset_note)
            .hint_text("Optional description")
            .desired_width(520.0),
    );
    ui.label(
        RichText::new(format!(
            "Custom presets are stored under {}.",
            editor_core::custom_deliver_preset_dir().display()
        ))
        .size(11.5)
        .color(THEME.text_mute),
    );

    ui.add_space(18.0);
    widgets::section_label(ui, "Sequence");
    ui.add_space(6.0);
    for track in &sequence.tracks {
        let count = if track.kind == editor_core::TrackKind::Caption {
            track.cues.len()
        } else {
            track.clips.len()
        };
        let kind = match track.kind {
            editor_core::TrackKind::Video => "Picture",
            editor_core::TrackKind::Audio => "Audio",
            editor_core::TrackKind::Caption => "Captions",
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&track.name).size(12.5).strong());
            ui.label(
                RichText::new(format!("{kind}   {count}"))
                    .size(12.0)
                    .color(THEME.text_dim),
            );
        });
    }
}
