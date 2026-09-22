//! Fairlight-inspired mixer: one strip per audio track, plus the master bus.
//!
//! Faders are decibels (−∞…+12). Pan is constant-power and unity at center.
//! Each strip has a 3-band EQ (L/M/H ±12 dB, optional low-cut), a dynamics
//! compressor (threshold, ratio, attack, release, makeup), and sidechain
//! ducking (enable, source track, threshold, amount, attack, release). Meters
//! show peak, a brighter RMS fill, and a peak-hold tick. Mute, solo, fader,
//! pan, EQ, compressor, duck, and clip gain all feed the same bus playback
//! and export use.

use editor_core::{
    clamp_attack_ms, clamp_duck_amount, clamp_duck_attack, clamp_duck_release,
    clamp_duck_threshold, clamp_eq_db, clamp_makeup_db, clamp_ratio, clamp_release_ms,
    clamp_threshold_db, fader_pos_to_linear, format_db, format_duck_amount, format_duck_threshold,
    format_eq_db, format_makeup_db, format_pan, format_ratio, format_threshold_db, format_time_ms,
    linear_to_fader_pos, meter_amount, set_clip_gain_at, set_master_fader, set_track_compressor,
    set_track_duck, set_track_duck_enabled, set_track_duck_source, set_track_eq,
    set_track_eq_low_cut, set_track_fader, set_track_pan, toggle_volume_key, ClipId,
    CompressorParam, DuckParam, EqBand, Frame, TrackCompressor, TrackDuck, TrackEq3, TrackId,
    TrackKind, AMOUNT_DB_MAX, AMOUNT_DB_MIN, ATTACK_MS_MAX, ATTACK_MS_MIN,
    DEFAULT_DUCK_AMOUNT_DB, DEFAULT_DUCK_ATTACK_MS, DEFAULT_DUCK_RELEASE_MS,
    DEFAULT_DUCK_THRESHOLD_DB, DUCK_ATTACK_MS_MAX, DUCK_ATTACK_MS_MIN, DUCK_RELEASE_MS_MAX,
    DUCK_RELEASE_MS_MIN, DUCK_THRESHOLD_DB_MAX, DUCK_THRESHOLD_DB_MIN, EQ_DB_MAX, EQ_DB_MIN,
    MAKEUP_DB_MAX, MAKEUP_DB_MIN, RATIO_MAX, RATIO_MIN, RELEASE_MS_MAX, RELEASE_MS_MIN,
    THRESHOLD_DB_MAX, THRESHOLD_DB_MIN,
};
use egui::{pos2, Align2, Color32, Id, Rect, Sense, Shape, Stroke, Vec2};

use crate::app::{note_track_flag, MeridianApp};
use crate::audio::MeterReadout;
use crate::theme::{self, THEME};
use crate::ui::widgets;

const STRIP_W: f32 = 118.0;

struct StripSnap {
    id: TrackId,
    name: String,
    fader: f32,
    pan: f32,
    eq: TrackEq3,
    compressor: TrackCompressor,
    duck: TrackDuck,
    muted: bool,
    solo: bool,
    clip_id: Option<ClipId>,
    clip_name: String,
    clip_in: i64,
    clip_gain: f32,
    clip_keyed: bool,
    meter: MeterReadout,
}

/// Channel strips for the Audio page bay. The page header and program meter
/// stay in `audio_workspace`.
pub(crate) fn mixer_bay(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let playhead = app.playhead;
    let strips = snapshot(app, playhead);
    let master = app
        .session
        .project()
        .active()
        .map(|sequence| sequence.master_fader)
        .unwrap_or(1.0);
    let master_meter = app.audio.master_meter();
    let height = ui.available_height().max(240.0);

    egui::ScrollArea::horizontal()
        .id_salt("mixer_strips")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_space(10.0);
                if strips.is_empty() {
                    ui.allocate_ui(Vec2::new(280.0, height), |ui| {
                        ui.add_space(24.0);
                        widgets::empty_note(
                            ui,
                            "No audio tracks. Sequence → Add Audio Track, then drop sound on the timeline.",
                        );
                    });
                }
                let lanes: Vec<(TrackId, String)> = strips
                    .iter()
                    .map(|strip| (strip.id, strip.name.clone()))
                    .collect();
                for strip in &strips {
                    channel_strip(ui, app, strip, &lanes, height);
                }
                master_strip(ui, app, master, master_meter, height);
                ui.add_space(10.0);
            });
        });
}

fn snapshot(app: &MeridianApp, playhead: i64) -> Vec<StripSnap> {
    let Some(sequence) = app.session.project().active() else {
        return Vec::new();
    };
    let mut strips = Vec::new();
    for track in &sequence.tracks {
        if track.kind != TrackKind::Audio {
            continue;
        }
        let clip = track.clips.iter().find(|clip| clip.covers(Frame(playhead)));
        let rel = clip
            .map(|clip| (playhead - clip.timeline_in.0).max(0))
            .unwrap_or(0);
        strips.push(StripSnap {
            id: track.id,
            name: track.name.clone(),
            fader: track.fader,
            pan: track.pan,
            eq: track.eq,
            compressor: track.compressor,
            duck: track.duck,
            muted: track.muted,
            solo: track.solo,
            clip_id: clip.map(|clip| clip.id),
            clip_name: clip
                .map(|clip| clip.name.clone())
                .unwrap_or_else(|| "—".into()),
            clip_in: clip.map(|clip| clip.timeline_in.0).unwrap_or(0),
            clip_gain: clip.map(|clip| clip.volume.value_at(rel)).unwrap_or(1.0),
            clip_keyed: clip.map(|clip| clip.volume.has_key(rel)).unwrap_or(false),
            meter: app.audio.track_meter(track.id.0),
        });
    }
    strips
}

fn channel_strip(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    strip: &StripSnap,
    lanes: &[(TrackId, String)],
    height: f32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, height), Sense::hover());
    paint_well(ui, rect);
    let accent = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 3.0));
    let name = Rect::from_min_size(
        pos2(rect.left() + 8.0, rect.top() + 10.0),
        Vec2::new(rect.width() - 16.0, 16.0),
    );
    let clip_rect = Rect::from_min_size(
        pos2(rect.left() + 8.0, name.bottom() + 1.0),
        Vec2::new(rect.width() - 16.0, 14.0),
    );
    {
        let painter = ui.painter();
        painter.rect_filled(accent, 0.0, THEME.audio);
        painter.text(
            name.left_center(),
            Align2::LEFT_CENTER,
            &strip.name,
            THEME.font(13.0),
            THEME.text,
        );
        painter.text(
            clip_rect.left_center(),
            Align2::LEFT_CENTER,
            &strip.clip_name,
            THEME.font(11.0),
            THEME.text_mute,
        );
    }
    if let Some(clip_id) = strip.clip_id {
        let hit = ui.interact(clip_rect, Id::new(("mix_clip", strip.id.0)), Sense::click());
        if hit.clicked() {
            app.selected = vec![clip_id];
            app.status = format!("Selected {}.", strip.clip_name);
        }
    }

    let mute = Rect::from_min_size(
        pos2(rect.left() + 10.0, clip_rect.bottom() + 8.0),
        Vec2::new(28.0, 20.0),
    );
    let solo = Rect::from_min_size(pos2(mute.right() + 6.0, mute.top()), Vec2::new(28.0, 20.0));
    if pill(
        ui,
        mute,
        Id::new(("mix_m", strip.id.0)),
        "M",
        strip.muted,
        THEME.danger,
    ) {
        note_track_flag(app, strip.id, editor_core::TrackFlag::Mute, !strip.muted);
    }
    if pill(
        ui,
        solo,
        Id::new(("mix_s", strip.id.0)),
        "S",
        strip.solo,
        THEME.amber,
    ) {
        note_track_flag(app, strip.id, editor_core::TrackFlag::Solo, !strip.solo);
    }

    let db = Rect::from_min_size(
        pos2(rect.left() + 8.0, mute.bottom() + 8.0),
        Vec2::new(rect.width() - 16.0, 18.0),
    );
    {
        let painter = ui.painter();
        painter.rect_filled(db, 2.0, THEME.inset);
        painter.text(
            db.center(),
            Align2::CENTER_CENTER,
            format_db(strip.fader),
            THEME.mono(12.0),
            THEME.text,
        );
    }

    let meter_top = db.bottom() + 8.0;
    let room = (rect.bottom() - 4.0 - meter_top).max(0.0);
    let fader_h = if room < 96.0 {
        room * 0.42
    } else {
        (room * 0.46).clamp(72.0, 200.0)
    };
    let meter_bottom = meter_top + fader_h;
    let meter_rect = Rect::from_min_max(
        pos2(rect.left() + 14.0, meter_top),
        pos2(rect.left() + 36.0, meter_bottom),
    );
    let fader_rect = Rect::from_min_max(
        pos2(rect.left() + 52.0, meter_top),
        pos2(rect.left() + 78.0, meter_bottom),
    );
    if meter_rect.height() > 24.0 {
        let columns = meter_columns(meter_rect);
        if clip_lamp(
            ui,
            meter_rect,
            Id::new(("mix_clip_lamp", strip.id.0)),
            strip.meter.clip,
        ) {
            app.audio.clear_track_clip(strip.id.0);
        }
        paint_meter_pair(ui.painter(), columns, strip.meter);
        fader(
            ui,
            fader_rect,
            Id::new(("mix_fader", strip.id.0)),
            strip.fader,
            |app, value| {
                let track = strip.id;
                let _ = app.session.edit("Fader", |project| {
                    let seq = project
                        .active_sequence
                        .ok_or(editor_core::EditError::NoActiveSequence)?;
                    set_track_fader(project, seq, track, value)
                });
            },
            app,
        );
    }

    let controls = Rect::from_min_max(
        pos2(rect.left(), meter_bottom + 6.0),
        pos2(rect.right(), rect.bottom() - 2.0),
    );
    if controls.height() > 8.0 {
        let mut body = ui.new_child(egui::UiBuilder::new().max_rect(controls));
        egui::ScrollArea::vertical()
            .id_salt(("mix_strip_body", strip.id.0))
            .auto_shrink([false, false])
            .show(&mut body, |ui| {
                let width = ui.available_width();
                let clip_h = if strip.clip_id.is_some() { 32.0 } else { 0.0 };
                let content_h = 22.0 + 16.0 + 58.0 + 74.0 + 84.0 + clip_h;
                let (content, _) =
                    ui.allocate_exact_size(Vec2::new(width, content_h), Sense::hover());
                let mut y = content.top();
                let pan_rect = Rect::from_min_size(
                    pos2(content.left() + 10.0, y),
                    Vec2::new(content.width() - 20.0, 22.0),
                );
                y += 24.0;
                let label = Rect::from_min_size(
                    pos2(content.left() + 8.0, y),
                    Vec2::new(content.width() - 16.0, 14.0),
                );
                y += 16.0;
                let eq_rect = Rect::from_min_size(
                    pos2(content.left() + 8.0, y),
                    Vec2::new(content.width() - 16.0, 54.0),
                );
                y += 58.0;
                let dyn_rect = Rect::from_min_size(
                    pos2(content.left() + 8.0, y),
                    Vec2::new(content.width() - 16.0, 70.0),
                );
                y += 74.0;
                let duck_rect = Rect::from_min_size(
                    pos2(content.left() + 8.0, y),
                    Vec2::new(content.width() - 16.0, 80.0),
                );
                y += 84.0;
                pan_slider(ui, app, pan_rect, strip);
                ui.painter().text(
                    label.center(),
                    Align2::CENTER_CENTER,
                    format!("Pan {}", format_pan(strip.pan)),
                    THEME.mono(10.0),
                    THEME.text_dim,
                );
                eq_section(ui, app, eq_rect, strip);
                dynamics_section(ui, app, dyn_rect, strip);
                duck_section(ui, app, duck_rect, strip, lanes);
                if strip.clip_id.is_some() {
                    let gain_rect = Rect::from_min_size(
                        pos2(content.left() + 8.0, y),
                        Vec2::new(content.width() - 16.0, 28.0),
                    );
                    clip_gain_row(ui, app, gain_rect, strip);
                }
            });
    }
}

fn eq_section(ui: &mut egui::Ui, app: &mut MeridianApp, rect: Rect, strip: &StripSnap) {
    let painter = ui.painter();
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "EQ",
        THEME.font(10.0),
        THEME.text_mute,
    );
    let lc = Rect::from_min_size(
        pos2(rect.right() - 24.0, rect.top()),
        Vec2::new(24.0, 14.0),
    );
    if pill(
        ui,
        lc,
        Id::new(("mix_eq_lc", strip.id.0)),
        "LC",
        strip.eq.low_cut,
        THEME.audio,
    ) {
        let track = strip.id;
        let enabled = !strip.eq.low_cut;
        let _ = app.session.edit("EQ low cut", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            set_track_eq_low_cut(project, seq, track, enabled)
        });
    }
    let row_h = 14.0;
    let start_y = rect.top() + 14.0;
    eq_band_row(
        ui,
        app,
        Rect::from_min_size(
            pos2(rect.left(), start_y),
            Vec2::new(rect.width(), row_h),
        ),
        strip,
        EqBand::Low,
        "L",
        strip.eq.low,
    );
    eq_band_row(
        ui,
        app,
        Rect::from_min_size(
            pos2(rect.left(), start_y + row_h),
            Vec2::new(rect.width(), row_h),
        ),
        strip,
        EqBand::Mid,
        "M",
        strip.eq.mid,
    );
    eq_band_row(
        ui,
        app,
        Rect::from_min_size(
            pos2(rect.left(), start_y + row_h * 2.0),
            Vec2::new(rect.width(), row_h),
        ),
        strip,
        EqBand::High,
        "H",
        strip.eq.high,
    );
}

fn eq_db_to_pos(db: f32) -> f32 {
    (clamp_eq_db(db) - EQ_DB_MIN) / (EQ_DB_MAX - EQ_DB_MIN)
}

fn eq_pos_to_db(pos: f32) -> f32 {
    EQ_DB_MIN + pos.clamp(0.0, 1.0) * (EQ_DB_MAX - EQ_DB_MIN)
}

fn eq_band_row(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    rect: Rect,
    strip: &StripSnap,
    band: EqBand,
    label: &str,
    gain_db: f32,
) {
    let painter = ui.painter();
    painter.text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        THEME.mono(10.0),
        THEME.text_dim,
    );
    let track = Rect::from_min_max(
        pos2(rect.left() + 14.0, rect.center().y - 2.0),
        pos2(rect.right() - 24.0, rect.center().y + 2.0),
    );
    let response = ui.interact(
        track.expand2(Vec2::new(0.0, 6.0)),
        Id::new(("mix_eq", strip.id.0, label)),
        Sense::click_and_drag(),
    );
    painter.rect_filled(track, 2.0, THEME.inset);
    painter.vline(
        track.center().x,
        track.top()..=track.bottom(),
        Stroke::new(1.0_f32, THEME.border),
    );
    let t = eq_db_to_pos(gain_db);
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 4.0, THEME.text);
    painter.circle_stroke(knob, 4.0, Stroke::new(1.0_f32, THEME.audio));
    painter.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        format_eq_db(gain_db),
        THEME.mono(9.0),
        THEME.text_mute,
    );
    if response.double_clicked() {
        app.session.end_interactive();
        let track_id = strip.id;
        let _ = app.session.edit("EQ", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            set_track_eq(project, seq, track_id, band, 0.0)
        });
        return;
    }
    if response.drag_started() {
        app.session.begin_interactive("EQ");
    }
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let next = eq_pos_to_db(((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0));
            let track_id = strip.id;
            let _ = app.session.edit("EQ", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                set_track_eq(project, seq, track_id, band, next)
            });
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
}

fn dynamics_section(ui: &mut egui::Ui, app: &mut MeridianApp, rect: Rect, strip: &StripSnap) {
    let painter = ui.painter();
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "DYN",
        THEME.font(10.0),
        THEME.text_mute,
    );
    let row_h = 12.0;
    let start_y = rect.top() + 14.0;
    let row_w = rect.width();
    dyn_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y), Vec2::new(row_w, row_h)),
        strip,
        CompressorParam::Threshold,
        "THR",
        strip.compressor.threshold_db,
        THRESHOLD_DB_MIN,
        THRESHOLD_DB_MAX,
        format_threshold_db,
    );
    dyn_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h), Vec2::new(row_w, row_h)),
        strip,
        CompressorParam::Ratio,
        "RAT",
        strip.compressor.ratio,
        RATIO_MIN,
        RATIO_MAX,
        format_ratio,
    );
    dyn_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h * 2.0), Vec2::new(row_w, row_h)),
        strip,
        CompressorParam::Attack,
        "ATK",
        strip.compressor.attack_ms,
        ATTACK_MS_MIN,
        ATTACK_MS_MAX,
        format_time_ms,
    );
    dyn_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h * 3.0), Vec2::new(row_w, row_h)),
        strip,
        CompressorParam::Release,
        "REL",
        strip.compressor.release_ms,
        RELEASE_MS_MIN,
        RELEASE_MS_MAX,
        format_time_ms,
    );
    dyn_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h * 4.0), Vec2::new(row_w, row_h)),
        strip,
        CompressorParam::Makeup,
        "MK",
        strip.compressor.makeup_db,
        MAKEUP_DB_MIN,
        MAKEUP_DB_MAX,
        format_makeup_db,
    );
}

fn dyn_row<F>(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    rect: Rect,
    strip: &StripSnap,
    param: CompressorParam,
    label: &str,
    value: f32,
    min: f32,
    max: f32,
    format: F,
) where
    F: Fn(f32) -> String,
{
    let painter = ui.painter();
    painter.text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        THEME.mono(9.0),
        THEME.text_dim,
    );
    let track = Rect::from_min_max(
        pos2(rect.left() + 24.0, rect.center().y - 2.0),
        pos2(rect.right() - 28.0, rect.center().y + 2.0),
    );
    let response = ui.interact(
        track.expand2(Vec2::new(0.0, 5.0)),
        Id::new(("mix_dyn", strip.id.0, label)),
        Sense::click_and_drag(),
    );
    painter.rect_filled(track, 2.0, THEME.inset);
    let t = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 3.0, THEME.text);
    painter.circle_stroke(knob, 3.0, Stroke::new(1.0_f32, THEME.amber));
    painter.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        format(value),
        THEME.mono(8.0),
        THEME.text_mute,
    );
    let default = match param {
        CompressorParam::Threshold => 0.0,
        CompressorParam::Ratio => 1.0,
        CompressorParam::Attack => editor_core::DEFAULT_ATTACK_MS,
        CompressorParam::Release => editor_core::DEFAULT_RELEASE_MS,
        CompressorParam::Makeup => 0.0,
    };
    if response.double_clicked() {
        app.session.end_interactive();
        let track_id = strip.id;
        let _ = app.session.edit("Dynamics", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            set_track_compressor(project, seq, track_id, param, default)
        });
        return;
    }
    if response.drag_started() {
        app.session.begin_interactive("Dynamics");
    }
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let pos = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let next = min + pos * (max - min);
            let next = match param {
                CompressorParam::Threshold => clamp_threshold_db(next),
                CompressorParam::Ratio => clamp_ratio(next),
                CompressorParam::Attack => clamp_attack_ms(next),
                CompressorParam::Release => clamp_release_ms(next),
                CompressorParam::Makeup => clamp_makeup_db(next),
            };
            let track_id = strip.id;
            let _ = app.session.edit("Dynamics", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                set_track_compressor(project, seq, track_id, param, next)
            });
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
}

fn duck_section(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    rect: Rect,
    strip: &StripSnap,
    lanes: &[(TrackId, String)],
) {
    let painter = ui.painter();
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "DUCK",
        THEME.font(10.0),
        THEME.text_mute,
    );
    let enable = Rect::from_min_size(pos2(rect.right() - 28.0, rect.top()), Vec2::new(28.0, 14.0));
    if pill(
        ui,
        enable,
        Id::new(("mix_duck_on", strip.id.0)),
        "ON",
        strip.duck.enabled,
        THEME.audio,
    ) {
        let track = strip.id;
        let enabled = !strip.duck.enabled;
        let _ = app.session.edit("Duck", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            set_track_duck_enabled(project, seq, track, enabled)
        });
    }
    let source_rect = Rect::from_min_size(
        pos2(rect.left(), rect.top() + 14.0),
        Vec2::new(rect.width(), 16.0),
    );
    duck_source(ui, app, source_rect, strip, lanes);
    let row_h = 12.0;
    let start_y = rect.top() + 32.0;
    let row_w = rect.width();
    duck_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y), Vec2::new(row_w, row_h)),
        strip,
        DuckParam::Threshold,
        "THR",
        strip.duck.threshold_db,
        DUCK_THRESHOLD_DB_MIN,
        DUCK_THRESHOLD_DB_MAX,
        format_duck_threshold,
    );
    duck_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h), Vec2::new(row_w, row_h)),
        strip,
        DuckParam::Amount,
        "AMT",
        strip.duck.amount_db,
        AMOUNT_DB_MIN,
        AMOUNT_DB_MAX,
        format_duck_amount,
    );
    duck_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h * 2.0), Vec2::new(row_w, row_h)),
        strip,
        DuckParam::Attack,
        "ATK",
        strip.duck.attack_ms,
        DUCK_ATTACK_MS_MIN,
        DUCK_ATTACK_MS_MAX,
        format_time_ms,
    );
    duck_row(
        ui,
        app,
        Rect::from_min_size(pos2(rect.left(), start_y + row_h * 3.0), Vec2::new(row_w, row_h)),
        strip,
        DuckParam::Release,
        "REL",
        strip.duck.release_ms,
        DUCK_RELEASE_MS_MIN,
        DUCK_RELEASE_MS_MAX,
        format_time_ms,
    );
}

fn duck_source(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    rect: Rect,
    strip: &StripSnap,
    lanes: &[(TrackId, String)],
) {
    let selected = strip.duck.source.and_then(|id| {
        lanes
            .iter()
            .find(|(lane, _)| lane.0 == id && *lane != strip.id)
            .map(|(_, name)| name.clone())
    });
    let label = selected.unwrap_or_else(|| {
        if strip.duck.source.is_some() {
            "Missing".into()
        } else {
            "None".into()
        }
    });
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let track_id = strip.id;
    let current = strip.duck.source;
    egui::ComboBox::from_id_salt(("duck_src", strip.id.0))
        .width(rect.width())
        .selected_text(label)
        .show_ui(&mut child, |ui| {
            if ui
                .selectable_label(current.is_none(), "None")
                .on_hover_text("No sidechain")
                .clicked()
            {
                let _ = app.session.edit("Duck source", |project| {
                    let seq = project
                        .active_sequence
                        .ok_or(editor_core::EditError::NoActiveSequence)?;
                    set_track_duck_source(project, seq, track_id, None)
                });
            }
            for (id, name) in lanes {
                if *id == track_id {
                    continue;
                }
                if ui
                    .selectable_label(current == Some(id.0), name)
                    .clicked()
                {
                    let source = *id;
                    let _ = app.session.edit("Duck source", |project| {
                        let seq = project
                            .active_sequence
                            .ok_or(editor_core::EditError::NoActiveSequence)?;
                        set_track_duck_source(project, seq, track_id, Some(source))
                    });
                }
            }
        });
}

fn duck_row<F>(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    rect: Rect,
    strip: &StripSnap,
    param: DuckParam,
    label: &str,
    value: f32,
    min: f32,
    max: f32,
    format: F,
) where
    F: Fn(f32) -> String,
{
    let painter = ui.painter();
    painter.text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        THEME.mono(9.0),
        THEME.text_dim,
    );
    let track = Rect::from_min_max(
        pos2(rect.left() + 24.0, rect.center().y - 2.0),
        pos2(rect.right() - 28.0, rect.center().y + 2.0),
    );
    let response = ui.interact(
        track.expand2(Vec2::new(0.0, 5.0)),
        Id::new(("mix_duck", strip.id.0, label)),
        Sense::click_and_drag(),
    );
    painter.rect_filled(track, 2.0, THEME.inset);
    let t = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 3.0, THEME.text);
    painter.circle_stroke(knob, 3.0, Stroke::new(1.0_f32, THEME.audio));
    painter.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        format(value),
        THEME.mono(8.0),
        THEME.text_mute,
    );
    let default = match param {
        DuckParam::Threshold => DEFAULT_DUCK_THRESHOLD_DB,
        DuckParam::Amount => DEFAULT_DUCK_AMOUNT_DB,
        DuckParam::Attack => DEFAULT_DUCK_ATTACK_MS,
        DuckParam::Release => DEFAULT_DUCK_RELEASE_MS,
    };
    if response.double_clicked() {
        app.session.end_interactive();
        let track_id = strip.id;
        let _ = app.session.edit("Duck", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            set_track_duck(project, seq, track_id, param, default)
        });
        return;
    }
    if response.drag_started() {
        app.session.begin_interactive("Duck");
    }
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let pos = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let next = min + pos * (max - min);
            let next = match param {
                DuckParam::Threshold => clamp_duck_threshold(next),
                DuckParam::Amount => clamp_duck_amount(next),
                DuckParam::Attack => clamp_duck_attack(next),
                DuckParam::Release => clamp_duck_release(next),
            };
            let track_id = strip.id;
            let _ = app.session.edit("Duck", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                set_track_duck(project, seq, track_id, param, next)
            });
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
}

fn master_strip(
    ui: &mut egui::Ui,
    app: &mut MeridianApp,
    fader_linear: f32,
    meter: MeterReadout,
    height: f32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, height), Sense::hover());
    paint_well(ui, rect);
    let painter = ui.painter();
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(rect.width(), 3.0)),
        0.0,
        THEME.accent,
    );
    painter.text(
        pos2(rect.left() + 8.0, rect.top() + 18.0),
        Align2::LEFT_CENTER,
        "Master",
        THEME.font(13.0),
        THEME.text,
    );
    painter.text(
        pos2(rect.left() + 8.0, rect.top() + 34.0),
        Align2::LEFT_CENTER,
        "Bus",
        THEME.font(11.0),
        THEME.text_mute,
    );
    let db = Rect::from_min_size(
        pos2(rect.left() + 8.0, rect.top() + 48.0),
        Vec2::new(rect.width() - 16.0, 18.0),
    );
    painter.rect_filled(db, 2.0, THEME.inset);
    painter.text(
        db.center(),
        Align2::CENTER_CENTER,
        format_db(fader_linear),
        THEME.mono(12.0),
        THEME.accent,
    );
    let meter_rect = Rect::from_min_max(
        pos2(rect.left() + 14.0, db.bottom() + 10.0),
        pos2(rect.left() + 36.0, rect.bottom() - 28.0),
    );
    let fader_rect = Rect::from_min_max(
        pos2(rect.left() + 52.0, meter_rect.top()),
        pos2(rect.left() + 78.0, meter_rect.bottom()),
    );
    if meter_rect.height() > 24.0 {
        let columns = meter_columns(meter_rect);
        if clip_lamp(ui, meter_rect, Id::new("mix_master_clip"), meter.clip) {
            app.audio.clear_master_clip();
        }
        paint_meter_pair(ui.painter(), columns, meter);
        fader(
            ui,
            fader_rect,
            Id::new("mix_master"),
            fader_linear,
            |app, value| {
                let _ = app.session.edit("Master", |project| {
                    let seq = project
                        .active_sequence
                        .ok_or(editor_core::EditError::NoActiveSequence)?;
                    set_master_fader(project, seq, value)
                });
            },
            app,
        );
    }
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 14.0),
        Align2::CENTER_CENTER,
        "peak  rms  hold",
        THEME.font(9.0),
        THEME.text_mute,
    );
}

fn paint_well(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, THEME.radius as f32, THEME.header);
    painter.rect_stroke(
        rect,
        THEME.radius as f32,
        theme::hairline_stroke(),
        egui::StrokeKind::Inside,
    );
}

fn pill(ui: &mut egui::Ui, rect: Rect, id: Id, label: &str, on: bool, on_fill: Color32) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, 3.0, on_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, THEME.control_hover);
    } else {
        painter.rect_filled(rect, 3.0, THEME.control);
    }
    painter.rect_stroke(
        rect,
        THEME.radius as f32,
        Stroke::new(1.0_f32, if on { on_fill } else { THEME.border }),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        THEME.font(11.0),
        if on { Color32::WHITE } else { THEME.text },
    );
    response.clicked()
}

fn fader(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    linear: f32,
    write: impl FnOnce(&mut MeridianApp, f32),
    app: &mut MeridianApp,
) {
    let response = ui
        .interact(rect, id, Sense::click_and_drag())
        .on_hover_text(format!("{}   double-click for 0 dB", format_db(linear)));
    let pos = linear_to_fader_pos(linear);
    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(4.0, rect.height() - 8.0));
    painter.rect_filled(track, 2.0, THEME.inset);
    let fill_top = track.bottom() - track.height() * pos;
    painter.rect_filled(
        Rect::from_min_max(pos2(track.left(), fill_top), track.right_bottom()),
        2.0,
        THEME.accent_dim,
    );
    for db in [0.0_f32, -12.0, -24.0, -48.0] {
        let mark = linear_to_fader_pos(editor_core::db_to_linear(db));
        let y = track.bottom() - track.height() * mark;
        painter.hline(
            (track.right() + 4.0)..=(track.right() + 9.0),
            y,
            Stroke::new(1.0_f32, THEME.text_mute),
        );
    }
    let knob = Rect::from_center_size(pos2(track.center().x, fill_top), Vec2::new(22.0, 10.0));
    painter.rect_filled(knob, 2.0, THEME.text);
    painter.hline(
        (knob.left() + 3.0)..=(knob.right() - 3.0),
        knob.center().y,
        Stroke::new(1.0_f32, THEME.bg),
    );
    if response.double_clicked() {
        app.session.end_interactive();
        write(app, 1.0);
        return;
    }
    if response.drag_started() {
        app.session.begin_interactive("Fader");
    }
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let next = 1.0 - ((pointer.y - track.top()) / track.height()).clamp(0.0, 1.0);
            write(app, fader_pos_to_linear(next));
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
}

fn pan_slider(ui: &mut egui::Ui, app: &mut MeridianApp, rect: Rect, strip: &StripSnap) {
    let response = ui.interact(
        rect,
        Id::new(("mix_pan", strip.id.0)),
        Sense::click_and_drag(),
    );
    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
    painter.rect_filled(track, 2.0, THEME.inset);
    painter.vline(
        track.center().x,
        track.top()..=track.bottom(),
        Stroke::new(1.0_f32, THEME.border),
    );
    let t = (strip.pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 5.0, THEME.text);
    painter.circle_stroke(knob, 5.0, Stroke::new(1.0_f32, THEME.accent));
    if response.double_clicked() {
        app.session.end_interactive();
        let track_id = strip.id;
        let _ = app.session.edit("Pan", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            set_track_pan(project, seq, track_id, 0.0)
        });
        return;
    }
    if response.drag_started() {
        app.session.begin_interactive("Pan");
    }
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let next = (((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0) * 2.0) - 1.0;
            let track_id = strip.id;
            let _ = app.session.edit("Pan", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                set_track_pan(project, seq, track_id, next)
            });
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
}

fn clip_gain_row(ui: &mut egui::Ui, app: &mut MeridianApp, rect: Rect, strip: &StripSnap) {
    let Some(clip_id) = strip.clip_id else {
        return;
    };
    let painter = ui.painter();
    painter.text(
        pos2(rect.left(), rect.top()),
        Align2::LEFT_TOP,
        "Clip",
        THEME.font(10.0),
        THEME.text_mute,
    );
    let diamond = Rect::from_center_size(
        pos2(rect.right() - 6.0, rect.top() + 6.0),
        Vec2::new(10.0, 10.0),
    );
    paint_diamond(painter, diamond, strip.clip_keyed);
    let key = ui.interact(
        diamond.expand(3.0),
        Id::new(("mix_gain_key", strip.id.0)),
        Sense::click(),
    );
    if key.clicked() {
        let rel = (app.playhead - strip.clip_in).max(0);
        let _ = app.session.edit("Clip gain key", |project| {
            let seq = project
                .active_sequence
                .ok_or(editor_core::EditError::NoActiveSequence)?;
            toggle_volume_key(project, seq, clip_id, rel)
        });
    }
    let track = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - 8.0),
        pos2(rect.right(), rect.bottom() - 4.0),
    );
    let response = ui.interact(
        track.expand2(Vec2::new(0.0, 6.0)),
        Id::new(("mix_gain", strip.id.0)),
        Sense::click_and_drag(),
    );
    let t = (strip.clip_gain / 2.0).clamp(0.0, 1.0);
    painter.rect_filled(track, 2.0, THEME.inset);
    painter.rect_filled(
        Rect::from_min_max(
            track.min,
            pos2(track.left() + track.width() * t, track.bottom()),
        ),
        2.0,
        THEME.accent_dim,
    );
    let knob = pos2(track.left() + track.width() * t, track.center().y);
    painter.circle_filled(knob, 4.0, THEME.text);
    if response.drag_started() {
        app.session.begin_interactive("Clip gain");
    }
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let next = (((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0)) * 2.0;
            let rel = (app.playhead - strip.clip_in).max(0);
            let _ = app.session.edit("Clip gain", |project| {
                let seq = project
                    .active_sequence
                    .ok_or(editor_core::EditError::NoActiveSequence)?;
                set_clip_gain_at(project, seq, clip_id, rel, next)
            });
        }
    }
    if response.drag_stopped() {
        app.session.end_interactive();
    }
}

fn meter_columns(meter_rect: Rect) -> Rect {
    Rect::from_min_max(
        pos2(meter_rect.left(), meter_rect.top() + 12.0),
        meter_rect.right_bottom(),
    )
}

/// Sticky full-scale lamp. Click clears it. Sits above the meter columns.
fn clip_lamp(ui: &mut egui::Ui, meter_rect: Rect, id: Id, on: bool) -> bool {
    let rect = Rect::from_min_size(meter_rect.min, Vec2::new(meter_rect.width(), 8.0));
    let response = ui.interact(rect, id, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 1.0, if on { THEME.danger } else { THEME.inset });
    painter.rect_stroke(
        rect,
        1.0,
        Stroke::new(1.0_f32, if on { THEME.danger } else { THEME.hairline }),
        egui::StrokeKind::Inside,
    );
    response
        .on_hover_text(if on {
            "Full scale — click to clear"
        } else {
            "Clip"
        })
        .clicked()
}

fn paint_meter_pair(painter: &egui::Painter, rect: Rect, meter: MeterReadout) {
    let gap = 3.0;
    let width = (rect.width() - gap) * 0.5;
    for channel in 0..2 {
        let column = Rect::from_min_size(
            pos2(rect.left() + channel as f32 * (width + gap), rect.top()),
            Vec2::new(width, rect.height()),
        );
        paint_meter(painter, column, meter, channel);
    }
}

fn paint_meter(painter: &egui::Painter, rect: Rect, meter: MeterReadout, channel: usize) {
    painter.rect_filled(rect, 1.0, THEME.inset);
    let peak = meter_amount(meter.peak[channel]);
    let rms = meter_amount(meter.rms[channel]).min(peak);
    let hold = meter_amount(meter.hold[channel]);
    let peak_top = rect.bottom() - rect.height() * peak;
    let rms_top = rect.bottom() - rect.height() * rms;
    if peak > 0.0 {
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.left(), peak_top), rect.right_bottom()),
            1.0,
            meter_color(meter.peak[channel]).linear_multiply(0.45),
        );
    }
    if rms > 0.0 {
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.left(), rms_top), rect.right_bottom()),
            1.0,
            meter_color(meter.rms[channel]),
        );
    }
    if hold > 0.001 {
        let y = rect.bottom() - rect.height() * hold;
        painter.hline(
            rect.left()..=rect.right(),
            y,
            Stroke::new(1.5_f32, THEME.text),
        );
    }
}

fn meter_color(linear: f32) -> Color32 {
    let db = editor_core::linear_to_db(linear);
    if db >= -3.0 {
        THEME.danger
    } else if db >= -12.0 {
        THEME.amber
    } else {
        THEME.audio
    }
}

fn paint_diamond(painter: &egui::Painter, rect: Rect, filled: bool) {
    let center = rect.center();
    let points = vec![
        pos2(center.x, rect.top()),
        pos2(rect.right(), center.y),
        pos2(center.x, rect.bottom()),
        pos2(rect.left(), center.y),
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
