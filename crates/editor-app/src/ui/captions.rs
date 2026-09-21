use editor_core::TrackKind;
use egui::RichText;

use crate::app::MeridianApp;
use crate::theme;
use crate::ui::format_tc;

pub fn captions_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("CAPTIONS").small().strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Auto Caption").clicked() {
                app.auto_caption();
            }
        });
    });
    ui.label(
        RichText::new("Stub transcriber — no API key. Replace CaptionTranscriber to call Whisper or a cloud STT.")
            .small()
            .color(theme::DIM),
    );

    let timebase = app.timebase();
    let cues: Vec<_> = app
        .session
        .project()
        .active()
        .map(|sequence| {
            sequence
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Caption)
                .flat_map(|t| {
                    t.cues.iter().map(|c| {
                        (
                            c.id,
                            c.timeline_in.0,
                            c.timeline_out.0,
                            c.text.clone(),
                            c.speaker.clone(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if cues.is_empty() {
        ui.label(
            RichText::new("No cues on the caption track.")
                .small()
                .color(theme::DIM),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("caption_list")
        .max_height(ui.available_height().max(80.0))
        .show(ui, |ui| {
            for (id, start, end, text, speaker) in cues {
                let selected = app.selected_cue == Some(id);
                let header = format!(
                    "{} – {}{}",
                    format_tc(start, timebase),
                    format_tc(end, timebase),
                    speaker
                        .as_ref()
                        .map(|s| format!("  {s}"))
                        .unwrap_or_default()
                );
                if ui
                    .selectable_label(selected, RichText::new(header).small())
                    .clicked()
                {
                    app.selected_cue = Some(id);
                    app.playhead = start;
                    app.playing = false;
                }
                if selected {
                    let mut body = text;
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut body)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    if response.gained_focus() {
                        app.session.begin_interactive("Edit caption");
                    }
                    if response.changed() {
                        if let Err(err) = app.session.update_cue_text(id, body) {
                            app.status = err.to_string();
                        }
                    }
                    if response.lost_focus() {
                        app.session.end_interactive();
                    }
                    if ui.small_button("Delete cue").clicked() {
                        if let Err(err) = app.session.delete_cue(id) {
                            app.status = err.to_string();
                        } else {
                            app.selected_cue = None;
                        }
                    }
                }
            }
        });
}
