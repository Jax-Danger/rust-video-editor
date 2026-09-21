use editor_core::TrackKind;
use egui::{pos2, Align2, FontId, RichText, Sense, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::format_tc;
use crate::ui::widgets;

pub fn captions_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    widgets::panel_header(ui, "Captions", |ui| {
        if widgets::action_button(ui, "Auto Caption", true) {
            app.auto_caption();
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(
            RichText::new(app.caption_backend_note())
                .size(11.0)
                .color(THEME.text_mute),
        );
    });

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
        widgets::empty_note(ui, "No cues yet. Auto Caption will write a first pass.");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("caption_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);
            for (id, start, end, text, speaker) in cues {
                let selected = app.selected_cue == Some(id);
                let header = format!(
                    "{}  –  {}{}",
                    format_tc(start, timebase),
                    format_tc(end, timebase),
                    speaker
                        .as_ref()
                        .map(|s| format!("   {s}"))
                        .unwrap_or_default()
                );
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width() - 8.0, 28.0),
                    Sense::click(),
                );
                let painter = ui.painter();
                let draw = rect.translate(Vec2::new(4.0, 0.0));
                if selected {
                    painter.rect_filled(draw, 3.0, THEME.accent_dim);
                    painter.rect_stroke(
                        draw,
                        3.0,
                        Stroke::new(1.0_f32, THEME.accent),
                        egui::StrokeKind::Inside,
                    );
                } else if response.hovered() {
                    painter.rect_filled(draw, 3.0, THEME.header);
                }
                painter.text(
                    pos2(draw.left() + 8.0, draw.center().y),
                    Align2::LEFT_CENTER,
                    header,
                    FontId::new(11.0, egui::FontFamily::Monospace),
                    if selected { THEME.text } else { THEME.text_dim },
                );
                if response.clicked() {
                    app.selected_cue = Some(id);
                    app.halt_transport();
                    app.playhead = start;
                }
                if selected {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let mut body = text;
                        let response = ui.add(
                            egui::TextEdit::multiline(&mut body)
                                .desired_rows(2)
                                .desired_width(ui.available_width() - 8.0),
                        );
                        app.note_text_focus(&response);
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
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        if widgets::ghost_button(ui, "Delete cue") {
                            if let Err(err) = app.session.delete_cue(id) {
                                app.status = err.to_string();
                            } else {
                                app.selected_cue = None;
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            }
        });
}
