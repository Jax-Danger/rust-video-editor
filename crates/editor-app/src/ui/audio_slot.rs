//! Audio workspace.
//!
//! The page chrome (header, timecode, program meter) comes from the layout
//! pass. The track bay is the mixer: one strip per audio track, plus master.

use editor_core::meter_amount;
use egui::{pos2, Align2, Rect, RichText, Sense, Vec2};

use crate::app::MeridianApp;
use crate::theme::{self, THEME};
use crate::ui::format_tc;
use crate::ui::mixer::mixer_bay;
use crate::ui::widgets;

pub fn audio_workspace(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let meter = app.audio.master_meter();
    let badge = app.audio.badge().to_string();
    let playhead = app.playhead;
    let timebase = app.timebase();
    widgets::panel_header(ui, "Audio", |ui| {
        widgets::readout(ui, &format_tc(playhead, timebase), 108.0, true);
        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new(&badge)
                .font(THEME.font(11.0))
                .color(if badge == "Audio" {
                    THEME.accent
                } else {
                    THEME.text_mute
                }),
        );
    });
    let status = app.audio.status().to_string();
    if !status.is_empty() {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_LG);
            ui.label(
                RichText::new(status)
                    .font(THEME.font(11.0))
                    .color(THEME.text_mute),
            );
        });
    }

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ui.available_height()),
        Sense::hover(),
    );
    let meter_rect = Rect::from_min_max(
        pos2(rect.right() - 108.0, rect.top() + theme::SPACE_MD),
        pos2(
            rect.right() - theme::SPACE_LG,
            rect.bottom() - theme::SPACE_MD,
        ),
    );
    let bay = Rect::from_min_max(
        pos2(rect.left(), rect.top()),
        pos2(meter_rect.left() - theme::SPACE_SM, rect.bottom()),
    );
    {
        let mut bay_ui = ui.new_child(egui::UiBuilder::new().max_rect(bay));
        mixer_bay(&mut bay_ui, app);
    }
    // The bay child can shrink this Ui's clip. Paint the program meter on the
    // panel layer so the column stays at the right edge of the page.
    let painter = ui
        .ctx()
        .layer_painter(ui.layer_id())
        .with_clip_rect(meter_rect);
    paint_program_meter(&painter, meter_rect, &meter, &badge);
}

fn paint_program_meter(
    painter: &egui::Painter,
    rect: Rect,
    meter: &crate::audio::MeterReadout,
    badge: &str,
) {
    painter.rect_filled(rect, THEME.radius as f32, THEME.inset);
    painter.rect_stroke(
        rect,
        THEME.radius as f32,
        theme::hairline_stroke(),
        egui::StrokeKind::Inside,
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 12.0),
        Align2::CENTER_CENTER,
        "PROGRAM",
        THEME.font(10.0),
        THEME.text_mute,
    );
    let column_h = (rect.height() - 64.0).max(40.0);
    for index in 0..2 {
        let x = rect.center().x + (index as f32 - 0.5) * 22.0 - 5.0;
        let column = Rect::from_min_size(pos2(x, rect.top() + 26.0), Vec2::new(10.0, column_h));
        painter.rect_filled(column, 1.0, THEME.bg);
        let level = meter_amount(meter.peak[index]);
        let fill_h = column.height() * level;
        let db = editor_core::linear_to_db(meter.peak[index]);
        let color = if db >= -3.0 {
            THEME.danger
        } else if db >= -12.0 {
            THEME.amber
        } else {
            THEME.audio
        };
        if level > 0.0 {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(column.left(), column.bottom() - fill_h),
                    column.right_bottom(),
                ),
                1.0,
                color,
            );
        }
        let hold = meter_amount(meter.hold[index]);
        if hold > 0.001 {
            let y = column.bottom() - column.height() * hold;
            painter.hline(column.x_range(), y, egui::Stroke::new(1.5_f32, THEME.text));
        }
        painter.rect_stroke(
            column,
            1.0,
            theme::hairline_stroke(),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        pos2(rect.center().x, rect.bottom() - 14.0),
        Align2::CENTER_CENTER,
        badge,
        THEME.font(10.0),
        THEME.text_dim,
    );
}
