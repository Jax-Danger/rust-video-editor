use editor_core::MediaId;
use egui::RichText;

use crate::app::{open_import, MeridianApp};
use crate::theme;
use crate::ui::format_tc;

pub fn media_pool(ui: &mut egui::Ui, app: &mut MeridianApp) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("MEDIA POOL").small().strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Import").clicked() {
                open_import(app);
            }
        });
    });
    ui.add_space(2.0);
    let bins: Vec<_> = app
        .session
        .project()
        .bins
        .iter()
        .map(|b| (b.id, b.name.clone(), b.parent))
        .collect();
    let media: Vec<_> = app
        .session
        .project()
        .media
        .iter()
        .map(|m| {
            (
                m.id,
                m.bin_id,
                m.name.clone(),
                m.duration.0,
                m.timebase,
                m.width,
                m.height,
                m.offline,
                m.kind_label(),
                m.has_video,
                m.has_audio,
            )
        })
        .collect();

    if bins.is_empty() && media.is_empty() {
        ui.label(
            RichText::new("Import media, then Overwrite or Insert it onto the timeline.")
                .small()
                .color(theme::DIM),
        );
        return;
    }

    let roots: Vec<_> = bins.iter().filter(|b| b.2.is_none()).cloned().collect();
    let list = if roots.is_empty() {
        bins.clone()
    } else {
        roots
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (id, name, _) in list {
            egui::CollapsingHeader::new(RichText::new(name).strong())
                .default_open(true)
                .id_salt(id.0)
                .show(ui, |ui| {
                    for (mid, bin, mname, dur, tb, w, h, offline, kind, _, _) in &media {
                        if *bin == id {
                            media_row(ui, app, *mid, mname, *dur, *tb, *w, *h, *offline, kind);
                        }
                    }
                    for (cid, cname, parent) in &bins {
                        if *parent == Some(id) {
                            ui.label(RichText::new(cname).small().color(theme::DIM));
                            for (mid, bin, mname, dur, tb, w, h, offline, kind, _, _) in &media {
                                if *bin == *cid {
                                    media_row(
                                        ui, app, *mid, mname, *dur, *tb, *w, *h, *offline, kind,
                                    );
                                }
                            }
                        }
                    }
                });
        }
    });
}

fn media_row(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    id: MediaId,
    name: &str,
    duration: i64,
    timebase: editor_core::Timebase,
    width: Option<u32>,
    height: Option<u32>,
    offline: bool,
    kind: &str,
) {
    let selected = app.selected_media == Some(id);
    let label = format!("{name}");
    let response = ui.selectable_label(selected, label);
    if response.clicked() {
        app.selected_media = Some(id);
    }
    if response.double_clicked() {
        app.selected_media = Some(id);
        app.place_selected_media(false);
    }
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let meta = match (width, height) {
            (Some(w), Some(h)) if w > 0 => {
                format!("{kind}  {w}×{h}  {}", format_tc(duration, timebase))
            }
            _ => format!("{kind}  {}", format_tc(duration, timebase)),
        };
        ui.label(RichText::new(meta).small().color(theme::DIM));
        if offline {
            ui.label(RichText::new("offline").small().color(theme::AMBER));
        }
    });
}
