//! Audio workspace slot.
//!
//! The page is chrome only: a read-only map of the sequence's audio tracks
//! and the program meter already driven by playback. Channel strips, routing,
//! and faders dock here later.

use editor_core::TrackKind;
use egui::{pos2, Align2, Rect, RichText, Sense, Stroke, Vec2};

use crate::app::MeridianApp;
use crate::theme::{self, THEME};
use crate::ui::format_tc;
use crate::ui::widgets;

pub fn audio_workspace(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let peaks = app.audio.peaks();
    let badge = app.audio.badge().to_string();
    let playhead = app.playhead;
    let timebase = app.timebase();
    widgets::panel_header(ui, "Audio", |ui| {
        widgets::readout(ui, &format_tc(playhead, timebase), 108.0, true);
        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new(&badge)
                .font(THEME.font(11.0))
                .color(THEME.text_mute),
        );
    });

    let Some(sequence) = app.session.project().active().cloned() else {
        widgets::empty_note(ui, "Open a sequence to see its audio tracks.");
        return;
    };
    let names: Vec<(String, bool, bool)> = sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
        .map(|track| (track.name.clone(), track.muted, track.solo))
        .collect();

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ui.available_height()),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, THEME.panel);

    let meter = Rect::from_min_max(
        pos2(rect.right() - 108.0, rect.top() + theme::SPACE_LG),
        pos2(
            rect.right() - theme::SPACE_LG,
            rect.bottom() - theme::SPACE_LG,
        ),
    );
    let bay = Rect::from_min_max(
        pos2(rect.left() + theme::SPACE_LG, rect.top() + theme::SPACE_LG),
        pos2(
            meter.left() - theme::SPACE_LG,
            rect.bottom() - theme::SPACE_LG,
        ),
    );

    painter.text(
        pos2(bay.left(), bay.top()),
        Align2::LEFT_TOP,
        "TRACKS",
        THEME.font(10.0),
        THEME.text_mute,
    );
    let strips = Rect::from_min_max(pos2(bay.left(), bay.top() + 18.0), bay.right_bottom());
    paint_strips(&painter, strips, &names);
    paint_program_meter(&painter, meter, peaks, &badge);
}

fn paint_strips(painter: &egui::Painter, bay: Rect, names: &[(String, bool, bool)]) {
    painter.rect_filled(bay, THEME.radius as f32, THEME.inset);
    painter.rect_stroke(
        bay,
        THEME.radius as f32,
        theme::hairline_stroke(),
        egui::StrokeKind::Inside,
    );
    if names.is_empty() {
        painter.text(
            bay.center(),
            Align2::CENTER_CENTER,
            "No audio tracks in this sequence",
            THEME.font(12.0),
            THEME.text_dim,
        );
        return;
    }
    let gap = theme::SPACE_MD;
    let count = names.len() as f32;
    let inner = bay.shrink2(Vec2::new(theme::SPACE_MD, theme::SPACE_MD));
    let strip_w = ((inner.width() - gap * (count - 1.0)) / count).clamp(48.0, 84.0);
    let used = strip_w * count + gap * (count - 1.0);
    let origin = inner.left() + (inner.width() - used).max(0.0) * 0.5;
    for (index, (name, muted, solo)) in names.iter().enumerate() {
        let strip = Rect::from_min_size(
            pos2(origin + index as f32 * (strip_w + gap), inner.top()),
            Vec2::new(strip_w, inner.height()),
        );
        painter.rect_filled(strip, THEME.radius as f32, THEME.header);
        painter.rect_stroke(
            strip,
            THEME.radius as f32,
            theme::hairline_stroke(),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            Rect::from_min_size(
                strip.left_bottom() - Vec2::new(0.0, 3.0),
                Vec2::new(strip.width(), 3.0),
            ),
            0.0,
            THEME.audio,
        );
        painter.text(
            pos2(strip.center().x, strip.top() + 14.0),
            Align2::CENTER_CENTER,
            name,
            THEME.font(12.0),
            THEME.text,
        );
        let flags = match (muted, solo) {
            (true, true) => "MUTE  SOLO",
            (true, false) => "MUTE",
            (false, true) => "SOLO",
            (false, false) => "",
        };
        if !flags.is_empty() {
            painter.text(
                pos2(strip.center().x, strip.top() + 30.0),
                Align2::CENTER_CENTER,
                flags,
                THEME.font(9.0),
                if *muted { THEME.danger } else { THEME.amber },
            );
        }
        let mid = Rect::from_min_max(
            pos2(strip.left() + 10.0, strip.top() + 48.0),
            pos2(strip.right() - 10.0, strip.bottom() - 16.0),
        );
        painter.hline(
            mid.x_range(),
            mid.center().y,
            Stroke::new(1.0_f32, THEME.border),
        );
    }
}

fn paint_program_meter(painter: &egui::Painter, rect: Rect, peaks: [f32; 2], badge: &str) {
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
    for (index, peak) in peaks.iter().enumerate() {
        let x = rect.center().x + (index as f32 - 0.5) * 22.0 - 5.0;
        let column = Rect::from_min_size(pos2(x, rect.top() + 26.0), Vec2::new(10.0, column_h));
        painter.rect_filled(column, 1.0, THEME.bg);
        let level = peak.clamp(0.0, 1.0);
        let fill_h = column.height() * level;
        let color = if level > 0.92 {
            THEME.danger
        } else if level > 0.7 {
            THEME.amber
        } else {
            THEME.audio
        };
        painter.rect_filled(
            Rect::from_min_max(
                pos2(column.left(), column.bottom() - fill_h),
                column.right_bottom(),
            ),
            1.0,
            color,
        );
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
