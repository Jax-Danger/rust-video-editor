//! Stereo mix shared by playback and export.
//!
//! Clip gain (constant or keyframed) is multiplied by the track fader.
//! Pan is constant-power and unity at center, so an unpanned clip sounds the
//! same as it did before the mixer existed. Mute and solo decide which tracks
//! reach the bus. The master fader scales the sum, then the sum is hard-limited.

use serde::{Deserialize, Serialize};

use crate::effects::Interpolation;
use crate::model::{Clip, Sequence, Track, TrackKind};

pub const GAIN_MIN: f32 = 0.0;
pub const GAIN_MAX: f32 = 4.0;
/// Fader floor and ceiling in the mixer UI, in decibels.
pub const FADER_DB_MIN: f32 = -60.0;
pub const FADER_DB_MAX: f32 = 12.0;
/// How many tracks one mix frame will meter. Extra tracks still sum into the bus.
pub const MAX_MIX_TRACKS: usize = 32;

pub fn clamp_gain(gain: f32) -> f32 {
    if !gain.is_finite() {
        return 0.0;
    }
    gain.clamp(GAIN_MIN, GAIN_MAX)
}

pub fn clamp_pan(pan: f32) -> f32 {
    if !pan.is_finite() {
        return 0.0;
    }
    pan.clamp(-1.0, 1.0)
}

pub fn limit_sample(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    sample.clamp(-1.0, 1.0)
}

/// Either channel is at or past full scale. Checked before the master limiter.
pub fn channel_clips(left: f32, right: f32) -> bool {
    left.abs() >= 1.0 || right.abs() >= 1.0
}

/// Constant-power pan, normalized so center is unity (0 dB) on both channels.
///
/// `pan` is −1 (hard left) … 0 (center) … +1 (hard right). Hard left is +3 dB
/// on the left and silence on the right. Center leaves a stereo pair unchanged.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let pan = clamp_pan(pan);
    let theta = (pan + 1.0) * 0.25 * std::f32::consts::PI;
    (
        theta.cos() * std::f32::consts::SQRT_2,
        theta.sin() * std::f32::consts::SQRT_2,
    )
}

pub fn linear_to_db(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 1.0e-8 {
        return f32::NEG_INFINITY;
    }
    20.0 * linear.log10()
}

pub fn db_to_linear(db: f32) -> f32 {
    if !db.is_finite() {
        return if db.is_sign_negative() { 0.0 } else { GAIN_MAX };
    }
    10f32.powf(db / 20.0)
}

/// Map a fader position (0 bottom, 1 top) onto linear gain.
/// The bottom detent is silence. 0 dB sits above the middle of the throw.
pub fn fader_pos_to_linear(pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0);
    if pos <= 0.02 {
        return 0.0;
    }
    let db = FADER_DB_MIN + (pos - 0.02) / 0.98 * (FADER_DB_MAX - FADER_DB_MIN);
    clamp_gain(db_to_linear(db))
}

pub fn linear_to_fader_pos(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 1.0e-5 {
        return 0.0;
    }
    let db = linear_to_db(clamp_gain(linear)).clamp(FADER_DB_MIN, FADER_DB_MAX);
    let t = (db - FADER_DB_MIN) / (FADER_DB_MAX - FADER_DB_MIN);
    0.02 + t * 0.98
}

pub fn format_db(linear: f32) -> String {
    if !linear.is_finite() || linear <= 1.0e-5 {
        return "-\u{221e}".to_string();
    }
    let db = linear_to_db(linear);
    if db.abs() < 0.05 {
        "0.0".to_string()
    } else {
        format!("{db:+.1}")
    }
}

pub fn format_pan(pan: f32) -> String {
    let pan = clamp_pan(pan);
    if pan.abs() < 0.02 {
        "C".to_string()
    } else if pan < 0.0 {
        format!("L{:.0}", -pan * 100.0)
    } else {
        format!("R{:.0}", pan * 100.0)
    }
}

/// Meter position 0…1 for a linear sample, from −60 dB to 0 dB.
pub fn meter_amount(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 1.0e-3 {
        return 0.0;
    }
    let db = linear_to_db(linear.abs());
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Peak-hold decay, about 12 dB per second, unless `peak` is higher.
pub fn update_hold(hold: f32, peak: f32, dt_secs: f32) -> f32 {
    let peak = peak.max(0.0);
    let hold = hold.max(0.0);
    if peak >= hold {
        return peak;
    }
    let db = linear_to_db(hold) - 12.0 * dt_secs.max(0.0);
    db_to_linear(db).max(0.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GainKey {
    /// Sequence frame.
    pub frame: i64,
    pub gain: f32,
    #[serde(default)]
    pub interpolation: Interpolation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GainCurve {
    /// Used when `keys` is empty.
    pub constant: f32,
    pub keys: Vec<GainKey>,
}

impl GainCurve {
    pub fn constant(gain: f32) -> Self {
        Self {
            constant: clamp_gain(gain),
            keys: Vec::new(),
        }
    }

    pub fn at(&self, frame: f64) -> f32 {
        if self.keys.is_empty() {
            return self.constant;
        }
        let frame_i = frame.floor() as i64;
        if frame <= self.keys[0].frame as f64 {
            return self.keys[0].gain;
        }
        let last = self.keys.len() - 1;
        if frame >= self.keys[last].frame as f64 {
            return self.keys[last].gain;
        }
        for pair in self.keys.windows(2) {
            let a = &pair[0];
            let b = &pair[1];
            if frame >= a.frame as f64 && frame <= b.frame as f64 {
                if a.interpolation == Interpolation::Hold || b.frame == a.frame {
                    return a.gain;
                }
                let span = (b.frame - a.frame) as f64;
                let t = ((frame - a.frame as f64) / span) as f32;
                return a.gain + (b.gain - a.gain) * t;
            }
        }
        let _ = frame_i;
        self.constant
    }
}

impl Default for GainCurve {
    fn default() -> Self {
        Self::constant(1.0)
    }
}

/// Clip gain only. Track fader and master are applied later.
pub fn clip_gain_curve(clip: &Clip) -> GainCurve {
    if clip.volume.keys.is_empty() {
        return GainCurve::constant(clip.volume.base);
    }
    let mut keys: Vec<GainKey> = clip
        .volume
        .keys
        .iter()
        .map(|key| GainKey {
            frame: clip.timeline_in.0 + key.frame,
            gain: clamp_gain(key.value),
            interpolation: key.interpolation,
        })
        .collect();
    keys.sort_by_key(|key| key.frame);
    keys.dedup_by_key(|key| key.frame);
    GainCurve {
        constant: clamp_gain(clip.volume.base),
        keys,
    }
}

pub fn scaled_curve(curve: &GainCurve, scale: f32) -> GainCurve {
    let scale = clamp_gain(scale);
    GainCurve {
        constant: curve.constant * scale,
        keys: curve
            .keys
            .iter()
            .map(|key| GainKey {
                frame: key.frame,
                gain: key.gain * scale,
                interpolation: key.interpolation,
            })
            .collect(),
    }
}

pub fn audio_solo_active(tracks: &[Track]) -> bool {
    tracks
        .iter()
        .any(|track| track.kind == TrackKind::Audio && track.solo)
}

pub fn track_is_audible(track: &Track, solo_active: bool) -> bool {
    track.kind == TrackKind::Audio && !track.muted && (!solo_active || track.solo)
}

/// One audible (or, when `include_silent`, merely present) audio clip.
#[derive(Clone, Debug, PartialEq)]
pub struct MixRegion {
    pub track_id: u64,
    pub clip_id: u64,
    pub media_id: crate::model::MediaId,
    pub timeline_in: i64,
    pub timeline_out: i64,
    pub pan: f32,
    /// Effective gain at the clip in-point: clip gain × track fader.
    pub gain: f32,
    /// Keyed effective gain. Empty when clip gain is a constant.
    pub gain_keys: Vec<GainKey>,
}

pub fn mix_regions(
    sequence: &Sequence,
    from: i64,
    to: i64,
    include_silent: bool,
) -> Vec<MixRegion> {
    let solo = audio_solo_active(&sequence.tracks);
    let mut regions = Vec::new();
    for track in &sequence.tracks {
        if track.kind != TrackKind::Audio {
            continue;
        }
        let audible = track_is_audible(track, solo);
        if !include_silent && !audible {
            continue;
        }
        let fader = clamp_gain(track.fader);
        let pan = clamp_pan(track.pan);
        for clip in &track.clips {
            if !clip.enabled || clip.timeline_out.0 <= from || clip.timeline_in.0 >= to {
                continue;
            }
            let Some(media_id) = clip.media_id else {
                continue;
            };
            let curve = scaled_curve(&clip_gain_curve(clip), fader);
            regions.push(MixRegion {
                track_id: track.id.0,
                clip_id: clip.id.0,
                media_id,
                timeline_in: clip.timeline_in.0,
                timeline_out: clip.timeline_out.0,
                pan,
                gain: curve.at(clip.timeline_in.0 as f64),
                gain_keys: curve.keys,
            });
        }
    }
    regions
}

#[derive(Clone, Debug, PartialEq)]
pub struct BusTrack {
    pub id: u64,
    pub fader: f32,
    pub pan: f32,
    pub audible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BusClip {
    pub id: u64,
    pub curve: GainCurve,
}

/// Live control state. Playback reads this every few milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct BusState {
    pub master: f32,
    pub tracks: Vec<BusTrack>,
    pub clips: Vec<BusClip>,
}

impl BusState {
    pub fn from_sequence(sequence: &Sequence) -> Self {
        let solo = audio_solo_active(&sequence.tracks);
        let mut tracks = Vec::new();
        let mut clips = Vec::new();
        for track in &sequence.tracks {
            if track.kind != TrackKind::Audio {
                continue;
            }
            tracks.push(BusTrack {
                id: track.id.0,
                fader: clamp_gain(track.fader),
                pan: clamp_pan(track.pan),
                audible: track_is_audible(track, solo),
            });
            for clip in &track.clips {
                if !clip.enabled {
                    continue;
                }
                clips.push(BusClip {
                    id: clip.id.0,
                    curve: clip_gain_curve(clip),
                });
            }
        }
        Self {
            master: clamp_gain(sequence.master_fader),
            tracks,
            clips,
        }
    }

    pub fn clip_gain(&self, clip_id: u64, frame: f64) -> f32 {
        self.clips
            .iter()
            .find(|clip| clip.id == clip_id)
            .map(|clip| clip.curve.at(frame))
            .unwrap_or(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixFrame {
    pub left: f32,
    pub right: f32,
    /// True when the pre-limit master sum reached full scale on either side.
    pub overload: bool,
    pub tracks: [(u64, f32, f32); MAX_MIX_TRACKS],
    pub track_count: u8,
}

impl Default for MixFrame {
    fn default() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
            overload: false,
            tracks: [(0, 0.0, 0.0); MAX_MIX_TRACKS],
            track_count: 0,
        }
    }
}

/// Sum pre-fader track audio (clip gains already applied) through fader, pan, and master.
///
/// `track_audio` entries are `(track_id, left, right)`.
pub fn mix_frame(track_audio: &[(u64, f32, f32)], bus: &BusState) -> MixFrame {
    let mut out = MixFrame::default();
    let master = clamp_gain(bus.master);
    let mut left = 0.0;
    let mut right = 0.0;
    for (id, in_left, in_right) in track_audio {
        let Some(track) = bus.tracks.iter().find(|track| track.id == *id) else {
            continue;
        };
        let (out_left, out_right) = if track.audible {
            stereo_frame(*in_left, *in_right, track.fader, track.pan)
        } else {
            (0.0, 0.0)
        };
        if (out.track_count as usize) < MAX_MIX_TRACKS {
            out.tracks[out.track_count as usize] = (*id, out_left, out_right);
            out.track_count += 1;
        }
        left += out_left;
        right += out_right;
    }
    let summed_left = left * master;
    let summed_right = right * master;
    out.overload = channel_clips(summed_left, summed_right);
    out.left = limit_sample(summed_left);
    out.right = limit_sample(summed_right);
    out
}

pub fn stereo_frame(left: f32, right: f32, fader: f32, pan: f32) -> (f32, f32) {
    let (gain_l, gain_r) = pan_gains(pan);
    let fader = clamp_gain(fader);
    (left * fader * gain_l, right * fader * gain_r)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
}

impl Default for Level {
    fn default() -> Self {
        Self {
            peak: [0.0, 0.0],
            rms: [0.0, 0.0],
        }
    }
}

pub fn measure_stereo(samples: &[f32]) -> Level {
    let mut peak = [0.0f32; 2];
    let mut sum = [0.0f32; 2];
    let mut count = [0u32; 2];
    for (index, sample) in samples.iter().enumerate() {
        let channel = index % 2;
        let value = if sample.is_finite() { *sample } else { 0.0 };
        peak[channel] = peak[channel].max(value.abs());
        sum[channel] += value * value;
        count[channel] = count[channel].saturating_add(1);
    }
    let rms = [0, 1].map(|channel| {
        if count[channel] == 0 {
            0.0
        } else {
            (sum[channel] / count[channel] as f32).sqrt()
        }
    });
    Level { peak, rms }
}

/// ffmpeg `volume` argument. Constants stay `0.2500`. Keyframes become an
/// escaped expression evaluated per frame, with `t = 0` at `overlap_in`.
pub fn ffmpeg_volume_arg(curve: &GainCurve, overlap_in: i64, fps: f64) -> String {
    if curve.keys.is_empty() {
        return format!("{:.4}", curve.constant.max(0.0));
    }
    if curve.keys.len() == 1 {
        return format!("{:.4}", curve.keys[0].gain.max(0.0));
    }
    let fps = if fps.is_finite() && fps > 1.0e-3 {
        fps
    } else {
        24.0
    };
    let mut keys = curve.keys.clone();
    keys.sort_by_key(|key| key.frame);
    let t_of = |frame: i64| (frame - overlap_in) as f64 / fps;
    let last = keys.last().expect("at least two keys");
    let mut expr = format!("{:.4}", last.gain.max(0.0));
    for index in (0..keys.len() - 1).rev() {
        let start = &keys[index];
        let end = &keys[index + 1];
        let t_end = t_of(end.frame);
        let segment = if start.interpolation == Interpolation::Hold || end.frame == start.frame {
            format!("{:.4}", start.gain.max(0.0))
        } else {
            let t_start = t_of(start.frame);
            format!(
                "{ga:.4}+(t-{ta:.6})/({tb:.6}-{ta:.6})*({gb:.4}-{ga:.4})",
                ga = start.gain.max(0.0),
                gb = end.gain.max(0.0),
                ta = t_start,
                tb = t_end
            )
        };
        expr = format!("if(lte(t\\,{t_end:.6})\\,{segment}\\,{expr})");
    }
    let first = &keys[0];
    expr = format!(
        "if(lt(t\\,{t0:.6})\\,{g0:.4}\\,{expr})",
        t0 = t_of(first.frame),
        g0 = first.gain.max(0.0)
    );
    format!("'{expr}':eval=frame")
}

/// `None` at center so an unpanned export keeps the old volume-only filter.
pub fn ffmpeg_pan_filter(pan: f32) -> Option<String> {
    let (left, right) = pan_gains(pan);
    if (left - 1.0).abs() < 1.0e-3 && (right - 1.0).abs() < 1.0e-3 {
        return None;
    }
    Some(format!(
        "pan=stereo|c0={left:.4}*c0|c1={right:.4}*c1",
        left = left,
        right = right
    ))
}

/// Topology that forces a re-decode: which clips exist and where they sit.
/// Fader, pan, mute, solo, and clip gain are live and stay out of the hash.
pub fn audio_topology(sequence: &Sequence) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    for track in &sequence.tracks {
        if track.kind != TrackKind::Audio {
            continue;
        }
        track.id.0.hash(&mut hasher);
        for clip in &track.clips {
            clip.id.0.hash(&mut hasher);
            clip.enabled.hash(&mut hasher);
            clip.media_id.map(|id| id.0).hash(&mut hasher);
            clip.timeline_in.0.hash(&mut hasher);
            clip.timeline_out.0.hash(&mut hasher);
            clip.source_in.0.hash(&mut hasher);
            clip.source_out.0.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Apply gain and pan to an interleaved stereo buffer, summing into `dest`.
pub fn accumulate_stereo(
    dest: &mut [f32],
    src: &[f32],
    dest_frame: usize,
    gain_at: impl Fn(usize) -> f32,
    pan: f32,
) {
    let (gain_l, gain_r) = pan_gains(pan);
    let frames = src.len() / 2;
    for index in 0..frames {
        let at = (dest_frame + index) * 2;
        if at + 1 >= dest.len() {
            break;
        }
        let gain = gain_at(index);
        dest[at] += src[index * 2] * gain * gain_l;
        dest[at + 1] += src[index * 2 + 1] * gain * gain_r;
    }
}

#[cfg(test)]
fn has_unescaped_comma(expr: &str) -> bool {
    let mut escaped = false;
    for ch in expr.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == ',' {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Interpolation;
    use crate::model::{Clip, MediaId, SequenceId, TrackId, TrackKind};
    use crate::time::{Frame, Timebase};

    fn audio_sequence() -> Sequence {
        let mut sequence = Sequence::new(SequenceId(1), "Mix", 1920, 1080, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Audio, "A1");
        sequence.add_track(TrackId(3), TrackKind::Audio, "A2");
        sequence
    }

    fn tone(id: u64, start: i64, end: i64) -> Clip {
        let mut clip = Clip::basic(id, start, end);
        clip.media_id = Some(MediaId(id));
        clip
    }

    #[test]
    fn center_pan_is_unity_and_hard_left_is_silent_on_the_right() {
        let (left, right) = pan_gains(0.0);
        assert!((left - 1.0).abs() < 1.0e-5, "left {left}");
        assert!((right - 1.0).abs() < 1.0e-5, "right {right}");
        let (left, right) = pan_gains(-1.0);
        assert!((left - std::f32::consts::SQRT_2).abs() < 1.0e-5);
        assert!(right.abs() < 1.0e-5);
        let (left, right) = pan_gains(1.0);
        assert!(left.abs() < 1.0e-5);
        assert!((right - std::f32::consts::SQRT_2).abs() < 1.0e-5);
        assert!(ffmpeg_pan_filter(0.0).is_none());
        let hard = ffmpeg_pan_filter(-1.0).unwrap();
        assert!(hard.contains("c0=1.4142*c0"));
        assert!(hard.contains("c1=0.0000*c1"));
    }

    #[test]
    fn fader_unity_is_zero_db_and_the_bottom_is_silent() {
        assert!((fader_pos_to_linear(linear_to_fader_pos(1.0)) - 1.0).abs() < 1.0e-3);
        assert_eq!(fader_pos_to_linear(0.0), 0.0);
        assert!((db_to_linear(0.0) - 1.0).abs() < 1.0e-5);
        assert_eq!(format_db(1.0), "0.0");
        assert_eq!(format_db(0.0), "-\u{221e}");
        assert_eq!(format_pan(0.0), "C");
        assert_eq!(format_pan(-0.5), "L50");
    }

    #[test]
    fn keyframed_gain_matches_animated_f32_and_scales_with_the_fader() {
        let mut clip = tone(4, 10, 40);
        clip.volume.base = 0.25;
        clip.volume.set_key(0, 0.0);
        clip.volume.set_key(10, 1.0);
        clip.volume.keys[0].interpolation = Interpolation::Linear;
        let curve = clip_gain_curve(&clip);
        assert!((curve.at(10.0) - clip.volume.value_at(0)).abs() < 1.0e-5);
        assert!((curve.at(15.0) - clip.volume.value_at(5)).abs() < 1.0e-4);
        assert!((curve.at(20.0) - 1.0).abs() < 1.0e-5);
        assert!((curve.at(0.0) - 0.0).abs() < 1.0e-5);
        let scaled = scaled_curve(&curve, 0.5);
        assert!((scaled.at(20.0) - 0.5).abs() < 1.0e-5);
        let expr = ffmpeg_volume_arg(&scaled, 10, 24.0);
        assert!(expr.contains("eval=frame"), "{expr}");
        assert!(
            !has_unescaped_comma(&expr),
            "raw comma would split the filter: {expr}"
        );
        assert_eq!(
            ffmpeg_volume_arg(&GainCurve::constant(0.25), 0, 24.0),
            "0.2500"
        );
    }

    #[test]
    fn hold_keys_stay_flat_until_the_next_frame() {
        let mut curve = GainCurve::constant(1.0);
        curve.keys = vec![
            GainKey {
                frame: 0,
                gain: 0.2,
                interpolation: Interpolation::Hold,
            },
            GainKey {
                frame: 8,
                gain: 0.8,
                interpolation: Interpolation::Linear,
            },
        ];
        assert!((curve.at(7.9) - 0.2).abs() < 1.0e-4);
        // The final key wins at its own frame, matching AnimatedF32.
        assert!((curve.at(8.0) - 0.8).abs() < 1.0e-4);
    }

    #[test]
    fn mute_solo_fader_and_master_follow_one_bus() {
        let mut sequence = audio_sequence();
        sequence.tracks[0].clips = vec![tone(4, 0, 24)];
        sequence.tracks[0].fader = 0.5;
        sequence.tracks[1].clips = vec![tone(5, 0, 24)];
        sequence.tracks[1].pan = -1.0;
        sequence.master_fader = 0.5;
        let regions = mix_regions(&sequence, 0, 24, false);
        assert_eq!(regions.len(), 2);
        assert!((regions[0].gain - 0.5).abs() < 1.0e-5);
        assert!((regions[1].pan + 1.0).abs() < 1.0e-5);

        sequence.tracks[0].muted = true;
        assert_eq!(mix_regions(&sequence, 0, 24, false).len(), 1);
        assert_eq!(mix_regions(&sequence, 0, 24, true).len(), 2);

        sequence.tracks[0].muted = false;
        sequence.tracks[1].solo = true;
        let solo = mix_regions(&sequence, 0, 24, false);
        assert_eq!(solo.len(), 1);
        assert_eq!(solo[0].track_id, 3);

        let bus = BusState::from_sequence(&sequence);
        assert!(!bus.tracks[0].audible);
        assert!(bus.tracks[1].audible);
        assert!((bus.master - 0.5).abs() < 1.0e-5);
        let frame = mix_frame(&[(2, 0.8, 0.8), (3, 0.4, 0.4)], &bus);
        let left_only = 0.4 * std::f32::consts::SQRT_2;
        assert!((frame.left - limit_sample(left_only * 0.5)).abs() < 1.0e-4);
        assert!(frame.right.abs() < 1.0e-4);
        let track_3 = frame.tracks.iter().find(|item| item.0 == 3).unwrap();
        assert!((track_3.1 - left_only).abs() < 1.0e-4);
    }

    #[test]
    fn overlapping_tracks_sum_then_limit_once() {
        let bus = BusState {
            master: 1.0,
            tracks: vec![
                BusTrack {
                    id: 1,
                    fader: 1.0,
                    pan: 0.0,
                    audible: true,
                },
                BusTrack {
                    id: 2,
                    fader: 1.0,
                    pan: 0.0,
                    audible: true,
                },
            ],
            clips: Vec::new(),
        };
        let frame = mix_frame(&[(1, 0.6, -0.6), (2, 0.6, -0.6)], &bus);
        assert!(frame.overload);
        assert!((frame.left - 1.0).abs() < 1.0e-5);
        assert!((frame.right + 1.0).abs() < 1.0e-5);
        let quiet = mix_frame(&[(1, 0.25, 0.25), (2, 0.25, 0.25)], &bus);
        assert!(!quiet.overload);
        assert!((quiet.left - 0.5).abs() < 1.0e-5);
    }

    #[test]
    fn meters_report_peak_and_rms() {
        let mut samples = Vec::new();
        for _ in 0..100 {
            samples.push(0.5);
            samples.push(-0.25);
        }
        let level = measure_stereo(&samples);
        assert!((level.peak[0] - 0.5).abs() < 1.0e-5);
        assert!((level.peak[1] - 0.25).abs() < 1.0e-5);
        assert!((level.rms[0] - 0.5).abs() < 1.0e-5);
        assert!((level.rms[1] - 0.25).abs() < 1.0e-5);
        assert!((update_hold(0.2, 0.8, 0.1) - 0.8).abs() < 1.0e-6);
        let fallen = update_hold(1.0, 0.0, 1.0);
        let expected = db_to_linear(-12.0);
        assert!((fallen - expected).abs() < 1.0e-3, "{fallen} vs {expected}");
    }

    #[test]
    fn accumulate_respects_gain_and_pan() {
        let src = [0.5f32, 0.5, 0.5, 0.5];
        let mut dest = vec![0.0; 4];
        accumulate_stereo(&mut dest, &src, 0, |_| 0.5, -1.0);
        let expected = 0.5 * 0.5 * std::f32::consts::SQRT_2;
        assert!((dest[0] - expected).abs() < 1.0e-5);
        assert!(dest[1].abs() < 1.0e-5);
        assert!((dest[2] - expected).abs() < 1.0e-5);
    }

    #[test]
    fn topology_ignores_fader_and_hears_a_trim() {
        let mut sequence = audio_sequence();
        sequence.tracks[0].clips = vec![tone(4, 0, 24)];
        let before = audio_topology(&sequence);
        sequence.tracks[0].fader = 0.2;
        sequence.tracks[0].pan = 0.4;
        sequence.tracks[0].muted = true;
        sequence.tracks[0].clips[0].volume.base = 0.3;
        assert_eq!(audio_topology(&sequence), before);
        sequence.tracks[0].clips[0].timeline_out = Frame(12);
        assert_ne!(audio_topology(&sequence), before);
    }

    #[test]
    fn clip_volume_json_accepts_a_number_or_keys() {
        let constant = Clip::basic(1, 0, 8);
        let json = serde_json::to_string(&constant).unwrap();
        assert!(!json.contains("volume"), "{json}");
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert!((loaded.volume.base - 1.0).abs() < 1.0e-5);
        assert!(loaded.volume.keys.is_empty());

        let mut quiet = Clip::basic(2, 0, 8);
        quiet.volume.base = 0.5;
        let json = serde_json::to_string(&quiet).unwrap();
        assert!(json.contains("\"volume\":0.5"), "{json}");
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert!((loaded.volume.base - 0.5).abs() < 1.0e-5);

        let legacy = json.replace("\"volume\":0.5", "\"volume\":2");
        let loaded: Clip = serde_json::from_str(&legacy).unwrap();
        assert!((loaded.volume.base - 2.0).abs() < 1.0e-5);

        let mut keyed = Clip::basic(3, 4, 20);
        keyed.volume.set_key(0, 0.0);
        keyed.volume.set_key(8, 1.0);
        let json = serde_json::to_string(&keyed).unwrap();
        assert!(json.contains("\"keys\""), "{json}");
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.volume.keys.len(), 2);
        assert!((loaded.volume.value_at(4) - 0.5).abs() < 1.0e-4);
    }
}
