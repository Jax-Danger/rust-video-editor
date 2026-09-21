use editor_core::MediaId;
use egui::{pos2, Align2, Color32, FontId, Id, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::format_tc;
use crate::ui::widgets;

pub fn media_pool(ui: &mut egui::Ui, app: &mut MeridianApp) {
    widgets::panel_header(ui, "Media Pool", |ui| {
        if widgets::ghost_button(ui, "Import") {
            app.import_dialog();
        }
    });

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

    let dropping = ui.input(|input| !input.raw.hovered_files.is_empty());
    if dropping {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, THEME.accent_dim);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop to import",
            FontId::new(12.0, egui::FontFamily::Proportional),
            THEME.accent,
        );
    }

    if media.is_empty() {
        ui.add_space(12.0);
        ui.label(
            RichText::new("The pool is empty.")
                .size(13.0)
                .color(THEME.text),
        );
        widgets::empty_note(
            ui,
            "File → Import, Ctrl+I, or drop video, audio, or stills here. Then double-click, drag onto the timeline, or press Overwrite / Insert.",
        );
        return;
    }

    let roots: Vec<_> = bins.iter().filter(|b| b.2.is_none()).cloned().collect();
    let list = if roots.is_empty() {
        bins.clone()
    } else {
        roots
    };

    egui::ScrollArea::vertical()
        .id_salt("media_pool_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);
            for (id, name, _) in list {
                let open_id = Id::new(("bin-open", id.0));
                let open = ui.ctx().data(|data| data.get_temp(open_id)).unwrap_or(true);
                if bin_header(ui, &name, open) {
                    ui.ctx().data_mut(|data| data.insert_temp(open_id, !open));
                }
                if !open {
                    continue;
                }
                for (mid, bin, mname, dur, tb, w, h, offline, kind, video, audio) in &media {
                    if *bin == id {
                        media_row(
                            ui, app, *mid, mname, *dur, *tb, *w, *h, *offline, kind, *video, *audio,
                        );
                    }
                }
                for (cid, cname, parent) in &bins {
                    if *parent == Some(id) {
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(format!("  {cname}"))
                                .size(11.0)
                                .color(THEME.text_mute),
                        );
                        for (mid, bin, mname, dur, tb, w, h, offline, kind, video, audio) in &media
                        {
                            if *bin == *cid {
                                media_row(
                                    ui, app, *mid, mname, *dur, *tb, *w, *h, *offline, kind,
                                    *video, *audio,
                                );
                            }
                        }
                    }
                }
            }
            ui.add_space(6.0);
        });
}

fn bin_header(ui: &mut egui::Ui, name: &str, open: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 0.0, THEME.header);
    }
    let mark = if open { "▾" } else { "▸" };
    painter.text(
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        mark,
        FontId::new(11.0, egui::FontFamily::Proportional),
        THEME.text_dim,
    );
    painter.text(
        pos2(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::new(12.0, egui::FontFamily::Proportional),
        THEME.text,
    );
    response.clicked()
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
    has_video: bool,
    has_audio: bool,
) {
    let selected = app.selected_media == Some(id);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click_and_drag());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 0.0, THEME.accent_dim);
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            0.0,
            THEME.accent,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, THEME.header);
    }

    let thumb = Rect::from_min_size(
        pos2(rect.left() + 10.0, rect.top() + 8.0),
        Vec2::new(36.0, 24.0),
    );
    let thumb_color = if has_video {
        THEME.video
    } else if has_audio {
        THEME.audio
    } else {
        THEME.caption
    };
    painter.rect_filled(thumb, 2.0, thumb_color);
    painter.rect_stroke(
        thumb,
        2.0,
        Stroke::new(1.0_f32, Color32::from_white_alpha(30)),
        egui::StrokeKind::Inside,
    );
    if has_audio && !has_video {
        painter.hline(
            (thumb.left() + 4.0)..=(thumb.right() - 4.0),
            thumb.center().y,
            Stroke::new(1.0_f32, Color32::from_white_alpha(160)),
        );
    } else {
        painter.vline(
            thumb.left() + 6.0,
            (thumb.top() + 4.0)..=(thumb.bottom() - 4.0),
            Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
        );
        painter.vline(
            thumb.right() - 6.0,
            (thumb.top() + 4.0)..=(thumb.bottom() - 4.0),
            Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
        );
    }

    painter.text(
        pos2(thumb.right() + 8.0, rect.top() + 8.0),
        Align2::LEFT_TOP,
        name,
        FontId::new(12.5, egui::FontFamily::Proportional),
        THEME.text,
    );
    let path = app
        .session
        .project()
        .media(id)
        .map(|media| media.path.clone())
        .unwrap_or_default();
    let missing = offline || super::media_missing(&path);
    let meta = if missing {
        "Offline — file is not on disk".to_string()
    } else {
        match (width, height) {
            (Some(w), Some(h)) if w > 0 => {
                format!("{kind}   {w}×{h}   {}", format_tc(duration, timebase))
            }
            _ => format!("{kind}   {}", format_tc(duration, timebase)),
        }
    };
    painter.text(
        pos2(thumb.right() + 8.0, rect.top() + 23.0),
        Align2::LEFT_TOP,
        meta,
        FontId::new(10.5, egui::FontFamily::Proportional),
        THEME.text_mute,
    );
    if missing {
        let pill = Rect::from_min_size(
            pos2(rect.right() - 58.0, rect.top() + 12.0),
            Vec2::new(48.0, 16.0),
        );
        painter.rect_stroke(
            pill,
            3.0,
            Stroke::new(1.0_f32, THEME.amber),
            egui::StrokeKind::Inside,
        );
        painter.text(
            pill.center(),
            Align2::CENTER_CENTER,
            "Offline",
            FontId::new(9.0, egui::FontFamily::Proportional),
            THEME.amber,
        );
    }

    if response.drag_started() {
        app.dragging_media = Some(id);
        app.selected_media = Some(id);
        app.status = "Drop on the timeline. Shift inserts instead of overwriting.".into();
    } else if response.clicked() {
        app.selected_media = Some(id);
    }
    if response.double_clicked() {
        app.selected_media = Some(id);
        app.dragging_media = None;
        app.place_selected_media(false);
    }
    if missing {
        if let Some(path) = app.session.project().media(id).map(|media| media.path.clone()) {
            response.on_hover_text(format!("Offline — {path}"));
        }
    }
}
