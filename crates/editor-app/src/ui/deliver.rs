use egui::{RichText, TextEdit};

use crate::app::MeridianApp;
use crate::theme;
use crate::ui::format_tc;

const CODECS: &[&str] = &["H.264", "H.265", "ProRes 422", "ProRes 4444", "DNxHR HQ"];
const CONTAINERS: &[&str] = &["mp4", "mov", "mxf"];

pub fn deliver_panel(ui: &mut egui::Ui, app: &mut MeridianApp) {
    ui.heading("Deliver");
    ui.label(
        RichText::new(
            "This build writes an export manifest. A picture encoder is not linked yet — the manifest records the sequence, codec, and range a later encoder would render.",
        )
        .color(theme::DIM),
    );
    ui.add_space(8.0);

    let sequence = app.session.project().active().cloned();
    let Some(sequence) = sequence else {
        ui.label("No sequence.");
        return;
    };

    ui.label(RichText::new(&sequence.name).strong());
    ui.label(format!(
        "{}×{}   {}   {}",
        sequence.width,
        sequence.height,
        format_tc(0, sequence.timebase).trim_start_matches('0'),
        format!("{:.3} fps", sequence.timebase.fps_f64())
    ));
    ui.label(format!(
        "Duration {}",
        format_tc(sequence.end_frame().0, sequence.timebase)
    ));
    if let (Some(inn), Some(out)) = (sequence.in_point, sequence.out_point) {
        ui.label(format!(
            "In {}   Out {}",
            format_tc(inn.0, sequence.timebase),
            format_tc(out.0, sequence.timebase)
        ));
    }
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Codec");
        egui::ComboBox::from_id_salt("codec")
            .selected_text(&app.deliver.codec)
            .show_ui(ui, |ui| {
                for codec in CODECS {
                    ui.selectable_value(&mut app.deliver.codec, (*codec).to_string(), *codec);
                }
            });
        ui.label("Container");
        egui::ComboBox::from_id_salt("container")
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
    ui.checkbox(&mut app.deliver.use_in_out, "Use in/out range");
    ui.horizontal(|ui| {
        ui.label("Manifest");
        ui.add(TextEdit::singleline(&mut app.deliver.output_path).desired_width(360.0));
    });
    ui.add_space(6.0);
    if ui.button("Write export manifest").clicked() {
        app.export_manifest();
    }
    if !app.deliver.report.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(&app.deliver.report).small());
    }

    ui.add_space(16.0);
    ui.separator();
    ui.label(RichText::new("Sequence contents").small().strong());
    for track in &sequence.tracks {
        let count = if track.kind == editor_core::TrackKind::Caption {
            track.cues.len()
        } else {
            track.clips.len()
        };
        ui.label(format!(
            "{}  {}  {} item{}",
            track.name,
            match track.kind {
                editor_core::TrackKind::Video => "picture",
                editor_core::TrackKind::Audio => "audio",
                editor_core::TrackKind::Caption => "captions",
            },
            count,
            if count == 1 { "" } else { "s" }
        ));
    }
}
