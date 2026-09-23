//! Project start window. Shown before a timeline is loaded.

use editor_core::{builtin_templates, RecentProject, Timebase};
use egui::{
    Align2, CornerRadius, FontId, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2,
};

use crate::app::MeridianApp;
use crate::theme::THEME;
use crate::ui::widgets;

pub fn start_screen(ui: &mut Ui, app: &mut MeridianApp) {
    let full = ui.available_width();
    let width = (full - 64.0).clamp(720.0, 1080.0);
    let pad = ((full - width) * 0.5).max(0.0);
    ui.add_space(36.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width);
            header(ui);
            ui.add_space(22.0);
            ui.columns(2, |cols| {
                cols[0].push_id("recent-column", |ui| recent_column(ui, app));
                cols[1].push_id("new-column", |ui| new_column(ui, app));
            });
            ui.add_space(14.0);
            footer(ui, app);
        });
    });
}

fn header(ui: &mut Ui) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, THEME.accent);
        painter.hline(
            (rect.left() + 4.0)..=(rect.right() - 4.0),
            rect.center().y,
            Stroke::new(2.0_f32, THEME.accent_text),
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Meridian")
                    .strong()
                    .size(26.0)
                    .color(THEME.text),
            );
            ui.label(
                RichText::new("Open a recent project, or start from a preset.")
                    .size(13.0)
                    .color(THEME.text_dim),
            );
        });
    });
}

fn column_frame() -> Frame {
    Frame::new()
        .fill(THEME.panel)
        .stroke(Stroke::new(1.0_f32, THEME.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(14))
}

fn recent_column(ui: &mut Ui, app: &mut MeridianApp) {
    ui.add_space(8.0);
    column_frame().show(ui, |ui| {
        ui.set_min_height(460.0);
        section_label(ui, "Recent");
        ui.add_space(8.0);
        let entries = app.recent.clone();
        if entries.is_empty() {
            ui.label(
                RichText::new("No recent projects")
                    .size(13.0)
                    .color(THEME.text_dim),
            );
            ui.label(
                RichText::new("Projects you open or save show up here.")
                    .size(11.0)
                    .color(THEME.text_mute),
            );
        } else {
            let mut open_path = None;
            let mut remove_path = None;
            egui::ScrollArea::vertical()
                .id_salt("recent-projects")
                .max_height(340.0)
                .show(ui, |ui| {
                    for entry in &entries {
                        match recent_row(ui, entry) {
                            Some(RowAction::Open) => open_path = Some(entry.path.clone()),
                            Some(RowAction::Remove) => remove_path = Some(entry.path.clone()),
                            None => {}
                        }
                        ui.add_space(4.0);
                    }
                });
            if let Some(path) = remove_path {
                app.remove_recent(&path);
            }
            if let Some(path) = open_path {
                app.open_recent(&path);
            }
        }
        ui.add_space(8.0);
        if widgets::action_button(ui, "Open…", false) {
            app.open_dialog();
        }
    });
}

fn new_column(ui: &mut Ui, app: &mut MeridianApp) {
    let templates = builtin_templates();
    let mut name = std::mem::take(&mut app.start_name);
    let mut template = app.start_template.min(templates.len().saturating_sub(1));
    let mut create = false;
    ui.add_space(8.0);
    column_frame().show(ui, |ui| {
        ui.set_min_height(460.0);
        section_label(ui, "New project");
        ui.add_space(8.0);
        ui.label(RichText::new("Name").size(11.0).color(THEME.text_mute));
        let response = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(ui.available_width())
                .hint_text("Untitled"),
        );
        app.note_text_focus(&response);
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            create = true;
        }
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("project-templates")
            .max_height(300.0)
            .show(ui, |ui| {
                for (index, preset) in templates.iter().enumerate() {
                    let fps = Timebase::new(preset.fps_num, preset.fps_den).fps_f64();
                    let meta = format!(
                        "{}×{}   {:.3} fps   V{} A{} C{}",
                        preset.width,
                        preset.height,
                        fps,
                        preset.video_tracks,
                        preset.audio_tracks,
                        preset.caption_tracks
                    );
                    if widgets::choice_card(ui, &preset.name, &meta, template == index).clicked() {
                        template = index;
                    }
                    ui.add_space(4.0);
                }
            });
        ui.add_space(8.0);
        if widgets::action_button(ui, "Create", true) {
            create = true;
        }
    });
    app.start_name = name;
    app.start_template = template;
    if create {
        app.create_from_start();
    }
}

fn footer(ui: &mut Ui, app: &mut MeridianApp) {
    ui.horizontal(|ui| {
        if ui
            .link(
                RichText::new("Open example (Northline)")
                    .size(12.0)
                    .color(THEME.accent),
            )
            .clicked()
        {
            app.open_example();
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new("File → Open Example does the same once the editor is open.")
                .size(11.0)
                .color(THEME.text_mute),
        );
    });
    if !app.start_note.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new(&app.start_note).size(12.0).color(THEME.amber));
    }
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_ascii_uppercase())
            .size(11.0)
            .strong()
            .color(THEME.text_mute),
    );
}

enum RowAction {
    Open,
    Remove,
}

fn recent_row(ui: &mut Ui, entry: &RecentProject) -> Option<RowAction> {
    let missing = editor_core::recent_project_missing(&entry.path);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 56.0), Sense::hover());
    let remove_rect = Rect::from_min_size(
        Pos2::new(rect.right() - 70.0, rect.center().y - 11.0),
        Vec2::new(58.0, 22.0),
    );
    let open_rect =
        Rect::from_min_max(rect.min, Pos2::new(remove_rect.left() - 6.0, rect.bottom()));
    let open = ui.interact(
        open_rect,
        ui.id().with(("open", &entry.path)),
        Sense::click(),
    );
    let remove = ui.interact(
        remove_rect,
        ui.id().with(("remove", &entry.path)),
        Sense::click(),
    );

    let painter = ui.painter();
    let fill = if open.hovered() || remove.hovered() {
        THEME.control
    } else {
        THEME.inset
    };
    painter.rect_filled(rect, 4.0, fill);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, THEME.hairline),
        egui::StrokeKind::Inside,
    );

    let name = recent_label(entry);
    painter.text(
        Pos2::new(rect.left() + 12.0, rect.top() + 8.0),
        Align2::LEFT_TOP,
        name,
        FontId::new(13.0, egui::FontFamily::Proportional),
        THEME.text,
    );
    let path_color = if missing {
        THEME.amber
    } else {
        THEME.text_mute
    };
    let prefix = if missing { "Missing  ·  " } else { "" };
    let path_text = ellipsize_middle(
        &format!("{prefix}{}", entry.path),
        ((width - 96.0) / 6.6) as usize,
    );
    painter.text(
        Pos2::new(rect.left() + 12.0, rect.top() + 28.0),
        Align2::LEFT_TOP,
        path_text,
        FontId::new(11.0, egui::FontFamily::Monospace),
        path_color,
    );

    let remove_fill = if remove.hovered() {
        THEME.control_hover
    } else {
        THEME.header
    };
    painter.rect_filled(remove_rect, 3.0, remove_fill);
    painter.text(
        remove_rect.center(),
        Align2::CENTER_CENTER,
        "Remove",
        FontId::new(11.0, egui::FontFamily::Proportional),
        if missing { THEME.amber } else { THEME.text_dim },
    );

    if remove.clicked() {
        Some(RowAction::Remove)
    } else if open.clicked() {
        Some(RowAction::Open)
    } else {
        None
    }
}

fn recent_label(entry: &RecentProject) -> String {
    let name = entry.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    std::path::Path::new(&entry.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(entry.path.as_str())
        .to_string()
}

fn ellipsize_middle(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(12);
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let head = keep / 2;
    let tail = keep - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}
