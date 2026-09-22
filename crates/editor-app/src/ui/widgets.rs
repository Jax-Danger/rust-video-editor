//! Painted controls. These replace stock egui buttons, sliders, and tabs.

use egui::{
    pos2, Align2, Color32, FontId, Id, Painter, Pos2, Rect, Response, RichText, Sense, Shape,
    Stroke, Ui, Vec2,
};

use crate::theme::{self, THEME};

pub fn panel_header(ui: &mut Ui, title: &str, extras: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, theme::HEADER_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, THEME.header);
    painter.hline(rect.x_range(), rect.bottom(), theme::hairline_stroke());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(10.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(
        RichText::new(title)
            .font(THEME.font(11.0))
            .strong()
            .color(THEME.text),
    );
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), extras);
}

pub fn section_label(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let font = FontId::new(11.0, egui::FontFamily::Proportional);
    let galley =
        ui.fonts(|fonts| fonts.layout_no_wrap(title.to_string(), font.clone(), THEME.text_dim));
    let painter = ui.painter();
    painter.galley(
        rect.left_center() - Vec2::new(0.0, galley.size().y * 0.5),
        galley.clone(),
        THEME.text_dim,
    );
    let x = rect.left() + galley.size().x + 8.0;
    if x < rect.right() {
        painter.hline(
            x..=rect.right(),
            rect.center().y,
            Stroke::new(1.0_f32, THEME.hairline),
        );
    }
}

pub fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, theme::hairline_stroke());
}

pub fn v_hairline(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, height), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(6.0),
        theme::hairline_stroke(),
    );
}

/// Horizontal splitter between two stacked regions. Returns the drag delta in
/// points (positive moves the split downward).
pub fn h_split(ui: &mut Ui) -> f32 {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::drag());
    let hot = response.hovered() || response.dragged();
    let painter = ui.painter();
    if hot {
        painter.rect_filled(rect, 0.0, THEME.header);
    }
    painter.hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0_f32, if hot { THEME.accent } else { THEME.border }),
    );
    if hot {
        response
            .clone()
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    }
    if response.dragged() {
        response.drag_delta().y
    } else {
        0.0
    }
}

pub fn empty_note(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).size(12.0).color(THEME.text_dim));
}

/// Square-ish tool cell with a painted icon and a caption.
pub fn tool_cell(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    caption: &str,
    active: bool,
    tip: &str,
    icon: impl FnOnce(&Painter, Rect),
) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(44.0, 36.0), Sense::click());
    let painter = ui.painter();
    let fill = if active {
        THEME.accent_dim
    } else if response.hovered() {
        THEME.control_hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, THEME.radius as f32, fill);
    if active {
        painter.hline(
            rect.x_range().shrink(6.0),
            rect.bottom() - 1.0,
            Stroke::new(2.0_f32, THEME.accent),
        );
    }
    let icon_rect = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + 13.0),
        Vec2::new(16.0, 16.0),
    );
    icon(painter, icon_rect);
    painter.text(
        pos2(rect.center().x, rect.bottom() - 7.0),
        Align2::CENTER_CENTER,
        caption,
        THEME.mono(9.0),
        if active {
            THEME.accent
        } else {
            THEME.text_mute
        },
    );
    let _ = id;
    response.on_hover_text(tip).clicked()
}

pub fn chip(ui: &mut Ui, label: &str, on: bool) -> bool {
    let font = FontId::new(11.5, egui::FontFamily::Proportional);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.to_string(), font.clone(), THEME.text));
    let width = galley.size().x + 18.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click());
    let painter = ui.painter();
    let fill = if on {
        THEME.accent_dim
    } else if response.hovered() {
        THEME.control_hover
    } else {
        THEME.inset
    };
    let stroke = if on {
        Stroke::new(1.0_f32, THEME.accent)
    } else {
        Stroke::new(1.0_f32, THEME.hairline)
    };
    painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
    painter.rect_filled(rect.shrink(1.0), 3.0, fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        if on { THEME.accent } else { THEME.text },
    );
    response.clicked()
}

pub fn action_button(ui: &mut Ui, label: &str, primary: bool) -> bool {
    let font = FontId::new(12.0, egui::FontFamily::Proportional);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.to_string(), font.clone(), THEME.text));
    let width = galley.size().x + 22.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width.max(72.0), 26.0), Sense::click());
    let painter = ui.painter();
    let (fill, text, stroke) = if primary {
        (
            if response.hovered() {
                Color32::from_rgb(84, 224, 198)
            } else {
                THEME.accent
            },
            THEME.accent_text,
            None,
        )
    } else if response.hovered() {
        (
            THEME.control_hover,
            THEME.text,
            Some(Stroke::new(1.0_f32, THEME.border)),
        )
    } else {
        (
            THEME.control,
            THEME.text,
            Some(Stroke::new(1.0_f32, THEME.hairline)),
        )
    };
    painter.rect_filled(rect, 3.0, fill);
    if let Some(stroke) = stroke {
        painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
    }
    painter.text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    response.clicked()
}

pub fn ghost_button(ui: &mut Ui, label: &str) -> bool {
    let font = FontId::new(11.0, egui::FontFamily::Proportional);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.to_string(), font.clone(), THEME.text_dim));
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 16.0, 20.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 3.0, THEME.control);
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        if response.hovered() {
            THEME.text
        } else {
            THEME.text_dim
        },
    );
    response.clicked()
}

pub fn icon_toggle(ui: &mut Ui, label: &str, on: bool, on_fill: Color32) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, 3.0, on_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, THEME.control_hover);
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0_f32, THEME.border),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0_f32, THEME.border),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(10.0, egui::FontFamily::Proportional),
        if on { Color32::WHITE } else { THEME.text_dim },
    );
    response.clicked()
}

pub struct SliderEdit {
    pub value: f32,
    pub started: bool,
    pub changed: bool,
    pub stopped: bool,
    pub key_clicked: bool,
}

pub fn param_slider(
    ui: &mut Ui,
    label: &str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    keyed: bool,
) -> SliderEdit {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
    let painter = ui.painter();
    let label_end = rect.left() + 92.0;
    painter.text(
        pos2(rect.left() + 2.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(12.0, egui::FontFamily::Proportional),
        THEME.text,
    );

    let diamond = Rect::from_center_size(
        pos2(rect.right() - 9.0, rect.center().y),
        Vec2::new(12.0, 12.0),
    );
    let value_rect = Rect::from_min_max(
        pos2(rect.right() - 78.0, rect.top()),
        pos2(rect.right() - 20.0, rect.bottom()),
    );
    let track = Rect::from_min_max(
        pos2(label_end, rect.center().y - 2.0),
        pos2(value_rect.left() - 6.0, rect.center().y + 2.0),
    );

    let min = *range.start();
    let max = *range.end();
    let span = (max - min).abs().max(0.0001);
    let t = ((value - min) / span).clamp(0.0, 1.0);
    painter.rect_filled(track, 2.0, THEME.inset);
    let filled = Rect::from_min_max(
        track.min,
        pos2(track.left() + track.width() * t, track.bottom()),
    );
    painter.rect_filled(filled, 2.0, THEME.accent_dim);
    let knob = Pos2::new(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 5.0, THEME.text);
    painter.circle_stroke(knob, 5.0, Stroke::new(1.0_f32, THEME.accent));

    let shown = if span >= 40.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    };
    painter.text(
        value_rect.right_center(),
        Align2::RIGHT_CENTER,
        shown,
        FontId::new(11.0, egui::FontFamily::Monospace),
        THEME.text_dim,
    );

    paint_diamond(painter, diamond, keyed);

    // The painted track is only a few pixels tall. The whole row, including the
    // numeric readout, is the hit target so a click sets the value.
    let hit = Rect::from_min_max(
        pos2(track.left(), rect.top()),
        pos2(value_rect.right(), rect.bottom()),
    );
    let drag = ui.interact(hit, Id::new(("param", label)), Sense::click_and_drag());
    let key = ui.interact(diamond.expand(3.0), Id::new(("key", label)), Sense::click());

    let mut next = value;
    let mut changed = false;
    if (drag.dragged() || drag.clicked()) && hit.width() > 1.0 {
        if let Some(pos) = drag.interact_pointer_pos() {
            let nt = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            next = min + nt * span;
            changed = (next - value).abs() > f32::EPSILON || drag.clicked();
        }
    }
    SliderEdit {
        value: next,
        started: drag.drag_started(),
        changed,
        stopped: drag.drag_stopped(),
        key_clicked: key.clicked(),
    }
}

/// Slider without a keyframe diamond. Title layout uses this; grades keep [`param_slider`].
pub fn value_slider(
    ui: &mut Ui,
    label: &str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
) -> SliderEdit {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
    let painter = ui.painter();
    let label_end = rect.left() + 92.0;
    painter.text(
        pos2(rect.left() + 2.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(12.0, egui::FontFamily::Proportional),
        THEME.text,
    );
    let value_rect = Rect::from_min_max(
        pos2(rect.right() - 58.0, rect.top()),
        pos2(rect.right() - 4.0, rect.bottom()),
    );
    let track = Rect::from_min_max(
        pos2(label_end, rect.center().y - 2.0),
        pos2(value_rect.left() - 6.0, rect.center().y + 2.0),
    );
    let min = *range.start();
    let max = *range.end();
    let span = (max - min).abs().max(0.0001);
    let t = ((value - min) / span).clamp(0.0, 1.0);
    painter.rect_filled(track, 2.0, THEME.inset);
    let filled = Rect::from_min_max(
        track.min,
        pos2(track.left() + track.width() * t, track.bottom()),
    );
    painter.rect_filled(filled, 2.0, THEME.accent_dim);
    let knob = Pos2::new(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 5.0, THEME.text);
    painter.circle_stroke(knob, 5.0, Stroke::new(1.0_f32, THEME.accent));
    let shown = if span >= 40.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    };
    painter.text(
        value_rect.right_center(),
        Align2::RIGHT_CENTER,
        shown,
        FontId::new(11.0, egui::FontFamily::Monospace),
        THEME.text_dim,
    );
    let hit = Rect::from_min_max(
        pos2(track.left(), rect.top()),
        pos2(value_rect.right(), rect.bottom()),
    );
    let drag = ui.interact(hit, Id::new(("value", label)), Sense::click_and_drag());
    let mut next = value;
    let mut changed = false;
    if (drag.dragged() || drag.clicked()) && hit.width() > 1.0 {
        if let Some(pos) = drag.interact_pointer_pos() {
            let nt = ((pos.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
            next = min + nt * span;
            changed = (next - value).abs() > f32::EPSILON || drag.clicked();
        }
    }
    SliderEdit {
        value: next,
        started: drag.drag_started(),
        changed,
        stopped: drag.drag_stopped(),
        key_clicked: false,
    }
}

fn paint_diamond(painter: &Painter, rect: Rect, filled: bool) {
    let c = rect.center();
    let points = vec![
        pos2(c.x, rect.top()),
        pos2(rect.right(), c.y),
        pos2(c.x, rect.bottom()),
        pos2(rect.left(), c.y),
    ];
    if filled {
        painter.add(Shape::convex_polygon(
            points,
            THEME.amber,
            Stroke::new(1.0_f32, THEME.amber),
        ));
    } else {
        painter.add(Shape::convex_polygon(
            points,
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, THEME.text_mute),
        ));
    }
}

pub fn workspace_modes(ui: &mut Ui, selected: usize, labels: &[&str]) -> Option<usize> {
    let mut clicked = None;
    let tab_w = 76.0;
    let tab_h = 26.0;
    let width = labels.len() as f32 * tab_w;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, tab_h), Sense::hover());
    let painter = ui.painter();
    painter.hline(rect.x_range(), rect.bottom(), theme::hairline_stroke());
    for (index, label) in labels.iter().enumerate() {
        let tab = Rect::from_min_size(
            pos2(rect.left() + index as f32 * tab_w, rect.top()),
            Vec2::new(tab_w, tab_h),
        );
        let response = ui.interact(tab, Id::new(("workspace", label)), Sense::click());
        let on = index == selected;
        if on {
            painter.rect_filled(tab, 0.0, THEME.panel);
            painter.hline(
                tab.x_range(),
                tab.bottom() - 1.0,
                Stroke::new(2.0_f32, THEME.accent),
            );
        } else if response.hovered() {
            painter.rect_filled(tab, 0.0, THEME.header);
        }
        if index > 0 {
            painter.vline(
                tab.left(),
                tab.y_range().shrink(6.0),
                theme::hairline_stroke(),
            );
        }
        painter.text(
            tab.center() - Vec2::new(0.0, 1.0),
            Align2::CENTER_CENTER,
            *label,
            THEME.font(12.0),
            if on { THEME.text } else { THEME.text_dim },
        );
        if response.clicked() {
            clicked = Some(index);
        }
        let _ = response.on_hover_text(format!("{label} workspace"));
    }
    clicked
}

pub fn timecode_well(ui: &mut Ui, caption: &str, value: &str, primary: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(124.0, 32.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, THEME.radius as f32, THEME.inset);
    painter.rect_stroke(
        rect,
        THEME.radius as f32,
        theme::hairline_stroke(),
        egui::StrokeKind::Inside,
    );
    painter.text(
        pos2(rect.left() + 6.0, rect.top() + 3.0),
        Align2::LEFT_TOP,
        caption,
        THEME.font(8.5),
        THEME.text_mute,
    );
    painter.text(
        pos2(rect.center().x, rect.bottom() - 10.0),
        Align2::CENTER_CENTER,
        value,
        THEME.mono(13.0),
        if primary {
            THEME.accent
        } else {
            THEME.text_dim
        },
    );
}

#[allow(dead_code)]
pub fn mini_slider(ui: &mut Ui, value: f32, range: std::ops::RangeInclusive<f32>) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(110.0, 18.0), Sense::click_and_drag());
    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
    painter.rect_filled(track, 2.0, THEME.inset);
    let min = *range.start();
    let span = (range.end() - min).abs().max(0.0001);
    let t = ((value - min) / span).clamp(0.0, 1.0);
    painter.rect_filled(
        Rect::from_min_max(
            track.min,
            pos2(track.left() + track.width() * t, track.bottom()),
        ),
        2.0,
        THEME.accent_dim,
    );
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 5.0, THEME.text);
    painter.circle_stroke(knob, 5.0, Stroke::new(1.0_f32, THEME.accent));
    if !(response.dragged() || response.clicked()) {
        return None;
    }
    let pos = response.interact_pointer_pos()?;
    let nt = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
    Some(min + nt * span)
}

/// Logarithmic slider so frame zoom and minute zoom share the track.
pub fn mini_slider_log(
    ui: &mut Ui,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(110.0, 18.0), Sense::click_and_drag());
    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
    painter.rect_filled(track, 2.0, THEME.inset);
    let min = range.start().max(1.0e-4).ln();
    let max = range.end().max(*range.start()).ln();
    let span = (max - min).abs().max(0.0001);
    let current = if value.is_finite() && value > 0.0 {
        value
    } else {
        *range.start()
    };
    let t = ((current.ln() - min) / span).clamp(0.0, 1.0);
    painter.rect_filled(
        Rect::from_min_max(
            track.min,
            pos2(track.left() + track.width() * t, track.bottom()),
        ),
        2.0,
        THEME.accent_dim,
    );
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 5.0, THEME.text);
    painter.circle_stroke(knob, 5.0, Stroke::new(1.0_f32, THEME.accent));
    if !(response.dragged() || response.clicked()) {
        return None;
    }
    let pos = response.interact_pointer_pos()?;
    let nt = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
    Some((min + nt * span).exp())
}

pub fn readout(ui: &mut Ui, text: &str, width: f32, emphasis: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, THEME.radius as f32, THEME.inset);
    painter.rect_stroke(
        rect,
        THEME.radius as f32,
        theme::hairline_stroke(),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        THEME.mono(11.0),
        if emphasis {
            THEME.accent
        } else {
            THEME.text_dim
        },
    );
}

pub fn transport_glyph(ui: &mut Ui, tip: &str, draw: impl FnOnce(&Painter, Rect)) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(26.0, 26.0), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        THEME.control_hover
    } else {
        THEME.control
    };
    painter.rect_filled(rect, 3.0, fill);
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0_f32, THEME.hairline),
        egui::StrokeKind::Inside,
    );
    draw(painter, rect.shrink(7.0));
    response.on_hover_text(tip).clicked()
}

pub fn play_button(ui: &mut Ui, playing: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(36.0, 28.0), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        Color32::from_rgb(84, 224, 198)
    } else {
        THEME.accent
    };
    painter.rect_filled(rect, 3.0, fill);
    let c = rect.center();
    if playing {
        painter.rect_filled(
            Rect::from_center_size(c - Vec2::new(3.0, 0.0), Vec2::new(3.0, 10.0)),
            0.5,
            THEME.accent_text,
        );
        painter.rect_filled(
            Rect::from_center_size(c + Vec2::new(3.0, 0.0), Vec2::new(3.0, 10.0)),
            0.5,
            THEME.accent_text,
        );
    } else {
        painter.add(Shape::convex_polygon(
            vec![
                pos2(c.x - 4.0, c.y - 6.0),
                pos2(c.x + 6.0, c.y),
                pos2(c.x - 4.0, c.y + 6.0),
            ],
            THEME.accent_text,
            Stroke::NONE,
        ));
    }
    response
        .on_hover_text(if playing { "Pause" } else { "Play" })
        .clicked()
}

pub fn choice_card(ui: &mut Ui, title: &str, meta: &str, selected: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        THEME.accent_dim
    } else if response.hovered() {
        THEME.control
    } else {
        THEME.inset
    };
    painter.rect_filled(rect, 4.0, fill);
    let stroke = if selected {
        Stroke::new(1.0_f32, THEME.accent)
    } else {
        Stroke::new(1.0_f32, THEME.hairline)
    };
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
    if selected {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
            0.0,
            THEME.accent,
        );
    }
    painter.text(
        pos2(rect.left() + 14.0, rect.top() + 12.0),
        Align2::LEFT_TOP,
        title,
        FontId::new(13.0, egui::FontFamily::Proportional),
        THEME.text,
    );
    painter.text(
        pos2(rect.left() + 14.0, rect.top() + 28.0),
        Align2::LEFT_TOP,
        meta,
        FontId::new(11.0, egui::FontFamily::Proportional),
        THEME.text_dim,
    );
    ui.add_space(4.0);
    response
}

pub fn paint_select(painter: &Painter, rect: Rect) {
    let color = THEME.text;
    painter.line_segment(
        [rect.left_top(), rect.center()],
        Stroke::new(1.4_f32, color),
    );
    painter.line_segment(
        [rect.left_top(), pos2(rect.left() + 4.0, rect.bottom())],
        Stroke::new(1.4_f32, color),
    );
    painter.line_segment(
        [
            rect.center(),
            pos2(rect.left() + 5.0, rect.center().y + 3.0),
        ],
        Stroke::new(1.4_f32, color),
    );
}

pub fn paint_razor(painter: &Painter, rect: Rect) {
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        Stroke::new(1.6_f32, THEME.text),
    );
    painter.circle_filled(rect.right_top(), 2.2, THEME.amber);
}

pub fn paint_ripple(painter: &Painter, rect: Rect) {
    let y = rect.center().y;
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.right(), y)],
        Stroke::new(1.3_f32, THEME.text),
    );
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.left() + 4.0, y - 3.0)],
        Stroke::new(1.3_f32, THEME.text),
    );
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.left() + 4.0, y + 3.0)],
        Stroke::new(1.3_f32, THEME.text),
    );
    painter.line_segment(
        [pos2(rect.right(), y), pos2(rect.right() - 4.0, y - 3.0)],
        Stroke::new(1.3_f32, THEME.text),
    );
    painter.line_segment(
        [pos2(rect.right(), y), pos2(rect.right() - 4.0, y + 3.0)],
        Stroke::new(1.3_f32, THEME.text),
    );
}

pub fn paint_roll(painter: &Painter, rect: Rect) {
    let x = rect.center().x;
    painter.vline(x, rect.y_range(), Stroke::new(1.4_f32, THEME.text));
    painter.line_segment(
        [
            pos2(rect.left(), rect.center().y),
            pos2(x - 2.0, rect.center().y),
        ],
        Stroke::new(1.3_f32, THEME.amber),
    );
    painter.line_segment(
        [
            pos2(x + 2.0, rect.center().y),
            pos2(rect.right(), rect.center().y),
        ],
        Stroke::new(1.3_f32, THEME.accent),
    );
}

pub fn paint_slip(painter: &Painter, rect: Rect) {
    let y = rect.center().y;
    painter.rect_stroke(
        Rect::from_min_max(
            pos2(rect.left() + 3.0, rect.top() + 2.0),
            pos2(rect.right() - 3.0, rect.bottom() - 2.0),
        ),
        1.0,
        Stroke::new(1.2_f32, THEME.text_dim),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.right(), y)],
        Stroke::new(1.3_f32, THEME.text),
    );
}

pub fn paint_slide(painter: &Painter, rect: Rect) {
    painter.rect_filled(
        Rect::from_center_size(rect.center(), Vec2::new(6.0, rect.height() - 2.0)),
        1.0,
        THEME.amber,
    );
    let y = rect.center().y;
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.center().x - 5.0, y)],
        Stroke::new(1.2_f32, THEME.text),
    );
    painter.line_segment(
        [pos2(rect.center().x + 5.0, y), pos2(rect.right(), y)],
        Stroke::new(1.2_f32, THEME.text),
    );
}

pub fn tri_left(painter: &Painter, rect: Rect) {
    let c = rect.center();
    painter.add(Shape::convex_polygon(
        vec![
            pos2(c.x + 4.0, c.y - 5.0),
            pos2(c.x - 4.0, c.y),
            pos2(c.x + 4.0, c.y + 5.0),
        ],
        THEME.text,
        Stroke::NONE,
    ));
}

pub fn tri_right(painter: &Painter, rect: Rect) {
    let c = rect.center();
    painter.add(Shape::convex_polygon(
        vec![
            pos2(c.x - 4.0, c.y - 5.0),
            pos2(c.x + 4.0, c.y),
            pos2(c.x - 4.0, c.y + 5.0),
        ],
        THEME.text,
        Stroke::NONE,
    ));
}

pub fn bar_left(painter: &Painter, rect: Rect) {
    painter.vline(
        rect.left() + 1.0,
        rect.y_range(),
        Stroke::new(1.5_f32, THEME.text),
    );
    tri_left(painter, rect.translate(Vec2::new(3.0, 0.0)));
}

pub fn bar_right(painter: &Painter, rect: Rect) {
    painter.vline(
        rect.right() - 1.0,
        rect.y_range(),
        Stroke::new(1.5_f32, THEME.text),
    );
    tri_right(painter, rect.translate(Vec2::new(-3.0, 0.0)));
}
