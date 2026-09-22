use editor_core::MediaId;
use egui::{pos2, Align2, Color32, FontId, Id, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::format_tc;
use crate::ui::widgets;

pub fn media_pool(ui: &mut egui::Ui, app: &mut MeridianApp) {
    widgets::panel_header(ui, "Media Pool", |ui| {
        if widgets::ghost_button(ui, "Relink") {
            app.relink_selected();
        }
        if widgets::ghost_button(ui, "Import") {
            app.import_dialog();
        }
        if widgets::ghost_button(ui, "New Title") {
            app.add_title();
        }
        if widgets::ghost_button(ui, "New Adjustment Layer") {
            app.add_adjustment_layer();
        }
    });
    proxy_bar(ui, app);

    let video_picks = app
        .pool_selection
        .iter()
        .filter(|id| {
            app.session
                .project()
                .media(**id)
                .is_some_and(|media| media.has_video)
        })
        .count();
    if video_picks >= 2 {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{video_picks} video angles"))
                    .size(11.0)
                    .color(THEME.text_dim),
            );
            if widgets::action_button(ui, "Create Multicam", true) {
                app.create_multicam_from_pool();
            }
        });
        ui.add_space(2.0);
    }

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
                m.proxy_path
                    .as_deref()
                    .is_some_and(|path| !super::media_missing(path)),
            )
        })
        .collect();

    let dropping = ui.input(|input| !input.raw.hovered_files.is_empty());
    if dropping {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
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
            "File → Import, Ctrl+I, or drop video, audio, or stills here. Then double-click to open in source, or use Overwrite / Insert.",
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
                for (mid, bin, mname, dur, tb, w, h, offline, kind, video, audio, proxy) in &media {
                    if *bin == id {
                        media_row(
                            ui, app, *mid, mname, *dur, *tb, *w, *h, *offline, kind, *video,
                            *audio, *proxy,
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
                        for (mid, bin, mname, dur, tb, w, h, offline, kind, video, audio, proxy) in
                            &media
                        {
                            if *bin == *cid {
                                media_row(
                                    ui, app, *mid, mname, *dur, *tb, *w, *h, *offline, kind,
                                    *video, *audio, *proxy,
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

fn proxy_bar(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let prefer = app.session.project().prefer_proxies;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let label = if prefer { "Proxies" } else { "Full" };
        if widgets::chip(ui, label, prefer) {
            app.toggle_proxies();
        }
        if widgets::ghost_button(ui, "Selected") {
            app.generate_proxies(false);
        }
        if widgets::ghost_button(ui, "Project") {
            app.generate_proxies(true);
        }
        if app.proxy_job.is_some() && widgets::ghost_button(ui, "Cancel") {
            app.cancel_proxies();
        }
    });
    if !app.proxy_note.is_empty() {
        ui.label(RichText::new(&app.proxy_note).size(11.0).color(THEME.amber));
    }
    ui.add_space(2.0);
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
    has_proxy: bool,
) {
    let selected = app.pool_selection.contains(&id);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 36.0),
        Sense::click_and_drag(),
    );
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
        pos2(rect.left() + 8.0, rect.top() + 6.0),
        Vec2::new(32.0, 24.0),
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
        pos2(thumb.right() + 8.0, rect.top() + 4.0),
        Align2::LEFT_TOP,
        name,
        THEME.font(12.0),
        THEME.text,
    );
    let path = app
        .session
        .project()
        .media(id)
        .map(|media| media.path.clone())
        .unwrap_or_default();
    let missing = offline || super::media_missing(&path);
    let online = Color32::from_rgb(110, 196, 150);
    let status_color = if missing { THEME.amber } else { online };
    painter.circle_filled(
        pos2(thumb.right() - 3.0, thumb.bottom() - 3.0),
        3.5,
        status_color,
    );
    let meta = if missing {
        "Offline — click the pill to relink".to_string()
    } else {
        let proxy = if has_proxy { "   Proxy" } else { "" };
        match (width, height) {
            (Some(w), Some(h)) if w > 0 => {
                format!(
                    "Online   {kind}   {w}×{h}   {}{proxy}",
                    format_tc(duration, timebase)
                )
            }
            _ => format!("Online   {kind}   {}{proxy}", format_tc(duration, timebase)),
        }
    };
    painter.text(
        pos2(thumb.right() + 8.0, rect.top() + 19.0),
        Align2::LEFT_TOP,
        meta,
        THEME.font(10.0),
        if missing {
            THEME.amber
        } else {
            THEME.text_mute
        },
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
        if !app.pool_selection.contains(&id) {
            app.pool_selection = vec![id];
        }
        app.status = "Drop on the timeline. Shift inserts instead of overwriting.".into();
    } else if response.clicked() {
        let mods = ui.input(|input| input.modifiers);
        if mods.command {
            if let Some(index) = app.pool_selection.iter().position(|item| *item == id) {
                app.pool_selection.remove(index);
                app.selected_media = app.pool_selection.last().copied();
            } else {
                app.pool_selection.push(id);
                app.selected_media = Some(id);
            }
        } else if mods.shift {
            if !app.pool_selection.contains(&id) {
                app.pool_selection.push(id);
            }
            app.selected_media = Some(id);
        } else {
            app.pool_selection = vec![id];
            app.selected_media = Some(id);
        }
        if missing {
            if let Some(pos) = response.interact_pointer_pos() {
                let pill = Rect::from_min_size(
                    pos2(rect.right() - 58.0, rect.top() + 12.0),
                    Vec2::new(48.0, 16.0),
                );
                if pill.contains(pos) {
                    app.relink_media(id);
                }
            }
        }
    }
    if response.double_clicked() {
        app.dragging_media = None;
        app.open_in_source(id);
    }
    if missing {
        if let Some(path) = app
            .session
            .project()
            .media(id)
            .map(|media| media.path.clone())
        {
            response.on_hover_text(format!("Offline — {path}"));
        }
    }
}
